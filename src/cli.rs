use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{Read, Write},
};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

mod cache_report;
use cache_report::cache;
#[cfg(test)]
use cache_report::{summarize_cache_events, summarize_debate_cache_events};

use crate::{
    config::{ConfigStore, parse_context_length, validate_model_id},
    core::{
        BridgeClient, CallRequest, ImageAttachment, McpServerConfiguration, Message,
        OpenAiCompatibleProvider, StreamEvent, node_executable, runtime_directory,
    },
    project_settings::ProjectSettingsStore,
    sandbox_cli,
    session_store::SessionStore,
    web_search,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const HELP: &str = r#"Usage:
  yeet remote [WORKSPACE] [--workspace PATH] [--bind ADDRESS] [--size COLSxROWS] [--origin URL]
  yeet remote status [WORKSPACE|--workspace PATH]
  yeet remote stop [WORKSPACE|--workspace PATH]
  yeet remote auth status [WORKSPACE|--workspace PATH]
  yeet remote auth key generate|set|clear [WORKSPACE|--workspace PATH]
  yeet remote auth passkey add|clear [WORKSPACE|--workspace PATH]
  yeet doctor
  yeet update [check]
  yeet model get
  yeet model set provider/model
  yeet model context [get [provider/model]|set [provider/model] length|auto [provider/model]]
  yeet cache [latest|SESSION_ID]
  yeet run [--model provider/model] [--image path] [prompt]
  yeet auth status [provider]
  yeet usage [codex|claude|gemini|provider]
  printf key | yeet auth set-key provider
  yeet auth login provider
  yeet auth logout provider
  yeet provider list
  yeet provider add id base-url [--require-api-key] [--header Name=Value]
  yeet provider remove id
  yeet extension list
  yeet extension path
  yeet extension validate PATH
  yeet extension install PATH [--project] [--force]
  yeet extension remove ID [--project]
  yeet skill list
  yeet skill show name
  yeet skill read name path
  yeet skill validate path
  yeet skill install path|git-url|zip
  yeet skill remove name
  yeet skyline setup [--workspace PATH]
  yeet mcp list
  yeet mcp add-stdio name command [args...]
  yeet mcp add-http name url [--header Name=Value]...
  yeet mcp remove name
  yeet mcp tools [server]
  yeet mcp call server/tool [json-arguments]
  yeet mcp resources [server]
  yeet mcp resource server uri
  yeet mcp prompts [server]
  yeet mcp prompt server name [json-string-arguments]
  yeet mcpserver start [--port PORT] [--bind HOST] [--workspace PATH] [--restrict-workspace|--no-restrict-workspace] [--public-url URL] [--auth key|oauth|none]
  yeet mcpserver status [--port PORT]
  yeet mcpserver stop [--port PORT]
  yeet mcpserver restart [--port PORT] [...]
  yeet mcpserver auth [status|mode|key] ...
  yeet mcpserver stdio [WORKSPACE|--workspace PATH] [--restrict-workspace]
  yeet memory status
  yeet memory recall <query>
  yeet memory on|off
  yeet memory models
  yeet memory model [provider/model]
  yeet memory reindex [provider/model]
  yeet memory import-foundation <export.jsonl>
  yeet memory export-foundation <export.jsonl>
  yeet sandbox [show|path|reset|scratch|workspace|network|env|secret|limits] ...
  yeet search [install [all|searxng|agent-reach]|status|path|remove [all|searxng|agent-reach]]

State:
  YEET_CONFIG_DIR overrides the per-user state directory.
  Existing ~/.yeet installs are preserved on every OS.
  Clean Linux installs use XDG config paths; clean Windows installs use AppData.
  Project state remains under ./.yeet/.

Runtime:
  Yeet is Rust-native. The TypeScript provider/MCP runtime is shipped separately
  and requires Node.js 20+. Set YEET_RUNTIME_DIR and YEET_NODE to override paths.

Remote:
  Remote mode is opt-in and runs as a detached background daemon serving the
  existing TUI over HTTP. It binds to 0.0.0.0:7331 by default and can target
  a workspace without changing the terminal's current directory. Access keys
  and WebAuthn passkeys are optional and scoped per workspace. Port forwarding
  and tunneling are user-managed."#;

pub fn run(arguments: &[String]) -> Result<i32> {
    let command = arguments.first().map(String::as_str).unwrap_or("help");
    let rest = if arguments.is_empty() {
        &[][..]
    } else {
        &arguments[1..]
    };
    let result = match command {
        "-h" | "--help" | "help" => {
            println!("yeet {VERSION}\n\n{HELP}");
            Ok(())
        }
        "-v" | "--version" | "version" => {
            println!("{VERSION}");
            Ok(())
        }
        "doctor" => doctor(),
        "update" => crate::update::run(rest),
        "model" => model(rest),
        "cache" => cache(rest),
        "run" => run_model(rest),
        "auth" => auth(rest),
        "usage" => usage_command(rest),
        "provider" | "providers" | "endpoint" | "endpoints" => provider(rest),
        "extension" | "extensions" => {
            let output = crate::extensions::run_cli(rest)?;
            if !output.is_empty() {
                println!("{output}");
            }
            Ok(())
        }
        "skill" | "skills" => skill(rest),
        "skyline" => skyline(rest),
        "mcp" => mcp(rest),
        "mcpserver" => {
            let output = crate::mcp_server::run_cli(rest)?;
            if !output.is_empty() {
                println!("{output}");
            }
            Ok(())
        }
        "foundation" | "memory" => foundation(rest),
        "search" => {
            let output = web_search::run_cli(rest)?;
            if !output.is_empty() {
                println!("{output}");
            }
            Ok(())
        }
        "sandbox" => {
            let root = std::env::current_dir()?
                .canonicalize()
                .unwrap_or(std::env::current_dir()?);
            let output = sandbox_cli::run(&root, rest)?;
            if !output.is_empty() {
                println!("{output}");
            }
            Ok(())
        }
        other => bail!("Unknown command: {other}"),
    };
    match result {
        Ok(()) => Ok(0),
        Err(error) => {
            eprintln!("yeet: {error}");
            Ok(1)
        }
    }
}

fn skyline(args: &[String]) -> Result<()> {
    match args {
        [command] if command == "setup" => {
            let root = crate::skyline::setup_installation()?;
            println!("Skyline runtime ready at {}", root.display());
            Ok(())
        }
        [command, flag, workspace] if command == "setup" && flag == "--workspace" => {
            let project = crate::skyline::setup_workspace(std::path::Path::new(workspace))?;
            println!("Skyline workspace ready at {}", project.display());
            Ok(())
        }
        _ => bail!("Usage: yeet skyline setup [--workspace PATH]"),
    }
}

fn foundation(args: &[String]) -> Result<()> {
    let workspace = std::env::current_dir()?.canonicalize()?;
    let settings = ProjectSettingsStore::new(&workspace)?;
    let memory = crate::memory::MemoryStore::default();
    let sub = args.first().map(String::as_str).unwrap_or("status");
    match sub {
        "status" => {
            let mut status = memory.status()?;
            status["enabled"] = json!(settings.load()?.foundation_memory.enabled);
            status["project"] = json!(settings.project_identity()?);
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        "on" | "off" => {
            if args.len() > 1 {
                bail!(
                    "Native memory uses Yeet's API. Select a model with `yeet memory model provider/model` (full re-index)."
                );
            }
            settings.save_foundation_memory(sub == "on", None)?;
            println!("{}", if sub == "on" { "enabled" } else { "disabled" });
        }
        "model" if args.len() == 1 => {
            println!(
                "{}",
                memory.status()?["embeddingModel"]
                    .as_str()
                    .unwrap_or("automatic until first write")
            );
        }
        "export-foundation" => {
            let path = args
                .get(1)
                .ok_or_else(|| anyhow!("Usage: yeet memory export-foundation <export.jsonl>"))?;
            let result = memory
                .export_foundation(std::path::Path::new(path), &settings.project_identity()?)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        "model" | "reindex" | "models" | "import-foundation" | "recall" => {
            let bridge = BridgeClient::start()?;
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let result = (|| -> Result<Value> {
                match sub {
                    "models" => Ok(json!(bridge.embedding_models(&cancel)?)),
                    "recall" => {
                        let query = args
                            .get(1)
                            .ok_or_else(|| anyhow!("Usage: yeet memory recall <query>"))?;
                        memory.call(
                            "memory_recall",
                            &settings.project_identity()?,
                            &json!({"query":query}),
                            &bridge,
                            &cancel,
                        )
                    }
                    "import-foundation" => {
                        let path = args.get(1).ok_or_else(|| {
                            anyhow!("Usage: yeet memory import-foundation <export.jsonl>")
                        })?;
                        memory.import_foundation(
                            std::path::Path::new(path),
                            &settings.project_identity()?,
                            &bridge,
                            &cancel,
                        )
                    }
                    _ => memory.reindex(args.get(1).map(String::as_str), &bridge, &cancel),
                }
            })();
            bridge.shutdown();
            println!("{}", serde_json::to_string_pretty(&result?)?);
        }
        _ => bail!(
            "Usage: yeet memory [status|on|off|recall <query>|models|model [provider/model]|reindex [provider/model]|import-foundation <export.jsonl>|export-foundation <export.jsonl>]"
        ),
    }
    Ok(())
}

fn doctor() -> Result<()> {
    let config = ConfigStore::default();
    config.ensure()?;
    let runtime = runtime_directory()?;
    let node = node_executable()?;
    let bridge = BridgeClient::start()?;
    let providers = bridge.list_providers()?;
    bridge.shutdown();
    #[cfg(target_os = "linux")]
    let sandbox_status = match nono::Sandbox::detect_abi() {
        Ok(detected) => format!("landlock {:?}", detected.abi),
        Err(_) => "unavailable (Landlock is not exposed by this kernel/container; sandboxed shell fails closed)".into(),
    };
    println!("yeet {VERSION}");
    println!("node: {}", node.display());
    println!("runtime: {}", runtime.display());
    println!("config: {}", config.directory.display());
    println!("providers: {}", providers.join(", "));
    #[cfg(target_os = "linux")]
    println!("sandbox: {sandbox_status}");
    Ok(())
}

fn model(args: &[String]) -> Result<()> {
    let config = ConfigStore::default();
    match args.first().map(String::as_str).unwrap_or("get") {
        "get" => {
            println!("{}", config.model()?.unwrap_or_else(|| "<not set>".into()));
            Ok(())
        }
        "set" => {
            let value = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("Usage: yeet model set provider/model"))?;
            println!("{}", config.set_model(value)?);
            Ok(())
        }
        "context" => model_context(&config, &args[1..]),
        _ => bail!("Usage: yeet model [get|set provider/model|context ...]"),
    }
}

fn model_context(config: &ConfigStore, args: &[String]) -> Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("get");
    match sub {
        "get" => {
            let model = if let Some(value) = args.get(1) {
                validate_model_id(value)?
            } else {
                config
                    .model()?
                    .ok_or_else(|| anyhow::anyhow!("No model is configured"))?
            };
            match config.context_length_override(&model)? {
                Some(length) => println!("{model}\t{length}\tmanual"),
                None => match config.context_length(&model)? {
                    Some(length) => println!("{model}\t{length}\tauto"),
                    None => println!("{model}\tunknown\tauto"),
                },
            }
            Ok(())
        }
        "set" => {
            let (model, value) = match (args.get(1), args.get(2)) {
                (Some(first), Some(second)) => (validate_model_id(first)?, second.as_str()),
                (Some(value), None) => (
                    config
                        .model()?
                        .ok_or_else(|| anyhow::anyhow!("No model is configured"))?,
                    value.as_str(),
                ),
                _ => bail!("Usage: yeet model context set [provider/model] length"),
            };
            let length = parse_context_length(value)?;
            config.set_context_length_override(&model, Some(length))?;
            println!("{model}\t{length}\tmanual");
            Ok(())
        }
        "auto" | "reset" => {
            let model = if let Some(value) = args.get(1) {
                validate_model_id(value)?
            } else {
                config
                    .model()?
                    .ok_or_else(|| anyhow::anyhow!("No model is configured"))?
            };
            config.set_context_length_override(&model, None)?;
            println!("{model}\tauto");
            Ok(())
        }
        _ => bail!(
            "Usage: yeet model context [get [provider/model]|set [provider/model] length|auto [provider/model]]"
        ),
    }
}

fn run_model(args: &[String]) -> Result<()> {
    let config = ConfigStore::default();
    let mut override_model = None;
    let mut prompt = Vec::new();
    let mut image_paths = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--model" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Usage: yeet run [--model provider/model] [--image path] [prompt]"
                    )
                })?;
                override_model = Some(validate_model_id(value)?);
                index += 2;
            }
            "--image" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| anyhow::anyhow!("--image requires a path"))?;
                image_paths.push(value.clone());
                index += 2;
            }
            _ => {
                prompt.push(args[index].clone());
                index += 1;
            }
        }
    }
    let model = override_model.or(config.model()?).ok_or_else(|| {
        anyhow::anyhow!("No model is configured. Run: yeet model set provider/model")
    })?;
    let prompt = if prompt.is_empty() {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        text.trim().to_owned()
    } else {
        prompt.join(" ")
    };
    if prompt.is_empty() {
        bail!("Usage: yeet run [--model provider/model] [--image path] prompt");
    }
    let images = image_paths
        .iter()
        .map(|path| ImageAttachment::from_file(std::path::Path::new(path)))
        .collect::<Result<Vec<_>>>()?;
    let bridge = BridgeClient::start()?;
    let request = CallRequest::simple(model, vec![Message::user_with_images(prompt, images)]);
    let mut stream = bridge.stream(&request)?;
    while let Some(event) = stream.recv()? {
        match event {
            StreamEvent::TextDelta(text) => {
                print!("{text}");
                std::io::stdout().flush()?;
            }
            StreamEvent::ToolCall { tool_call, .. } => println!("\n[tool] {}", tool_call.name),
            _ => {}
        }
    }
    println!();
    bridge.shutdown();
    Ok(())
}

