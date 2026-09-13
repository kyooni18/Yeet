use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use axum::extract::ws::{Message, WebSocket};
use serde_json::{Map, Value};
use tokio::sync::broadcast;

use crate::{
    background::BackgroundConnection,
    model::{BridgeEnvelope, BridgeState, ConversationEntry, ConversationKind, FrontendCommand},
};

use super::protocol::{
    ClientMessage, REMOTE_PROTOCOL_VERSION, ServerMessage, decode_client_message,
    negotiate_version, validate_exact_version,
};

const EVENT_HISTORY_LIMIT: usize = 1024;
const CLIENT_RUNTIME_TTL: Duration = Duration::from_secs(15 * 60);
const SESSION_AFFINITY_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) struct RemoteHub {
    default_workspace: PathBuf,
    clients: Mutex<HashMap<(PathBuf, String), Arc<RemoteClientRuntime>>>,
}

impl RemoteHub {
    pub(crate) fn new(workspace: PathBuf) -> Self {
        let workspace = workspace.canonicalize().unwrap_or(workspace);
        Self {
            default_workspace: workspace,
            clients: Mutex::new(HashMap::new()),
        }
    }

    fn resolve_workspace(&self, requested: Option<&str>) -> Result<PathBuf> {
        let requested = requested.map(str::trim).filter(|value| !value.is_empty());
        let mut path = match requested {
            None => self.default_workspace.clone(),
            Some("~") => dirs::home_dir().ok_or_else(|| {
                anyhow!("home directory is unavailable; use an absolute workspace path")
            })?,
            Some(value) if value.starts_with("~/") || value.starts_with("~\\") => {
                let home = dirs::home_dir().ok_or_else(|| {
                    anyhow!("home directory is unavailable; use an absolute workspace path")
                })?;
                home.join(&value[2..])
            }
            Some(value) => PathBuf::from(value),
        };
        if !path.is_absolute() {
            path = self.default_workspace.join(path);
        }
        let path = path
            .canonicalize()
            .with_context(|| format!("resolve Remote workspace {}", path.display()))?;
        if !path.is_dir() {
            return Err(anyhow!(
                "Remote workspace is not a directory: {}",
                path.display()
            ));
        }
        Ok(path)
    }

    fn runtime(&self, workspace: &Path, client_id: &str) -> Result<Arc<RemoteClientRuntime>> {
        let now = Instant::now();
        let mut clients = self
            .clients
            .lock()
            .map_err(|_| anyhow!("remote client registry lock poisoned"))?;
        clients.retain(|_, runtime| {
            Arc::strong_count(runtime) > 1
                || runtime.is_streaming()
                || now.duration_since(runtime.last_touched()) < CLIENT_RUNTIME_TTL
        });
        let key = (workspace.to_path_buf(), client_id.to_owned());
        if let Some(runtime) = clients.get(&key) {
            runtime.touch();
            return Ok(Arc::clone(runtime));
        }
        let runtime = Arc::new(RemoteClientRuntime::spawn(workspace)?);
        clients.insert(key, Arc::clone(&runtime));
        Ok(runtime)
    }
}

struct RuntimeShared {
    state: BridgeState,
    sequence: u64,
    history: VecDeque<ServerMessage>,
    last_touched: Instant,
}

enum RuntimeControl {
    Command(FrontendCommand),
    Shutdown,
}

struct ResumePlan {
    sequence: u64,
    revision: u64,
    session_id: Option<String>,
    resumed: bool,
    replay: Vec<ServerMessage>,
    snapshot: Option<BridgeState>,
}

struct RemoteClientRuntime {
    shared: Arc<Mutex<RuntimeShared>>,
    commands: mpsc::Sender<RuntimeControl>,
    events: broadcast::Sender<ServerMessage>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl RemoteClientRuntime {
    fn spawn(workspace: &Path) -> Result<Self> {
        let mut connection = BackgroundConnection::connect_scoped(workspace, Some("remote"))
            .context("connect semantic Remote client to Yeet background service")?;
        let initial_state = wait_for_initial_state(&mut connection)?;
        let shared = Arc::new(Mutex::new(RuntimeShared {
            state: initial_state,
            sequence: 0,
            history: VecDeque::new(),
            last_touched: Instant::now(),
        }));
        let (events, _) = broadcast::channel(EVENT_HISTORY_LIMIT);
        let (commands, command_rx) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        let thread_events = events.clone();
        let thread = thread::Builder::new()
            .name("yeet-remote-semantic-client".into())
            .spawn(move || {
                runtime_loop(connection, command_rx, thread_shared, thread_events);
            })
            .context("start semantic Remote client runtime")?;
        Ok(Self {
            shared,
            commands,
            events,
            thread: Mutex::new(Some(thread)),
        })
    }

    fn touch(&self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.last_touched = Instant::now();
        }
    }

