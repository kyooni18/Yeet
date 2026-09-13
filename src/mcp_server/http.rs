use std::{
    collections::{HashMap, VecDeque},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use socket2::{Domain, Protocol, Socket, Type};
use url::{Url, form_urlencoded};
use uuid::Uuid;

use super::{
    MODERN_PROTOCOL_VERSION,
    auth::{AuthMode, AuthStore, AuthorizationRequest, OAuthRuntime, bearer_token},
    mcp_json_nesting_within_limit,
    trace::runtime_request_summary,
};

mod runtime_support;

use runtime_support::{
    JsonRpcErrorShape, mcp_jsonrpc_error_response, payload_contains_blocking_tool_call,
    runtime_timeout_for_payload,
};

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_WORKER_STACK_BYTES: usize = 4 * 1024 * 1024;
const MAX_HTTP_WORKERS: usize = 64;
const MAX_MCP_SESSIONS: usize = 32;
// Keep enough lazily-created runtimes for concurrent workers while reserving a
// quarter of the pool for short control/file calls. Foreground shell and native
// desktop calls are deliberately capped below the full pool so they cannot
// starve the MCP control plane.
const MAX_LEGACY_RUNTIMES: usize = 16;
const MAX_BLOCKING_TOOL_RUNTIMES: usize = 12;
const MAX_LEGACY_AFFINITY_HANDLES: usize = 8192;
const MCP_SESSION_IDLE_TTL: Duration = Duration::from_secs(30 * 60);
const MCP_RUNTIME_LANE_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const MCP_RUNTIME_FIXED_LANE_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const MCP_RUNTIME_LANE_RETRY_DELAY: Duration = Duration::from_millis(10);
const MCP_RUNTIME_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(15);
const MCP_RUNTIME_DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);
const MCP_RUNTIME_MAX_TIMEOUT: Duration = Duration::from_secs(15 * 60 + 30);
const MCP_RUNTIME_UNAVAILABLE_ERROR_CODE: i64 = -32001;
const MCP_SESSION_CAPACITY_ERROR_CODE: i64 = -32002;
const MCP_SESSION_HEADER: &str = "mcp-session-id";

#[derive(Debug, Clone)]
pub(super) struct HttpOptions {
    pub bind_host: String,
    pub port: u16,
    pub default_workspace: PathBuf,
    pub public_url: Option<Url>,
}

pub(super) struct HttpServer {
    listener: TcpListener,
    address: SocketAddr,
    public_url: Url,
    issuer: Url,
    runtimes: Arc<McpRuntimeManager>,
    oauth: OAuthRuntime,
}

impl HttpServer {
    pub(super) fn bind(options: HttpOptions, auth_store: AuthStore) -> Result<Self> {
        let listener = bind_reusable(&options.bind_host, options.port)?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let public_url = match options.public_url {
            Some(url) => normalize_public_url(url)?,
            None => inferred_public_url(&options.bind_host, address)?,
        };
        let issuer = issuer_for_resource(&public_url)?;
        let oauth = OAuthRuntime::new(auth_store, issuer.clone(), public_url.clone())?;
        let runtimes = Arc::new(McpRuntimeManager::new(options.default_workspace)?);
        Ok(Self {
            listener,
            address,
            public_url,
            issuer,
            runtimes,
            oauth,
        })
    }

    pub(super) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(super) fn public_url(&self) -> &Url {
        &self.public_url
    }

