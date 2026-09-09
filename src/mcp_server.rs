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

pub fn run_cli(args: &[String]) -> Result<String> {
    daemon::run_cli(args)
}

const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
const LEGACY_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const TOOL_LIST_TTL_MS: u64 = 300_000;

pub fn serve_stdio(args: &[String]) -> Result<()> {
    let launch_workspace = current_workspace()?;
    let default_workspace = parse_workspace_option(args, &launch_workspace)?
        .unwrap_or_else(|| launch_workspace.clone());
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut server = McpServer::new(default_workspace);
    serve_io(&mut server, stdin.lock(), stdout.lock())
}

pub fn print_stdio_config(args: &[String]) -> Result<()> {
    let launch_workspace = current_workspace()?;
    let workspace = parse_workspace_option(args, &launch_workspace)?;
    let executable = std::env::current_exe().context("locate Yeet executable")?;
    let mut command_args = vec![json!("mcpserver"), json!("stdio")];
    if let Some(workspace) = workspace {
        command_args.push(json!("--workspace"));
        command_args.push(json!(workspace));
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

fn parse_workspace_option(args: &[String], base: &Path) -> Result<Option<PathBuf>> {
    let mut workspace = None;
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
            value if !value.starts_with('-') && workspace.is_none() => {
                workspace = Some(resolve_workspace_path(base, value)?);
            }
            value => bail!("unknown MCP server option: {value}"),
        }
        index += 1;
    }
    Ok(workspace)
}

pub(super) fn resolve_workspace_path(base: &Path, value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
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
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => value,
            Err(error) => {
                write_json(
                    &mut writer,
                    &jsonrpc_error(Value::Null, -32700, format!("Parse error: {error}")),
                )?;
                continue;
            }
        };
        if let Some(items) = request.as_array() {
            let responses = items
                .iter()
                .filter_map(|item| server.handle(item.clone()))
                .collect::<Vec<_>>();
            if !responses.is_empty() {
                write_json(&mut writer, &Value::Array(responses))?;
            }
            continue;
        }
        if let Some(response) = server.handle(request) {
            write_json(&mut writer, &response)?;
        }
    }
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
    fn new(workspace: PathBuf, bridge: BridgeClient) -> Result<Self> {
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
    bridge: Option<BridgeClient>,
    workspaces: HashMap<PathBuf, WorkspaceRuntime>,
}

impl McpServer {
    pub(super) fn new(default_workspace: PathBuf) -> Self {
        Self {
            default_workspace,
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
            let bridge = self.bridge()?;
            let runtime = WorkspaceRuntime::new(workspace.clone(), bridge)?;
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
        if id.is_none() {
            return None;
        }
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
        if name != "sandbox_get"
            && name != "sandbox_configure"
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
        match value {
            None | Some(Value::Null) => Ok(self.default_workspace.clone()),
            Some(Value::String(value)) if !value.trim().is_empty() => {
                resolve_workspace_path(&self.default_workspace, value.trim())
            }
            Some(_) => bail!("workspace must be a non-empty path string"),
        }
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
        .filter(|value| LEGACY_PROTOCOL_VERSIONS.contains(value))
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
            | "computer_use"
            | "desktop_control"
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_by_name<'a>(result: &'a Value, name: &str) -> &'a Value {
        result["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap()
    }

    #[test]
    fn modern_discovery_describes_direct_tools() {
        let result = discover_result();
        assert_eq!(result["supportedVersions"][0], MODERN_PROTOCOL_VERSION);
        assert_eq!(result["resultType"], "complete");
        assert!(
            result["instructions"]
                .as_str()
                .unwrap()
                .contains("No Yeet agent turn is started")
        );
    }

    #[test]
    fn legacy_initialize_preserves_supported_requested_version() {
        let result = initialize_result(Some(&json!({"protocolVersion":"2025-06-18"})));
        assert_eq!(result["protocolVersion"], "2025-06-18");
        let fallback = initialize_result(Some(&json!({"protocolVersion":"2099-01-01"})));
        assert_eq!(fallback["protocolVersion"], "2025-11-25");
    }

    #[test]
    fn tool_catalog_exports_native_tools_and_workspace_argument() {
        let result = tools_list_result(true);
        assert_eq!(result["ttlMs"], TOOL_LIST_TTL_MS);
        assert_eq!(result["cacheScope"], "public");
        let names = result["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"apply_file_edits"));
        assert!(names.contains(&"run_shell"));
        assert!(names.contains(&"web_search"));
        assert!(names.contains(&"computer_use"));
        assert!(names.contains(&"computer_use_reset"));
        assert!(names.contains(&"desktop_control"));
        assert!(names.contains(&"desktop_control_reset"));
        assert!(names.contains(&"project_memory_recall"));
        assert!(names.contains(&"sandbox_get"));
        assert!(names.contains(&"sandbox_configure"));
        assert!(!names.iter().any(|name| name.starts_with("yeet_")));
        assert!(!names.contains(&"activate_capability"));
        assert!(
            tool_by_name(&result, "read_file")["inputSchema"]["properties"]
                .get("workspace")
                .is_some()
        );
        assert!(
            tool_by_name(&result, "computer_use")["inputSchema"]["properties"]
                .get("workspace")
                .is_some()
        );
        assert_eq!(
            tool_by_name(&result, "computer_use")["annotations"]["openWorldHint"],
            true
        );
        assert_eq!(
            tool_by_name(&result, "desktop_control")["annotations"]["openWorldHint"],
            true
        );
    }

    #[test]
    fn workspace_argument_is_removed_before_internal_tool_execution() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let server = McpServer::new(root.path().canonicalize().unwrap());
        let mut arguments = Map::from_iter([
            ("workspace".into(), json!(other.path())),
            ("path".into(), json!("README.md")),
        ]);
        let selected = server.workspace_from_arguments(&mut arguments).unwrap();
        assert_eq!(selected, other.path().canonicalize().unwrap());
        assert!(!arguments.contains_key("workspace"));
        assert_eq!(arguments["path"], "README.md");
    }

    #[test]
    fn sandbox_configuration_is_workspace_scoped_and_does_not_start_runtime_bridge() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let other_path = other.path().canonicalize().unwrap();
        let mut server = McpServer::new(root_path.clone());

        let response = server
            .call_tool(
                Some(&json!({
                    "name":"sandbox_configure",
                    "arguments":{
                        "workspace":root_path,
                        "mode":"unlimited",
                        "autoApprove":true,
                        "workspaceRead":{"mode":"paths","paths":["src","Tests"]},
                        "networkAllow":[{"host":"example.com","port":443}],
                        "environment":{"YEET_TEST":"1"},
                        "secretIDs":["TOKEN"],
                        "limits":{"wallTimeSeconds":45,"maxProcesses":12}
                    }
                })),
                true,
            )
            .unwrap();
        assert!(!response["isError"].as_bool().unwrap());

        let configured = SandboxStore::new(&root_path).unwrap().load().unwrap();
        assert_eq!(configured.mode, SandboxMode::Unlimited);
        assert!(configured.auto_approve);
        assert_eq!(
            configured.environment.get("YEET_TEST").map(String::as_str),
            Some("1")
        );
        assert_eq!(configured.limits.wall_time_seconds, 45);
        assert_eq!(configured.limits.max_processes, 12);
        assert!(matches!(configured.workspace_read, WorkspaceRead::Paths(_)));
        assert_eq!(configured.network_allow.len(), 1);
        assert!(configured.secret_ids.contains("TOKEN"));

        let untouched = SandboxStore::new(&other_path).unwrap().load().unwrap();
        assert_eq!(untouched, SandboxPolicy::default());
        assert!(server.bridge.is_none());
        assert!(server.workspaces.is_empty());
    }

