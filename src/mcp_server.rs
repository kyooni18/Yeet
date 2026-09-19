use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
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
    core::{BridgeClient, BridgeEvent, ToolCall, ToolDefinition},
    permission::PermissionBroker,
    project_settings::ProjectSettingsStore,
    sandbox::{
        NetworkEndpoint, SandboxMode, SandboxPolicy, SandboxStore, WorkspaceRead,
        validate_environment, validate_relative_path, validate_secret_id,
    },
    session_store::SessionStore,
    tools::{ToolRegistry, direct_mcp_tool_definitions},
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
    if let Some(items) = request.as_array() {
        let responses = items
            .iter()
            .filter_map(|item| server.handle(item.clone()))
            .collect::<Vec<_>>();
        if !responses.is_empty() {
            write_json(writer, &Value::Array(responses))?;
        }
        return Ok(());
    }
    if let Some(response) = server.handle(request) {
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
    bridge: BridgeClient,
    config: ConfigStore,
    project_settings: ProjectSettingsStore,
    project_identity: String,
    permission_was_denied: Arc<AtomicBool>,
}

impl WorkspaceRuntime {
    fn new(
        workspace: PathBuf,
        bridge: BridgeClient,
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
        let mut registry = ToolRegistry::new(bridge.clone(), workspace, workers, permission)?;
        registry.set_hard_access_root(hard_access_root)?;
        registry.set_disabled_capabilities(project.capabilities.disabled.clone());
        registry.configure_foundation_memory(
            project.foundation_memory.enabled,
            project.foundation_memory.server.clone(),
            project_identity.clone(),
        );
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
            project.foundation_memory.server,
            self.project_identity.clone(),
        );
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
            let bridge = self.bridge.clone();
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
    bridge: Option<BridgeClient>,
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
            bridge: None,
            workspaces: HashMap::new(),
        }
    }

    fn bridge(&mut self) -> Result<BridgeClient> {
        if self.bridge.is_none() {
            self.bridge = Some(BridgeClient::start()?);
        }
        Ok(self.bridge.as_ref().expect("bridge initialized").clone())
    }

    fn runtime(&mut self, workspace: PathBuf) -> Result<&mut WorkspaceRuntime> {
        if !self.workspaces.contains_key(&workspace) {
            let hard_access_root = self
                .restrict_workspace
                .then(|| self.default_workspace.clone());
            let bridge = self.bridge()?;
            let runtime = WorkspaceRuntime::new(workspace.clone(), bridge, hard_access_root)?;
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
            && requested_protocol.is_some_and(|version| version != MODERN_PROTOCOL_VERSION)
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
        let lazy_tool = crate::skyline::direct_mcp_tool_definitions()
            .iter()
            .any(|tool| tool.name == name);
        if name != "sandbox_get"
            && name != "sandbox_configure"
            && !lazy_tool
            && !direct_mcp_tool_definitions()
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
        let execution_name = match name {
            "desktop_control" => "computer_use",
            "desktop_control_reset" => "computer_use_reset",
            _ => name,
        };
        let result = match name {
            "sandbox_get" => sandbox_get(&workspace, arguments),
            "sandbox_configure" => sandbox_configure(&workspace, arguments),
            "activate_capability" => crate::skyline::activate(&workspace, arguments),
            "invoke_capability" => crate::skyline::invoke(&workspace, arguments),
            _ => self
                .runtime(workspace)
                .and_then(|runtime| runtime.execute(execution_name, arguments)),
        };
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

fn sandbox_get(workspace: &Path, arguments: Map<String, Value>) -> Result<String> {
    if let Some(key) = arguments.keys().next() {
        bail!("unknown sandbox_get argument: {key}");
    }
    let store = SandboxStore::new(workspace)?;
    store.render(&store.load()?)
}

fn sandbox_configure(workspace: &Path, arguments: Map<String, Value>) -> Result<String> {
    const ALLOWED: &[&str] = &[
        "reset",
        "mode",
        "autoApprove",
        "scratchWritable",
        "workspaceRead",
        "networkAllow",
        "environment",
        "secretIDs",
        "limits",
    ];
    if let Some(key) = arguments
        .keys()
        .find(|key| !ALLOWED.contains(&key.as_str()))
    {
        bail!("unknown sandbox_configure argument: {key}");
    }
    if arguments.is_empty() {
        bail!("sandbox_configure requires at least one setting");
    }

    let store = SandboxStore::new(workspace)?;
    let reset = optional_bool_value(&arguments, "reset")?.unwrap_or(false);
    let mut policy = if reset {
        SandboxPolicy::default()
    } else {
        store.load()?
    };

    if let Some(mode) = arguments.get("mode") {
        policy.mode = match mode.as_str() {
            Some("sandboxed") => SandboxMode::Sandboxed,
            Some("unlimited") => SandboxMode::Unlimited,
            _ => bail!("mode must be sandboxed or unlimited"),
        };
    }
    if let Some(value) = optional_bool_value(&arguments, "autoApprove")? {
        policy.auto_approve = value;
    }
    if let Some(value) = optional_bool_value(&arguments, "scratchWritable")? {
        policy.scratch_writable = value;
    }
    if let Some(value) = arguments.get("workspaceRead") {
        policy.workspace_read = parse_workspace_read(value)?;
    }
    if let Some(value) = arguments.get("networkAllow") {
        policy.network_allow = parse_network_allow(value)?;
        policy.normalize_network();
    }
    if let Some(value) = arguments.get("environment") {
        policy.environment = parse_environment(value)?;
    }
    if let Some(value) = arguments.get("secretIDs") {
        policy.secret_ids = parse_secret_ids(value)?;
    }
    if let Some(value) = arguments.get("limits") {
        apply_limits_patch(&mut policy, value)?;
    }

    if reset && arguments.len() == 1 {
        store.reset()?;
        return store.render(&SandboxPolicy::default());
    }
    store.save(&policy)?;
    store.render(&policy)
}

fn optional_bool_value(arguments: &Map<String, Value>, key: &str) -> Result<Option<bool>> {
    let Some(value) = arguments.get(key) else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .ok_or_else(|| anyhow!("{key} must be a boolean"))
}

fn parse_workspace_read(value: &Value) -> Result<WorkspaceRead> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("workspaceRead must be an object"))?;
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "mode" | "paths"))
    {
        bail!("unknown workspaceRead field: {key}");
    }
    let mode = object
        .get("mode")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("workspaceRead.mode is required"))?;
    match mode {
        "none" => {
            if object.contains_key("paths") {
                bail!("workspaceRead.paths is only valid when mode=paths");
            }
            Ok(WorkspaceRead::None)
        }
        "all" => {
            if object.contains_key("paths") {
                bail!("workspaceRead.paths is only valid when mode=paths");
            }
            Ok(WorkspaceRead::All)
        }
        "paths" => {
            let paths = object
                .get("paths")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("workspaceRead.paths is required when mode=paths"))?;
            if paths.is_empty() {
                bail!("workspaceRead.paths must not be empty");
            }
            let mut values = BTreeSet::new();
            for path in paths {
                let path = path
                    .as_str()
                    .ok_or_else(|| anyhow!("workspaceRead.paths entries must be strings"))?;
                values.insert(validate_relative_path(path)?);
            }
            Ok(WorkspaceRead::Paths(values))
        }
        _ => bail!("workspaceRead.mode must be none, all, or paths"),
    }
}

