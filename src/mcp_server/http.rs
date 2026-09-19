use std::{
    collections::{HashMap, HashSet, VecDeque},
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
mod wire;

use crate::platform::force_terminate_process_tree;
use runtime_support::{
    JsonRpcErrorShape, mcp_jsonrpc_error_response, payload_contains_blocking_tool_call,
    runtime_timeout_for_payload,
};
use wire::{
    HttpRequest, HttpResponse, inferred_public_url, issuer_for_resource, normalize_public_url,
    parse_form, read_request, split_target, write_response,
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
const MCP_LEGACY_RUNTIME_RETAIN_COUNT: usize = 4;
const MCP_LEGACY_RUNTIME_IDLE_TTL: Duration = Duration::from_secs(5 * 60);
const MCP_LEGACY_RUNTIME_REAP_INTERVAL: Duration = Duration::from_secs(30);
const MCP_LEGACY_HANDLE_IDLE_TTL: Duration = Duration::from_secs(30 * 60);
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
    pub restrict_workspace: bool,
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
        let runtimes = Arc::new(McpRuntimeManager::new(
            options.default_workspace,
            options.restrict_workspace,
        )?);
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
        let mut next_runtime_reap = Instant::now() + MCP_LEGACY_RUNTIME_REAP_INTERVAL;
        while !stop.load(std::sync::atomic::Ordering::Acquire) {
            if Instant::now() >= next_runtime_reap {
                self.runtimes.reap_idle_legacy_runtimes();
                next_runtime_reap = Instant::now() + MCP_LEGACY_RUNTIME_REAP_INTERVAL;
            }
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
    last_used: Instant,
}

impl SessionRuntime {
    fn new(default_workspace: PathBuf, restrict_workspace: bool) -> Result<Self> {
        Ok(Self {
            process: McpProcess::new(default_workspace, restrict_workspace)?,
            last_used: Instant::now(),
        })
    }

    fn handle(&mut self, payload: Value) -> Result<Option<Value>> {
        if !self.process.is_alive() {
            eprintln!(
                "yeet mcpserver: MCP session runtime was not alive; restarting before request"
            );
            self.process.restart()?;
        }
        let result = self.process.handle(payload);
        self.last_used = Instant::now();
        result
    }
}

struct SessionRuntimePool {
    runtimes: RuntimeLanePool,
    last_used: Mutex<Instant>,
}

impl SessionRuntimePool {
    fn new(default_workspace: PathBuf, restrict_workspace: bool) -> Result<Self> {
        Ok(Self {
            runtimes: RuntimeLanePool::new(default_workspace, restrict_workspace)?,
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
    restrict_workspace: bool,
    lanes: Vec<Arc<Mutex<Option<SessionRuntime>>>>,
    affinity: Mutex<LegacyAffinityState>,
}

#[derive(Clone, Copy)]
struct LegacyHandleAffinity {
    lane: usize,
    last_used: Instant,
}

#[derive(Default)]
struct LegacyAffinityState {
    handles: HashMap<String, LegacyHandleAffinity>,
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
    fn new(default_workspace: PathBuf, restrict_workspace: bool) -> Result<Self> {
        let mut lanes = Vec::with_capacity(MAX_LEGACY_RUNTIMES);
        lanes.push(Arc::new(Mutex::new(Some(SessionRuntime::new(
            default_workspace.clone(),
            restrict_workspace,
        )?))));
        for _ in 1..MAX_LEGACY_RUNTIMES {
            lanes.push(Arc::new(Mutex::new(None)));
        }
        Ok(Self {
            default_workspace,
            restrict_workspace,
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
        let use_reserved_short_lanes =
            !blocking && info.sticky_key.is_none() && blocking_end < self.lanes.len();
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
                    self.restrict_workspace,
                    payload,
                    lane,
                )?;
                (lane, response)
            }
            LegacyRoute::Flexible(start) => {
                let protected_lanes = self.sticky_lanes_in_range(lane_start, lane_end)?;
                call_flexible_runtime_bounded(
                    &self.lanes,
                    &self.default_workspace,
                    self.restrict_workspace,
                    payload,
                    lane_start..lane_end,
                    start,
                    &protected_lanes,
                )?
            }
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
        let now = Instant::now();
        affinity.prune_expired_handles(now);
        let mut fixed_lane = None;
        for handle in &info.handles {
            let Some(binding) = affinity.handles.get_mut(handle) else {
                continue;
            };
            binding.last_used = now;
            let lane = binding.lane;
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
                return Ok(LegacyRoute::Fixed(lane));
            }
            // Short file calls create daemon-local snapshot/artifact state. Bind the
            // workspace before execution so concurrent first-use requests cannot spill
            // across reserved lanes and later produce incompatible handles.
            affinity.preferred.remove(key);
            let lane = lane_start + affinity.next_lane % lane_count;
            affinity.next_lane = affinity.next_lane.wrapping_add(1);
            affinity.preferred.insert(key.clone(), lane);
            return Ok(LegacyRoute::Fixed(lane));
        }
        let start = lane_start + affinity.next_lane % lane_count;
        affinity.next_lane = affinity.next_lane.wrapping_add(1);
        Ok(LegacyRoute::Flexible(start))
    }

    fn sticky_lanes_in_range(
        &self,
        lane_start: usize,
        lane_end: usize,
    ) -> std::result::Result<HashSet<usize>, RuntimeCallError> {
        let affinity = self.affinity.lock().map_err(|_| {
            RuntimeCallError::Unavailable(anyhow!("legacy MCP affinity lock poisoned"))
        })?;
        Ok(affinity
            .sticky
            .values()
            .copied()
            .filter(|lane| (lane_start..lane_end).contains(lane))
            .collect())
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
        let now = Instant::now();
        affinity.prune_expired_handles(now);
        for handle in handles {
            if !affinity.handles.contains_key(&handle) {
                affinity.handle_order.push_back(handle.clone());
            }
            affinity.handles.insert(
                handle,
                LegacyHandleAffinity {
                    lane,
                    last_used: now,
                },
            );
        }
        while affinity.handles.len() > MAX_LEGACY_AFFINITY_HANDLES {
            let Some(oldest) = affinity.handle_order.pop_front() else {
                break;
            };
            affinity.handles.remove(&oldest);
        }
        Ok(())
    }

    fn reap_idle_legacy_runtimes(&self) -> usize {
        let now = Instant::now();
        let mut active_count = 0usize;
        let mut candidates = Vec::new();

        for (lane_index, lane) in self.lanes.iter().enumerate() {
            match lane.try_lock() {
                Ok(slot) => {
                    if let Some(runtime) = slot.as_ref() {
                        active_count += 1;
                        if now.saturating_duration_since(runtime.last_used)
                            >= MCP_LEGACY_RUNTIME_IDLE_TTL
                        {
                            candidates.push((runtime.last_used, lane_index));
                        }
                    }
                }
                Err(std::sync::TryLockError::WouldBlock)
                | Err(std::sync::TryLockError::Poisoned(_)) => active_count += 1,
            }
        }

        if active_count <= MCP_LEGACY_RUNTIME_RETAIN_COUNT {
            return 0;
        }
        candidates.sort_unstable_by_key(|(last_used, _)| *last_used);

        let mut retired = 0usize;
        for (_, lane_index) in candidates {
            if active_count <= MCP_LEGACY_RUNTIME_RETAIN_COUNT {
                break;
            }
            let Ok(mut slot) = self.lanes[lane_index].try_lock() else {
                continue;
            };
            let Some(runtime) = slot.as_ref() else {
                continue;
            };
            if now.saturating_duration_since(runtime.last_used) < MCP_LEGACY_RUNTIME_IDLE_TTL {
                continue;
            }
            let Ok(mut affinity) = self.affinity.lock() else {
                continue;
            };
            affinity.prune_expired_handles(now);
            if affinity.lane_is_pinned(lane_index) {
                continue;
            }
            affinity.forget_preferred_lane(lane_index);
            let runtime = slot.take();
            drop(affinity);
            drop(slot);
            drop(runtime);
            active_count -= 1;
            retired += 1;
        }
        retired
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
    restrict_workspace: bool,
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
                return call_locked_runtime(
                    &mut runtime,
                    default_workspace,
                    restrict_workspace,
                    payload,
                    lane_index,
                );
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
    restrict_workspace: bool,
    payload: Value,
    lane_range: std::ops::Range<usize>,
    start: usize,
    protected_lanes: &HashSet<usize>,
) -> std::result::Result<(usize, Option<Value>), RuntimeCallError> {
    let lane_start = lane_range.start;
    let lane_end = lane_range.end;
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
    let lane_order = (0..lane_count)
        .map(|offset| lane_start + (start_offset + offset) % lane_count)
        .collect::<Vec<_>>();
    let request = runtime_request_summary(&payload, default_workspace);
    let deadline = Instant::now() + MCP_RUNTIME_LANE_WAIT_TIMEOUT;
    let mut payload = Some(payload);
    loop {
        // Prefer lanes without sticky shell/computer state. Stateful lanes remain
        // fallback capacity when every unpinned lane is busy or unavailable.
        for protected_phase in [false, true] {
            // Prefer an already initialized idle runtime within this priority group.
            for &lane_index in &lane_order {
                if protected_lanes.contains(&lane_index) != protected_phase {
                    continue;
                }
                match lanes[lane_index].try_lock() {
                    Ok(mut runtime) if runtime.is_some() => {
                        let response = call_locked_runtime(
                            &mut runtime,
                            default_workspace,
                            restrict_workspace,
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

            // A cold unpinned slot is preferable to borrowing a lane that carries
            // stateful affinity; initialization cost is bounded and paid once.
            for &lane_index in &lane_order {
                if protected_lanes.contains(&lane_index) != protected_phase {
                    continue;
                }
                match lanes[lane_index].try_lock() {
                    Ok(mut runtime) => {
                        let response = call_locked_runtime(
                            &mut runtime,
                            default_workspace,
                            restrict_workspace,
                            payload
                                .take()
                                .expect("payload consumed only after lane selection"),
                            lane_index,
                        )?;
                        return Ok((lane_index, response));
                    }
                    Err(
                        std::sync::TryLockError::WouldBlock | std::sync::TryLockError::Poisoned(_),
                    ) => continue,
                }
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

fn runtime_pool_busy_error(lane_index: usize, wait: Duration, request: &str) -> RuntimeCallError {
    RuntimeCallError::Unavailable(anyhow!(
        "MCP runtime lane {lane_index} stayed busy for {} ms; retry shortly ({request})",
        wait.as_millis()
    ))
}

fn call_locked_runtime(
    slot: &mut Option<SessionRuntime>,
    default_workspace: &Path,
    restrict_workspace: bool,
    payload: Value,
    lane_index: usize,
) -> std::result::Result<Option<Value>, RuntimeCallError> {
    if slot.is_none() {
        *slot = Some(
            SessionRuntime::new(default_workspace.to_path_buf(), restrict_workspace).map_err(
                |error| {
                    RuntimeCallError::Unavailable(
                        error.context(format!("initialize MCP runtime lane {lane_index}")),
                    )
                },
            )?,
        );
    }
    slot.as_mut()
        .expect("runtime initialized")
        .handle(payload)
        .map_err(|error| {
            RuntimeCallError::Unavailable(error.context(format!("MCP runtime lane {lane_index}")))
        })
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
    restrict_workspace: bool,
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
    fn new(default_workspace: PathBuf, restrict_workspace: bool) -> Result<Self> {
        let legacy = RuntimeLanePool::new(default_workspace.clone(), restrict_workspace)?;
        Ok(Self {
            default_workspace,
            restrict_workspace,
            legacy,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    fn call_legacy(&self, payload: Value) -> std::result::Result<Option<Value>, RuntimeCallError> {
        self.legacy.call(payload)
    }

    fn reap_idle_legacy_runtimes(&self) {
        let retired = self.legacy.reap_idle_legacy_runtimes();
        if retired > 0 {
            eprintln!("yeet mcpserver: retired {retired} idle legacy runtime(s)");
        }
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

        let runtime = Arc::new(SessionRuntimePool::new(
            self.default_workspace.clone(),
            self.restrict_workspace,
        )?);
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
    restrict_workspace: bool,
    child: Child,
    stdin: BufWriter<ChildStdin>,
    responses: Receiver<std::result::Result<String, String>>,
}

impl McpProcess {
    fn new(default_workspace: PathBuf, restrict_workspace: bool) -> Result<Self> {
        let mut process = Self::spawn(default_workspace, restrict_workspace)?;
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

    fn spawn(default_workspace: PathBuf, restrict_workspace: bool) -> Result<Self> {
        let executable = std::env::current_exe().context("locate Yeet executable")?;
        let mut command = Command::new(executable);
        command
            .arg("mcpserver")
            .arg("stdio")
            .arg("--workspace")
            .arg(&default_workspace);
        if restrict_workspace {
            command.arg("--restrict-workspace");
        }
        let mut child = command
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
            restrict_workspace,
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
        self.terminate();
        let replacement = Self::new(self.default_workspace.clone(), self.restrict_workspace)?;
        *self = replacement;
        Ok(())
    }

    fn terminate(&mut self) {
        match self.child.try_wait() {
            Ok(None) => {
                let pid = self.child.id();
                if force_terminate_process_tree(pid).is_err() {
                    let _ = self.child.kill();
                }
            }
            Ok(Some(_)) => {}
            Err(_) => {
                let _ = self.child.kill();
            }
        }
        let _ = self.child.wait();
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
        self.terminate();
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
