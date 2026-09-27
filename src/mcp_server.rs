use std::{
    collections::HashMap,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[cfg(unix)]
use std::sync::mpsc;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    config::ConfigStore,
    core::{BridgeEvent, ToolCall, ToolDefinition},
    permission::PermissionBroker,
    project_settings::ProjectSettingsStore,
    session_store::SessionStore,
    tools::{BridgeHandle, ToolRegistry, direct_mcp_tool_definitions},
    workers::WorkerRegistry,
};

mod auth;
mod daemon;
mod http;
mod trace;

pub fn run_cli(args: &[String]) -> Result<String> {
    daemon::run_cli(args)
}

const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
const LEGACY_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

fn protocol_version_supported(version: &str) -> bool {
    version == MODERN_PROTOCOL_VERSION || LEGACY_PROTOCOL_VERSIONS.contains(&version)
}

fn supported_protocol_versions() -> Vec<&'static str> {
    std::iter::once(MODERN_PROTOCOL_VERSION)
        .chain(LEGACY_PROTOCOL_VERSIONS.iter().copied())
        .collect()
}
const TOOL_LIST_TTL_MS: u64 = 300_000;
const MAX_MCP_JSON_NESTING: usize = 64;
#[cfg(unix)]
const STDIO_ORPHAN_STARTUP_GRACE: Duration = Duration::from_secs(3);

pub fn serve_stdio(args: &[String]) -> Result<()> {
    let launch_workspace = current_workspace()?;
    let (workspace, restrict_workspace) = parse_workspace_options(args, &launch_workspace)?;
    let default_workspace = workspace.unwrap_or_else(|| launch_workspace.clone());
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut server =
        McpServer::new_with_workspace_restriction(default_workspace, restrict_workspace);
    let result = serve_stdio_guarded(&mut server, stdin, stdout);
    if result.as_ref().is_err_and(is_stdio_disconnect) {
        return Ok(());
    }
    result
}

#[cfg(unix)]
fn serve_stdio_guarded(
    server: &mut McpServer,
    stdin: std::io::Stdin,
    stdout: std::io::Stdout,
) -> Result<()> {
    enum ReadEvent {
        Line(String),
        Eof,
        Error(std::io::Error),
    }

    let launch_parent = unsafe { libc::getppid() };
    let started = std::time::Instant::now();
    let mut saw_input = false;
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("yeet-mcp-stdio-reader".into())
        .spawn(move || {
            let mut reader = stdin.lock();
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        let _ = sender.send(ReadEvent::Eof);
                        return;
                    }
                    Ok(_) => {
                        if sender.send(ReadEvent::Line(line)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(ReadEvent::Error(error));
                        return;
                    }
                }
            }
        })?;

    let mut writer = stdout.lock();
    loop {
        match receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(ReadEvent::Line(line)) => {
                saw_input = true;
                handle_io_line(server, &mut writer, &line)?;
            }
            Ok(ReadEvent::Eof) => return Ok(()),
            Ok(ReadEvent::Error(error)) => return Err(error.into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let current_parent = unsafe { libc::getppid() };
                if stdio_parent_is_gone(launch_parent, current_parent, saw_input, started.elapsed())
                {
                    return Ok(());
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

#[cfg(unix)]
fn stdio_parent_is_gone(
    launch_parent: libc::pid_t,
    current_parent: libc::pid_t,
    saw_input: bool,
    elapsed: Duration,
) -> bool {
    (launch_parent > 1 && current_parent != launch_parent)
        || (launch_parent == 1 && !saw_input && elapsed >= STDIO_ORPHAN_STARTUP_GRACE)
}

#[cfg(not(unix))]
fn serve_stdio_guarded(
    server: &mut McpServer,
    stdin: std::io::Stdin,
    stdout: std::io::Stdout,
) -> Result<()> {
    serve_io(server, stdin.lock(), stdout.lock())
}

pub fn print_stdio_config(args: &[String]) -> Result<()> {
    let launch_workspace = current_workspace()?;
    let (workspace, restrict_workspace) = parse_workspace_options(args, &launch_workspace)?;
    let executable = std::env::current_exe().context("locate Yeet executable")?;
    let mut command_args = vec![json!("mcpserver"), json!("stdio")];
    if let Some(workspace) = workspace {
        command_args.push(json!("--workspace"));
        command_args.push(json!(workspace));
    }
    if restrict_workspace {
        command_args.push(json!("--restrict-workspace"));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "command": executable,
            "args": command_args,
        }))?
    );
    Ok(())
}