fn usage_command(args: &[String]) -> Result<()> {
    if args.len() > 1 {
        bail!("Usage: yeet usage [codex|claude|gemini|provider]");
    }
    let providers = if let Some(provider) = args.first() {
        vec![match provider.as_str() {
            "codex" => "codex-cli".to_owned(),
            "claude" => "claude".to_owned(),
            "google" => "gemini-web".to_owned(),
            other => other.to_owned(),
        }]
    } else {
        vec![
            "openai".to_owned(),
            "codex-cli".to_owned(),
            "anthropic".to_owned(),
            "claude".to_owned(),
            "gemini".to_owned(),
            "gemini-web".to_owned(),
        ]
    };

    let bridge = BridgeClient::start()?;
    let result = (|| -> Result<()> {
        for provider in providers {
            let usage = bridge.provider_usage(&provider)?;
            let plan = usage
                .plan
                .as_deref()
                .map(|plan| format!(" · plan {plan}"))
                .unwrap_or_default();
            println!("{} · {}{plan}", usage.provider, usage.source);
            if usage.windows.is_empty() {
                println!(
                    "  {}",
                    usage.message.as_deref().unwrap_or("usage unavailable")
                );
                continue;
            }
            for window in &usage.windows {
                let reset = window
                    .resets_at
                    .as_deref()
                    .map(|reset| format!(" · resets {reset}"))
                    .unwrap_or_default();
                println!(
                    "  {}: {}% left · {}% used{reset}",
                    window.label, window.remaining_percent, window.used_percent
                );
            }
        }
        Ok(())
    })();
    bridge.shutdown();
    result
}