    pub(super) fn serve(self, stop: Arc<std::sync::atomic::AtomicBool>) -> Result<()> {
        let active_workers = Arc::new(AtomicUsize::new(0));
        while !stop.load(std::sync::atomic::Ordering::Acquire) {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    let active = active_workers.fetch_add(1, Ordering::AcqRel);
                    if active >= MAX_HTTP_WORKERS {
                        active_workers.fetch_sub(1, Ordering::AcqRel);
                        eprintln!(
                            "yeet mcpserver: HTTP worker capacity reached ({MAX_HTTP_WORKERS}); returning 429"
                        );
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                        let mut response = HttpResponse::json(
                            429,
                            json!({
                                "error":"server_busy",
                                "error_description":"Yeet MCP has reached its concurrent HTTP connection limit; retry shortly"
                            }),
                        );
                        response.headers.push(("Retry-After".into(), "1".into()));
                        let _ = write_response(&mut stream, response);
                        continue;
                    }
                    let runtimes = Arc::clone(&self.runtimes);
                    let oauth = self.oauth.clone();
                    let public_url = self.public_url.clone();
                    let issuer = self.issuer.clone();
                    let active_workers_for_thread = Arc::clone(&active_workers);
                    let spawn = thread::Builder::new()
                        .name("yeet-mcp-http".into())
                        .stack_size(HTTP_WORKER_STACK_BYTES)
                        .spawn(move || {
                            let _slot = HttpWorkerSlot(active_workers_for_thread);
                            handle_connection(stream, runtimes, oauth, public_url, issuer);
                        });
                    if let Err(error) = spawn {
                        active_workers.fetch_sub(1, Ordering::AcqRel);
                        return Err(error.into());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

struct HttpWorkerSlot(Arc<AtomicUsize>);

impl Drop for HttpWorkerSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct SessionRuntime {
    process: McpProcess,
}

impl SessionRuntime {
    fn new(default_workspace: PathBuf) -> Result<Self> {
        Ok(Self {
            process: McpProcess::new(default_workspace)?,
        })
    }

    fn handle(&mut self, payload: Value) -> Result<Option<Value>> {
        if !self.process.is_alive() {
            eprintln!(
                "yeet mcpserver: MCP session runtime was not alive; restarting before request"
            );
            self.process.restart()?;
        }
        self.process.handle(payload)
    }
}

struct SessionRuntimePool {
    runtimes: RuntimeLanePool,
    last_used: Mutex<Instant>,
}

impl SessionRuntimePool {
    fn new(default_workspace: PathBuf) -> Result<Self> {
        Ok(Self {
            runtimes: RuntimeLanePool::new(default_workspace)?,
            last_used: Mutex::new(Instant::now()),
        })
    }

    fn call(&self, payload: Value) -> std::result::Result<Option<Value>, RuntimeCallError> {
        if let Ok(mut last_used) = self.last_used.lock() {
            *last_used = Instant::now();
        }
        self.runtimes.call(payload)
    }

    fn add_health(&self, health: &mut RuntimeHealth) {
        self.runtimes.add_health(health);
    }

    fn is_idle_for(&self, ttl: Duration) -> bool {
        self.last_used
            .lock()
            .map(|last_used| last_used.elapsed() >= ttl)
            .unwrap_or(false)
    }
}

struct RuntimeLanePool {
    default_workspace: PathBuf,
    lanes: Vec<Arc<Mutex<Option<SessionRuntime>>>>,
    affinity: Mutex<LegacyAffinityState>,
}

#[derive(Default)]
struct LegacyAffinityState {
    handles: HashMap<String, usize>,
    handle_order: VecDeque<String>,
    sticky: HashMap<String, usize>,
    preferred: HashMap<String, usize>,
    next_lane: usize,
}

#[derive(Debug)]
struct LegacyRequestInfo {
    handles: Vec<String>,
    sticky_key: Option<String>,
    preferred_key: Option<String>,
}

enum LegacyRoute {
    Fixed(usize),
    Flexible(usize),
}

impl RuntimeLanePool {
    fn new(default_workspace: PathBuf) -> Result<Self> {
        let mut lanes = Vec::with_capacity(MAX_LEGACY_RUNTIMES);
        lanes.push(Arc::new(Mutex::new(Some(SessionRuntime::new(
            default_workspace.clone(),
        )?))));
        for _ in 1..MAX_LEGACY_RUNTIMES {
            lanes.push(Arc::new(Mutex::new(None)));
        }
        Ok(Self {
            default_workspace,
            lanes,
            affinity: Mutex::new(LegacyAffinityState::default()),
        })
    }

    fn call(&self, payload: Value) -> std::result::Result<Option<Value>, RuntimeCallError> {
        let blocking = payload_contains_blocking_tool_call(&payload);
        let info = legacy_request_info(&payload, &self.default_workspace);
        let blocking_end = MAX_BLOCKING_TOOL_RUNTIMES.min(self.lanes.len());
        // Stateful sticky domains (shell/native computer control) stay on the blocking
        // side of the pool. Everything else that can safely choose a fresh runtime
        // starts in the reserved short-call lanes, so snapshots/artifacts are not
        // created on a lane that a long shell or desktop call can monopolize.
        let use_reserved_short_lanes = !blocking
            && info.sticky_key.is_none()
            && blocking_end < self.lanes.len();
        let (lane_start, lane_end) = if use_reserved_short_lanes {
            (blocking_end, self.lanes.len())
        } else {
            (0, blocking_end)
        };
        let route = self.route(&info, lane_start, lane_end)?;
        let (lane, response) = match route {
            LegacyRoute::Fixed(lane) => {
                let response = call_runtime_lane_bounded(
                    &self.lanes[lane],
                    &self.default_workspace,
                    payload,
                    lane,
                )?;
                (lane, response)
            }
            LegacyRoute::Flexible(start) => call_flexible_runtime_bounded(
                &self.lanes,
                &self.default_workspace,
                payload,
                lane_start,
                lane_end,
                start,
            )?,
        };
        if let Some(key) = info.preferred_key.as_ref() {
            self.affinity
                .lock()
                .map_err(|_| {
                    RuntimeCallError::Unavailable(anyhow!("legacy MCP affinity lock poisoned"))
                })?
                .preferred
                .entry(key.clone())
                .or_insert(lane);
        }
        if let Some(response) = response.as_ref() {
            self.record_response_handles(response, lane)
                .map_err(RuntimeCallError::Unavailable)?;
        }
        Ok(response)
    }

    fn route(
        &self,
        info: &LegacyRequestInfo,
        lane_start: usize,
        lane_end: usize,
    ) -> std::result::Result<LegacyRoute, RuntimeCallError> {
        debug_assert!(lane_start < lane_end && lane_end <= self.lanes.len());
        let lane_count = lane_end - lane_start;
        let mut affinity = self.affinity.lock().map_err(|_| {
            RuntimeCallError::Unavailable(anyhow!("legacy MCP affinity lock poisoned"))
        })?;
        let mut fixed_lane = None;
        for handle in &info.handles {
            let Some(&lane) = affinity.handles.get(handle) else {
                continue;
            };
            match fixed_lane {
                None => fixed_lane = Some(lane),
                Some(existing) if existing == lane => {}
                Some(existing) => {
                    return Err(RuntimeCallError::Unavailable(anyhow!(
                        "legacy MCP request references state from multiple runtime lanes ({existing} and {lane}); re-read the affected state in one call or use MCP sessions"
                    )));
                }
            }
        }
        if let Some(lane) = fixed_lane {
            return Ok(LegacyRoute::Fixed(lane));
        }
        if let Some(key) = info.sticky_key.as_ref() {
            if let Some(&lane) = affinity.sticky.get(key) {
                return Ok(LegacyRoute::Fixed(lane));
            }
            let lane = lane_start + affinity.next_lane % lane_count;
            affinity.next_lane = affinity.next_lane.wrapping_add(1);
            affinity.sticky.insert(key.clone(), lane);
            return Ok(LegacyRoute::Fixed(lane));
        }
        if let Some(key) = info.preferred_key.as_ref() {
            if let Some(&lane) = affinity.preferred.get(key)
                && (lane_start..lane_end).contains(&lane)
            {
                return Ok(LegacyRoute::Flexible(lane));
            }
            // Preferred affinity is only a performance hint, not state ownership.
            // Drop a stale mapping when pool policy moves this workspace to another
            // lane class (for example after reserving short-call lanes).
            affinity.preferred.remove(key);
            let lane = lane_start + affinity.next_lane % lane_count;
            affinity.next_lane = affinity.next_lane.wrapping_add(1);
            return Ok(LegacyRoute::Flexible(lane));
        }
        let start = lane_start + affinity.next_lane % lane_count;
        affinity.next_lane = affinity.next_lane.wrapping_add(1);
        Ok(LegacyRoute::Flexible(start))
    }

    fn record_response_handles(&self, response: &Value, lane: usize) -> Result<()> {
        let mut handles = Vec::new();
        collect_response_handles(response, &mut handles);
        if handles.is_empty() {
            return Ok(());
        }
        let mut affinity = self
            .affinity
            .lock()
            .map_err(|_| anyhow!("legacy MCP affinity lock poisoned"))?;
        for handle in handles {
            if !affinity.handles.contains_key(&handle) {
                affinity.handle_order.push_back(handle.clone());
            }
            affinity.handles.insert(handle, lane);
        }
        while affinity.handles.len() > MAX_LEGACY_AFFINITY_HANDLES {
            let Some(oldest) = affinity.handle_order.pop_front() else {
                break;
            };
            affinity.handles.remove(&oldest);
        }
        Ok(())
    }

    fn add_health(&self, health: &mut RuntimeHealth) {
        for lane in &self.lanes {
            health.total += 1;
            match lane.try_lock() {
                Ok(mut lane) => {
                    if let Some(runtime) = lane.as_mut() {
                        if runtime.process.is_alive() {
                            health.available += 1;
                        } else {
                            health.dead += 1;
                        }
                    } else {
                        health.available += 1;
                    }
                }
                Err(std::sync::TryLockError::WouldBlock) => health.busy += 1,
                Err(std::sync::TryLockError::Poisoned(_)) => health.dead += 1,
            }
        }
    }
}

fn call_runtime_lane_bounded(
    lane: &Arc<Mutex<Option<SessionRuntime>>>,
    default_workspace: &Path,
    payload: Value,
    lane_index: usize,
) -> std::result::Result<Option<Value>, RuntimeCallError> {
    // Fixed routes carry state (snapshot/artifact/job/native-session affinity),
    // so spilling to another lane would be incorrect. Queue briefly on that
    // lane instead of applying the aggressive flexible-pool admission timeout.
    let request = runtime_request_summary(&payload, default_workspace);
    let deadline = Instant::now() + MCP_RUNTIME_FIXED_LANE_WAIT_TIMEOUT;
    loop {
        match lane.try_lock() {
            Ok(mut runtime) => {
                return call_locked_runtime(&mut runtime, default_workspace, payload, lane_index);
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(runtime_pool_busy_error(
                        lane_index,
                        MCP_RUNTIME_FIXED_LANE_WAIT_TIMEOUT,
                        &request,
                    ));
                }
                thread::sleep(MCP_RUNTIME_LANE_RETRY_DELAY);
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(RuntimeCallError::Unavailable(anyhow!(
                    "MCP runtime lane {lane_index} lock poisoned ({request})"
                )));
            }
        }
    }
}

fn call_flexible_runtime_bounded(
    lanes: &[Arc<Mutex<Option<SessionRuntime>>>],
    default_workspace: &Path,
    payload: Value,
    lane_start: usize,
    lane_end: usize,
    start: usize,
) -> std::result::Result<(usize, Option<Value>), RuntimeCallError> {
    if lanes.is_empty() || lane_start >= lane_end || lane_end > lanes.len() {
        return Err(RuntimeCallError::Unavailable(anyhow!(
            "MCP runtime pool has no eligible lanes"
        )));
    }

    let lane_count = lane_end - lane_start;
    let start_offset = if (lane_start..lane_end).contains(&start) {
        start - lane_start
    } else {
        start % lane_count
    };
    let request = runtime_request_summary(&payload, default_workspace);
    let deadline = Instant::now() + MCP_RUNTIME_LANE_WAIT_TIMEOUT;
    let mut payload = Some(payload);
    loop {
        // Prefer the routed warm lane before spilling or growing the selected pool.
        for offset in 0..lane_count {
            let lane_index = lane_start + (start_offset + offset) % lane_count;
            let lane = &lanes[lane_index];
            match lane.try_lock() {
                Ok(mut runtime) if runtime.is_some() => {
                    let response = call_locked_runtime(
                        &mut runtime,
                        default_workspace,
                        payload
                            .take()
                            .expect("payload consumed only after lane selection"),
                        lane_index,
                    )?;
                    return Ok((lane_index, response));
                }
                Ok(_) | Err(std::sync::TryLockError::WouldBlock) => {}
                Err(std::sync::TryLockError::Poisoned(_)) => continue,
            }
        }

        // Grow only when every warm eligible lane is currently busy. Rotate the cold
        // lane choice for fairness under real concurrency, but never pay the
        // process/startup cost merely because the next_lane cursor advanced.
        for offset in 0..lane_count {
            let lane_index = lane_start + (start_offset + offset) % lane_count;
            match lanes[lane_index].try_lock() {
                Ok(mut runtime) => {
                    let response = call_locked_runtime(
                        &mut runtime,
                        default_workspace,
                        payload
                            .take()
                            .expect("payload consumed only after lane selection"),
                        lane_index,
                    )?;
                    return Ok((lane_index, response));
                }
                Err(std::sync::TryLockError::WouldBlock) => continue,
                Err(std::sync::TryLockError::Poisoned(_)) => continue,
            }
        }

        if Instant::now() >= deadline {
            return Err(RuntimeCallError::Unavailable(anyhow!(
                "MCP runtime pool stayed busy for {} ms; retry shortly ({request})",
                MCP_RUNTIME_LANE_WAIT_TIMEOUT.as_millis()
            )));
        }
        thread::sleep(MCP_RUNTIME_LANE_RETRY_DELAY);
    }
}

fn call_locked_runtime(
    slot: &mut Option<SessionRuntime>,
    default_workspace: &Path,
    payload: Value,
    lane_index: usize,
) -> std::result::Result<Option<Value>, RuntimeCallError> {
    if slot.is_none() {
        *slot = Some(
            SessionRuntime::new(default_workspace.to_path_buf()).map_err(|error| {
                RuntimeCallError::Unavailable(
                    error.context(format!("initialize MCP runtime lane {lane_index}")),
                )
            })?,
        );
    }
    slot.as_mut()
        .expect("runtime initialized")
        .handle(payload)
        .map_err(|error| {
            RuntimeCallError::Unavailable(error.context(format!("MCP runtime lane {lane_index}")))
        })
}

fn runtime_pool_busy_error(lane_index: usize, wait: Duration, request: &str) -> RuntimeCallError {
    RuntimeCallError::Unavailable(anyhow!(
        "MCP runtime lane {lane_index} stayed busy for {} ms; retry shortly ({request})",
        wait.as_millis()
    ))
}

fn legacy_request_info(payload: &Value, default_workspace: &Path) -> LegacyRequestInfo {
    let mut handles = Vec::new();
    let mut sticky_keys = Vec::new();
    let mut preferred_keys = Vec::new();
    collect_legacy_request_info(
        payload,
        default_workspace,
        &mut handles,
        &mut sticky_keys,
        &mut preferred_keys,
    );
    handles.sort();
    handles.dedup();
    sticky_keys.sort();
    sticky_keys.dedup();
    preferred_keys.sort();
    preferred_keys.dedup();
    let sticky_key = affinity_key(&sticky_keys);
    let preferred_key = affinity_key(&preferred_keys);
    LegacyRequestInfo {
        handles,
        sticky_key,
        preferred_key,
    }
}

fn affinity_key(keys: &[String]) -> Option<String> {
    match keys {
        [] => None,
        [single] => Some(single.clone()),
        many => Some(format!("batch:{}", many.join("|"))),
    }
}

fn collect_legacy_request_info(
    payload: &Value,
    default_workspace: &Path,
    handles: &mut Vec<String>,
    sticky_keys: &mut Vec<String>,
    preferred_keys: &mut Vec<String>,
) {
    let mut pending = vec![payload];
    while let Some(payload) = pending.pop() {
        if let Value::Array(items) = payload {
            pending.extend(items.iter().rev());
            continue;
        }
        let Some(object) = payload.as_object() else {
            continue;
        };
        if object.get("method").and_then(Value::as_str) != Some("tools/call") {
            continue;
        }
        let Some(params) = object.get("params").and_then(Value::as_object) else {
            continue;
        };
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let arguments = params
            .get("arguments")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        collect_named_handles(&Value::Object(arguments.clone()), handles);
        if matches!(name, "artifact_info" | "read_artifact" | "search_artifact")
            && let Some(id) = arguments.get("id").and_then(Value::as_str)
        {
            handles.push(id.to_owned());
        }

        let sticky_domain = match name {
            "computer_use" | "computer_use_reset" | "desktop_control" | "desktop_control_reset" => {
                Some("computer")
            }
            "run_shell" if arguments.get("background").and_then(Value::as_bool) == Some(true) => {
                Some("shell")
            }
            "shell_job" if arguments.get("jobId").and_then(Value::as_str).is_none() => {
                Some("shell")
            }
            _ => None,
        };
        if let Some(domain) = sticky_domain {
            let workspace = legacy_workspace_key(&arguments, default_workspace);
            sticky_keys.push(format!("{domain}:{workspace}"));
        }

        if matches!(
            name,
            "read_file" | "read_files" | "apply_file_edits" | "list_files" | "search_workspace"
        ) {
            let workspace = legacy_workspace_key(&arguments, default_workspace);
            preferred_keys.push(format!("edit:{workspace}"));
        }
    }
}

fn legacy_workspace_key(
    arguments: &serde_json::Map<String, Value>,
    default_workspace: &Path,
) -> String {
    let requested = arguments
        .get("workspace")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let path = requested
        .map(PathBuf::from)
        .unwrap_or_else(|| default_workspace.to_path_buf());
    let path = if path.is_absolute() {
        path
    } else {
        default_workspace.join(path)
    };
    path.canonicalize()
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn collect_named_handles(value: &Value, handles: &mut Vec<String>) {
    // Use an explicit work stack because both requests and tool results are
    // model-controlled JSON. Recursive traversal can otherwise turn a deeply
    // nested but valid value into a process-wide Rust stack overflow.
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(items) => pending.extend(items.iter().rev()),
            Value::Object(object) => {
                for (key, value) in object.iter().rev() {
                    if matches!(key.as_str(), "snapshot" | "artifactId" | "jobId")
                        && let Some(handle) = value.as_str()
                        && !handle.is_empty()
                    {
                        handles.push(handle.to_owned());
                    }
                    pending.push(value);
                }
            }
            _ => {}
        }
    }
}

fn collect_response_handles(value: &Value, handles: &mut Vec<String>) {
    collect_named_handles(value, handles);
    handles.sort();
    handles.dedup();
}

struct McpRuntimeManager {
    default_workspace: PathBuf,
    legacy: RuntimeLanePool,
    sessions: Mutex<HashMap<String, Arc<SessionRuntimePool>>>,
}

#[derive(Debug, Default)]
struct RuntimeHealth {
    total: usize,
    available: usize,
    busy: usize,
    dead: usize,
}

impl RuntimeHealth {
    fn healthy(&self) -> bool {
        self.available + self.busy > 0
    }

    fn ready(&self) -> bool {
        self.available > 0
    }

    fn as_json(&self) -> Value {
        json!({
            "healthy": self.healthy(),
            "ready": self.ready(),
            "total": self.total,
            "available": self.available,
            "busy": self.busy,
            "dead": self.dead,
        })
    }
}

#[derive(Debug)]
enum RuntimeCallError {
    Unavailable(anyhow::Error),
}

enum RuntimeTarget {
    Session(Arc<SessionRuntimePool>),
    Legacy,
}

impl McpRuntimeManager {
    fn new(default_workspace: PathBuf) -> Result<Self> {
        let legacy = RuntimeLanePool::new(default_workspace.clone())?;
        Ok(Self {
            default_workspace,
            legacy,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    fn call_legacy(&self, payload: Value) -> std::result::Result<Option<Value>, RuntimeCallError> {
        self.legacy.call(payload)
    }

    fn get_session(&self, id: &str) -> Result<Option<Arc<SessionRuntimePool>>> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow!("MCP session table lock poisoned"))?;
        let session = sessions.get(id).cloned();
        if let Some(session) = session.as_ref()
            && let Ok(mut last_used) = session.last_used.lock()
        {
            *last_used = Instant::now();
        }
        Ok(session)
    }

    fn create_session(&self) -> Result<(String, Arc<SessionRuntimePool>)> {
        {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| anyhow!("MCP session table lock poisoned"))?;
            prune_idle_sessions(&mut sessions);
            if sessions.len() >= MAX_MCP_SESSIONS {
                bail!(
                    "Yeet MCP session capacity reached ({MAX_MCP_SESSIONS}); retry after an idle session expires"
                );
            }
        }

        let runtime = Arc::new(SessionRuntimePool::new(self.default_workspace.clone())?);
        let id = Uuid::new_v4().to_string();
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow!("MCP session table lock poisoned"))?;
        prune_idle_sessions(&mut sessions);
        if sessions.len() >= MAX_MCP_SESSIONS {
            bail!(
                "Yeet MCP session capacity reached ({MAX_MCP_SESSIONS}); retry after an idle session expires"
            );
        }
        sessions.insert(id.clone(), Arc::clone(&runtime));
        Ok((id, runtime))
    }

    fn remove_session(&self, id: &str) -> Result<bool> {
        let removed = self
            .sessions
            .lock()
            .map_err(|_| anyhow!("MCP session table lock poisoned"))?
            .remove(id)
            .is_some();
        Ok(removed)
    }

    fn health(&self) -> RuntimeHealth {
        let mut health = RuntimeHealth::default();
        self.legacy.add_health(&mut health);
        if let Ok(mut sessions) = self.sessions.lock() {
            prune_idle_sessions(&mut sessions);
            for runtime in sessions.values() {
                inspect_runtime_health(runtime, &mut health);
            }
        }
        health
    }
}

fn prune_idle_sessions(sessions: &mut HashMap<String, Arc<SessionRuntimePool>>) {
    sessions.retain(|_, runtime| {
        if Arc::strong_count(runtime) > 1 {
            return true;
        }
        !runtime.is_idle_for(MCP_SESSION_IDLE_TTL)
    });
}

fn inspect_runtime_health(runtime: &Arc<SessionRuntimePool>, health: &mut RuntimeHealth) {
    runtime.add_health(health);
}

fn call_session_runtime(
    runtime: &Arc<SessionRuntimePool>,
    payload: Value,
) -> std::result::Result<Option<Value>, RuntimeCallError> {
    runtime.call(payload)
}

fn bind_reusable(host: &str, port: u16) -> Result<TcpListener> {
    let addresses = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("resolve Yeet MCP bind address {host}:{port}"))?;
    let mut last_error = None;
    for address in addresses {
        let socket = Socket::new(
            Domain::for_address(address),
            Type::STREAM,
            Some(Protocol::TCP),
        )?;
        #[cfg(not(windows))]
        socket.set_reuse_address(true)?;
        if let Err(error) = socket.bind(&address.into()) {
            last_error = Some(error);
            continue;
        }
        socket.listen(128)?;
        return Ok(socket.into());
    }
    Err(last_error
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow!("no usable bind address for {host}:{port}")))
}