pub(super) fn current_workspace() -> Result<PathBuf> {
    let path = std::env::current_dir()?;
    path.canonicalize()
        .with_context(|| format!("resolve current workspace {}", path.display()))
}

fn parse_workspace_options(args: &[String], base: &Path) -> Result<(Option<PathBuf>, bool)> {
    let mut workspace = None;
    let mut restrict_workspace = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--workspace" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| anyhow!("--workspace requires a path"))?;
                if workspace.is_some() {
                    bail!("workspace was specified more than once");
                }
                workspace = Some(resolve_workspace_path(base, value)?);
            }
            "--restrict-workspace" => {
                if restrict_workspace {
                    bail!("--restrict-workspace was specified more than once");
                }
                restrict_workspace = true;
            }
            value if !value.starts_with('-') && workspace.is_none() => {
                workspace = Some(resolve_workspace_path(base, value)?);
            }
            value => bail!("unknown MCP server option: {value}"),
        }
        index += 1;
    }
    Ok((workspace, restrict_workspace))
}

pub(super) fn resolve_workspace_path(base: &Path, value: &str) -> Result<PathBuf> {
    let path = match value {
        "~" => dirs::home_dir().ok_or_else(|| anyhow!("home directory is unavailable"))?,
        value if value.starts_with("~/") || value.starts_with("~\\") => {
            let home = dirs::home_dir().ok_or_else(|| anyhow!("home directory is unavailable"))?;
            home.join(&value[2..])
        }
        value => PathBuf::from(value),
    };
    let path = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    let path = path
        .canonicalize()
        .with_context(|| format!("resolve MCP workspace {}", path.display()))?;
    if !path.is_dir() {
        bail!("MCP workspace is not a directory: {}", path.display());
    }
    Ok(path)
}

#[cfg(any(test, not(unix)))]
#[allow(dead_code)] // Used by non-Unix stdio serving; Unix test builds compile it without calling it.
fn serve_io<R: BufRead, W: Write>(
    server: &mut McpServer,
    mut reader: R,
    mut writer: W,
) -> Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        handle_io_line(server, &mut writer, &line)?;
    }
}

fn handle_io_line(server: &mut McpServer, writer: &mut impl Write, line: &str) -> Result<()> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let request = match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => value,
        Err(error) => {
            write_json(
                writer,
                &jsonrpc_error(Value::Null, -32700, format!("Parse error: {error}")),
            )?;
            return Ok(());
        }
    };
    if !mcp_json_nesting_within_limit(&request) {
        let id = request
            .as_object()
            .and_then(|object| object.get("id"))
            .cloned()
            .unwrap_or(Value::Null);
        write_json(
            writer,
            &jsonrpc_error(
                id,
                -32600,
                format!("MCP request nesting exceeds {MAX_MCP_JSON_NESTING} levels"),
            ),
        )?;
        return Ok(());
    }
    if let Some(response) = server.handle_payload(request) {
        write_json(writer, &response)?;
    }
    Ok(())
}

pub(super) fn mcp_json_nesting_within_limit(value: &Value) -> bool {
    let mut pending = vec![(value, 0_usize)];
    while let Some((value, depth)) = pending.pop() {
        match value {
            Value::Array(items) => {
                if depth >= MAX_MCP_JSON_NESTING && !items.is_empty() {
                    return false;
                }
                pending.extend(items.iter().map(|item| (item, depth + 1)));
            }
            Value::Object(object) => {
                if depth >= MAX_MCP_JSON_NESTING && !object.is_empty() {
                    return false;
                }
                pending.extend(object.values().map(|item| (item, depth + 1)));
            }
            _ => {}
        }
    }
    true
}

fn is_stdio_disconnect(error: &anyhow::Error) -> bool {
    fn is_disconnect_kind(kind: std::io::ErrorKind) -> bool {
        matches!(
            kind,
            std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
        )
    }

    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| is_disconnect_kind(io.kind()))
            || cause
                .downcast_ref::<serde_json::Error>()
                .and_then(serde_json::Error::io_error_kind)
                .is_some_and(is_disconnect_kind)
    })
}