    fn last_touched(&self) -> Instant {
        self.shared
            .lock()
            .map(|shared| shared.last_touched)
            .unwrap_or_else(|_| Instant::now())
    }

    fn is_streaming(&self) -> bool {
        self.shared
            .lock()
            .map(|shared| shared.state.is_streaming)
            .unwrap_or(false)
    }

    fn subscribe(&self) -> broadcast::Receiver<ServerMessage> {
        self.events.subscribe()
    }

    fn send_command(&self, command: FrontendCommand) -> Result<()> {
        self.touch();
        self.commands
            .send(RuntimeControl::Command(command))
            .map_err(|_| anyhow!("semantic Remote runtime is no longer available"))
    }

    async fn ensure_session(&self, session_id: Option<&str>) -> Result<()> {
        let Some(session_id) = session_id else {
            return Ok(());
        };
        if self.current_session_id().as_deref() == Some(session_id) {
            return Ok(());
        }
        self.send_command(FrontendCommand::LoadSession {
            session_id: session_id.to_owned(),
        })?;
        let deadline = tokio::time::Instant::now() + SESSION_AFFINITY_TIMEOUT;
        loop {
            if self.current_session_id().as_deref() == Some(session_id) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(anyhow!(
                    "timed out restoring Remote session affinity for {session_id}"
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn current_session_id(&self) -> Option<String> {
        self.shared
            .lock()
            .ok()
            .and_then(|shared| shared.state.current_session_id.clone())
    }

    fn resume_plan(&self, last_sequence: Option<u64>, last_revision: Option<u64>) -> ResumePlan {
        self.touch();
        let shared = self
            .shared
            .lock()
            .expect("remote runtime state lock poisoned");
        let requested_sequence = last_sequence.unwrap_or(shared.sequence.saturating_add(1));
        let exact_revision = if requested_sequence == shared.sequence {
            Some(shared.state.conversation_revision)
        } else {
            shared
                .history
                .iter()
                .find(|message| message.sequence() == Some(requested_sequence))
                .and_then(ServerMessage::revision)
        };
        let revision_is_compatible = match (last_revision, exact_revision) {
            (Some(reported), Some(expected)) => reported == expected,
            (Some(reported), None) => reported <= shared.state.conversation_revision,
            (None, _) => false,
        };
        let history_start = shared.history.front().and_then(ServerMessage::sequence);
        let history_covers = requested_sequence == shared.sequence
            || (requested_sequence < shared.sequence
                && history_start
                    .is_some_and(|first| first <= requested_sequence.saturating_add(1)));
        let resumed = last_sequence.is_some()
            && revision_is_compatible
            && requested_sequence <= shared.sequence
            && history_covers;
        let replay = if resumed {
            shared
                .history
                .iter()
                .filter(|message| {
                    message
                        .sequence()
                        .is_some_and(|sequence| sequence > requested_sequence)
                })
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        ResumePlan {
            sequence: shared.sequence,
            revision: shared.state.conversation_revision,
            session_id: shared.state.current_session_id.clone(),
            resumed,
            replay,
            snapshot: (!resumed).then(|| shared.state.clone()),
        }
    }
}

impl Drop for RemoteClientRuntime {
    fn drop(&mut self) {
        let _ = self.commands.send(RuntimeControl::Shutdown);
        if let Ok(mut handle) = self.thread.lock()
            && let Some(handle) = handle.take()
        {
            let _ = handle.join();
        }
    }
}

fn wait_for_initial_state(connection: &mut BackgroundConnection) -> Result<BridgeState> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(envelope) = connection.try_recv() {
            if envelope.kind == "state"
                && let Some(state) = envelope.state
            {
                return Ok(state);
            }
            if envelope.kind == "error" {
                return Err(anyhow!(
                    "background service rejected semantic Remote connection: {}",
                    envelope.message.unwrap_or_else(|| "unknown error".into())
                ));
            }
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out waiting for initial semantic Remote state"
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn runtime_loop(
    mut connection: BackgroundConnection,
    commands: mpsc::Receiver<RuntimeControl>,
    shared: Arc<Mutex<RuntimeShared>>,
    events: broadcast::Sender<ServerMessage>,
) {
    let mut shutdown = false;
    while !shutdown {
        while let Ok(control) = commands.try_recv() {
            match control {
                RuntimeControl::Command(command) => {
                    if let Err(error) = connection.send(&command) {
                        let _ = events.send(ServerMessage::error(
                            "backend_command_failed",
                            error.to_string(),
                            false,
                            None,
                        ));
                    }
                }
                RuntimeControl::Shutdown => {
                    shutdown = true;
                    break;
                }
            }
        }
        while let Some(envelope) = connection.try_recv() {
            process_envelope(&shared, &events, envelope);
        }
        if !shutdown {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn process_envelope(
    shared: &Arc<Mutex<RuntimeShared>>,
    events: &broadcast::Sender<ServerMessage>,
    envelope: BridgeEnvelope,
) {
    match envelope.kind.as_str() {
        "state" => {
            if let Some(state) = envelope.state {
                process_state_update(shared, events, state);
            }
        }
        "error" => {
            let _ = events.send(ServerMessage::error(
                "backend_error",
                envelope
                    .message
                    .unwrap_or_else(|| "unknown backend error".into()),
                false,
                None,
            ));
        }
        _ => {}
    }
}

fn process_state_update(
    shared: &Arc<Mutex<RuntimeShared>>,
    events: &broadcast::Sender<ServerMessage>,
    mut update: BridgeState,
) {
    let mut shared = match shared.lock() {
        Ok(shared) => shared,
        Err(_) => return,
    };
    let previous = shared.state.clone();
    let had_conversation = update.conversation.is_some();
    if update.conversation.is_none() {
        update.conversation = previous.conversation.clone();
    }
    let next = update;
    let revision = next.conversation_revision;
    let session_changed = previous.current_session_id != next.current_session_id;
    let mut outgoing = Vec::new();

    if session_changed {
        let sequence = next_sequence(&mut shared);
        outgoing.push(ServerMessage::Snapshot {
            version: REMOTE_PROTOCOL_VERSION,
            sequence,
            revision,
            state: next.clone(),
        });
    } else {
        if had_conversation {
            append_conversation_updates(&mut shared, &previous, &next, &mut outgoing);
        }
        append_stream_updates(&mut shared, &previous, &next, &mut outgoing);
        let patch = state_patch(&previous, &next);
        if !patch.is_empty() {
            let sequence = next_sequence(&mut shared);
            outgoing.push(ServerMessage::StateUpdate {
                version: REMOTE_PROTOCOL_VERSION,
                sequence,
                revision,
                patch,
            });
        }
    }

    shared.state = next;
    shared.last_touched = Instant::now();
    for message in &outgoing {
        if message.sequence().is_some() {
            shared.history.push_back(message.clone());
            while shared.history.len() > EVENT_HISTORY_LIMIT {
                shared.history.pop_front();
            }
        }
    }
    drop(shared);
    for message in outgoing {
        let _ = events.send(message);
    }
}

fn append_conversation_updates(
    shared: &mut RuntimeShared,
    previous: &BridgeState,
    next: &BridgeState,
    outgoing: &mut Vec<ServerMessage>,
) {
    let old = previous.conversation.as_deref().unwrap_or_default();
    let new = next.conversation.as_deref().unwrap_or_default();
    let old_ids = old
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    let new_ids = new
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    let append_compatible = old_ids.len() <= new_ids.len()
        && old_ids
            .iter()
            .zip(new_ids.iter())
            .all(|(old, new)| old == new);
    if !append_compatible {
        let sequence = next_sequence(shared);
        outgoing.push(ServerMessage::ConversationReset {
            version: REMOTE_PROTOCOL_VERSION,
            sequence,
            revision: next.conversation_revision,
            conversation: new.to_vec(),
        });
        return;
    }

    for (index, entry) in new.iter().enumerate() {
        let changed = old
            .get(index)
            .map(|previous| entry_changed(previous, entry))
            .unwrap_or(true);
        if !changed {
            continue;
        }
        let sequence = next_sequence(shared);
        match &entry.kind {
            ConversationKind::ToolCall { tool_call } => outgoing.push(ServerMessage::ToolUpdate {
                version: REMOTE_PROTOCOL_VERSION,
                sequence,
                revision: next.conversation_revision,
                entry: entry.clone(),
                tool_call: Some(tool_call.clone()),
            }),
            ConversationKind::Activity { activity } => {
                outgoing.push(ServerMessage::ActivityUpdate {
                    version: REMOTE_PROTOCOL_VERSION,
                    sequence,
                    revision: next.conversation_revision,
                    entry: entry.clone(),
                    activity: activity.clone(),
                })
            }
            _ => outgoing.push(ServerMessage::ConversationEntry {
                version: REMOTE_PROTOCOL_VERSION,
                sequence,
                revision: next.conversation_revision,
                entry: entry.clone(),
            }),
        }
    }
}

fn append_stream_updates(
    shared: &mut RuntimeShared,
    previous: &BridgeState,
    next: &BridgeState,
    outgoing: &mut Vec<ServerMessage>,
) {
    if let Some((delta, reset)) = text_delta(
        previous.active_assistant_entry_id.as_deref(),
        &previous.active_assistant_text,
        next.active_assistant_entry_id.as_deref(),
        &next.active_assistant_text,
    ) {
        let sequence = next_sequence(shared);
        outgoing.push(ServerMessage::AssistantDelta {
            version: REMOTE_PROTOCOL_VERSION,
            sequence,
            revision: next.conversation_revision,
            entry_id: next.active_assistant_entry_id.clone(),
            delta,
            content: if reset {
                next.active_assistant_text.clone()
            } else {
                String::new()
            },
            reset,
        });
    }

    if let Some((delta, reset)) = text_delta(
        previous.active_reasoning_entry_id.as_deref(),
        &previous.active_reasoning_text,
        next.active_reasoning_entry_id.as_deref(),
        &next.active_reasoning_text,
    ) {
        let sequence = next_sequence(shared);
        outgoing.push(ServerMessage::ReasoningDelta {
            version: REMOTE_PROTOCOL_VERSION,
            sequence,
            revision: next.conversation_revision,
            entry_id: next.active_reasoning_entry_id.clone(),
            delta,
            content: if reset {
                next.active_reasoning_text.clone()
            } else {
                String::new()
            },
            summary: false,
            reset,
        });
    }

    if let Some((delta, reset)) = text_delta(
        previous.active_reasoning_entry_id.as_deref(),
        &previous.active_reasoning_summary,
        next.active_reasoning_entry_id.as_deref(),
        &next.active_reasoning_summary,
    ) {
        let sequence = next_sequence(shared);
        outgoing.push(ServerMessage::ReasoningDelta {
            version: REMOTE_PROTOCOL_VERSION,
            sequence,
            revision: next.conversation_revision,
            entry_id: next.active_reasoning_entry_id.clone(),
            delta,
            content: if reset {
                next.active_reasoning_summary.clone()
            } else {
                String::new()
            },
            summary: true,
            reset,
        });
    }
}

fn text_delta(
    previous_id: Option<&str>,
    previous: &str,
    next_id: Option<&str>,
    next: &str,
) -> Option<(String, bool)> {
    if previous_id == next_id && previous == next {
        return None;
    }
    // When the backend seals a live segment it clears the active buffer and
    // publishes the completed conversation entry in the same/full state. The
    // conversation event is authoritative; emitting an empty reset here would
    // make a browser briefly erase the just-completed assistant/reasoning row.
    if next_id.is_none() && next.is_empty() {
        return None;
    }
    if previous_id == next_id && next.starts_with(previous) {
        return Some((next[previous.len()..].to_owned(), false));
    }
    Some((String::new(), true))
}

fn entry_changed(previous: &ConversationEntry, next: &ConversationEntry) -> bool {
    serde_json::to_value(previous).ok() != serde_json::to_value(next).ok()
}

fn state_patch(previous: &BridgeState, next: &BridgeState) -> Map<String, Value> {
    let previous = compact_state_value(previous);
    let next = compact_state_value(next);
    let mut patch = Map::new();
    for (key, value) in &next {
        if previous.get(key) != Some(value) {
            patch.insert(key.clone(), value.clone());
        }
    }
    for key in previous.keys() {
        if !next.contains_key(key) {
            patch.insert(key.clone(), Value::Null);
        }
    }
    patch
}

fn compact_state_value(state: &BridgeState) -> Map<String, Value> {
    let mut value = serde_json::to_value(state.without_conversation())
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    value.remove("conversation");
    value.remove("conversation_revision");
    value.remove("active_assistant_text");
    value.remove("active_reasoning_text");
    value.remove("active_reasoning_summary");
    value
}

fn next_sequence(shared: &mut RuntimeShared) -> u64 {
    shared.sequence = shared.sequence.wrapping_add(1).max(1);
    shared.sequence
}

fn valid_client_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn command_allowed(command: &FrontendCommand) -> bool {
    !matches!(command, FrontendCommand::Shutdown)
}

async fn send_json(socket: &mut WebSocket, message: &ServerMessage) -> Result<()> {
    let fatal = matches!(message, ServerMessage::Error { fatal: true, .. });
    let text = serde_json::to_string(message)?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|error| anyhow!(error.to_string()))?;
    if fatal {
        socket
            .send(Message::Close(None))
            .await
            .map_err(|error| anyhow!(error.to_string()))?;
    }
    Ok(())
}

pub(crate) async fn serve_socket(mut socket: WebSocket, hub: Arc<RemoteHub>) {
    let Some(Ok(Message::Text(first))) = socket.recv().await else {
        return;
    };
    let hello = match decode_client_message(&first) {
        Ok(ClientMessage::Hello {
            min_version,
            max_version,
            client_id,
            workspace,
            session_id,
            last_sequence,
            last_revision,
        }) => (
            min_version,
            max_version,
            client_id,
            workspace,
            session_id,
            last_sequence,
            last_revision,
        ),
        Ok(_) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error(
                    "hello_required",
                    "the first Remote WebSocket message must be hello",
                    true,
                    None,
                ),
            )
            .await;
            return;
        }
        Err(error) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error("malformed_message", error.to_string(), true, None),
            )
            .await;
            return;
        }
    };

    if negotiate_version(hello.0, hello.1).is_err() {
        let _ = send_json(
            &mut socket,
            &ServerMessage::error(
                "protocol_mismatch",
                format!(
                    "client supports Remote protocol {}..{}, server supports version {}",
                    hello.0, hello.1, REMOTE_PROTOCOL_VERSION
                ),
                true,
                None,
            ),
        )
        .await;
        return;
    }
    let workspace = match hub.resolve_workspace(hello.3.as_deref()) {
        Ok(workspace) => workspace,
        Err(error) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error("workspace_invalid", error.to_string(), true, None),
            )
            .await;
            return;
        }
    };
    let workspace_display = workspace.to_string_lossy().into_owned();

    let client_id = match hello.2 {
        Some(client_id) if valid_client_id(&client_id) => client_id,
        Some(_) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error(
                    "invalid_client_id",
                    "client_id must be 1-128 ASCII letters, digits, '.', '_' or '-'",
                    true,
                    None,
                ),
            )
            .await;
            return;
        }
        None => uuid::Uuid::new_v4().to_string(),
    };
    let runtime_hub = Arc::clone(&hub);
    let runtime_client_id = client_id.clone();
    let runtime_workspace = workspace.clone();
    let runtime = match tokio::task::spawn_blocking(move || {
        runtime_hub.runtime(&runtime_workspace, &runtime_client_id)
    })
    .await
    {
        Ok(Ok(runtime)) => runtime,
        Ok(Err(error)) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error("backend_unavailable", error.to_string(), true, None),
            )
            .await;
            return;
        }
        Err(error) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error(
                    "backend_unavailable",
                    format!("failed to initialize Remote backend runtime: {error}"),
                    true,
                    None,
                ),
            )
            .await;
            return;
        }
    };
    if let Err(error) = runtime.ensure_session(hello.4.as_deref()).await {
        let _ = send_json(
            &mut socket,
            &ServerMessage::error("session_unavailable", error.to_string(), true, None),
        )
        .await;
        return;
    }

    let mut event_rx = runtime.subscribe();
    let plan = runtime.resume_plan(hello.5, hello.6);
    if send_json(
        &mut socket,
        &ServerMessage::Welcome {
            version: REMOTE_PROTOCOL_VERSION,
            client_id: client_id.clone(),
            workspace: workspace_display,
            session_id: plan.session_id.clone(),
            sequence: plan.sequence,
            revision: plan.revision,
            resumed: plan.resumed,
        },
    )
    .await
    .is_err()
    {
        return;
    }
    if let Some(state) = plan.snapshot {
        if send_json(
            &mut socket,
            &ServerMessage::Snapshot {
                version: REMOTE_PROTOCOL_VERSION,
                sequence: plan.sequence,
                revision: plan.revision,
                state,
            },
        )
        .await
        .is_err()
        {
            return;
        }
    } else {
        for message in plan.replay {
            if send_json(&mut socket, &message).await.is_err() {
                return;
            }
        }
    }

    loop {
        tokio::select! {
            inbound = socket.recv() => {
                let Some(inbound) = inbound else { break; };
                let Ok(inbound) = inbound else { break; };
                match inbound {
                    Message::Text(text) => {
                        let message = match decode_client_message(&text) {
                            Ok(message) => message,
                            Err(error) => {
                                let _ = send_json(&mut socket, &ServerMessage::error(
                                    "malformed_message", error.to_string(), false, None,
                                )).await;
                                continue;
                            }
                        };
                        if let Some(version) = message.exact_version()
                            && validate_exact_version(version).is_err()
                        {
                            let _ = send_json(&mut socket, &ServerMessage::error(
                                "protocol_mismatch",
                                format!("unsupported Remote protocol version {version}"),
                                true,
                                None,
                            )).await;
                            break;
                        }
                        match message {
                            ClientMessage::Hello { .. } => {
                                let _ = send_json(&mut socket, &ServerMessage::error(
                                    "already_initialized", "hello may only be sent once per WebSocket", false, None,
                                )).await;
                            }
                            ClientMessage::Ping { nonce, .. } => {
                                if send_json(&mut socket, &ServerMessage::Pong {
                                    version: REMOTE_PROTOCOL_VERSION,
                                    nonce,
                                }).await.is_err() {
                                    break;
                                }
                            }
                            ClientMessage::Command { request_id, command, .. } => {
                                if !command_allowed(&command) {
                                    let _ = send_json(&mut socket, &ServerMessage::error(
                                        "command_forbidden",
                                        "shutdown is not exposed through the Remote frontend protocol",
                                        false,
                                        request_id,
                                    )).await;
                                    continue;
                                }
                                match runtime.send_command(command) {
                                    Ok(()) => {
                                        if send_json(&mut socket, &ServerMessage::Ack {
                                            version: REMOTE_PROTOCOL_VERSION,
                                            request_id,
                                        }).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(error) => {
                                        let _ = send_json(&mut socket, &ServerMessage::error(
                                            "backend_unavailable", error.to_string(), false, request_id,
                                        )).await;
                                    }
                                }
                            }
                        }
                    }
                    Message::Ping(payload) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Message::Pong(_) => {}
                    Message::Close(_) => break,
                    Message::Binary(_) => {
                        let _ = send_json(&mut socket, &ServerMessage::error(
                            "malformed_message", "Remote protocol messages must be UTF-8 JSON text", false, None,
                        )).await;
                    }
                }
            }
            event = event_rx.recv() => {
                match event {
                    Ok(message) => {
                        if send_json(&mut socket, &message).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let plan = runtime.resume_plan(None, None);
                        if let Some(state) = plan.snapshot
                            && send_json(&mut socket, &ServerMessage::Snapshot {
                                version: REMOTE_PROTOCOL_VERSION,
                                sequence: plan.sequence,
                                revision: plan.revision,
                                state,
                            }).await.is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    runtime.touch();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConversationKind, ModelActivity};
    use serde_json::json;

    #[test]
    fn remote_hub_accepts_per_client_workspace_selection() {
        let root = tempfile::tempdir().unwrap();
        let default_workspace = root.path().join("default");
        let selected_workspace = root.path().join("selected");
        std::fs::create_dir_all(&default_workspace).unwrap();
        std::fs::create_dir_all(&selected_workspace).unwrap();
        let hub = RemoteHub::new(default_workspace.clone());

        assert_eq!(
            hub.resolve_workspace(None).unwrap(),
            default_workspace.canonicalize().unwrap()
        );
        assert_eq!(
            hub.resolve_workspace(Some(selected_workspace.to_str().unwrap()))
                .unwrap(),
            selected_workspace.canonicalize().unwrap()
        );
        assert!(
            hub.resolve_workspace(Some(root.path().join("missing").to_str().unwrap()))
                .is_err()
        );
    }

    fn base_state() -> BridgeState {
        BridgeState {
            active_model: "model".into(),
            active_reasoning_level: "auto".into(),
            conversation: Some(Vec::new()),
            ..BridgeState::default()
        }
    }

    #[test]
    fn compact_streaming_update_emits_delta_without_full_text_patch() {
        let mut previous = base_state();
        previous.active_assistant_entry_id = Some("a".into());
        previous.active_assistant_text = "hel".into();
        let mut next = previous.clone();
        next.active_assistant_text = "hello".into();
        next.conversation_revision = 2;
        let shared = Arc::new(Mutex::new(RuntimeShared {
            state: previous,
            sequence: 0,
            history: VecDeque::new(),
            last_touched: Instant::now(),
        }));
        let (events, mut rx) = broadcast::channel(16);
        process_state_update(&shared, &events, next);
        let first = rx.try_recv().unwrap();
        match first {
            ServerMessage::AssistantDelta { delta, content, .. } => {
                assert_eq!(delta, "lo");
                assert!(content.is_empty());
            }
            other => panic!("unexpected message: {other:?}"),
        }
        while let Ok(message) = rx.try_recv() {
            if let ServerMessage::StateUpdate { patch, .. } = message {
                assert!(!patch.contains_key("active_assistant_text"));
            }
        }
    }

    #[test]
    fn replay_history_stays_bounded_during_long_state_streams() {
        let shared = Arc::new(Mutex::new(RuntimeShared {
            state: base_state(),
            sequence: 0,
            history: VecDeque::new(),
            last_touched: Instant::now(),
        }));
        let (events, _rx) = broadcast::channel(EVENT_HISTORY_LIMIT);
        for value in 1..=(EVENT_HISTORY_LIMIT + 128) {
            let mut next = shared.lock().unwrap().state.clone();
            next.current_context_tokens = Some(value as u64);
            process_state_update(&shared, &events, next);
        }
        let state = shared.lock().unwrap();
        assert_eq!(state.history.len(), EVENT_HISTORY_LIMIT);
        assert_eq!(state.sequence, EVENT_HISTORY_LIMIT as u64 + 128);
        assert!(
            state
                .history
                .front()
                .and_then(ServerMessage::sequence)
                .is_some_and(|sequence| sequence > 1)
        );
    }

    #[test]
    fn sealing_stream_does_not_emit_empty_reset_after_completed_entry() {
        assert_eq!(text_delta(Some("a"), "complete", None, ""), None);
        assert_eq!(text_delta(Some("r"), "reasoning", None, ""), None);
    }

    #[test]
    fn stream_reset_carries_full_content_only_on_reset() {
        let previous = base_state();
        let mut next = previous.clone();
        next.active_assistant_entry_id = Some("a".into());
        next.active_assistant_text = "initial chunk".into();
        let shared = Arc::new(Mutex::new(RuntimeShared {
            state: previous,
            sequence: 0,
            history: VecDeque::new(),
            last_touched: Instant::now(),
        }));
        let (events, mut rx) = broadcast::channel(16);
        process_state_update(&shared, &events, next);
        let message = rx.try_recv().unwrap();
        match message {
            ServerMessage::AssistantDelta {
                delta,
                content,
                reset,
                ..
            } => {
                assert!(reset);
                assert!(delta.is_empty());
                assert_eq!(content, "initial chunk");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn tool_and_activity_entries_get_semantic_updates() {
        let previous = base_state();
        let mut next = previous.clone();
        next.conversation_revision = 2;
        next.conversation = Some(vec![
            ConversationEntry {
                id: "tool".into(),
                kind: ConversationKind::ToolCall {
                    tool_call: crate::model::ConversationToolCall {
                        id: "tool".into(),
                        index: Some(0),
                        call_id: Some("call".into()),
                        name: "read_file".into(),
                        arguments: "{}".into(),
                        status: crate::model::ToolCallStatus::Streaming,
                    },
                },
            },
            ConversationEntry {
                id: "activity".into(),
                kind: ConversationKind::Activity {
                    activity: ModelActivity {
                        phase: json!("running"),
                        title: "Reading".into(),
                        detail: None,
                        run_id: Some("run".into()),
                    },
                },
            },
        ]);
        let shared = Arc::new(Mutex::new(RuntimeShared {
            state: previous,
            sequence: 0,
            history: VecDeque::new(),
            last_touched: Instant::now(),
        }));
        let (events, mut rx) = broadcast::channel(16);
        process_state_update(&shared, &events, next);
        let messages = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(
            messages
                .iter()
                .any(|message| matches!(message, ServerMessage::ToolUpdate { .. }))
        );
        assert!(
            messages
                .iter()
                .any(|message| matches!(message, ServerMessage::ActivityUpdate { .. }))
        );
    }

    #[test]
    fn resume_replays_available_sequences_and_resyncs_on_gap() {
        let runtime = RemoteClientRuntime {
            shared: Arc::new(Mutex::new(RuntimeShared {
                state: BridgeState {
                    conversation_revision: 4,
                    ..base_state()
                },
                sequence: 4,
                history: VecDeque::from([
                    ServerMessage::StateUpdate {
                        version: 1,
                        sequence: 3,
                        revision: 3,
                        patch: Map::new(),
                    },
                    ServerMessage::StateUpdate {
                        version: 1,
                        sequence: 4,
                        revision: 4,
                        patch: Map::new(),
                    },
                ]),
                last_touched: Instant::now(),
            })),
            commands: mpsc::channel().0,
            events: broadcast::channel(4).0,
            thread: Mutex::new(None),
        };
        let resumed = runtime.resume_plan(Some(3), Some(3));
        assert!(resumed.resumed);
        assert_eq!(resumed.replay.len(), 1);
        let resync = runtime.resume_plan(Some(1), Some(1));
        assert!(!resync.resumed);
        assert!(resync.snapshot.is_some());

        let stale_current_revision = runtime.resume_plan(Some(4), Some(3));
        assert!(!stale_current_revision.resumed);
        assert!(stale_current_revision.snapshot.is_some());

        let mismatched_retained_revision = runtime.resume_plan(Some(3), Some(2));
        assert!(!mismatched_retained_revision.resumed);
        assert!(mismatched_retained_revision.snapshot.is_some());
    }

    #[test]
    fn initial_connect_without_cursor_requires_full_snapshot() {
        let runtime = RemoteClientRuntime {
            shared: Arc::new(Mutex::new(RuntimeShared {
                state: BridgeState {
                    conversation_revision: 9,
                    current_session_id: Some("session-a".into()),
                    ..base_state()
                },
                sequence: 12,
                history: VecDeque::new(),
                last_touched: Instant::now(),
            })),
            commands: mpsc::channel().0,
            events: broadcast::channel(4).0,
            thread: Mutex::new(None),
        };
        let plan = runtime.resume_plan(None, None);
        assert!(!plan.resumed);
        assert!(plan.replay.is_empty());
        assert_eq!(plan.sequence, 12);
        assert_eq!(plan.revision, 9);
        assert_eq!(plan.session_id.as_deref(), Some("session-a"));
        assert!(plan.snapshot.is_some());
    }

    #[test]
    fn session_affinity_is_scoped_to_client_runtime() {
        let state = BridgeState {
            current_session_id: Some("session-a".into()),
            ..base_state()
        };
        let runtime = RemoteClientRuntime {
            shared: Arc::new(Mutex::new(RuntimeShared {
                state,
                sequence: 0,
                history: VecDeque::new(),
                last_touched: Instant::now(),
            })),
            commands: mpsc::channel().0,
            events: broadcast::channel(4).0,
            thread: Mutex::new(None),
        };
        assert_eq!(runtime.current_session_id().as_deref(), Some("session-a"));
    }

    #[tokio::test]
    async fn session_affinity_loads_requested_session_before_handshake() {
        let shared = Arc::new(Mutex::new(RuntimeShared {
            state: BridgeState {
                current_session_id: Some("session-a".into()),
                ..base_state()
            },
            sequence: 0,
            history: VecDeque::new(),
            last_touched: Instant::now(),
        }));
        let (commands, command_rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let worker = thread::spawn(move || {
            let command = command_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            match command {
                RuntimeControl::Command(FrontendCommand::LoadSession { session_id }) => {
                    worker_shared.lock().unwrap().state.current_session_id = Some(session_id);
                }
                _ => panic!("expected load_session command"),
            }
        });
        let runtime = RemoteClientRuntime {
            shared,
            commands,
            events: broadcast::channel(4).0,
            thread: Mutex::new(None),
        };

        runtime.ensure_session(Some("session-b")).await.unwrap();
        worker.join().unwrap();
        assert_eq!(runtime.current_session_id().as_deref(), Some("session-b"));
    }

    #[test]
    fn shutdown_is_not_a_remote_frontend_command() {
        assert!(!command_allowed(&FrontendCommand::Shutdown));
        assert!(command_allowed(&FrontendCommand::Interrupt));
        assert!(command_allowed(&FrontendCommand::Submit {
            text: "hi".into()
        }));
    }

    #[test]
    fn websocket_command_forwarding_preserves_frontend_command() {
        let (commands, command_rx) = mpsc::channel();
        let runtime = RemoteClientRuntime {
            shared: Arc::new(Mutex::new(RuntimeShared {
                state: base_state(),
                sequence: 0,
                history: VecDeque::new(),
                last_touched: Instant::now(),
            })),
            commands,
            events: broadcast::channel(4).0,
            thread: Mutex::new(None),
        };
        runtime
            .send_command(FrontendCommand::Submit {
                text: "semantic submit".into(),
            })
            .unwrap();
        match command_rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            RuntimeControl::Command(FrontendCommand::Submit { text }) => {
                assert_eq!(text, "semantic submit");
            }
            _ => panic!("Remote command was translated away from FrontendCommand"),
        }
    }

    #[test]
    fn disconnected_runtime_keeps_active_state_for_resume() {
        let runtime = RemoteClientRuntime {
            shared: Arc::new(Mutex::new(RuntimeShared {
                state: BridgeState {
                    is_streaming: true,
                    active_run_id: Some("run".into()),
                    ..base_state()
                },
                sequence: 0,
                history: VecDeque::new(),
                last_touched: Instant::now(),
            })),
            commands: mpsc::channel().0,
            events: broadcast::channel(4).0,
            thread: Mutex::new(None),
        };
        assert!(runtime.is_streaming());
        assert!(
            runtime
                .resume_plan(None, None)
                .snapshot
                .unwrap()
                .is_streaming
        );
    }
}