pub(super) fn ensure_bind_available(host: &str, port: u16) -> Result<()> {
    drop(bind_reusable(host, port)?);
    Ok(())
}

fn handle_connection(
    mut stream: TcpStream,
    runtimes: Arc<McpRuntimeManager>,
    oauth: OAuthRuntime,
    public_url: Url,
    issuer: Url,
) {
    // The listening socket is nonblocking so the daemon can poll its stop flag.
    // On macOS an accepted socket can retain that mode, which makes a normal
    // request read fail immediately with EAGAIN (os error 35). Restore blocking
    // mode before applying the actual per-connection read/write timeouts.
    if let Err(error) = stream.set_nonblocking(false) {
        eprintln!("yeet mcpserver: set accepted socket blocking mode: {error}");
        return;
    }
    if let Err(error) = stream.set_read_timeout(Some(CONNECTION_TIMEOUT)) {
        eprintln!("yeet mcpserver: set read timeout: {error}");
        return;
    }
    if let Err(error) = stream.set_write_timeout(Some(CONNECTION_TIMEOUT)) {
        eprintln!("yeet mcpserver: set write timeout: {error}");
        return;
    }
    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(error) => {
            let response = HttpResponse::json(
                request_error_status(&error),
                json!({"error":"invalid_request","error_description":error.to_string()}),
            );
            if let Err(write_error) = write_response(&mut stream, response) {
                if !is_peer_disconnect(&write_error) {
                    eprintln!(
                        "yeet mcpserver: {error}; failed to write error response: {write_error}"
                    );
                }
            } else {
                eprintln!("yeet mcpserver: {error}");
            }
            return;
        }
    };
    let target = request.target.clone();
    let response =
        route_request(request, runtimes, oauth, public_url, issuer).unwrap_or_else(|error| {
            let status = if target.starts_with("/authorize")
                || target.starts_with("/token")
                || target.starts_with("/register")
                || target.starts_with("/mcp")
            {
                400
            } else {
                500
            };
            HttpResponse::json(
                status,
                json!({"error":"invalid_request","error_description":error.to_string()}),
            )
        });
    if let Err(error) = write_response(&mut stream, response)
        && !is_peer_disconnect(&error)
    {
        eprintln!("yeet mcpserver: write response: {error}");
    }
}