fn write_json(writer: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

struct WorkspaceRuntime {
    registry: ToolRegistry,
    bridge: BridgeHandle,
    config: ConfigStore,
    project_settings: ProjectSettingsStore,
    project_identity: String,
    permission_was_denied: Arc<AtomicBool>,
}

impl WorkspaceRuntime {
    fn new(
        workspace: PathBuf,
        bridge: BridgeHandle,
        hard_access_root: Option<PathBuf>,
    ) -> Result<Self> {
        let config = ConfigStore::default();
        config.ensure()?;
        let project_settings = ProjectSettingsStore::new(&workspace)?;
        project_settings.ensure()?;
        let project_identity = project_settings.project_identity()?;
        let project = project_settings.load()?;

        let permission = PermissionBroker::default();
        let permission_for_notify = permission.clone();
        let permission_was_denied = Arc::new(AtomicBool::new(false));
        let denied_for_notify = permission_was_denied.clone();
        permission.set_notifier(Arc::new(move || {
            if permission_for_notify.pending_shell().is_some() {
                denied_for_notify.store(true, Ordering::Release);
                let _ = permission_for_notify.resolve_shell(false);
            }
            if permission_for_notify.pending_native_app().is_some() {
                denied_for_notify.store(true, Ordering::Release);
                let _ = permission_for_notify.resolve_native_app(false);
            }
        }));

        let workers = WorkerRegistry::new(Vec::new())?;
        let mut registry =
            ToolRegistry::new_with_bridge_handle(bridge.clone(), workspace, workers, permission)?;
        registry.set_hard_access_root(hard_access_root)?;
        registry.set_artifacts_enabled(false);
        registry.set_disabled_capabilities(project.capabilities.disabled.clone());
        registry.configure_foundation_memory(
            project.foundation_memory.enabled,
            project.foundation_memory.backend,
            project.foundation_memory.server.clone(),
            project_identity.clone(),
        );
        registry.configure_web_backend(project.web.backend, project.web.server.clone());
        let sessions = SessionStore::new(&config.directory);
        sessions.prepare()?;
        registry.set_session_runtime(sessions, None);

        Ok(Self {
            registry,
            bridge,
            config,
            project_settings,
            project_identity,
            permission_was_denied,
        })
    }

    fn refresh_project_settings(&mut self) -> Result<()> {
        let project = self.project_settings.load()?;
        self.registry
            .set_disabled_capabilities(project.capabilities.disabled);
        self.registry.configure_foundation_memory(
            project.foundation_memory.enabled,
            project.foundation_memory.backend,
            project.foundation_memory.server,
            self.project_identity.clone(),
        );
        self.registry
            .configure_web_backend(project.web.backend, project.web.server);
        Ok(())
    }

    fn execute(&mut self, name: &str, arguments: Map<String, Value>) -> Result<String> {
        self.refresh_project_settings()?;
        self.permission_was_denied.store(false, Ordering::Release);
        let model = self.config.model()?.unwrap_or_default();
        let cancel = AtomicBool::new(false);
        let call = ToolCall {
            id: Uuid::new_v4().to_string(),
            name: name.to_owned(),
            arguments: Value::Object(arguments),
        };
        let stop_bridge_events = Arc::new(AtomicBool::new(false));
        let bridge_event_worker = if matches!(name, "computer_use" | "computer_use_reset") {
            let bridge = self.bridge.client()?;
            let stop = stop_bridge_events.clone();
            Some(thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    match bridge.try_recv_event() {
                        Some(BridgeEvent::NativeAppApprovalRequest(request)) => {
                            // A direct MCP computer_use invocation is itself the
                            // authorization boundary for native UI access. The
                            // headless MCP server cannot present Yeet's TUI
                            // approval dialog, so approve only native-app
                            // requests emitted while an explicit computer_use
                            // tool call is active. Unsolicited bridge requests
                            // are never serviced by this worker.
                            let _ =
                                bridge.send_native_app_approval_decision(&request.request_id, true);
                        }
                        Some(BridgeEvent::Closed) => break,
                        None => thread::sleep(Duration::from_millis(10)),
                    }
                }
            }))
        } else {
            None
        };
        // Direct MCP has no reliable knowledge of the client's current
        // model-visible context. Never carry duplicate-suppression coverage across
        // independent MCP calls; dependent safety/source state is retained separately.
        self.registry.reset_direct_mcp_visibility();
        let result = self.registry.execute(&call, &model, &cancel);
        stop_bridge_events.store(true, Ordering::Release);
        if let Some(worker) = bridge_event_worker {
            let _ = worker.join();
        }
        match result {
            Ok(output) => Ok(output),
            Err(error) if self.permission_was_denied.load(Ordering::Acquire) => bail!(
                "Yeet direct MCP tools cannot open an interactive approval prompt. Allow this operation through the workspace sandbox policy (autoApprove or unlimited) and retry. Original error: {error}"
            ),
            Err(error) => Err(error),
        }
    }
}

