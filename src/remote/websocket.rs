use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use axum::extract::ws::{Message, WebSocket};
use serde_json::{Map, Value, json};
use tokio::sync::{broadcast, oneshot};

use crate::{
    backend::{BackendEvent, BackendService},
    background::Wake,
    core::ImageAttachment,
    model::{BridgeEnvelope, BridgeState, ConversationEntry, ConversationKind, FrontendCommand},
};

use super::protocol::{
    ClientMessage, REMOTE_PROTOCOL_VERSION, ServerMessage, decode_client_message,
    negotiate_version, validate_exact_version,
};

const EVENT_HISTORY_LIMIT: usize = 1024;
const CLIENT_RUNTIME_TTL: Duration = Duration::from_secs(15 * 60);
const CLIENT_HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const WEBSOCKET_SEND_TIMEOUT: Duration = Duration::from_secs(8);
const BACKEND_COMMAND_DELIVERY_TIMEOUT: Duration = Duration::from_secs(12);
const SESSION_AFFINITY_TIMEOUT: Duration = Duration::from_secs(10);
// Commands and backend events wake the runtime loop directly; this only bounds
// idle polling when no work is pending.
const RUNTIME_LOOP_IDLE_WAIT: Duration = Duration::from_millis(100);
const ATTACHMENT_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_PENDING_ATTACHMENTS: usize = 64;
const MAX_PENDING_ATTACHMENT_BYTES: usize = 64 * 1024 * 1024;
const MAX_ATTACHMENTS_PER_SUBMIT: usize = 8;
pub(crate) const MAX_REMOTE_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;
const MAX_SUBMIT_ATTACHMENT_BYTES: usize = MAX_REMOTE_ATTACHMENT_BYTES;
const REMOTE_FILE_CONTEXT_OPEN: &str = "\n\n<yeet_remote_files>";
const REMOTE_FILE_CONTEXT_CLOSE: &str = "</yeet_remote_files>";

type ClientRuntimeKey = (PathBuf, String);

struct PendingRemoteAttachment {
    media_type: String,
    bytes: Vec<u8>,
    name: Option<String>,
    byte_len: usize,
    created_at: Instant,
}

pub(crate) struct RemoteHub {
    default_workspace: PathBuf,
    clients: Mutex<HashMap<ClientRuntimeKey, Arc<RemoteClientRuntime>>>,
    attachments: Mutex<HashMap<String, PendingRemoteAttachment>>,
}

