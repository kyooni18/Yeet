use std::{
    collections::HashMap,
    io::{Read, Write},
};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{
    config::{ConfigStore, parse_context_length, validate_model_id},
    core::{
        BridgeClient, CallRequest, ImageAttachment, McpServerConfiguration, Message,
        OpenAiCompatibleProvider, StreamEvent, node_executable, runtime_directory,
    },
    project_settings::ProjectSettingsStore,
    sandbox_cli, web_search,
};

pub const VERSION: &str = "0.1.0";

pub const HELP: &str = r#"yeet 0.1.0

Usage:
  yeet remote [WORKSPACE] [--workspace PATH] [--bind ADDRESS] [--size COLSxROWS] [--origin URL]
  yeet remote status [WORKSPACE|--workspace PATH]
  yeet remote stop [WORKSPACE|--workspace PATH]
  yeet remote auth status [WORKSPACE|--workspace PATH]
  yeet remote auth key generate|set|clear [WORKSPACE|--workspace PATH]
  yeet remote auth passkey add|clear [WORKSPACE|--workspace PATH]
  yeet doctor
  yeet model get
  yeet model set provider/model
  yeet model context [get [provider/model]|set [provider/model] length|auto [provider/model]]
  yeet run [--model provider/model] [--image path] [prompt]
  yeet auth status [provider]
  printf key | yeet auth set-key provider
  yeet auth login provider
  yeet auth logout provider
  yeet provider list
  yeet provider add id base-url [--require-api-key] [--header Name=Value]
  yeet provider remove id
  yeet skill list
  yeet skill show name
  yeet skill read name path
  yeet skill validate path
  yeet skill install path|git-url|zip
  yeet skill remove name
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
  yeet memory status
  yeet memory recall <query>
  yeet memory on|off
  yeet memory models
  yeet memory model [provider/model]
  yeet memory reindex [provider/model]
  yeet memory import-foundation <export.jsonl>
  yeet sandbox [show|path|reset|scratch|workspace|network|env|secret|limits] ...
  yeet search [install [all|searxng|agent-reach]|status|path|remove [all|searxng|agent-reach]]

State:
  ~/.yeet/config.json
  ~/.yeet/credentials.json
  ~/.yeet/mcp.json
  ~/.yeet/skills/
  ~/.yeet/remote/
  ~/.yeet/search/
  ./.yeet/settings.json
  ./.yeet/sandbox.json

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
            println!("{HELP}");
            Ok(())
        }
        "-v" | "--version" | "version" => {
            println!("{VERSION}");
            Ok(())
        }
        "doctor" => doctor(),
        "model" => model(rest),
        "run" => run_model(rest),
        "auth" => auth(rest),
        "provider" | "providers" | "endpoint" | "endpoints" => provider(rest),
        "skill" | "skills" => skill(rest),
        "mcp" => mcp(rest),
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
            "Usage: yeet memory [status|on|off|recall <query>|models|model [provider/model]|reindex [provider/model]|import-foundation <export.jsonl>]"
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
    println!("yeet {VERSION}");
    println!("node: {}", node.display());
    println!("runtime: {}", runtime.display());
    println!("config: {}", config.directory.display());
    println!("providers: {}", providers.join(", "));
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
                let options = if provider == "gemini" {
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