pub(super) struct McpServer {
    default_workspace: PathBuf,
    restrict_workspace: bool,
    bridge: BridgeHandle,
    workspaces: HashMap<PathBuf, WorkspaceRuntime>,
}

impl McpServer {
    #[cfg(test)]
    pub(super) fn new(default_workspace: PathBuf) -> Self {
        Self::new_with_workspace_restriction(default_workspace, false)
    }

    pub(super) fn new_with_workspace_restriction(
        default_workspace: PathBuf,
        restrict_workspace: bool,
    ) -> Self {
        Self {
            default_workspace,
            restrict_workspace,
            bridge: BridgeHandle::lazy(),
            workspaces: HashMap::new(),
        }
    }

    pub(super) fn handle_payload(&mut self, request: Value) -> Option<Value> {
        if let Value::Array(items) = request {
            let responses = items
                .into_iter()
                .filter_map(|item| self.handle(item))
                .collect::<Vec<_>>();
            return (!responses.is_empty()).then_some(Value::Array(responses));
        }
        self.handle(request)
    }

    fn runtime(&mut self, workspace: PathBuf) -> Result<&mut WorkspaceRuntime> {
        if !self.workspaces.contains_key(&workspace) {
            let hard_access_root = self
                .restrict_workspace
                .then(|| self.default_workspace.clone());
            let runtime =
                WorkspaceRuntime::new(workspace.clone(), self.bridge.clone(), hard_access_root)?;
            self.workspaces.insert(workspace.clone(), runtime);
        }
        Ok(self
            .workspaces
            .get_mut(&workspace)
            .expect("workspace runtime initialized"))
    }