fn auth(args: &[String]) -> Result<()> {
    let sub = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("Usage: yeet auth [status|set-key|login|logout] ..."))?;
    let bridge = BridgeClient::start()?;
    let result = (|| -> Result<()> {
        match sub {
            "status" => {
                let providers = if let Some(provider) = args.get(1) {
                    vec![provider.clone()]
                } else {
                    bridge.list_providers()?
                };
                for provider in providers {
                    let status = bridge.auth_status(&provider)?;
                    println!(
                        "{}: {} via {}",
                        provider,
                        if status.authenticated {
                            "authenticated"
                        } else {
                            "not authenticated"
                        },
                        status.method
                    );
                }
                Ok(())
            }
            "set-key" => {
                let provider = args.get(1).ok_or_else(|| {
                    anyhow::anyhow!("Usage: printf key | yeet auth set-key provider")
                })?;
                let mut key = String::new();
                std::io::stdin().read_to_string(&mut key)?;
                let key = key.trim();
                if key.is_empty() {
                    bail!("Usage: printf key | yeet auth set-key provider");
                }
                let status = bridge.set_api_key(provider, key)?;
                println!("{}: authenticated", status.provider);
                Ok(())
            }
            "login" => {
                let provider = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet auth login provider"))?;
                let options = if provider == "gemini" || provider == "gemini-web" {
                    let env = std::env::vars().collect::<HashMap<_, _>>();
                    Some(
                        json!({"clientId":env.get("GEMINI_OAUTH_CLIENT_ID").or_else(||env.get("GOOGLE_CLIENT_ID")),"clientSecret":env.get("GEMINI_OAUTH_CLIENT_SECRET").or_else(||env.get("GOOGLE_CLIENT_SECRET")),"projectId":env.get("GEMINI_PROJECT_ID").or_else(||env.get("GOOGLE_CLOUD_PROJECT"))}),
                    )
                } else {
                    None
                };
                let status = bridge.login_browser(provider, options)?;
                println!("{}: authenticated via {}", status.provider, status.method);
                Ok(())
            }
            "logout" => {
                let provider = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet auth logout provider"))?;
                let status = bridge.logout(provider)?;
                println!("{}: logged out", status.provider);
                Ok(())
            }
            _ => bail!("Usage: yeet auth [status|set-key|login|logout] ..."),
        }
    })();
    bridge.shutdown();
    result
}