fn parse_network_allow(value: &Value) -> Result<BTreeSet<NetworkEndpoint>> {
    let items = value
        .as_array()
        .ok_or_else(|| anyhow!("networkAllow must be an array"))?;
    let mut endpoints = BTreeSet::new();
    for item in items {
        let object = item
            .as_object()
            .ok_or_else(|| anyhow!("networkAllow entries must be objects"))?;
        if let Some(key) = object
            .keys()
            .find(|key| !matches!(key.as_str(), "host" | "port"))
        {
            bail!("unknown networkAllow field: {key}");
        }
        let host = object
            .get("host")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("networkAllow.host is required"))?;
        let port = match object.get("port") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let raw = value
                    .as_u64()
                    .ok_or_else(|| anyhow!("networkAllow.port must be an integer or null"))?;
                let port =
                    u16::try_from(raw).map_err(|_| anyhow!("invalid network port: {raw}"))?;
                if port == 0 {
                    bail!("invalid network port: 0");
                }
                Some(port)
            }
        };
        endpoints.insert(NetworkEndpoint::new(host, port)?);
    }
    Ok(endpoints)
}

fn parse_environment(value: &Value) -> Result<BTreeMap<String, String>> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("environment must be an object of string values"))?;
    let mut environment = BTreeMap::new();
    for (key, value) in object {
        let value = value
            .as_str()
            .ok_or_else(|| anyhow!("environment values must be strings"))?;
        environment.insert(key.clone(), value.to_owned());
    }
    validate_environment(&environment)?;
    Ok(environment)
}