fn is_peer_disconnect(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
            )
        })
    })
}

fn request_error_status(error: &anyhow::Error) -> u16 {
    if error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            )
        })
    }) {
        408
    } else {
        400
    }
}

fn route_request(
    request: HttpRequest,
    runtimes: Arc<McpRuntimeManager>,
    oauth: OAuthRuntime,
    public_url: Url,
    issuer: Url,
) -> Result<HttpResponse> {
    let (path, query) = split_target(&request.target);
    match (request.method.as_str(), path) {
        ("GET", "/health") => {
            let health = runtimes.health();
            // Treat a fully occupied runtime pool as unavailable here as well as
            // on /ready. Some reverse proxies only support one health endpoint;
            // returning 200 with zero admission capacity made them keep routing
            // traffic into an already saturated MCP server.
            let status = if health.ready() { 200 } else { 503 };
            if status != 200 {
                eprintln!(
                    "yeet mcpserver: runtime health unavailable: available={} busy={} dead={} total={}",
                    health.available, health.busy, health.dead, health.total
                );
            }
            Ok(HttpResponse::json(
                status,
                json!({
                    "ok": health.ready(),
                    "alive": health.healthy(),
                    "service":"yeet-mcpserver",
                    "mcp":public_url,
                    "runtime":health.as_json(),
                }),
            ))
        }
        ("GET", "/ready") => {
            let health = runtimes.health();
            let status = if health.ready() { 200 } else { 503 };
            Ok(HttpResponse::json(
                status,
                json!({
                    "ok": health.ready(),
                    "service":"yeet-mcpserver",
                    "mcp":public_url,
                    "runtime":health.as_json(),
                }),
            ))
        }
        ("GET", "/.well-known/oauth-protected-resource")
        | ("GET", "/.well-known/oauth-protected-resource/mcp") => {
            Ok(HttpResponse::json(200, oauth.protected_resource_metadata()))
        }
        ("GET", "/.well-known/oauth-authorization-server") => Ok(HttpResponse::json(
            200,
            oauth.authorization_server_metadata(),
        )),
        ("POST", "/register") => {
            ensure_oauth_mode(&oauth)?;
            let body: Value = serde_json::from_slice(&request.body)
                .context("decode OAuth client registration")?;
            Ok(HttpResponse::json(201, oauth.register_client(&body)?))
        }
        ("GET", "/authorize") => {
            ensure_oauth_mode(&oauth)?;
            let params = parse_form(query.as_bytes());
            let authorization = oauth.parse_authorization_request(&params)?;
            Ok(HttpResponse::html(200, authorization_page(&authorization)))
        }
        ("POST", "/authorize") => {
            ensure_oauth_mode(&oauth)?;
            let params = parse_form(&request.body);
            let key = params
                .get("access_key")
                .ok_or_else(|| anyhow!("missing MCP authorization key"))?;
            let authorization = oauth.parse_authorization_request(&params)?;
            let redirect = oauth.authorize_with_key(authorization, key)?;
            Ok(HttpResponse::redirect(redirect.as_str()))
        }
        ("POST", "/token") => {
            ensure_oauth_mode(&oauth)?;
            let params = parse_form(&request.body);
            match oauth.exchange_token(&params) {
                Ok(token) => Ok(HttpResponse::json(200, token)),
                Err(error) => Ok(HttpResponse::json(
                    400,
                    json!({"error":"invalid_grant","error_description":error.to_string()}),
                )),
            }
        }
        ("POST", "/mcp") => {
            let auth = oauth.auth_store().status()?;
            if !mcp_authorized(&request.headers, &oauth, auth.mode, auth.oauth_enabled)? {
                return Ok(unauthorized_response(
                    auth.mode,
                    auth.oauth_enabled,
                    &issuer,
                ));
            }
            let mut payload: Value =
                serde_json::from_slice(&request.body).context("decode MCP JSON-RPC request")?;
            if !mcp_json_nesting_within_limit(&payload) {
                return Ok(HttpResponse::json(
                    400,
                    json!({
                        "error":"invalid_request",
                        "error_description":"MCP request nesting exceeds the server safety limit"
                    }),
                ));
            }
            if let Some(version) = request.headers.get("mcp-protocol-version") {
                attach_protocol_version(&mut payload, version);
            }
            let requested_session = request.headers.get(MCP_SESSION_HEADER).cloned();
            let (target, created_session) = match requested_session.as_deref() {
                Some(id) => match runtimes.get_session(id)? {
                    Some(runtime) => (RuntimeTarget::Session(runtime), None),
                    None => {
                        return Ok(HttpResponse::json(
                            404,
                            json!({
                                "error":"invalid_session",
                                "error_description":"MCP session is unknown or expired; initialize a new session",
                            }),
                        ));
                    }
                },
                None if payload_contains_method(&payload, "initialize") => {
                    match runtimes.create_session() {
                        Ok((id, runtime)) => (RuntimeTarget::Session(runtime), Some(id)),
                        Err(error) => {
                            let description = error.to_string();
                            eprintln!("yeet mcpserver: cannot create MCP session: {description}");
                            return Ok(mcp_jsonrpc_error_response(
                                &payload,
                                MCP_SESSION_CAPACITY_ERROR_CODE,
                                format!(
                                    "Yeet MCP session capacity is temporarily exhausted; retry shortly: {description}"
                                ),
                            ));
                        }
                    }
                }
                None => (RuntimeTarget::Legacy, None),
            };
            let error_shape = JsonRpcErrorShape::from_payload(&payload);
            let call = match target {
                RuntimeTarget::Session(runtime) => call_session_runtime(&runtime, payload),
                RuntimeTarget::Legacy => runtimes.call_legacy(payload),
            };
            let response = match call {
                Ok(response) => response,
                Err(RuntimeCallError::Unavailable(error)) => {
                    if let Some(session_id) = created_session.as_deref() {
                        let _ = runtimes.remove_session(session_id);
                    }
                    let description = format!("{error:#}");
                    eprintln!("yeet mcpserver: MCP runtime unavailable: {description}");
                    return Ok(error_shape.into_response(
                        MCP_RUNTIME_UNAVAILABLE_ERROR_CODE,
                        format!("Yeet MCP runtime is temporarily unavailable; retry shortly: {description}"),
                    ));
                }
            };
            match response {
                Some(value) => {
                    let mut response = HttpResponse::json(200, value);
                    response.headers.push((
                        "MCP-Protocol-Version".into(),
                        request
                            .headers
                            .get("mcp-protocol-version")
                            .cloned()
                            .unwrap_or_else(|| MODERN_PROTOCOL_VERSION.into()),
                    ));
                    if let Some(session_id) = created_session {
                        response.headers.push(("Mcp-Session-Id".into(), session_id));
                    }
                    Ok(response)
                }
                None => {
                    let mut response = HttpResponse::empty(202);
                    if let Some(session_id) = created_session {
                        response.headers.push(("Mcp-Session-Id".into(), session_id));
                    }
                    Ok(response)
                }
            }
        }
        ("DELETE", "/mcp") => {
            let auth = oauth.auth_store().status()?;
            if !mcp_authorized(&request.headers, &oauth, auth.mode, auth.oauth_enabled)? {
                return Ok(unauthorized_response(
                    auth.mode,
                    auth.oauth_enabled,
                    &issuer,
                ));
            }
            let Some(session_id) = request.headers.get(MCP_SESSION_HEADER) else {
                return Ok(HttpResponse::json(
                    400,
                    json!({
                        "error":"invalid_session",
                        "error_description":"Mcp-Session-Id is required to terminate an MCP session",
                    }),
                ));
            };
            if runtimes.remove_session(session_id)? {
                Ok(HttpResponse::empty(204))
            } else {
                Ok(HttpResponse::json(
                    404,
                    json!({
                        "error":"invalid_session",
                        "error_description":"MCP session is unknown or already expired",
                    }),
                ))
            }
        }
        ("GET", "/mcp") => Ok(HttpResponse::text(
            405,
            "MCP uses HTTP POST. The legacy HTTP+SSE GET transport is not served.",
        )),
        _ => Ok(HttpResponse::text(404, "Not found")),
    }
}