fn provider(args: &[String]) -> Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("list");
    let bridge = BridgeClient::start()?;
    let result = (|| -> Result<()> {
        match sub {
            "list" => {
                let values = bridge.list_provider_configurations()?;
                if values.is_empty() {
                    println!("No custom API endpoints are configured.");
                }
                for provider in values {
                    println!(
                        "{}\t{}{}",
                        provider.id,
                        provider.base_url,
                        if provider.require_api_key == Some(true) {
                            "\trequires-api-key"
                        } else {
                            ""
                        }
                    );
                }
                Ok(())
            }
            "add" | "set" => {
                if args.len() < 3 {
                    bail!(
                        "Usage: yeet provider add id base-url [--require-api-key] [--header Name=Value]"
                    );
                }
                let mut require = false;
                let mut headers = HashMap::new();
                let mut i = 3;
                while i < args.len() {
                    match args[i].as_str() {
                        "--require-api-key" => {
                            require = true;
                            i += 1
                        }
                        "--header" => {
                            let pair = args
                                .get(i + 1)
                                .ok_or_else(|| anyhow::anyhow!("--header requires Name=Value"))?;
                            let (k, v) = pair
                                .split_once('=')
                                .ok_or_else(|| anyhow::anyhow!("--header requires Name=Value"))?;
                            headers.insert(k.into(), v.into());
                            i += 2
                        }
                        other => bail!("Unknown provider option: {other}"),
                    }
                }
                let saved = bridge.save_provider_configuration(&OpenAiCompatibleProvider {
                    kind: "openai-compatible".into(),
                    id: args[1].clone(),
                    base_url: args[2].clone(),
                    api_key: None,
                    headers: (!headers.is_empty()).then_some(headers),
                    require_api_key: Some(require),
                })?;
                println!("{}\t{}", saved.id, saved.base_url);
                Ok(())
            }
            "remove" | "delete" => {
                if args.len() != 2 {
                    bail!("Usage: yeet provider remove id");
                }
                println!(
                    "{}",
                    if bridge.remove_provider_configuration(&args[1])? {
                        "removed"
                    } else {
                        "not found"
                    }
                );
                Ok(())
            }
            _ => bail!("Usage: yeet provider [list|add id base-url|remove id]"),
        }
    })();
    bridge.shutdown();
    result
}

fn skill(args: &[String]) -> Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("list");
    let bridge = BridgeClient::start()?;
    let result = (|| -> Result<()> {
        match sub {
            "list" => {
                for skill in bridge.list_skills()? {
                    println!("{}\t{}", skill.name, skill.description);
                }
                Ok(())
            }
            "show" => {
                let name = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet skill show name"))?;
                println!("{}", bridge.load_skill(name)?.instructions);
                Ok(())
            }
            "read" => {
                if args.len() < 3 {
                    bail!("Usage: yeet skill read name path");
                }
                println!("{}", bridge.read_skill_file(&args[1], &args[2])?);
                Ok(())
            }
            "validate" => {
                let source = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet skill validate path"))?;
                for skill in bridge.validate_skills(source)? {
                    println!("{}\t{}", skill.name, skill.description);
                }
                Ok(())
            }
            "install" => {
                let source = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet skill install path|git-url|zip"))?;
                for name in bridge.install_skills(source)? {
                    println!("installed\t{name}");
                }
                Ok(())
            }
            "remove" | "uninstall" => {
                let name = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet skill remove name"))?;
                println!(
                    "{}",
                    if bridge.remove_skill(name)? {
                        "removed"
                    } else {
                        "not found"
                    }
                );
                Ok(())
            }
            _ => bail!("Usage: yeet skill [list|show|read|validate|install|remove] ..."),
        }
    })();
    bridge.shutdown();
    result
}