fn parse_secret_ids(value: &Value) -> Result<BTreeSet<String>> {
    let values = value
        .as_array()
        .ok_or_else(|| anyhow!("secretIDs must be an array"))?;
    let mut ids = BTreeSet::new();
    for value in values {
        let id = value
            .as_str()
            .ok_or_else(|| anyhow!("secretIDs entries must be strings"))?;
        validate_secret_id(id)?;
        ids.insert(id.to_owned());
    }
    Ok(ids)
}

fn apply_limits_patch(policy: &mut SandboxPolicy, value: &Value) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("limits must be an object"))?;
    if object.is_empty() {
        bail!("limits must contain at least one field");
    }
    const ALLOWED: &[&str] = &[
        "wallTimeSeconds",
        "maxStdoutBytes",
        "maxStderrBytes",
        "maxMemoryBytes",
        "maxProcesses",
    ];
    if let Some(key) = object.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        bail!("unknown limits field: {key}");
    }
    if let Some(value) = object.get("wallTimeSeconds") {
        policy.limits.wall_time_seconds = required_u64(value, "limits.wallTimeSeconds")?;
    }
    if let Some(value) = object.get("maxStdoutBytes") {
        policy.limits.max_stdout_bytes = required_usize(value, "limits.maxStdoutBytes")?;
    }
    if let Some(value) = object.get("maxStderrBytes") {
        policy.limits.max_stderr_bytes = required_usize(value, "limits.maxStderrBytes")?;
    }
    if let Some(value) = object.get("maxMemoryBytes") {
        policy.limits.max_memory_bytes = required_u64(value, "limits.maxMemoryBytes")?;
    }
    if let Some(value) = object.get("maxProcesses") {
        policy.limits.max_processes = required_usize(value, "limits.maxProcesses")?;
    }
    policy.limits.validate()
}

fn required_u64(value: &Value, field: &str) -> Result<u64> {
    value
        .as_u64()
        .ok_or_else(|| anyhow!("{field} must be a non-negative integer"))
}

fn required_usize(value: &Value, field: &str) -> Result<usize> {
    let value = required_u64(value, field)?;
    usize::try_from(value).map_err(|_| anyhow!("{field} is too large"))
}

impl Drop for McpServer {
    fn drop(&mut self) {
        if let Some(bridge) = self.bridge.take() {
            bridge.shutdown();
        }
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
        "instructions": "Yeet exposes its native tools directly over MCP. No Yeet agent turn is started. Every tool accepts workspace to select the project directory for that call; omitting it uses the server default workspace. Tool state such as file snapshots, artifacts, and background shell jobs is isolated per workspace. sandbox_get and sandbox_configure read or update the same per-workspace .yeet/sandbox.json policy used by Yeet CLI and TUI.",
        "ttlMs": TOOL_LIST_TTL_MS,
        "cacheScope": "public"
    })
}