fn mcp_authorized(
    headers: &HashMap<String, String>,
    oauth: &OAuthRuntime,
    auth_mode: AuthMode,
    oauth_enabled: bool,
) -> Result<bool> {
    let header = headers.get("authorization").map(String::as_str);
    match auth_mode {
        AuthMode::None => Ok(true),
        AuthMode::Key => {
            let Some(token) = bearer_token(header) else {
                return Ok(false);
            };
            if oauth.auth_store().verify_key(token)? {
                return Ok(true);
            }
            if oauth_enabled {
                oauth.verify_oauth_token(token)
            } else {
                Ok(false)
            }
        }
        AuthMode::Oauth => bearer_token(header)
            .map(|token| oauth.verify_oauth_token(token))
            .transpose()
            .map(|value| value.unwrap_or(false)),
    }
}

fn payload_contains_method(payload: &Value, method: &str) -> bool {
    let mut pending = vec![payload];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(items) => pending.extend(items.iter()),
            Value::Object(object)
                if object.get("method").and_then(Value::as_str) == Some(method) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

struct McpProcess {
    default_workspace: PathBuf,
    child: Child,
    stdin: BufWriter<ChildStdin>,
    responses: Receiver<std::result::Result<String, String>>,
}

impl McpProcess {
    fn new(default_workspace: PathBuf) -> Result<Self> {
        let mut process = Self::spawn(default_workspace)?;
        // Prove the child is ready before advertising the HTTP server. This also
        // makes daemon startup fail fast if the stdio runtime cannot initialize.
        let response = process.call_with_timeout(
            json!({"jsonrpc":"2.0","id":"http-runtime-health","method":"initialize","params":{}}),
            MCP_RUNTIME_INITIALIZE_TIMEOUT,
        )?;
        if response.get("error").is_some() {
            bail!("Yeet MCP stdio runtime failed initialization: {response}");
        }
        Ok(process)
    }

    fn spawn(default_workspace: PathBuf) -> Result<Self> {
        let executable = std::env::current_exe().context("locate Yeet executable")?;
        let mut child = Command::new(executable)
            .arg("mcpserver")
            .arg("stdio")
            .arg("--workspace")
            .arg(&default_workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("start isolated Yeet MCP stdio runtime")?;
        let stdin = child
            .stdin
            .take()
            .context("MCP runtime stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("MCP runtime stdout unavailable")?;
        let (response_tx, responses) = mpsc::channel();
        thread::Builder::new()
            .name("yeet-mcp-runtime-reader".into())
            .spawn(move || runtime_stdout_reader(stdout, response_tx))
            .context("start MCP runtime stdout reader")?;
        Ok(Self {
            default_workspace,
            child,
            stdin: BufWriter::new(stdin),
            responses,
        })
    }

    fn handle(&mut self, payload: Value) -> Result<Option<Value>> {
        let expects_response = payload
            .as_object()
            .is_none_or(|object| object.contains_key("id"));
        if !expects_response {
            return match self.send(payload) {
                Ok(()) => Ok(None),
                Err(error) => {
                    let restart = self.restart();
                    match restart {
                        Ok(()) => Err(anyhow!(
                            "isolated MCP runtime failed while sending a notification and was restarted: {error}"
                        )),
                        Err(restart_error) => Err(anyhow!(
                            "isolated MCP runtime failed while sending a notification: {error}; restart also failed: {restart_error}"
                        )),
                    }
                }
            };
        }
        let timeout = runtime_timeout_for_payload(&payload);
        match self.call_with_timeout(payload, timeout) {
            Ok(response) => Ok(Some(response)),
            Err(error) => {
                // A Rust stack overflow aborts the runtime process and cannot be
                // caught in-process. Recreate it for the next request, but never
                // replay the failed request because tool calls may be mutating.
                let restart = self.restart();
                match restart {
                    Ok(()) => Err(anyhow!(
                        "isolated MCP tool runtime failed and was restarted; the failed request was not replayed: {error}"
                    )),
                    Err(restart_error) => Err(anyhow!(
                        "isolated MCP tool runtime failed: {error}; restart also failed: {restart_error}"
                    )),
                }
            }
        }
    }

    fn call_with_timeout(&mut self, payload: Value, timeout: Duration) -> Result<Value> {
        let request = runtime_request_summary(&payload, &self.default_workspace);
        let pid = self.child.id();
        self.send(payload)?;
        match self.responses.recv_timeout(timeout) {
            Ok(Ok(line)) => serde_json::from_str(&line).with_context(|| {
                format!("decode MCP stdio runtime response ({request}; pid={pid})")
            }),
            Ok(Err(error)) => {
                let status = self
                    .child
                    .try_wait()?
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "unknown".into());
                bail!(
                    "MCP stdio runtime stdout failed ({error}; status {status}; {request}; pid={pid})"
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => bail!(
                "MCP stdio runtime response timed out after {} seconds ({request}; pid={pid})",
                timeout.as_secs(),
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let status = self
                    .child
                    .try_wait()?
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "unknown".into());
                bail!(
                    "MCP stdio runtime reader disconnected (status {status}; {request}; pid={pid})"
                );
            }
        }
    }

    fn send(&mut self, payload: Value) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, &payload)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn restart(&mut self) -> Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let replacement = Self::new(self.default_workspace.clone())?;
        *self = replacement;
        Ok(())
    }

    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

fn runtime_stdout_reader(
    stdout: ChildStdout,
    sender: mpsc::Sender<std::result::Result<String, String>>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                let _ = sender.send(Err("stdout closed".into()));
                return;
            }
            Ok(_) if line.trim().is_empty() => continue,
            Ok(_) => {
                if sender.send(Ok(line)).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(error.to_string()));
                return;
            }
        }
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ensure_oauth_mode(oauth: &OAuthRuntime) -> Result<()> {
    if !oauth.auth_store().status()?.oauth_enabled {
        bail!("OAuth authentication is not enabled for this MCP daemon");
    }
    Ok(())
}

