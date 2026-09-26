use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{Read, Write},
};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

mod cache_report;
use cache_report::cache;

use crate::{
    config::{ConfigStore, parse_context_length, validate_model_id},
    core::{
        BridgeClient, CallRequest, ImageAttachment, McpServerConfiguration, Message,
        OpenAiCompatibleProvider, StreamEvent, node_executable, runtime_directory,
    },
    harness::Harness,
    model::{ConversationKind, FrontendCommand, SandboxAction},
    project_settings::{ProjectSettingsStore, ServiceBackend},
    sandbox_cli,
    session_store::SessionStore,
    web_search,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const HELP: &str = r#"Usage:
  yeet remote [--background] [--bind ADDRESS] [--size COLSxROWS] [--origin URL]
  yeet remote status
  yeet remote stop
  yeet remote auth status
  yeet remote auth key generate|set|clear
  yeet remote auth passkey add|clear
  yeet doctor
  yeet install [binary|runtime|foundation|web|mcp|remote|all]...
  yeet uninstall [binary|runtime|foundation|web|mcp|remote|all]...
  yeet install status
  yeet update [check]
  yeet model get
  yeet model set provider/model
  yeet model context [get [provider/model]|set [provider/model] length|auto [provider/model]]
  yeet theme get
  yeet theme list
  yeet theme set [dark|light|both] NAME_OR_PATH
  yeet theme appearance [auto|dark|light]
  yeet cache [latest|SESSION_ID]
  yeet run [--model provider/model] [--image path] [prompt]
  yeet agent [--model provider/model] [--reasoning LEVEL] [--benchmark] [--json] [--timeout-seconds N] [prompt]
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
  yeet memory backend [builtin|mcp [SERVER]]
  yeet sandbox [show|path|reset|scratch|workspace|network|env|secret|limits] ...
  yeet search [backend [builtin|mcp [SERVER]]|install [all|searxng|agent-reach]|status|path|remove [all|searxng|agent-reach]]

State:
  YEET_CONFIG_DIR overrides the per-user state directory.
  Existing ~/.yeet installs are preserved on every OS.
  Clean Linux installs use XDG config paths; clean Windows installs use AppData.
  Project state remains under ./.yeet/.

Runtime:
  Yeet is Rust-native. The TypeScript provider/MCP runtime is shipped separately
  and requires Node.js 20+. Set YEET_RUNTIME_DIR and YEET_NODE to override paths.

Remote:
  Remote mode is opt-in, workspace-neutral, and runs in the current Yeet process.
  Add --background to detach one Remote process without installing a supervisor or
  restart policy. It serves the semantic WebUI on 0.0.0.0:7331 by default. Each
  browser client restores or selects its own workspace; a new client falls back to
  the home directory. Client backends stay in-process, while provider/edit/search
  sidecars start only when needed. Access keys and WebAuthn passkeys are optional.
  Port forwarding and tunneling are user-managed."#;

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
        "install" | "uninstall" => {
            let output = crate::install::run_cli(rest, command == "uninstall")?;
            if !output.is_empty() {
                println!("{output}");
            }
            Ok(())
        }
        "update" => crate::update::run(rest),
        "model" => model(rest),
        "theme" => theme(rest),
        "cache" => cache(rest),
        "run" => run_model(rest),
        "agent" => run_agent(rest),
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
    let sub = args.first().map(String::as_str).unwrap_or("status");
    match sub {
        "status" => {
            let project = settings.load()?;
            let bridge = BridgeClient::start()?;
            let mut status = match project.foundation_memory.backend {
                ServiceBackend::Builtin => crate::foundation_backend::builtin_status(&bridge)?,
                ServiceBackend::Mcp => {
                    let result = bridge.list_mcp_tools(Some(&project.foundation_memory.server));
                    let connected = result.is_ok();
                    let tools = result.unwrap_or_default();
                    let project_scoped_memory =
                        crate::foundation_backend::has_required_project_scoped_tools(&tools);
                    let visible =
                        crate::foundation_backend::project_scoped_model_tool_names(&tools);
                    json!({
                        "storage":"external-mcp",
                        "connected":connected,
                        "projectScopedMemory":project_scoped_memory,
                        "availableToolCount":tools.len(),
                        "tools":visible,
                    })
                }
            };
            bridge.shutdown();
            status["enabled"] = json!(project.foundation_memory.enabled);
            status["backend"] = json!(project.foundation_memory.backend.as_str());
            status["server"] = json!(project.foundation_memory.server);
            status["project"] = json!(settings.project_identity()?);
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        "backend" => {
            let project = settings.load()?;
            match args.get(1).map(String::as_str) {
                None => println!(
                    "{}{}",
                    project.foundation_memory.backend.as_str(),
                    if project.foundation_memory.backend == ServiceBackend::Mcp {
                        format!(" {}", project.foundation_memory.server)
                    } else {
                        String::new()
                    }
                ),
                Some(value) => {
                    let backend = ServiceBackend::parse(value)?;
                    let server = args.get(2).map(String::as_str);
                    if args.len() > 3 {
                        bail!("Usage: yeet memory backend [builtin|mcp [SERVER]]");
                    }
                    settings.save_foundation_backend(backend, server)?;
                    let saved = settings.load()?;
                    println!(
                        "memory backend={}{}",
                        saved.foundation_memory.backend.as_str(),
                        if saved.foundation_memory.backend == ServiceBackend::Mcp {
                            format!(" server={}", saved.foundation_memory.server)
                        } else {
                            String::new()
                        }
                    );
                }
            }
        }
        "on" | "off" => {
            if args.len() > 1 {
                bail!("Usage: yeet memory on|off");
            }
            if sub == "on" {
                let output = crate::install::run_cli(&["foundation".into()], false)?;
                if !output.is_empty() {
                    println!("{output}");
                }
            } else {
                settings.save_foundation_memory(false, None)?;
                println!("disabled");
            }
        }
        "recall" => {
            let query = args
                .get(1)
                .ok_or_else(|| anyhow!("Usage: yeet memory recall <query>"))?;
            if args.len() > 2 {
                bail!("Usage: yeet memory recall <query>");
            }
            let project = settings.load()?;
            let bridge = BridgeClient::start()?;
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let result = (|| -> Result<Value> {
                let server = crate::foundation_backend::server_for_backend(
                    &bridge,
                    project.foundation_memory.backend,
                    &project.foundation_memory.server,
                )?;
                let tools = bridge.list_mcp_tools(Some(&server))?;
                crate::foundation_backend::validate_project_scoped_tools(
                    &tools,
                    &format!("Foundation backend {server}"),
                )?;
                let arguments = Map::from_iter([
                    ("project".into(), json!(settings.project_identity()?)),
                    ("query".into(), json!(query)),
                    ("limit".into(), json!(8)),
                ]);
                let result = bridge.call_mcp_tool_cancellable(
                    &server,
                    "memory_recall",
                    &arguments,
                    &cancel,
                )?;
                if result.get("isError").and_then(Value::as_bool) == Some(true) {
                    bail!("Foundation {server}/memory_recall returned an error: {result}");
                }
                Ok(result)
            })();
            bridge.shutdown();
            println!("{}", serde_json::to_string_pretty(&result?)?);
        }
        "models" | "model" | "reindex" | "import-foundation" | "export-foundation" => {
            bail!(
                "yeet memory {sub} belonged to the retired native Rust memory store; builtin now runs the managed Foundation backend. Use Foundation's admin/configuration tools instead."
            )
        }
        _ => bail!(
            "Usage: yeet memory [status|on|off|backend [builtin|mcp [SERVER]]|recall <query>]"
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

fn theme(args: &[String]) -> Result<()> {
    let config = ConfigStore::default();
    match args.first().map(String::as_str).unwrap_or("get") {
        "get" => {
            let settings = config.theme_settings()?;
            println!(
                "appearance: {}",
                settings.appearance.as_deref().unwrap_or("auto")
            );
            println!("dark: {}", settings.dark.as_deref().unwrap_or("kanagawa"));
            println!("light: {}", settings.light.as_deref().unwrap_or("adwaita"));
            Ok(())
        }
        "list" => {
            for theme in crate::theme::catalog() {
                println!("{}\t{}\t{}", theme.appearance, theme.id, theme.label);
            }
            Ok(())
        }
        "set" => {
            let (mode, value) = match args {
                [_, value] => ("both", value.as_str()),
                [_, mode, value] => (mode.as_str(), value.as_str()),
                _ => bail!("Usage: yeet theme set [dark|light|both] NAME_OR_PATH"),
            };
            let value = if mode != "both" && (value == "dark" || value == "light") {
                bail!("Usage: yeet theme set [dark|light|both] NAME_OR_PATH")
            } else {
                value
            };
            config.set_theme(mode, value)?;
            println!("{mode}: {value}");
            Ok(())
        }
        "appearance" => {
            let appearance = args.get(1).map(String::as_str).unwrap_or("auto");
            config.set_appearance(appearance)?;
            println!("{appearance}");
            Ok(())
        }
        _ => bail!(
            "Usage: yeet theme [get|list|set [dark|light|both] NAME_OR_PATH|appearance [auto|dark|light]]"
        ),
    }
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

fn run_agent(args: &[String]) -> Result<()> {
    let mut model = None;
    let mut reasoning = None;
    let mut benchmark = false;
    let mut json_output = false;
    let mut timeout_seconds = std::env::var("YEET_AGENT_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(3600);
    let mut prompt_parts = Vec::new();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--model" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| anyhow!("--model requires provider/model"))?;
                model = Some(validate_model_id(value)?);
                index += 2;
            }
            "--reasoning" => {
                reasoning = Some(
                    args.get(index + 1)
                        .ok_or_else(|| anyhow!("--reasoning requires a level"))?
                        .clone(),
                );
                index += 2;
            }
            "--benchmark" => {
                benchmark = true;
                index += 1;
            }
            "--json" => {
                json_output = true;
                index += 1;
            }
            "--timeout-seconds" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| anyhow!("--timeout-seconds requires an integer"))?;
                timeout_seconds = value
                    .parse::<u64>()
                    .map_err(|_| anyhow!("invalid --timeout-seconds value: {value}"))?;
                index += 2;
            }
            "--" => {
                prompt_parts.extend(args[index + 1..].iter().cloned());
                break;
            }
            option if option.starts_with("--") => {
                bail!("Unknown yeet agent option: {option}");
            }
            _ => {
                prompt_parts.push(args[index].clone());
                index += 1;
            }
        }
    }

    let prompt = if prompt_parts.is_empty() {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        text.trim().to_owned()
    } else {
        prompt_parts.join(" ")
    };
    if prompt.is_empty() {
        bail!(
            "Usage: yeet agent [--model provider/model] [--reasoning LEVEL] [--benchmark] [--json] [--timeout-seconds N] prompt"
        );
    }

    let mut harness = Harness::embedded(std::env::current_dir()?)?;
    harness.send(FrontendCommand::NewSession)?;
    if let Some(model) = model {
        harness.send(FrontendCommand::SelectModel { model })?;
    }
    if let Some(level) = reasoning {
        harness.send(FrontendCommand::SelectReasoning { level })?;
    }
    if benchmark {
        harness.send(FrontendCommand::SetGoal { enabled: false })?;
        harness.send(FrontendCommand::UpdateSandbox {
            action: SandboxAction::SetExecutionMode {
                mode: "unlimited".into(),
            },
        })?;
        harness.send(FrontendCommand::UpdateSandbox {
            action: SandboxAction::SetAutoApprove { enabled: true },
        })?;
    }
    harness.send(FrontendCommand::Submit {
        text: prompt,
        images: Vec::new(),
        attachment_ids: Vec::new(),
    })?;

    let started_at = std::time::Instant::now();
    let mut saw_streaming = false;
    let mut transport_error = None;

    let state = loop {
        if let Some(envelope) = harness.try_recv() {
            if let Some(message) = envelope.message
                && envelope.kind == "error"
            {
                transport_error = Some(message);
            }
            if let Some(state) = envelope.state {
                if state.is_streaming {
                    saw_streaming = true;
                }

                if !benchmark
                    && (state.pending_shell_permission.is_some()
                        || state.pending_native_app_permission.is_some())
                {
                    let _ = harness.send(FrontendCommand::Interrupt);
                    bail!(
                        "Headless agent is waiting for an interactive permission. Configure sandbox auto-approval or use --benchmark for an isolated benchmark container."
                    );
                }

                if saw_streaming && !state.is_streaming {
                    break state;
                }
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        if timeout_seconds > 0
            && started_at.elapsed() >= std::time::Duration::from_secs(timeout_seconds)
        {
            let _ = harness.send(FrontendCommand::Interrupt);
            bail!("Headless agent timed out after {timeout_seconds} seconds");
        }

        if !saw_streaming
            && started_at.elapsed() >= std::time::Duration::from_secs(10)
            && let Some(error) = transport_error.take()
        {
            bail!("Headless agent failed to start: {error}");
        }
    };
    let response = state
        .conversation
        .as_ref()
        .and_then(|conversation| {
            conversation
                .iter()
                .rev()
                .find_map(|entry| match &entry.kind {
                    ConversationKind::Assistant { content, .. } if !content.trim().is_empty() => {
                        Some(content.clone())
                    }
                    _ => None,
                })
        })
        .or_else(|| {
            (!state.active_assistant_text.trim().is_empty())
                .then(|| state.active_assistant_text.clone())
        })
        .unwrap_or_default();

    if json_output {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "response": response,
                "model": state.active_model,
                "reasoning": state.active_reasoning_level,
                "sessionId": state.current_session_id,
                "tokenUsage": state.token_usage,
                "creditUsage": state.credit_usage,
                "error": state.error_message,
            }))?
        );
    } else if !response.is_empty() {
        println!("{response}");
    }

    if let Some(error) = state.error_message
        && !error.trim().is_empty()
    {
        bail!("{error}");
    }
    if let Some(error) = transport_error {
        bail!("{error}");
    }
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
                    Some(json!({
                        "projectId": std::env::var("GEMINI_PROJECT_ID").ok()
                            .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT").ok())
                            .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT_ID").ok())
                    }))
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