fn initialize_result(params: Option<&Value>) -> Value {
    let requested = params
        .and_then(|value| value.get("protocolVersion"))
        .and_then(Value::as_str);
    let protocol = requested
        .filter(|value| {
            *value == MODERN_PROTOCOL_VERSION || LEGACY_PROTOCOL_VERSIONS.contains(value)
        })
        .unwrap_or(LEGACY_PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": protocol,
        "capabilities": {"tools": {}},
        "serverInfo": {"name":"yeet","version":env!("CARGO_PKG_VERSION")},
        "instructions": "Yeet exposes its native file, shell, document, data, web, session, artifact, and project-memory tools directly. Pass workspace on a tool call to select its project directory."
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
    let mut tools = direct_mcp_tool_definitions()
        .into_iter()
        .chain(crate::skyline::direct_mcp_tool_definitions())
        .map(export_tool_definition)
        .collect::<Vec<_>>();
    tools.push(sandbox_get_definition());
    tools.push(sandbox_configure_definition());
    tools
}

fn sandbox_get_definition() -> Value {
    json!({
        "name":"sandbox_get",
        "description":"Read the selected workspace's current Yeet sandbox policy from .yeet/sandbox.json. Omit workspace to use the MCP server default workspace.",
        "inputSchema":{
            "type":"object",
            "properties":{"workspace":workspace_property()},
            "additionalProperties":false
        },
        "annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}
    })
}

fn sandbox_configure_definition() -> Value {
    json!({
        "name":"sandbox_configure",
        "description":"Partially update or reset the selected workspace's Yeet sandbox policy. Changes persist to the same .yeet/sandbox.json used by Yeet CLI and TUI and apply to later MCP tool calls in that workspace.",
        "inputSchema":{
            "type":"object",
            "properties":{
                "workspace":workspace_property(),
                "reset":{"type":"boolean","description":"Start from Yeet's default sandbox policy before applying other supplied fields. If used alone, remove the persisted sandbox file."},
                "mode":{"type":"string","enum":["sandboxed","unlimited"]},
                "autoApprove":{"type":"boolean"},
                "scratchWritable":{"type":"boolean"},
                "workspaceRead":{
                    "type":"object",
                    "properties":{
                        "mode":{"type":"string","enum":["none","all","paths"]},
                        "paths":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}}
                    },
                    "required":["mode"],
                    "additionalProperties":false
                },
                "networkAllow":{
                    "type":"array",
                    "items":{
                        "type":"object",
                        "properties":{
                            "host":{"type":"string","minLength":1},
                            "port":{"oneOf":[{"type":"integer","minimum":1,"maximum":65535},{"type":"null"}]}
                        },
                        "required":["host"],
                        "additionalProperties":false
                    }
                },
                "environment":{"type":"object","additionalProperties":{"type":"string"}},
                "secretIDs":{"type":"array","items":{"type":"string","minLength":1}},
                "limits":{
                    "type":"object",
                    "minProperties":1,
                    "properties":{
                        "wallTimeSeconds":{"type":"integer","minimum":1,"maximum":86400},
                        "maxStdoutBytes":{"type":"integer","minimum":1,"maximum":67108864},
                        "maxStderrBytes":{"type":"integer","minimum":1,"maximum":67108864},
                        "maxMemoryBytes":{"type":"integer","minimum":67108864,"maximum":34_359_738_368u64},
                        "maxProcesses":{"type":"integer","minimum":1,"maximum":1024}
                    },
                    "additionalProperties":false
                }
            },
            "additionalProperties":false,
            "minProperties":1
        },
        "annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":false}
    })
}

fn workspace_property() -> Value {
    json!({
        "type":"string",
        "minLength":1,
        "description":"Workspace directory for this call. Relative paths are resolved from the server default workspace. Omit to use the server default."
    })
}

fn export_tool_definition(tool: ToolDefinition) -> Value {
    let mut schema = Value::Object(tool.input_schema);
    if let Some(schema_object) = schema.as_object_mut() {
        let properties = schema_object
            .entry("properties")
            .or_insert_with(|| json!({}));
        if let Some(properties) = properties.as_object_mut() {
            properties.insert("workspace".into(), workspace_property());
        }
    }
    let description = format!(
        "{} Workspace can be selected per call with the workspace argument.",
        tool.description.unwrap_or_default()
    );
    json!({
        "name": tool.name,
        "description": description,
        "inputSchema": schema,
        "annotations": tool_annotations(&tool.name),
    })
}

fn tool_annotations(name: &str) -> Value {
    let read_only = matches!(
        name,
        "read_file"
            | "list_files"
            | "search_workspace"
            | "read_document"
            | "analyze_data"
            | "artifact_info"
            | "read_artifact"
            | "search_artifact"
            | "list_sessions"
            | "web_search"
            | "web_read"
            | "project_memory_recall"
            | "project_memory_get"
            | "project_memory_connections"
    );
    let destructive = matches!(
        name,
        "apply_file_edits"
            | "run_shell"
            | "shell_job"
            | "export_session"
            | "project_memory_remember"
            | "project_memory_update"
            | "project_memory_replace"
            | "project_memory_forget"
            | "project_memory_restore"
            | "project_memory_relate"
            | "computer_use"
            | "desktop_control"
            | "activate_capability"
            | "invoke_capability"
    );
    let open_world = matches!(
        name,
        "run_shell" | "web_search" | "web_read" | "computer_use" | "desktop_control"
    );
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
                "supported":[MODERN_PROTOCOL_VERSION],
                "requested":requested
            }
        }
    })
}