fn unauthorized_response(mode: AuthMode, oauth_enabled: bool, issuer: &Url) -> HttpResponse {
    let mut response = HttpResponse::json(401, json!({"error":"unauthorized"}));
    let challenge = if oauth_enabled || mode == AuthMode::Oauth {
        let metadata = issuer
            .join(".well-known/oauth-protected-resource")
            .expect("metadata URL");
        format!("Bearer resource_metadata=\"{}\", scope=\"mcp\"", metadata)
    } else {
        "Bearer".into()
    };
    response
        .headers
        .push(("WWW-Authenticate".into(), challenge));
    response
}

fn attach_protocol_version(payload: &mut Value, version: &str) {
    if let Some(items) = payload.as_array_mut() {
        for item in items {
            attach_protocol_version(item, version);
        }
        return;
    }
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    let params = object.entry("params").or_insert_with(|| json!({}));
    let Some(params) = params.as_object_mut() else {
        return;
    };
    let meta = params.entry("_meta").or_insert_with(|| json!({}));
    let Some(meta) = meta.as_object_mut() else {
        return;
    };
    meta.entry("io.modelcontextprotocol/protocolVersion")
        .or_insert_with(|| json!(version));
}

fn authorization_page(request: &AuthorizationRequest) -> String {
    let state = request.state.as_deref().unwrap_or("");
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Authorize Yeet MCP</title>\
<style>body{{font:16px system-ui;max-width:620px;margin:10vh auto;padding:24px}}input,button{{font:inherit;padding:10px;width:100%;box-sizing:border-box}}code{{overflow-wrap:anywhere}}</style>\
<h1>Authorize Yeet MCP</h1><p>Client: <code>{}</code></p><p>Resource: <code>{}</code></p>\
<form method=\"post\" action=\"/authorize\">\
<input type=\"hidden\" name=\"response_type\" value=\"code\">\
<input type=\"hidden\" name=\"client_id\" value=\"{}\">\
<input type=\"hidden\" name=\"redirect_uri\" value=\"{}\">\
<input type=\"hidden\" name=\"state\" value=\"{}\">\
<input type=\"hidden\" name=\"code_challenge\" value=\"{}\">\
<input type=\"hidden\" name=\"code_challenge_method\" value=\"S256\">\
<input type=\"hidden\" name=\"resource\" value=\"{}\">\
<input type=\"hidden\" name=\"scope\" value=\"{}\">\
<label>Authorization key<br><input autofocus type=\"password\" name=\"access_key\" autocomplete=\"current-password\" required></label><p><button type=\"submit\">Authorize</button></p></form>",
        html_escape(&request.client_id),
        html_escape(&request.resource),
        html_escape(&request.client_id),
        html_escape(&request.redirect_uri),
        html_escape(state),
        html_escape(&request.code_challenge),
        html_escape(&request.resource),
        html_escape(&request.scope),
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn inferred_public_url(bind_host: &str, address: SocketAddr) -> Result<Url> {
    if matches!(bind_host, "0.0.0.0" | "::") {
        bail!(
            "--public-url is required for wildcard MCP binds; use the externally reachable /mcp URL"
        );
    }
    let host = if bind_host.is_empty() {
        address.ip().to_string()
    } else {
        bind_host.to_owned()
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    Url::parse(&format!("http://{host}:{}/mcp", address.port())).map_err(Into::into)
}

fn normalize_public_url(mut url: Url) -> Result<Url> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("--public-url must be an HTTP(S) MCP endpoint without credentials/query/fragment");
    }
    if url.path() == "/" || url.path().is_empty() {
        url.set_path("/mcp");
    }
    Ok(url)
}