    pub(super) fn handle(&mut self, request: Value) -> Option<Value> {
        let object = match request.as_object() {
            Some(value) if value.get("jsonrpc").and_then(Value::as_str) == Some("2.0") => value,
            _ => return Some(jsonrpc_error(Value::Null, -32600, "Invalid Request")),
        };
        let id = object.get("id").cloned();
        let method = match object.get("method").and_then(Value::as_str) {
            Some(value) => value,
            None => return id.map(|id| jsonrpc_error(id, -32600, "Invalid Request")),
        };
        id.as_ref()?;
        let id = id.unwrap_or(Value::Null);
        let requested_protocol = request_protocol(object.get("params"));
        if method != "initialize"
            && method != "server/discover"
            && requested_protocol.is_some_and(|version| !protocol_version_supported(version))
        {
            return Some(unsupported_protocol_error(
                id,
                requested_protocol.unwrap_or_default(),
            ));
        }
        let modern =
            method == "server/discover" || requested_protocol == Some(MODERN_PROTOCOL_VERSION);
        let result = match method {
            "server/discover" => Ok(discover_result()),
            "initialize" => Ok(initialize_result(object.get("params"))),
            "ping" if !modern => Ok(json!({})),
            "tools/list" => Ok(tools_list_result(modern)),
            "tools/call" => self.call_tool(object.get("params"), modern),
            _ => {
                return Some(jsonrpc_error(
                    id,
                    -32601,
                    format!("Method not found: {method}"),
                ));
            }
        };
        Some(match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(error) => jsonrpc_error(id, -32602, error.to_string()),
        })
    }

    fn call_tool(&mut self, params: Option<&Value>, modern: bool) -> Result<Value> {
        let params = params
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow!("tools/call params must be an object"))?;
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("tools/call requires a tool name"))?;
        if !direct_mcp_tool_definitions()
            .iter()
            .any(|tool| tool.name == name)
        {
            return Ok(tool_error(
                format!("unknown Yeet tool: {name}"),
                None,
                modern,
            ));
        }
        let mut arguments = match params.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(value)) => value.clone(),
            Some(_) => return Ok(tool_error("tool arguments must be an object", None, modern)),
        };
        let workspace = match self.workspace_from_arguments(&mut arguments) {
            Ok(workspace) => workspace,
            Err(error) => return Ok(tool_error(error.to_string(), None, modern)),
        };
        let workspace_text = workspace.display().to_string();
        if name == "read_file" && arguments.contains_key("requests") {
            return Ok(tool_error(
                "read_file accepts exactly one file per MCP call; use path with an optional line range",
                Some(&workspace_text),
                modern,
            ));
        }
        let mut execution_name = name.to_owned();
        if name == "run_shell" {
            let job = arguments.remove("job");
            let has_command = arguments
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| !command.trim().is_empty());
            match (has_command, job) {
                (true, None) => {}
                (false, Some(Value::Object(job))) => {
                    arguments = job;
                    execution_name = "shell_job".into();
                }
                (false, Some(_)) => {
                    return Ok(tool_error(
                        "run_shell job must be an object",
                        Some(&workspace_text),
                        modern,
                    ));
                }
                _ => {
                    return Ok(tool_error(
                        "run_shell requires exactly one of command or job",
                        Some(&workspace_text),
                        modern,
                    ));
                }
            }
        } else if name == "web" {
            let action = arguments
                .remove("action")
                .and_then(|value| value.as_str().map(str::to_owned));
            execution_name = match action.as_deref() {
                Some("search") => "web_search".into(),
                Some("read") => "web_read".into(),
                _ => {
                    return Ok(tool_error(
                        "web action must be search or read",
                        Some(&workspace_text),
                        modern,
                    ));
                }
            };
        }

        if name == "computer_use" {
            let reset = match arguments.remove("reset") {
                None => false,
                Some(Value::Bool(value)) => value,
                Some(_) => {
                    return Ok(tool_error(
                        "computer_use reset must be a boolean",
                        Some(&workspace_text),
                        modern,
                    ));
                }
            };
            let has_code = arguments.get("code").and_then(Value::as_str).is_some();
            if reset == has_code {
                return Ok(tool_error(
                    "computer_use requires exactly one of code or reset=true",
                    Some(&workspace_text),
                    modern,
                ));
            }
            if reset {
                execution_name = "computer_use_reset".into();
            }
        }
        let result = self
            .runtime(workspace)
            .and_then(|runtime| runtime.execute(&execution_name, arguments));
        Ok(match result {
            Ok(output) => tool_success(output, &workspace_text, modern),
            Err(error) => tool_error(error.to_string(), Some(&workspace_text), modern),
        })
    }

    fn workspace_from_arguments(&self, arguments: &mut Map<String, Value>) -> Result<PathBuf> {
        let value = arguments.remove("workspace");
        let workspace = match value {
            None | Some(Value::Null) => self.default_workspace.clone(),
            Some(Value::String(value)) if !value.trim().is_empty() => {
                resolve_workspace_path(&self.default_workspace, value.trim())?
            }
            Some(_) => bail!("workspace must be a non-empty path string"),
        };
        if self.restrict_workspace
            && workspace != self.default_workspace
            && !workspace.starts_with(&self.default_workspace)
        {
            bail!(
                "MCP workspace access is restricted to {} and its descendants; requested {}",
                self.default_workspace.display(),
                workspace.display()
            );
        }
        Ok(workspace)
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.bridge.shutdown();
    }
}

fn tool_success(output: String, workspace: &str, modern: bool) -> Value {
    let structured = serde_json::from_str::<Value>(&output).unwrap_or_else(|_| json!(output));
    let mut result = json!({
        "content": [{"type":"text","text":output}],
        "structuredContent": {"workspace":workspace,"result":structured},
        "isError": false,
    });
    if modern {
        modernize_result(&mut result);
    }
    result
}