    #[test]
    fn sandbox_get_returns_persisted_policy_without_runtime_bridge() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let store = SandboxStore::new(&root_path).unwrap();
        let mut policy = SandboxPolicy::default();
        policy.auto_approve = true;
        store.save(&policy).unwrap();
        let mut server = McpServer::new(root_path.clone());
        let response = server
            .call_tool(
                Some(&json!({
                    "name":"sandbox_get",
                    "arguments":{"workspace":root_path}
                })),
                true,
            )
            .unwrap();
        assert_eq!(response["structuredContent"]["result"]["autoApprove"], true);
        assert!(server.bridge.is_none());
    }

    #[test]
    fn stdio_protocol_lists_tools_without_starting_runtime_bridge() {
        let workspace = tempfile::tempdir().unwrap();
        let mut server = McpServer::new(workspace.path().canonicalize().unwrap());
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"server/discover\",\"params\":{\"_meta\":{\"io.modelcontextprotocol/protocolVersion\":\"2026-07-28\"}}}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{\"_meta\":{\"io.modelcontextprotocol/protocolVersion\":\"2026-07-28\"}}}\n";
        let mut output = Vec::new();
        serve_io(
            &mut server,
            std::io::BufReader::new(&input[..]),
            &mut output,
        )
        .unwrap();
        let values = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(values.len(), 2);
        assert_eq!(values[1]["result"]["tools"][0]["name"], "read_file");
        assert!(server.bridge.is_none());
        assert!(server.workspaces.is_empty());
    }

    #[test]
    fn legacy_tool_list_keeps_legacy_wire_shape() {
        let result = tools_list_result(false);
        assert!(result.get("resultType").is_none());
        assert!(result.get("ttlMs").is_none());
        assert_eq!(result["tools"][0]["name"], "read_file");
    }

    #[test]
    fn modern_tool_results_include_required_wire_discriminator() {
        let success = tool_success("ok".into(), "/tmp/project", true);
        let failure = tool_error("nope", Some("/tmp/project"), true);
        assert_eq!(success["resultType"], "complete");
        assert_eq!(failure["resultType"], "complete");
        assert_eq!(
            success["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "yeet"
        );
    }

    #[test]
    fn unknown_modern_protocol_version_is_rejected_with_supported_versions() {
        let workspace = tempfile::tempdir().unwrap();
        let mut server = McpServer::new(workspace.path().canonicalize().unwrap());
        let response = server
            .handle(json!({
                "jsonrpc":"2.0",
                "id":7,
                "method":"tools/list",
                "params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2099-01-01"}}
            }))
            .unwrap();
        assert_eq!(response["error"]["code"], -32022);
        assert_eq!(response["error"]["data"]["requested"], "2099-01-01");
        assert_eq!(
            response["error"]["data"]["supported"][0],
            MODERN_PROTOCOL_VERSION
        );
        assert!(server.bridge.is_none());
    }
}