fn issuer_for_resource(resource: &Url) -> Result<Url> {
    let mut issuer = resource.clone();
    issuer.set_path("/");
    issuer.set_query(None);
    issuer.set_fragment(None);
    Ok(issuer)
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            bail!("HTTP connection closed before request headers");
        }
        buffer.extend_from_slice(&chunk[..count]);
        if buffer.len() > MAX_HEADER_BYTES {
            bail!("HTTP headers exceed {MAX_HEADER_BYTES} bytes");
        }
        if let Some(index) = find_bytes(&buffer, b"\r\n\r\n") {
            break index + 4;
        }
    };
    let header_text = std::str::from_utf8(&buffer[..header_end - 4])?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| anyhow!("missing HTTP request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing HTTP method"))?
        .to_owned();
    let target = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing HTTP target"))?
        .to_owned();
    let version = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing HTTP version"))?;
    if !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        bail!("unsupported HTTP version: {version}");
    }
    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow!("malformed HTTP header"))?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
    }
    let content_length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>())
        .transpose()
        .context("invalid Content-Length")?
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        bail!("HTTP body exceeds {MAX_BODY_BYTES} bytes");
    }
    while buffer.len() - header_end < content_length {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            bail!("HTTP connection closed before request body completed");
        }
        buffer.extend_from_slice(&chunk[..count]);
    }
    Ok(HttpRequest {
        method,
        target,
        headers,
        body: buffer[header_end..header_end + content_length].to_vec(),
    })
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn split_target(target: &str) -> (&str, &str) {
    target.split_once('?').unwrap_or((target, ""))
}

fn parse_form(bytes: &[u8]) -> HashMap<String, String> {
    form_urlencoded::parse(bytes)
        .into_owned()
        .collect::<HashMap<_, _>>()
}