fn mcp(args: &[String]) -> Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("list");
    let bridge = BridgeClient::start()?;
    let result = (|| -> Result<()> {
        match sub {
            "list" => {
                for server in bridge.list_mcp_servers()? {
                    println!(
                        "{}\t{}\t{}",
                        server.name,
                        server.transport,
                        if server.connected {
                            "connected"
                        } else {
                            "idle"
                        }
                    );
                }
                Ok(())
            }
            "add-stdio" => {
                if args.len() < 3 {
                    bail!("Usage: yeet mcp add-stdio name command [args...]");
                }
                let server = McpServerConfiguration {
                    name: args[1].clone(),
                    transport: "stdio".into(),
                    command: Some(args[2].clone()),
                    args: Some(args[3..].to_vec()),
                    env: None,
                    cwd: None,
                    url: None,
                    headers: None,
                };
                println!("{}", bridge.set_mcp_server(&server)?.name);
                Ok(())
            }
            "add-http" => {
                if args.len() < 3 {
                    bail!("Usage: yeet mcp add-http name url [--header Name=Value]...");
                }
                let mut headers = HashMap::new();
                let mut index = 3usize;
                while index < args.len() {
                    if args[index] != "--header" || index + 1 >= args.len() {
                        bail!("Usage: yeet mcp add-http name url [--header Name=Value]...");
                    }
                    let (name, value) = args[index + 1]
                        .split_once('=')
                        .ok_or_else(|| anyhow::anyhow!("MCP header must use Name=Value"))?;
                    let name = name.trim();
                    if name.is_empty() {
                        bail!("MCP header name cannot be empty");
                    }
                    headers.insert(name.to_owned(), value.to_owned());
                    index += 2;
                }
                let server = McpServerConfiguration {
                    name: args[1].clone(),
                    transport: "http".into(),
                    command: None,
                    args: None,
                    env: None,
                    cwd: None,
                    url: Some(args[2].clone()),
                    headers: (!headers.is_empty()).then_some(headers),
                };
                println!("{}", bridge.set_mcp_server(&server)?.name);
                Ok(())
            }
            "remove" => {
                let name = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet mcp remove name"))?;
                println!(
                    "{}",
                    if bridge.remove_mcp_server(name)? {
                        "removed"
                    } else {
                        "not found"
                    }
                );
                Ok(())
            }
            "tools" => {
                for tool in bridge.list_mcp_tools(args.get(1).map(String::as_str))? {
                    println!(
                        "{}\t{}",
                        tool.qualified_name,
                        tool.description.unwrap_or_default()
                    );
                }
                Ok(())
            }
            "call" => {
                let qualified = args.get(1).ok_or_else(|| {
                    anyhow::anyhow!("Usage: yeet mcp call server/tool [json-arguments]")
                })?;
                let (server, tool) = qualified
                    .split_once('/')
                    .ok_or_else(|| anyhow::anyhow!("MCP tool must be server/tool"))?;
                let arguments: Map<String, Value> =
                    serde_json::from_str(args.get(2).map(String::as_str).unwrap_or("{}"))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&bridge.call_mcp_tool(server, tool, &arguments)?)?
                );
                Ok(())
            }
            "resources" => {
                for resource in bridge.list_mcp_resources(args.get(1).map(String::as_str))? {
                    println!("{}\t{}\t{}", resource.server, resource.uri, resource.name);
                }
                Ok(())
            }
            "resource" => {
                if args.len() < 3 {
                    bail!("Usage: yeet mcp resource server uri");
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&bridge.read_mcp_resource(&args[1], &args[2])?)?
                );
                Ok(())
            }
            "prompts" => {
                for prompt in bridge.list_mcp_prompts(args.get(1).map(String::as_str))? {
                    println!(
                        "{}\t{}",
                        prompt.qualified_name,
                        prompt.description.unwrap_or_default()
                    );
                }
                Ok(())
            }
            "prompt" => {
                if args.len() < 3 {
                    bail!("Usage: yeet mcp prompt server name [json-string-arguments]");
                }
                let arguments: Map<String, Value> =
                    serde_json::from_str(args.get(3).map(String::as_str).unwrap_or("{}"))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &bridge.get_mcp_prompt(&args[1], &args[2], &arguments)?
                    )?
                );
                Ok(())
            }
            _ => bail!(
                "Usage: yeet mcp [list|add-stdio|add-http|remove|tools|call|resources|resource|prompts|prompt] ..."
            ),
        }
    })();
    bridge.shutdown();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn cache_event(
        run_id: &str,
        tool_round: u64,
        status: &str,
        input: u64,
        cached: Option<u64>,
        epoch: u64,
        reason: &str,
        breakpoint: &str,
    ) -> String {
        let ordinary = cached.map(|cached| input.saturating_sub(cached));
        json!({
            "type": "agent-model-attempt-finished",
            "runId": run_id,
            "payload": {
                "cacheDiagnostics": {
                    "cacheStatus": status,
                    "inputTokens": input,
                    "cachedInputTokens": cached,
                    "cacheWriteInputTokens": Value::Null,
                    "ordinaryInputTokens": ordinary,
                    "toolRound": tool_round.to_string(),
                    "expectedCacheReuses": "1",
                    "cacheEpoch": epoch,
                    "cacheEpochReason": reason,
                    "historyPrefixContinues": if reason == "window-start" { Value::Null } else { json!(true) },
                    "stablePrefixHash": "stable-prefix",
                    "toolSchemaHash": "tool-schema",
                    "cacheSurfaceHash": "cache-surface",
                    "breakpointPlanHash": breakpoint,
                    "promptCacheKeyHash": "prompt-key",
                    "contextKey": "context-key",
                    "contextWindowId": "window-1",
                    "estimatedCacheSurfaceTokens": 2048,
                }
            }
        })
        .to_string()
    }

    fn with_provider_cache_telemetry(
        event: String,
        cost_equivalent_input_tokens: u64,
        provider_miss: Option<(&str, u64, u64)>,
    ) -> String {
        let mut record: Value = serde_json::from_str(&event).unwrap();
        let diagnostics = &mut record["payload"]["cacheDiagnostics"];
        diagnostics["costEquivalentInputTokens"] = json!(cost_equivalent_input_tokens);
        if let Some((reason, missed_tokens, reusable_tokens)) = provider_miss {
            diagnostics["providerCacheDiagnosticType"] = json!("cache_miss");
            diagnostics["providerCacheMissReason"] = json!(reason);
            diagnostics["providerCacheMissedTokens"] = json!(missed_tokens);
            diagnostics["providerComparisonReusableTokens"] = json!(reusable_tokens);
            diagnostics["cacheMissAttribution"] = json!("provider-reported");
        }
        record.to_string()
    }

    fn tool_call_event(run_id: &str, name: &str, arguments: Value) -> String {
        json!({
            "type": "agent-tool-call",
            "runId": run_id,
            "payload": {"call": {"name": name, "arguments": arguments}}
        })
        .to_string()
    }

    fn tool_finished_event(run_id: &str, name: &str, result: Value) -> String {
        json!({
            "type": "agent-tool-finished",
            "runId": run_id,
            "payload": {
                "call": {"name": name, "arguments": {}},
                "result": result.to_string()
            }
        })
        .to_string()
    }

    #[test]
    fn cache_summary_reports_round_and_duplicate_tool_efficiency() {
        let mut root: Value = serde_json::from_str(&cache_event(
            "run-a",
            0,
            "miss",
            100,
            Some(0),
            1,
            "window-start",
            "bp-a",
        ))
        .unwrap();
        root["payload"]["cacheDiagnostics"]["modelVisibleToolResultChars"] = json!(0);
        root["payload"]["cacheDiagnostics"]["costEquivalentInputTokens"] = json!(100);
        let mut round_one: Value = serde_json::from_str(&cache_event(
            "run-a",
            1,
            "hit",
            160,
            Some(96),
            1,
            "stable",
            "bp-b",
        ))
        .unwrap();
        round_one["payload"]["cacheDiagnostics"]["modelVisibleToolResultChars"] = json!(1200);
        round_one["payload"]["cacheDiagnostics"]["costEquivalentInputTokens"] = json!(74);

        let content = [
            tool_call_event("run-a", "read_file", json!({"path":"src/lib.rs"})),
            tool_call_event("run-a", "read_file", json!({"path":"src/lib.rs"})),
            tool_call_event("run-a", "run_shell", json!({"command":"cargo check"})),
            tool_finished_event(
                "run-a",
                "read_file",
                json!({
                    "duplicate": true,
                    "contentAlreadyReturned": true,
                    "duplicateReadBytesAvoided": 4096
                }),
            ),
            root.to_string(),
            round_one.to_string(),
        ]
        .join("\n");

        let report = summarize_cache_events(&content);
        assert_eq!(report["turns"], 1);
        assert_eq!(report["attempts"], 2);
        assert_eq!(report["executedToolRounds"], 1);
        assert_eq!(report["totalToolCalls"], 3);
        assert_eq!(report["exactRepeatedToolCalls"], 1);
        assert_eq!(report["duplicateToolResults"], 1);
        assert_eq!(report["duplicateReadResults"], 1);
        assert_eq!(report["duplicateReadBytesAvoided"], 4096);
        assert_eq!(report["toolCallCounts"]["read_file"], 2);
        assert_eq!(report["toolCallCounts"]["run_shell"], 1);
        assert_eq!(
            report["maximumNewModelVisibleToolResultCharsPerRound"],
            1200
        );
        assert!((report["modelAttemptsPerTurn"].as_f64().unwrap() - 2.0).abs() < 1e-12);
        assert!((report["toolRoundsPerTurn"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        assert!((report["toolCallsPerRound"].as_f64().unwrap() - 3.0).abs() < 1e-12);
        assert!(
            (report["costEquivalentInputTokensPerTurn"].as_f64().unwrap() - 174.0).abs() < 1e-12
        );
    }

    #[test]
    fn cache_summary_separates_unreported_disabled_and_retries() {
        let content = [
            cache_event("run-a", 0, "miss", 100, Some(0), 1, "window-start", "bp-a"),
            cache_event("run-a", 1, "unreported", 200, None, 1, "stable", "bp-b"),
            cache_event("run-a", 1, "hit", 300, Some(240), 1, "stable", "bp-c"),
            cache_event(
                "run-b",
                0,
                "disabled",
                400,
                Some(0),
                1,
                "window-start",
                "bp-d",
            ),
        ]
        .join("\n");

        let report = summarize_cache_events(&content);
        assert_eq!(report["attempts"], 4);
        assert_eq!(report["turns"], 2);
        assert_eq!(report["toolRounds"], 3);
        assert_eq!(report["rootModelAttempts"], 2);
        assert_eq!(report["repeatedToolRoundAttempts"], 1);
        assert_eq!(report["measuredAttempts"], 2);
        assert_eq!(report["unreportedAttempts"], 1);
        assert_eq!(report["disabledAttempts"], 1);
        assert_eq!(report["hitAttempts"], 1);
        assert_eq!(report["missAttempts"], 1);
        assert_eq!(report["inputTokens"], 1000);
        assert_eq!(report["cacheMeasuredInputTokens"], 400);
        assert_eq!(report["cacheUnreportedInputTokens"], 200);
        assert_eq!(report["cacheDisabledInputTokens"], 400);
        assert_eq!(report["cachedInputTokens"], 240);
        assert_eq!(report["intraTurnBreakpointPlanChanges"], 2);
        assert_eq!(report["intraTurnCacheSurfaceChanges"], 0);
        assert_eq!(report["stableSurfaceMeasuredAttempts"], 1);
        assert_eq!(report["stableSurfaceHitAttempts"], 1);
        assert!((report["cacheHitRate"].as_f64().unwrap() - 0.6).abs() < 1e-12);
        assert!((report["rootCacheHitRate"].as_f64().unwrap() - 0.0).abs() < 1e-12);
        assert!((report["followupCacheHitRate"].as_f64().unwrap() - 0.8).abs() < 1e-12);
        assert!((report["cacheMeasurementCoverageRate"].as_f64().unwrap() - 0.4).abs() < 1e-12);
        assert!((report["cacheAttemptHitRate"].as_f64().unwrap() - 0.5).abs() < 1e-12);
        assert!((report["stableSurfaceAttemptHitRate"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        assert_eq!(report["stablePreviousRequestInputTokens"], 200);
        assert_eq!(report["stablePreviousRequestCachedInputTokens"], 200);
        assert!((report["stablePreviousRequestReuseRate"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        assert_eq!(report["stablePreviousRequestCurrentInputTokens"], 300);
        assert_eq!(report["stableNovelInputTokens"], 100);
        assert!(
            (report["stablePreviousRequestShareCeilingRate"]
                .as_f64()
                .unwrap()
                - (2.0 / 3.0))
                .abs()
                < 1e-12
        );
        assert!((report["stableNovelInputRate"].as_f64().unwrap() - (1.0 / 3.0)).abs() < 1e-12);
    }

    #[test]
    fn cache_summary_keeps_breakpoint_rotation_separate_from_stable_surface() {
        let content = [
            cache_event("run-a", 0, "miss", 100, Some(0), 1, "window-start", "bp-a"),
            cache_event("run-a", 1, "miss", 200, Some(0), 1, "stable", "bp-b"),
            cache_event("run-a", 2, "hit", 300, Some(240), 1, "stable", "bp-c"),
        ]
        .join("\n");

        let report = summarize_cache_events(&content);
        assert_eq!(report["intraTurnBreakpointPlanChanges"], 2);
        assert_eq!(report["intraTurnStablePrefixChanges"], 0);
        assert_eq!(report["intraTurnToolSchemaChanges"], 0);
        assert_eq!(report["intraTurnCacheSurfaceChanges"], 0);
        assert_eq!(report["intraTurnPromptCacheKeyChanges"], 0);
        assert_eq!(report["intraTurnContextKeyChanges"], 0);
        assert_eq!(report["intraTurnContextWindowChanges"], 0);
        assert_eq!(report["intraTurnCacheEpochChanges"], 0);
        assert_eq!(report["historyPrefixRewrites"], 0);
        assert_eq!(report["stableSurfaceMeasuredAttempts"], 2);
        assert_eq!(report["stableSurfaceHitAttempts"], 1);
        assert_eq!(report["stableSurfaceMissAttempts"], 1);
        assert_eq!(report["cacheResetMissAttempts"], 1);
        assert_eq!(report["missAttemptsByCacheEpochReason"]["stable"], 1);
        assert_eq!(report["missAttemptsByCacheEpochReason"]["window-start"], 1);
        assert!((report["stableSurfaceCacheHitRate"].as_f64().unwrap() - 0.48).abs() < 1e-12);
        assert!((report["stableSurfaceAttemptHitRate"].as_f64().unwrap() - 0.5).abs() < 1e-12);
        assert_eq!(report["stablePreviousRequestInputTokens"], 300);
        assert_eq!(report["stablePreviousRequestCachedInputTokens"], 200);
        assert!(
            (report["stablePreviousRequestReuseRate"].as_f64().unwrap() - (2.0 / 3.0)).abs()
                < 1e-12
        );
        assert_eq!(report["stablePreviousRequestCurrentInputTokens"], 500);
        assert_eq!(report["stableNovelInputTokens"], 200);
        assert!(
            (report["stablePreviousRequestShareCeilingRate"]
                .as_f64()
                .unwrap()
                - 0.6)
                .abs()
                < 1e-12
        );
        assert!((report["stableNovelInputRate"].as_f64().unwrap() - 0.4).abs() < 1e-12);
    }

    #[test]
    fn previous_request_reuse_excludes_cache_reset_attempts() {
        let content = [
            cache_event("run-a", 0, "miss", 100, Some(0), 1, "window-start", "bp-a"),
            cache_event(
                "run-a",
                1,
                "miss",
                150,
                Some(0),
                2,
                "tool-envelope-change",
                "bp-b",
            ),
            cache_event("run-a", 2, "hit", 300, Some(128), 2, "stable", "bp-c"),
        ]
        .join("\n");

        let report = summarize_cache_events(&content);
        assert_eq!(report["stablePreviousRequestInputTokens"], 150);
        assert_eq!(report["stablePreviousRequestCachedInputTokens"], 128);
        assert!(
            (report["stablePreviousRequestReuseRate"].as_f64().unwrap() - (128.0 / 150.0)).abs()
                < 1e-12
        );
        assert_eq!(report["stablePreviousRequestCurrentInputTokens"], 300);
        assert_eq!(report["stableNovelInputTokens"], 150);
        assert!(
            (report["stablePreviousRequestShareCeilingRate"]
                .as_f64()
                .unwrap()
                - 0.5)
                .abs()
                < 1e-12
        );
        assert!((report["stableNovelInputRate"].as_f64().unwrap() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn cache_economics_fixture_distinguishes_novel_tail_from_cache_failures() {
        let content = [
            cache_event("run-a", 0, "miss", 100, Some(0), 1, "window-start", "bp-a"),
            cache_event("run-a", 1, "hit", 200, Some(100), 1, "stable", "bp-b"),
            cache_event("run-a", 2, "hit", 400, Some(200), 1, "stable", "bp-c"),
            cache_event(
                "run-a",
                3,
                "miss",
                300,
                Some(0),
                2,
                "tool-envelope-change",
                "bp-d",
            ),
            cache_event("run-a", 4, "hit", 600, Some(256), 2, "stable", "bp-e"),
        ]
        .join("\n");

        let report = summarize_cache_events(&content);
        assert_eq!(report["attempts"], 5);
        assert_eq!(report["hitAttempts"], 3);
        assert_eq!(report["missAttempts"], 2);
        assert_eq!(report["cacheResetMissAttempts"], 2);
        assert_eq!(report["stableSurfaceMeasuredAttempts"], 3);
        assert_eq!(report["stableSurfaceHitAttempts"], 3);
        assert_eq!(report["stablePreviousRequestCurrentInputTokens"], 1200);
        assert_eq!(report["stablePreviousRequestInputTokens"], 600);
        assert_eq!(report["stableNovelInputTokens"], 600);
        assert!((report["cacheHitRate"].as_f64().unwrap() - (556.0 / 1600.0)).abs() < 1e-12);
        assert!((report["cacheAttemptHitRate"].as_f64().unwrap() - 0.6).abs() < 1e-12);
        assert!((report["stableSurfaceAttemptHitRate"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        assert!(
            (report["stablePreviousRequestReuseRate"].as_f64().unwrap() - (556.0 / 600.0)).abs()
                < 1e-12
        );
        assert!(
            (report["stablePreviousRequestShareCeilingRate"]
                .as_f64()
                .unwrap()
                - 0.5)
                .abs()
                < 1e-12
        );
        assert!((report["stableNovelInputRate"].as_f64().unwrap() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn cache_economics_replay_attributes_stable_provider_miss_without_reclassifying_it() {
        let content = [
            with_provider_cache_telemetry(
                cache_event(
                    "run-a",
                    0,
                    "miss",
                    1_000,
                    Some(0),
                    1,
                    "window-start",
                    "bp-a",
                ),
                1_000,
                None,
            ),
            with_provider_cache_telemetry(
                cache_event("run-a", 1, "hit", 1_200, Some(800), 1, "stable", "bp-b"),
                600,
                None,
            ),
            with_provider_cache_telemetry(
                cache_event("run-a", 2, "miss", 1_500, Some(0), 1, "stable", "bp-c"),
                1_500,
                Some(("input_changed", 700, 800)),
            ),
        ]
        .join("\n");

        let report = summarize_cache_events(&content);
        assert_eq!(report["attempts"], 3);
        assert_eq!(report["hitAttempts"], 1);
        assert_eq!(report["missAttempts"], 2);
        assert_eq!(report["stableSurfaceMissAttempts"], 1);
        assert_eq!(report["stableProviderReportedMissAttempts"], 1);
        assert_eq!(report["stableProviderUnattributedMissAttempts"], 0);
        assert_eq!(report["providerCacheDiagnosticAttempts"], 1);
        assert_eq!(report["providerCacheMissAttempts"], 1);
        assert_eq!(report["providerCacheMissReasons"]["input_changed"], 1);
        assert_eq!(report["providerCacheMissedTokens"], 700);
        assert_eq!(report["providerComparisonReusableTokens"], 800);
        assert_eq!(report["costEquivalentInputTokens"], 3_100);
        assert!(
            (report["costEquivalentInputRate"].as_f64().unwrap() - (3_100.0 / 3_700.0)).abs()
                < 1e-12
        );
        assert_eq!(report["stablePreviousRequestInputTokens"], 2_200);
        assert_eq!(report["stablePreviousRequestCachedInputTokens"], 800);
        assert_eq!(report["stablePreviousRequestCurrentInputTokens"], 2_700);
        assert_eq!(report["stableNovelInputTokens"], 500);
        assert!(
            (report["stablePreviousRequestReuseRate"].as_f64().unwrap() - (800.0 / 2_200.0)).abs()
                < 1e-12
        );
        assert!(
            (report["stablePreviousRequestShareCeilingRate"]
                .as_f64()
                .unwrap()
                - (2_200.0 / 2_700.0))
                .abs()
                < 1e-12
        );
        assert!((report["cacheHitRate"].as_f64().unwrap() - (800.0 / 3_700.0)).abs() < 1e-12);
    }

    #[test]
    fn debate_cache_summary_reads_response_usage() {
        let content = [
            json!({
                "type":"debate-research","runId":"run-a",
                "payload":{"event":"response","stage":0,"pro":true,"detail":{"round":0,"response":{"usage":{
                    "inputTokens":100,"cacheMeasuredInputTokens":100,"cachedInputTokens":0,
                    "cacheWriteInputTokens":0,"costEquivalentInputTokens":100
                }}}}
            }).to_string(),
            json!({
                "type":"debate-research","runId":"run-a",
                "payload":{"event":"response","stage":0,"pro":true,"detail":{"round":1,"response":{"usage":{
                    "inputTokens":200,"cacheMeasuredInputTokens":200,"cachedInputTokens":80,
                    "cacheWriteInputTokens":0,"costEquivalentInputTokens":128
                }}}}
            }).to_string(),
        ].join("\n");
        let report = summarize_debate_cache_events(&content);
        assert_eq!(report["turns"], 1);
        assert_eq!(report["attempts"], 2);
        assert_eq!(report["measuredAttempts"], 2);
        assert_eq!(report["hitAttempts"], 1);
        assert_eq!(report["missAttempts"], 1);
        assert_eq!(report["inputTokens"], 300);
        assert_eq!(report["cachedInputTokens"], 80);
        assert_eq!(report["rootMissAttempts"], 1);
        assert_eq!(report["followupHitAttempts"], 1);
        assert_eq!(report["telemetrySource"], "debates/events.jsonl");
        assert!((report["cacheHitRate"].as_f64().unwrap() - (80.0 / 300.0)).abs() < 1e-12);
    }
}