fn tool_error(message: impl Into<String>, workspace: Option<&str>, modern: bool) -> Value {
    let message = message.into();
    let mut structured = json!({"status":"error","error":message});
    if let Some(workspace) = workspace
        && let Some(object) = structured.as_object_mut()
    {
        object.insert("workspace".into(), json!(workspace));
    }
    let mut result = json!({
        "content": [{"type":"text","text":message}],
        "structuredContent": structured,
        "isError": true,
    });
    if modern {
        modernize_result(&mut result);
    }
    result
}

fn discover_result() -> Value {
    json!({
        "resultType": "complete",
        "supportedVersions": [MODERN_PROTOCOL_VERSION],
        "capabilities": {"tools": {}},
        "_meta": server_meta(),
        "instructions": "Yeet exposes a small native work surface directly over MCP. No Yeet agent turn is started and the server does not rewrite the client's conversation context. Every tool accepts workspace to select the project directory for that call; omitting it uses the server default workspace. Artifact, memory, session-management, sandbox-management, capability-management, and compatibility-alias tools are intentionally not exported. MCP runtimes do not create Yeet artifacts.",
        "ttlMs": TOOL_LIST_TTL_MS,
        "cacheScope": "public"
    })
}

fn initialize_result(params: Option<&Value>) -> Value {
    let requested = params
        .and_then(|value| value.get("protocolVersion"))
        .and_then(Value::as_str);
    let protocol = requested
        .filter(|value| protocol_version_supported(value))
        .unwrap_or(LEGACY_PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": protocol,
        "capabilities": {"tools": {}},
        "serverInfo": {"name":"yeet","version":env!("CARGO_PKG_VERSION")},
        "instructions": "Yeet exposes exactly five core MCP tools: read_file, apply_file_edits, run_shell, computer_use, and web. Pass workspace on a tool call to select its project directory. run_shell also manages detached jobs; web handles both search and source reads. Artifact storage and Yeet administrative/state-management tools are not part of the MCP surface."
    })
}

fn tools_list_result(modern: bool) -> Value {
    let mut result = json!({"tools": tool_definitions()});
    if modern {
        modernize_result(&mut result);
        if let Some(object) = result.as_object_mut() {
            object.insert("ttlMs".into(), json!(TOOL_LIST_TTL_MS));
            object.insert("cacheScope".into(), json!("public"));
        }
    }
    result
}

fn request_protocol(params: Option<&Value>) -> Option<&str> {
    params
        .and_then(Value::as_object)
        .and_then(|params| params.get("_meta"))
        .and_then(Value::as_object)
        .and_then(|meta| meta.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str)
}

fn server_meta() -> Value {
    json!({
        "io.modelcontextprotocol/serverInfo": {
            "name": "yeet",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

fn modernize_result(result: &mut Value) {
    if let Some(object) = result.as_object_mut() {
        object.insert("resultType".into(), json!("complete"));
        object.insert("_meta".into(), server_meta());
    }
}

fn tool_definitions() -> Vec<Value> {
    direct_mcp_tool_definitions()
        .into_iter()
        .map(export_tool_definition)
        .collect()
}

fn workspace_property() -> Value {
    json!({
        "type":"string",
        "minLength":1,
        "description":"Workspace directory for this call. Relative paths are resolved from the server default workspace. Omit to use the server default."
    })
}

fn export_tool_definition(tool: ToolDefinition) -> Value {
    let name = tool.name.clone();
    let mut schema = Value::Object(tool.input_schema);
    if let Some(schema_object) = schema.as_object_mut() {
        let properties = schema_object
            .entry("properties")
            .or_insert_with(|| json!({}));
        if let Some(properties) = properties.as_object_mut() {
            properties.insert("workspace".into(), workspace_property());
            if name == "computer_use" {
                properties.insert(
                    "reset".into(),
                    json!({
                        "type":"boolean",
                        "description":"Reset the persistent Computer Use JavaScript session. Use reset=true without code."
                    }),
                );
            }
        }
        if name == "computer_use" {
            schema_object.remove("required");
            schema_object.insert(
                "anyOf".into(),
                json!([
                    {"required":["code"]},
                    {"properties":{"reset":{"const":true}},"required":["reset"]}
                ]),
            );
        }
    }
    let base_description = tool.description.unwrap_or_default();
    let description = if name == "computer_use" {
        format!(
            "{base_description} Pass reset=true without code to reset persistent JavaScript bindings. Workspace can be selected per call with the workspace argument."
        )
    } else {
        format!(
            "{base_description} Workspace can be selected per call with the workspace argument."
        )
    };
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema,
        "annotations": tool_annotations(&tool.name),
    })
}

fn tool_annotations(name: &str) -> Value {
    let read_only = matches!(name, "read_file" | "web");
    let destructive = matches!(name, "apply_file_edits" | "run_shell" | "computer_use");
    let open_world = matches!(name, "run_shell" | "web" | "computer_use");
    json!({
        "readOnlyHint": read_only,
        "destructiveHint": destructive,
        "idempotentHint": read_only,
        "openWorldHint": open_world,
    })
}

fn jsonrpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message.into()}})
}