struct HttpResponse {
    status: u16,
    content_type: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            headers: Vec::new(),
            body: serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec()),
        }
    }

    fn text(status: u16, value: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            headers: Vec::new(),
            body: value.as_bytes().to_vec(),
        }
    }

    fn html(status: u16, value: String) -> Self {
        Self {
            status,
            content_type: "text/html; charset=utf-8",
            headers: Vec::new(),
            body: value.into_bytes(),
        }
    }

    fn redirect(location: &str) -> Self {
        Self {
            status: 302,
            content_type: "text/plain; charset=utf-8",
            headers: vec![("Location".into(), location.into())],
            body: b"Redirecting".to_vec(),
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: "text/plain",
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

fn write_response(stream: &mut TcpStream, response: HttpResponse) -> Result<()> {
    let reason = match response.status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Response",
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len()
    )?;
    for (name, value) in response.headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(&response.body)?;
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_disconnect_errors_are_not_server_failures() {
        let error = anyhow::Error::from(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "peer left",
        ));
        assert!(is_peer_disconnect(&error));
    }

    #[test]
    fn protocol_header_is_added_to_request_metadata_without_overwriting_body() {
        let mut request = json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}});
        attach_protocol_version(&mut request, MODERN_PROTOCOL_VERSION);
        assert_eq!(
            request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            MODERN_PROTOCOL_VERSION
        );
        attach_protocol_version(&mut request, "old");
        assert_eq!(
            request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            MODERN_PROTOCOL_VERSION
        );
    }

    #[test]
    fn public_url_defaults_to_mcp_path() {
        let url = normalize_public_url(Url::parse("https://example.com/").unwrap()).unwrap();
        assert_eq!(url.as_str(), "https://example.com/mcp");
        assert_eq!(
            issuer_for_resource(&url).unwrap().as_str(),
            "https://example.com/"
        );
    }

    #[test]
    fn bind_availability_detects_an_existing_listener() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(ensure_bind_available("127.0.0.1", port).is_err());
        drop(listener);
        assert!(ensure_bind_available("127.0.0.1", port).is_ok());
    }

    #[test]
    fn payload_method_detection_handles_batches() {
        let payload = json!([
            {"jsonrpc":"2.0","id":1,"method":"ping"},
            {"jsonrpc":"2.0","id":2,"method":"initialize","params":{}}
        ]);
        assert!(payload_contains_method(&payload, "initialize"));
        assert!(!payload_contains_method(&payload, "tools/call"));
    }

    #[test]
    fn runtime_timeout_honors_long_shell_requests_without_unbounded_waits() {
        let payload = json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"tools/call",
            "params":{
                "name":"run_shell",
                "arguments":{"command":"sleep 1","timeoutSeconds":600}
            }
        });
        assert_eq!(
            runtime_timeout_for_payload(&payload),
            Duration::from_secs(630)
        );

        let oversized = json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"run_shell",
                "arguments":{"command":"sleep 1","timeoutSeconds":99_999}
            }
        });
        assert_eq!(
            runtime_timeout_for_payload(&oversized),
            MCP_RUNTIME_MAX_TIMEOUT
        );
    }

    #[test]
    fn runtime_pool_returns_busy_instead_of_blocking_forever() {
        let lanes = (0..2)
            .map(|_| Arc::new(Mutex::new(None)))
            .collect::<Vec<_>>();
        let _guards = lanes
            .iter()
            .map(|lane| lane.lock().unwrap())
            .collect::<Vec<_>>();
        let started = Instant::now();
        let result = call_flexible_runtime_bounded(
            &lanes,
            &PathBuf::from("/tmp"),
            json!({"jsonrpc":"2.0","id":1,"method":"ping"}),
            0,
            lanes.len(),
            0,
        );
        let RuntimeCallError::Unavailable(error) = result.unwrap_err();
        assert!(error.to_string().contains("runtime pool stayed busy"));
        assert!(started.elapsed() >= MCP_RUNTIME_LANE_WAIT_TIMEOUT);
        assert!(started.elapsed() < MCP_RUNTIME_LANE_WAIT_TIMEOUT + Duration::from_secs(1));
    }

    #[test]
    fn fixed_state_lanes_get_a_longer_queue_budget_than_flexible_admission() {
        assert!(MCP_RUNTIME_FIXED_LANE_WAIT_TIMEOUT > MCP_RUNTIME_LANE_WAIT_TIMEOUT);
        assert!(MCP_RUNTIME_FIXED_LANE_WAIT_TIMEOUT < MCP_RUNTIME_DEFAULT_TIMEOUT);
    }

    #[test]
    fn saturated_runtime_is_alive_but_not_ready_for_admission() {
        let health = RuntimeHealth {
            total: MAX_LEGACY_RUNTIMES,
            available: 0,
            busy: MAX_LEGACY_RUNTIMES,
            dead: 0,
        };
        assert!(health.healthy());
        assert!(!health.ready());
    }

    #[test]
    fn initialize_uses_short_runtime_timeout() {
        let payload = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        assert_eq!(
            runtime_timeout_for_payload(&payload),
            MCP_RUNTIME_INITIALIZE_TIMEOUT
        );
    }

    #[test]
    fn legacy_file_calls_prefer_workspace_lane_but_route_snapshot_by_handle() {
        let workspace = PathBuf::from("/tmp/yeet-mcp-affinity");
        let read = json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"tools/call",
            "params":{
                "name":"read_file",
                "arguments":{"path":"src/lib.rs","workspace":"/tmp/yeet-mcp-affinity"}
            }
        });
        let apply = json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{
                "name":"apply_file_edits",
                "arguments":{
                    "workspace":"/tmp/yeet-mcp-affinity",
                    "changes":[{"path":"src/lib.rs","snapshot":"snap-123","edits":[]}]
                }
            }
        });
        let read_info = legacy_request_info(&read, &workspace);
        let apply_info = legacy_request_info(&apply, &workspace);
        assert!(read_info.sticky_key.is_none());
        assert!(apply_info.sticky_key.is_none());
        assert_eq!(read_info.preferred_key, apply_info.preferred_key);
        assert_eq!(
            read_info.preferred_key.as_deref(),
            Some("edit:/tmp/yeet-mcp-affinity")
        );
        assert_eq!(apply_info.handles, vec!["snap-123"]);
    }

    #[test]
    fn new_preferred_route_is_bound_only_after_execution() {
        let key = "edit:/tmp/yeet-mcp-affinity".to_owned();
        let pool = RuntimeLanePool {
            default_workspace: PathBuf::from("/tmp"),
            lanes: vec![Arc::new(Mutex::new(None)), Arc::new(Mutex::new(None))],
            affinity: Mutex::new(LegacyAffinityState::default()),
        };
        let info = LegacyRequestInfo {
            handles: Vec::new(),
            sticky_key: None,
            preferred_key: Some(key.clone()),
        };
        let route = pool.route(&info, 0, pool.lanes.len());
        assert!(matches!(route, Ok(LegacyRoute::Flexible(_))));
        assert!(!pool.affinity.lock().unwrap().preferred.contains_key(&key));
    }

    #[test]
    fn legacy_foreground_shell_is_flexible_but_background_shell_is_sticky() {
        let workspace = PathBuf::from("/tmp/yeet-mcp-affinity");
        let foreground = json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_shell","arguments":{"command":"sleep 1"}}
        });
        let background = json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"run_shell","arguments":{"command":"sleep 1","background":true}}
        });
        assert!(
            legacy_request_info(&foreground, &workspace)
                .sticky_key
                .is_none()
        );
        assert!(
            legacy_request_info(&background, &workspace)
                .sticky_key
                .is_some()
        );
    }

    #[test]
    fn blocking_calls_are_capped_but_background_shells_are_not() {
        let foreground = json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_shell","arguments":{"command":"sleep 30"}}
        });
        let background = json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"run_shell","arguments":{"command":"sleep 30","background":true}}
        });
        let read = json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"read_file","arguments":{"path":"src/lib.rs"}}
        });
        let computer = json!({
            "jsonrpc":"2.0","id":4,"method":"tools/call",
            "params":{"name":"computer_use","arguments":{"code":"await cua.getState()"}}
        });
        assert!(payload_contains_blocking_tool_call(&foreground));
        assert!(!payload_contains_blocking_tool_call(&background));
        assert!(!payload_contains_blocking_tool_call(&read));
        assert!(payload_contains_blocking_tool_call(&computer));
    }

    #[test]
    fn blocking_routes_leave_reserved_lanes_out_of_rotation() {
        let lanes = (0..MAX_LEGACY_RUNTIMES)
            .map(|_| Arc::new(Mutex::new(None)))
            .collect::<Vec<_>>();
        let pool = RuntimeLanePool {
            default_workspace: PathBuf::from("/tmp"),
            lanes,
            affinity: Mutex::new(LegacyAffinityState {
                next_lane: MAX_BLOCKING_TOOL_RUNTIMES,
                ..Default::default()
            }),
        };
        let info = LegacyRequestInfo {
            handles: Vec::new(),
            sticky_key: None,
            preferred_key: None,
        };
        let route = match pool.route(&info, 0, MAX_BLOCKING_TOOL_RUNTIMES) {
            Ok(route) => route,
            Err(_) => panic!("blocking route unexpectedly failed"),
        };
        let LegacyRoute::Flexible(start) = route else {
            panic!("expected flexible route");
        };
        assert!(start < MAX_BLOCKING_TOOL_RUNTIMES);
        assert!(MAX_BLOCKING_TOOL_RUNTIMES < MAX_LEGACY_RUNTIMES);
    }


    #[test]
    fn short_stateful_work_is_routed_into_reserved_lanes() {
        let lanes = (0..MAX_LEGACY_RUNTIMES)
            .map(|_| Arc::new(Mutex::new(None)))
            .collect::<Vec<_>>();
        let pool = RuntimeLanePool {
            default_workspace: PathBuf::from("/tmp"),
            lanes,
            affinity: Mutex::new(LegacyAffinityState::default()),
        };
        let info = LegacyRequestInfo {
            handles: Vec::new(),
            sticky_key: None,
            preferred_key: Some("edit:/tmp/project".into()),
        };
        let route = pool
            .route(&info, MAX_BLOCKING_TOOL_RUNTIMES, MAX_LEGACY_RUNTIMES)
            .expect("short-call route should be available");
        let LegacyRoute::Flexible(start) = route else {
            panic!("expected flexible route");
        };
        assert!(start >= MAX_BLOCKING_TOOL_RUNTIMES);
        assert!(start < MAX_LEGACY_RUNTIMES);
    }

    #[test]
    fn runtime_unavailable_is_a_jsonrpc_response_not_an_http_5xx() {
        let payload = json!({"jsonrpc":"2.0","id":"call-1","method":"tools/call","params":{}});
        let response = mcp_jsonrpc_error_response(
            &payload,
            MCP_RUNTIME_UNAVAILABLE_ERROR_CODE,
            "retry shortly".into(),
        );
        assert_eq!(response.status, 200);
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["id"], "call-1");
        assert_eq!(body["error"]["code"], MCP_RUNTIME_UNAVAILABLE_ERROR_CODE);

        let notification = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        let response = mcp_jsonrpc_error_response(
            &notification,
            MCP_RUNTIME_UNAVAILABLE_ERROR_CODE,
            "retry shortly".into(),
        );
        assert_eq!(response.status, 202);
    }

    #[test]
    fn runtime_unavailable_preserves_batch_response_shape() {
        let payload = json!([
            {"jsonrpc":"2.0","id":1,"method":"ping"},
            {"jsonrpc":"2.0","method":"notifications/initialized"},
            {"jsonrpc":"2.0","id":2,"method":"ping"}
        ]);
        let response = mcp_jsonrpc_error_response(
            &payload,
            MCP_RUNTIME_UNAVAILABLE_ERROR_CODE,
            "retry shortly".into(),
        );
        assert_eq!(response.status, 200);
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        let items = body.as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], 1);
        assert_eq!(items[1]["id"], 2);
    }

    #[test]
    fn legacy_response_handle_collection_tracks_stateful_outputs() {
        let response = json!({
            "result":{
                "structuredContent":{
                    "result":{
                        "snapshot":"snap-1",
                        "artifactId":"artifact-1",
                        "nested":{"jobId":"job-1"}
                    }
                }
            }
        });
        let mut handles = Vec::new();
        collect_response_handles(&response, &mut handles);
        assert_eq!(handles, vec!["artifact-1", "job-1", "snap-1"]);
    }

    #[test]
    fn legacy_handle_collection_uses_bounded_process_stack() {
        let mut response = json!({"snapshot":"deep-snapshot"});
        for _ in 0..512 {
            response = json!({"nested":response});
        }
        let mut handles = Vec::new();
        collect_response_handles(&response, &mut handles);
        assert_eq!(handles, vec!["deep-snapshot"]);
    }
}