impl RemoteHub {
    pub(crate) fn new(workspace: PathBuf) -> Self {
        let workspace = workspace.canonicalize().unwrap_or(workspace);
        Self {
            default_workspace: workspace,
            clients: Mutex::new(HashMap::new()),
            attachments: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn store_attachment(
        &self,
        media_type: String,
        bytes: Vec<u8>,
        name: Option<String>,
    ) -> Result<String> {
        let byte_len = bytes.len();
        if byte_len > MAX_REMOTE_ATTACHMENT_BYTES {
            return Err(anyhow!("attachment exceeds the 20 MiB limit"));
        }

        let now = Instant::now();
        let mut attachments = self
            .attachments
            .lock()
            .map_err(|_| anyhow!("remote attachment store lock poisoned"))?;
        prune_attachments(&mut attachments, now);

        loop {
            let total = attachments
                .values()
                .map(|value| value.byte_len)
                .sum::<usize>();
            if attachments.len() < MAX_PENDING_ATTACHMENTS
                && total.saturating_add(byte_len) <= MAX_PENDING_ATTACHMENT_BYTES
            {
                break;
            }

            let Some(oldest) = attachments
                .iter()
                .min_by_key(|(_, value)| value.created_at)
                .map(|(id, _)| id.clone())
            else {
                return Err(anyhow!("remote attachment cache capacity exceeded"));
            };
            attachments.remove(&oldest);
        }

        let id = uuid::Uuid::new_v4().to_string();
        attachments.insert(
            id.clone(),
            PendingRemoteAttachment {
                media_type,
                bytes,
                name,
                byte_len,
                created_at: now,
            },
        );
        Ok(id)
    }

    pub(crate) fn remove_attachment(&self, id: &str) -> Result<bool> {
        let mut attachments = self
            .attachments
            .lock()
            .map_err(|_| anyhow!("remote attachment store lock poisoned"))?;
        prune_attachments(&mut attachments, Instant::now());
        Ok(attachments.remove(id).is_some())
    }

    fn take_attachments(&self, ids: &[String]) -> Result<Vec<(String, PendingRemoteAttachment)>> {
        if ids.len() > MAX_ATTACHMENTS_PER_SUBMIT {
            return Err(anyhow!(
                "at most {MAX_ATTACHMENTS_PER_SUBMIT} attachments may be attached to one message"
            ));
        }

        let mut seen = HashSet::new();
        if ids.iter().any(|id| !seen.insert(id.as_str())) {
            return Err(anyhow!("duplicate Remote attachment ID"));
        }

        let mut attachments = self
            .attachments
            .lock()
            .map_err(|_| anyhow!("remote attachment store lock poisoned"))?;
        prune_attachments(&mut attachments, Instant::now());

        let mut total = 0usize;
        for id in ids {
            let Some(value) = attachments.get(id) else {
                return Err(anyhow!("Remote attachment is missing or expired: {id}"));
            };
            total = total.saturating_add(value.byte_len);
        }
        if total > MAX_SUBMIT_ATTACHMENT_BYTES {
            return Err(anyhow!("combined attachments exceed 20 MiB"));
        }

        let mut resolved = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(value) = attachments.remove(id) {
                resolved.push((id.clone(), value));
            }
        }
        Ok(resolved)
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
        let key = (workspace.to_path_buf(), client_id.to_owned());
        {
            let now = Instant::now();
            let mut clients = self.lock_clients();
            clients.retain(|_, runtime| {
                Arc::strong_count(runtime) > 1
                    || runtime.is_streaming()
                    || now.duration_since(runtime.last_touched()) < CLIENT_RUNTIME_TTL
            });
            if let Some(runtime) = clients.get(&key) {
                runtime.touch();
                return Ok(Arc::clone(runtime));
            }
        }
        // Backend construction can touch workspace/session metadata. Keep it
        // outside the client registry lock so one new client never stalls others.
        let runtime = Arc::new(RemoteClientRuntime::spawn(workspace)?);
        let mut clients = self.lock_clients();
        if let Some(existing) = clients.get(&key) {
            // A concurrent reconnect of the same client won the race.
            existing.touch();
            return Ok(Arc::clone(existing));
        }
        clients.insert(key, Arc::clone(&runtime));
        Ok(runtime)
    }

    fn lock_clients(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<ClientRuntimeKey, Arc<RemoteClientRuntime>>> {
        self.clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn prune_attachments(attachments: &mut HashMap<String, PendingRemoteAttachment>, now: Instant) {
    attachments.retain(|_, value| now.duration_since(value.created_at) < ATTACHMENT_TTL);
}

struct RuntimeShared {
    state: BridgeState,
    sequence: u64,
    history: VecDeque<ServerMessage>,
    last_touched: Instant,
}

enum RuntimeControl {
    Command {
        command: FrontendCommand,
        completion: oneshot::Sender<Result<(), String>>,
    },
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
    wake: Wake,
    events: broadcast::Sender<ServerMessage>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl RemoteClientRuntime {
    fn spawn(workspace: &Path) -> Result<Self> {
        // Remote is itself the long-lived owner. Keep each client backend inside
        // this PID; external provider/edit/MCP sidecars remain lazy and appear
        // only when that client actually invokes a capability that needs one.
        let wake = Wake::new();
        let service = BackendService::spawn(workspace.to_path_buf(), Some(wake.clone()))?;
        let initial_state = service.state_snapshot();
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
        let thread_wake = wake.clone();
        let thread = thread::Builder::new()
            .name("yeet-remote-semantic-client".into())
            .spawn(move || {
                runtime_loop(
                    service,
                    command_rx,
                    thread_shared,
                    thread_events,
                    thread_wake,
                );
            })
            .context("start semantic Remote client runtime")?;
        Ok(Self {
            shared,
            commands,
            wake,
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

    async fn send_command(&self, command: FrontendCommand) -> Result<()> {
        self.touch();
        let (completion, result) = oneshot::channel();
        self.commands
            .send(RuntimeControl::Command {
                command,
                completion,
            })
            .map_err(|_| anyhow!("semantic Remote runtime is no longer available"))?;
        self.wake.notify();

        match tokio::time::timeout(BACKEND_COMMAND_DELIVERY_TIMEOUT, result).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => Err(anyhow!(error)),
            Ok(Err(_)) => Err(anyhow!(
                "semantic Remote runtime stopped before confirming command delivery"
            )),
            Err(_) => Err(anyhow!(
                "timed out delivering Remote command to the in-process backend"
            )),
        }
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
        })
        .await?;
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
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        self.wake.notify();
        // Eviction runs while the hub's client registry is locked. The worker
        // may still be finishing backend work, so joining here can stall every
        // client. Shutdown is cooperative; the worker owns and drops its backend.
        if let Ok(mut handle) = self.thread.lock() {
            drop(handle.take());
        }
    }
}

fn runtime_loop(
    mut service: BackendService,
    commands: mpsc::Receiver<RuntimeControl>,
    shared: Arc<Mutex<RuntimeShared>>,
    events: broadcast::Sender<ServerMessage>,
    wake: Wake,
) {
    let mut shutdown = false;
    while !shutdown {
        while let Ok(control) = commands.try_recv() {
            match control {
                RuntimeControl::Command {
                    command,
                    completion,
                } => {
                    if completion.is_closed() {
                        continue;
                    }
                    let result = service.send(command).map_err(|error| format!("{error:#}"));
                    let _ = completion.send(result);
                }
                RuntimeControl::Shutdown => {
                    shutdown = true;
                    break;
                }
            }
        }
        while let Some(event) = service.try_recv() {
            let BackendEvent::Envelope(envelope) = event;
            process_envelope(&shared, &events, envelope);
        }
        if !shutdown {
            wake.wait_timeout(RUNTIME_LOOP_IDLE_WAIT);
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
    let mut shared = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Move the old state out instead of cloning it. Compact streaming updates
    // intentionally omit conversation history; moving that history into the new
    // state avoids copying the full transcript on every token/reasoning delta.
    let mut previous = std::mem::take(&mut shared.state);
    let had_conversation = update.conversation.is_some();
    let session_changed = previous.current_session_id != update.current_session_id;
    if update.conversation.is_none() && !session_changed {
        update.conversation = previous.conversation.take();
    }
    let next = update;
    let revision = next.conversation_revision;
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
    // Byte comparison avoids building two `Value` trees per transcript entry
    // on every committed update of a long session.
    serde_json::to_vec(previous).ok() != serde_json::to_vec(next).ok()
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

fn resolve_remote_attachments(
    hub: &RemoteHub,
    workspace: &Path,
    command: &mut FrontendCommand,
) -> Result<()> {
    let FrontendCommand::Submit {
        text,
        images,
        attachment_ids,
        ..
    } = command
    else {
        return Ok(());
    };

    if !images.is_empty() {
        return Err(anyhow!(
            "inline Remote images are not accepted; upload them through /api/attachments"
        ));
    }
    if attachment_ids.is_empty() {
        return Ok(());
    }

    let resolved = hub.take_attachments(attachment_ids)?;
    let mut files = Vec::new();

    for (id, attachment) in resolved {
        if matches!(
            attachment.media_type.as_str(),
            "image/png" | "image/jpeg" | "image/webp" | "image/gif"
        ) {
            images.push(ImageAttachment::from_bytes(
                attachment.media_type,
                &attachment.bytes,
                attachment.name,
            )?);
            continue;
        }

        files.push(materialize_remote_file(workspace, &id, attachment)?);
    }

    if !files.is_empty() {
        let payload = serde_json::to_string(&json!({ "files": files }))?;
        text.push_str(REMOTE_FILE_CONTEXT_OPEN);
        text.push_str(&payload);
        text.push_str(REMOTE_FILE_CONTEXT_CLOSE);
    }

    attachment_ids.clear();
    Ok(())
}

fn materialize_remote_file(
    workspace: &Path,
    id: &str,
    attachment: PendingRemoteAttachment,
) -> Result<Value> {
    let root = workspace.join(".yeet").join("remote-attachments");
    fs::create_dir_all(&root).context("create Remote attachment directory")?;
    let root = root
        .canonicalize()
        .context("canonicalize Remote attachment directory")?;

    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    if !root.starts_with(&workspace) {
        bail!("Remote attachment directory escapes the active workspace");
    }

    let directory = root.join(id);
    fs::create_dir_all(&directory).context("create Remote attachment item directory")?;
    let directory = directory
        .canonicalize()
        .context("canonicalize Remote attachment item directory")?;
    if !directory.starts_with(&root) {
        bail!("Remote attachment item directory escapes its storage root");
    }

    let name = sanitize_remote_attachment_name(attachment.name.as_deref());
    let path = directory.join(&name);
    fs::write(&path, &attachment.bytes).context("write Remote file attachment")?;

    let relative = path
        .strip_prefix(&workspace)
        .context("Remote file attachment is outside the active workspace")?
        .to_string_lossy()
        .into_owned();

    Ok(json!({
        "name": name,
        "media_type": attachment.media_type,
        "size": attachment.byte_len,
        "path": relative,
    }))
}

fn sanitize_remote_attachment_name(name: Option<&str>) -> String {
    let fallback = "attachment.bin";
    let basename = name
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .unwrap_or(fallback);

    let sanitized = basename
        .chars()
        .map(|character| {
            if character.is_control() || matches!(character, '/' | '\\') {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();

    let trimmed = sanitized.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.chars().take(160).collect()
    }
}

async fn send_ws(socket: &mut WebSocket, message: Message) -> Result<()> {
    tokio::time::timeout(WEBSOCKET_SEND_TIMEOUT, socket.send(message))
        .await
        .map_err(|_| anyhow!("Remote WebSocket send timed out"))?
        .map_err(|error| anyhow!(error.to_string()))
}

async fn send_json(socket: &mut WebSocket, message: &ServerMessage) -> Result<()> {
    let fatal = matches!(message, ServerMessage::Error { fatal: true, .. });
    let text = serde_json::to_string(message)?;
    send_ws(socket, Message::Text(text.into())).await?;
    if fatal {
        send_ws(socket, Message::Close(None)).await?;
    }
    Ok(())
}

pub(crate) async fn serve_socket(mut socket: WebSocket, hub: Arc<RemoteHub>) {
    let first = match tokio::time::timeout(CLIENT_HELLO_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(first)))) => first,
        Ok(_) => return,
        Err(_) => {
            let _ = send_json(
                &mut socket,
                &ServerMessage::error(
                    "hello_timeout",
                    "timed out waiting for the initial Remote hello message",
                    true,
                    None,
                ),
            )
            .await;
            return;
        }
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
                &ServerMessage::error("backend_unavailable", format!("{error:#}"), true, None),
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
                            ClientMessage::Command { request_id, mut command, .. } => {
                                if !command_allowed(&command) {
                                    let _ = send_json(&mut socket, &ServerMessage::error(
                                        "command_forbidden",
                                        "shutdown is not exposed through the Remote frontend protocol",
                                        false,
                                        request_id,
                                    )).await;
                                    continue;
                                }
                                if let Err(error) =
                                    resolve_remote_attachments(&hub, &workspace, &mut command)
                                {
                                    let _ = send_json(&mut socket, &ServerMessage::error(
                                        "attachment_invalid", error.to_string(), false, request_id,
                                    )).await;
                                    continue;
                                }
                                match runtime.send_command(command).await {
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
                        if send_ws(&mut socket, Message::Pong(payload)).await.is_err() {
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