fn unsupported_protocol_error(id: Value, requested: &str) -> Value {
    json!({
        "jsonrpc":"2.0",
        "id":id,
        "error":{
            "code":-32022,
            "message":format!("Unsupported protocol version: {requested}"),
            "data":{
                "supported":supported_protocol_versions(),
                "requested":requested
            }
        }
    })
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn direct_mcp_surface_is_minimal() {
        let tools = tool_definitions();
        let names = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "read_file",
                "apply_file_edits",
                "run_shell",
                "computer_use",
                "web",
            ]
        );
    }

    #[test]
    fn direct_mcp_read_file_is_single_file_only() {
        let tools = tool_definitions();
        let read_file = tools
            .iter()
            .find(|tool| tool["name"] == "read_file")
            .expect("read_file tool");
        let schema = &read_file["inputSchema"];
        assert!(schema["properties"].get("requests").is_none());
        assert_eq!(schema["required"], json!(["path"]));
    }

    #[test]
    fn direct_mcp_rejects_read_file_batches_even_if_sent_raw() {
        let directory = tempfile::tempdir().unwrap();
        let mut server = McpServer::new(directory.path().to_path_buf());
        let response = server
            .handle(json!({
                "jsonrpc":"2.0",
                "id":1,
                "method":"tools/call",
                "params":{
                    "name":"read_file",
                    "arguments":{
                        "requests":[{"path":"a"},{"path":"b"}]
                    }
                }
            }))
            .expect("tools/call response");
        assert_eq!(response["result"]["isError"], json!(true));
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("exactly one file per MCP call"))
        );
    }

    #[test]
    fn ordinary_file_call_does_not_start_provider_or_edit_sidecars() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("example.txt"), "hello\n").unwrap();
        let workspace = directory.path().canonicalize().unwrap();
        let mut server = McpServer::new(workspace.clone());
        assert!(!server.bridge.is_started());

        let response = server
            .handle(json!({
                "jsonrpc":"2.0",
                "id":1,
                "method":"tools/call",
                "params":{
                    "name":"read_file",
                    "arguments":{"path":"example.txt"}
                }
            }))
            .expect("tools/call response");
        assert_eq!(response["result"]["isError"], json!(false), "{response}");
        assert!(!server.bridge.is_started());
        let runtime = server
            .workspaces
            .get(&workspace)
            .expect("workspace runtime");
        assert!(!runtime.registry.is_edit_started());
    }

    #[test]
    fn edit_after_in_process_read_starts_edit_sidecar_only_when_needed() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("example.txt"), "hello\nworld\n").unwrap();
        let workspace = directory.path().canonicalize().unwrap();
        let mut server = McpServer::new(workspace.clone());
        let read = server
            .handle(json!({
                "jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"read_file","arguments":{"path":"example.txt","startLine":1,"endLine":1}}
            }))
            .expect("read response");
        assert_eq!(read["result"]["isError"], json!(false), "{read}");
        assert!(!server.workspaces[&workspace].registry.is_edit_started());

        let edit = server
            .handle(json!({
                "jsonrpc":"2.0","id":2,"method":"tools/call",
                "params":{"name":"apply_file_edits","arguments":{"changes":[{
                    "path":"example.txt",
                    "edits":[{"kind":"replace","range":{"start":1,"end":1},"text":"hi"}]
                }]}}
            }))
            .expect("edit response");
        assert_eq!(edit["result"]["isError"], json!(false), "{edit}");
        assert!(server.workspaces[&workspace].registry.is_edit_started());
        assert_eq!(
            std::fs::read_to_string(workspace.join("example.txt")).unwrap(),
            "hi\nworld\n"
        );
    }

    #[test]
    fn lazy_edit_rehydration_rejects_file_changed_after_read() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("example.txt"), "hello\nworld\n").unwrap();
        let workspace = directory.path().canonicalize().unwrap();
        let mut server = McpServer::new(workspace.clone());
        let read = server
            .handle(json!({
                "jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"read_file","arguments":{"path":"example.txt","startLine":1,"endLine":1}}
            }))
            .expect("read response");
        assert_eq!(read["result"]["isError"], json!(false), "{read}");
        assert!(!server.workspaces[&workspace].registry.is_edit_started());

        std::fs::write(workspace.join("example.txt"), "changed\nworld\n").unwrap();
        let edit = server
            .handle(json!({
                "jsonrpc":"2.0","id":2,"method":"tools/call",
                "params":{"name":"apply_file_edits","arguments":{"changes":[{
                    "path":"example.txt",
                    "edits":[{"kind":"replace","range":{"start":1,"end":1},"text":"hi"}]
                }]}}
            }))
            .expect("edit response");
        assert_eq!(edit["result"]["isError"], json!(true), "{edit}");
        assert!(
            edit["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("changed after it was read")),
            "{edit}"
        );
        assert!(!server.workspaces[&workspace].registry.is_edit_started());
        assert_eq!(
            std::fs::read_to_string(workspace.join("example.txt")).unwrap(),
            "changed\nworld\n"
        );
    }

    #[test]
    fn lazy_edit_rehydration_preserves_seen_line_enforcement() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("example.txt"), "hello\nworld\n").unwrap();
        let workspace = directory.path().canonicalize().unwrap();
        let mut server = McpServer::new(workspace.clone());
        let read = server
            .handle(json!({
                "jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"read_file","arguments":{"path":"example.txt","startLine":1,"endLine":1}}
            }))
            .expect("read response");
        assert_eq!(read["result"]["isError"], json!(false), "{read}");

        let edit = server
            .handle(json!({
                "jsonrpc":"2.0","id":2,"method":"tools/call",
                "params":{"name":"apply_file_edits","arguments":{"changes":[{
                    "path":"example.txt",
                    "edits":[{"kind":"replace","range":{"start":2,"end":2},"text":"changed"}]
                }]}}
            }))
            .expect("edit response");
        assert_eq!(edit["result"]["isError"], json!(true), "{edit}");
        assert_eq!(
            std::fs::read_to_string(workspace.join("example.txt")).unwrap(),
            "hello\nworld\n"
        );
    }

    #[test]
    fn legacy_protocol_header_is_accepted_after_initialize() {
        let directory = tempfile::tempdir().unwrap();
        let mut server = McpServer::new(directory.path().to_path_buf());

        for version in LEGACY_PROTOCOL_VERSIONS {
            let response = server
                .handle(json!({
                    "jsonrpc":"2.0",
                    "id":1,
                    "method":"tools/list",
                    "params":{
                        "_meta":{
                            "io.modelcontextprotocol/protocolVersion":version
                        }
                    }
                }))
                .expect("tools/list response");
            assert!(
                response.get("error").is_none(),
                "legacy protocol {version} was rejected: {response}"
            );
            assert!(response["result"]["tools"].is_array());
        }
    }

    #[test]
    fn unsupported_protocol_error_advertises_all_supported_versions() {
        let response = unsupported_protocol_error(json!(1), "2099-01-01");
        let supported = response["error"]["data"]["supported"]
            .as_array()
            .expect("supported versions");
        for version in supported_protocol_versions() {
            assert!(
                supported
                    .iter()
                    .any(|value| value.as_str() == Some(version))
            );
        }
    }

    #[test]
    fn computer_use_exports_inline_reset_instead_of_a_reset_tool() {
        let tools = tool_definitions();
        assert!(
            tools
                .iter()
                .all(|tool| tool["name"] != "computer_use_reset")
        );
        let computer = tools
            .iter()
            .find(|tool| tool["name"] == "computer_use")
            .expect("computer_use tool");
        assert_eq!(
            computer["inputSchema"]["properties"]["reset"]["type"],
            json!("boolean")
        );
    }
}
