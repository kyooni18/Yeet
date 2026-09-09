use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    agent::{
        AgentCoordinator, AgentEvent, AgentRunOutcome, AgentRunRequest,
        is_internal_coordinator_system_message,
    },
    config::{ConfigStore, parse_context_length},
    core::{
        BridgeClient, BridgeEvent, CallRequest, HarnessCapabilityDescriptor, ImageAttachment,
        Message, Usage,
    },
    model::{
        AuthProviderItem, BridgeEnvelope, BridgeState, CapabilityToggleItem, ConversationEntry,
        ConversationKind, ConversationToolCall, FrontendCommand, ModelActivity,
        NativeAppPermission, ProviderConfigurationItem, SandboxAction, SandboxEnvironmentItem,
        SandboxLimitsState, SandboxNetworkItem, SandboxSettingsState, ToolCallStatus,
    },
    permission::PermissionBroker,
    project_settings::ProjectSettingsStore,
    sandbox::{
        NetworkEndpoint, SandboxMode, SandboxPolicy, SandboxStore, WorkspaceRead,
        validate_environment, validate_relative_path, validate_secret_id,
    },
    session_store::{RunStatus, SessionStore, StoredRun, StoredSession},
    tools::ToolRegistry,
    workers::WorkerRegistry,
};

mod debate_context;
mod debate_runtime;
mod events;
mod lifecycle;
mod settings;
mod settings_support;
mod state;
mod titles;
mod transport;

#[cfg(test)]
use debate_context::{DEBATE_PROJECT_BRIEF_CHARS, topic_mentions_identifier};
use debate_context::{debate_project_brief, discover_debate_subject};
#[cfg(test)]
use events::storage_model_history;
use events::{agent_event_log_value, apply_agent_event, persist_locked, record_usage};
#[cfg(test)]
use settings_support::detect_sandbox_preset;
use settings_support::{
    apply_sandbox_action, auth_login_options, finish_auth_action, finish_provider_action,
    foundation_server_ready, load_auth_providers, load_provider_configurations,
    sandbox_settings_state,
};
use state::SharedSession;
#[cfg(test)]
use titles::{
    TITLE_INPUT_HEAD_CHARS, TITLE_INPUT_TAIL_CHARS, normalize_generated_title, title_prompt_excerpt,
};
use titles::{fallback_title, generate_session_title, prepare_title_request};
use transport::{
    pretty_json, state_envelope, state_envelope_without_conversation, tool_activity_title,
    tool_detail,
};

use crate::background::BackgroundConnection;

fn default_attached_harness(capabilities: &[HarnessCapabilityDescriptor]) -> Vec<String> {
    let mut values = capabilities
        .iter()
        .filter(|capability| capability.default_attached)
        .map(|capability| capability.id.clone())
        .collect::<Vec<_>>();
    if !values.iter().any(|value| value == "web-search") {
        values.push("web-search".into());
    }
    values.sort();
    values.dedup();
    values
}

pub enum BackendEvent {
    Envelope(BridgeEnvelope),
}

pub struct Backend {
    connection: BackgroundConnection,
}

impl Backend {
    pub fn spawn() -> Result<Self> {
        Self::spawn_scoped(None)
    }

    pub fn spawn_remote() -> Result<Self> {
        Self::spawn_scoped(Some("remote"))
    }

    fn spawn_scoped(scope: Option<&str>) -> Result<Self> {
        let workspace = std::env::current_dir()?
            .canonicalize()
            .unwrap_or(std::env::current_dir()?);
        Ok(Self {
            connection: BackgroundConnection::connect_scoped(&workspace, scope)?,
        })
    }

    pub fn send(&mut self, command: FrontendCommand) -> Result<()> {
        self.connection.send(&command)
    }

    pub fn try_recv(&mut self) -> Option<BackendEvent> {
        self.connection.try_recv().map(BackendEvent::Envelope)
    }
}

pub(crate) struct BackendService {
    shared: Arc<Mutex<SharedSession>>,
    coordinator: Arc<Mutex<AgentCoordinator>>,
    bridge: BridgeClient,
    config: ConfigStore,
    project_settings: ProjectSettingsStore,
    project_identity: String,
    store: SessionStore,
    workspace_root: PathBuf,
    permission: PermissionBroker,
    active_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    events: Receiver<BackendEvent>,
    tx: Sender<BackendEvent>,
    closed: bool,
}

enum DebateTerminalOutcome {
    Completed,
    JuryUnavailable(String),
}

impl BackendService {
    pub(crate) fn spawn(workspace_root: PathBuf) -> Result<Self> {
        let workspace_root = workspace_root.canonicalize().unwrap_or(workspace_root);
        let config = ConfigStore::default();
        config.ensure()?;
        let model = config.model()?.unwrap_or_default();
        let reasoning_level = config.reasoning_level()?.unwrap_or_else(|| "auto".into());
        let project_settings = ProjectSettingsStore::new(&workspace_root)?;
        project_settings.ensure()?;
        let project = project_settings.load()?;
        let project_identity = project_settings.project_identity()?;
        let bridge = BridgeClient::start()?;
        bridge.set_openai_flex(project.openai_flex);
        let permission = PermissionBroker::default();
        let workers = WorkerRegistry::new(Vec::new())?;
        let mut registry = ToolRegistry::new(
            bridge.clone(),
            workspace_root.clone(),
            workers,
            permission.clone(),
        )?;
        registry.configure_foundation_memory(
            project.foundation_memory.enabled,
            project.foundation_memory.server.clone(),
            project_identity.clone(),
        );
        let coordinator = Arc::new(Mutex::new(AgentCoordinator::new(bridge.clone(), registry)));
        let store = SessionStore::new(&config.directory);
        store.prepare()?;
        if let Ok(mut coordinator) = coordinator.lock() {
            coordinator.set_session_runtime(store.clone(), None);
        }
        let mut session = SharedSession::new(model, reasoning_level);
        session.state.sandbox_settings = SandboxStore::new(&workspace_root)
            .and_then(|store| store.load())
            .ok()
            .map(|policy| sandbox_settings_state(&policy));
        session.state.openai_flex = project.openai_flex;
        session.state.foundation_memory_enabled = project.foundation_memory.enabled;
        session.state.foundation_memory_server = project.foundation_memory.server.clone();
        session.meta.attached_capabilities = project.capabilities.attached;
        session.meta.disabled_capabilities = project.capabilities.disabled;
        let shared = Arc::new(Mutex::new(session));
        let (tx, events) = mpsc::channel();
        {
            let shared = shared.clone();
            let tx = tx.clone();
            let permission_for_notify = permission.clone();
            permission.set_notifier(Arc::new(move || {
                if let Ok(mut state) = shared.lock() {
                    state.state.pending_shell_permission = permission_for_notify.pending_shell();
                    state.state.pending_native_app_permission =
                        permission_for_notify.pending_native_app();
                    let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
                }
            }));
        }
        let backend = Self {
            shared,
            coordinator,
            bridge,
            config,
            project_settings,
            project_identity,
            store,
            workspace_root,
            permission,
            active_cancel: Arc::new(Mutex::new(None)),
            events,
            tx,
            closed: false,
        };
        backend.refresh_context_length();
        backend.publish_state();
        Ok(backend)
    }

    pub(crate) fn send(&mut self, command: FrontendCommand) -> Result<()> {
        match command {
            FrontendCommand::Submit { text } => self.submit(text),
            FrontendCommand::StartDebate { topic, models } => self.start_debate(topic, models),
            FrontendCommand::Interrupt => {
                self.interrupt();
                Ok(())
            }
            FrontendCommand::AllowShell => {
                self.permission.resolve_shell(true);
                self.publish_state();
                Ok(())
            }
            FrontendCommand::DenyShell => {
                self.permission.resolve_shell(false);
                self.publish_state();
                Ok(())
            }
            FrontendCommand::AllowNativeApp => {
                self.permission.resolve_native_app(true);
                self.publish_state();
                Ok(())
            }
            FrontendCommand::DenyNativeApp => {
                self.permission.resolve_native_app(false);
                self.publish_state();
                Ok(())
            }
            FrontendCommand::RequestModels => {
                self.request_models();
                Ok(())
            }
            FrontendCommand::SelectModel { model } => self.select_model(model),
            FrontendCommand::SelectReasoning { level } => self.select_reasoning(level),
            FrontendCommand::RequestSessions => {
                self.request_sessions();
                Ok(())
            }
            FrontendCommand::LoadSession { session_id } => self.load_session(&session_id),
            FrontendCommand::NewSession => {
                self.new_session();
                Ok(())
            }
            FrontendCommand::RequestCapabilities => {
                self.request_capabilities();
                Ok(())
            }
            FrontendCommand::ToggleCapability { id } => self.toggle_capability(&id),
            FrontendCommand::RequestAuth => {
                self.request_auth();
                Ok(())
            }
            FrontendCommand::AuthLogin { provider } => {
                self.auth_login(provider);
                Ok(())
            }
            FrontendCommand::AuthLogout { provider } => {
                self.auth_logout(provider);
                Ok(())
            }
            FrontendCommand::AuthSetApiKey { provider, key } => {
                self.auth_set_api_key(provider, key);
                Ok(())
            }
            FrontendCommand::RequestProviders => {
                self.request_providers();
                Ok(())
            }
            FrontendCommand::SaveProvider {
                id,
                base_url,
                require_api_key,
            } => {
                self.save_provider(id, base_url, require_api_key);
                Ok(())
            }
            FrontendCommand::RemoveProvider { id } => {
                self.remove_provider(id);
                Ok(())
            }
            FrontendCommand::RequestSettings => {
                self.request_settings();
                Ok(())
            }
            FrontendCommand::SetOpenAiFlex { enabled } => {
                self.set_openai_flex(enabled);
                Ok(())
            }
            FrontendCommand::SetFoundationMemory { enabled } => {
                self.set_foundation_memory(enabled);
                Ok(())
            }
            FrontendCommand::RequestSandbox => {
                self.request_sandbox();
                Ok(())
            }
            FrontendCommand::UpdateSandbox { action } => {
                self.update_sandbox(action);
                Ok(())
            }
            FrontendCommand::Shutdown => {
                self.shutdown();
                Ok(())
            }
        }
    }

    pub(crate) fn try_recv(&self) -> Option<BackendEvent> {
        if let Some(event) = self.bridge.try_recv_event() {
            self.handle_bridge_event(event);
        }
        self.events.try_recv().ok()
    }

    fn handle_bridge_event(&self, event: BridgeEvent) {
        let BridgeEvent::NativeAppApprovalRequest(request) = event else {
            if self.permission.pending_native_app().is_some() {
                self.permission.resolve_native_app(false);
            }
            return;
        };
        let permission = self.permission.clone();
        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let request_id = request.request_id.clone();
            let native = NativeAppPermission {
                id: request_id.clone(),
                server: request.server,
                tool: request.tool,
                bundle_id: request.bundle_id,
                app_name: request.app_name,
                operation: request.operation,
                reason: request.message,
            };
            let granted = permission.request_native(native);
            let _ = bridge.send_native_app_approval_decision(&request_id, granted);
            if let Ok(mut state) = shared.lock() {
                state.state.pending_shell_permission = permission.pending_shell();
                state.state.pending_native_app_permission = permission.pending_native_app();
                let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
            }
        });
    }

    pub(crate) fn state_snapshot(&self) -> BridgeState {
        self.shared.lock().unwrap().state.clone()
    }

    pub(crate) fn is_streaming(&self) -> bool {
        self.shared.lock().unwrap().state.is_streaming
    }

    pub(crate) fn current_session_id(&self) -> Option<String> {
        self.shared.lock().unwrap().state.current_session_id.clone()
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed
    }

    fn submit(&mut self, text: String) -> Result<()> {
        let input = text.trim().to_owned();
        if input.is_empty() {
            return Ok(());
        }
        if input.starts_with('/') {
            return self.run_command(&input);
        }
        self.reload_project_capabilities()?;
        let turn_id = Uuid::new_v4().to_string();
        let images = {
            let mut shared = self.shared.lock().unwrap();
            if shared.state.is_streaming {
                return Ok(());
            }
            if !shared.meta.pending_images.is_empty()
                && shared
                    .meta
                    .attached_capabilities
                    .as_ref()
                    .is_some_and(|values| !values.iter().any(|value| value == "vision"))
            {
                shared.state.error_message = Some("Vision is detached. Enable it in /capabilities or run /attach vision before sending images.".into());
                drop(shared);
                self.publish_state();
                return Ok(());
            }
            std::mem::take(&mut shared.meta.pending_images)
        };
        {
            let mut shared = self.shared.lock().unwrap();
            shared.state.error_message = None;
            shared.state.is_streaming = true;
            shared.state.active_assistant_entry_id = None;
            shared.state.active_assistant_text.clear();
            shared.state.active_activity_entry_id = None;
            shared.meta.pending_tool_calls.clear();
            let run_model = shared.state.active_model.clone();
            shared.start_run(turn_id.clone(), "agent", run_model);
            let visible_input = if images.is_empty() {
                input.clone()
            } else {
                let names = images
                    .iter()
                    .filter_map(|image| image.name.as_deref())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{input}\n[{} image{}{}]",
                    images.len(),
                    if images.len() == 1 { "" } else { "s" },
                    if names.is_empty() {
                        String::new()
                    } else {
                        format!(": {names}")
                    }
                )
            };
            shared.append(ConversationKind::User {
                content: visible_input,
            });
            shared.set_activity("thinking", "Thinking", None);
        }
        self.publish_state();

        let model = self
            .shared
            .lock()
            .unwrap()
            .state
            .active_model
            .trim()
            .to_owned();
        let reasoning_level = self
            .shared
            .lock()
            .unwrap()
            .state
            .active_reasoning_level
            .clone();
        if model.is_empty() {
            let mut shared = self.shared.lock().unwrap();
            shared.set_activity("failed", "Failed", Some("No model selected".into()));
            shared.state.is_streaming = false;
            shared.state.error_message =
                Some("No model is configured. Run: yeet model set provider/model".into());
            let run_error = shared.state.error_message.clone();
            shared.finish_run(RunStatus::Failed, run_error);
            shared.meta.current_turn = None;
            drop(shared);
            self.publish_state();
            return Ok(());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *self.active_cancel.lock().unwrap() = Some(cancel.clone());
        {
            let history = self.coordinator.lock().unwrap().model_history();
            let mut s = self.shared.lock().unwrap();
            if let Err(error) = persist_locked(&mut s, &self.store, &self.workspace_root, history) {
                s.state.is_streaming = false;
                s.finish_run(RunStatus::Failed, Some(format!("Save failed: {error}")));
                s.meta.current_turn = None;
                s.state.error_message = Some(format!("Save failed: {error}"));
                drop(s);
                *self.active_cancel.lock().unwrap() = None;
                self.publish_state();
                return Ok(());
            }
        }
        if let Some(id) = self.shared.lock().unwrap().state.current_session_id.clone()
            && let Ok(mut coordinator) = self.coordinator.lock()
        {
            coordinator.set_protected_write_paths([self.store.directory.join(&id)]);
            coordinator.set_session_runtime(self.store.clone(), Some(id));
        }

        let shared = self.shared.clone();
        let coordinator = self.coordinator.clone();
        let tx = self.tx.clone();
        let store = self.store.clone();
        let workspace = self.workspace_root.clone();
        let active_cancel = self.active_cancel.clone();
        let permission = self.permission.clone();
        let bridge = self.bridge.clone();
        thread::spawn(move || {
            let (attached, disabled_capabilities) = shared
                .lock()
                .map(|value| {
                    (
                        value.meta.attached_capabilities.clone(),
                        value.meta.disabled_capabilities.clone(),
                    )
                })
                .unwrap_or_default();
            let result = coordinator
                .lock()
                .map_err(|_| anyhow!("agent coordinator lock poisoned"))
                .and_then(|mut coordinator| {
                    coordinator.run(
                        AgentRunRequest {
                            input: &input,
                            images,
                            model: &model,
                            reasoning_level: &reasoning_level,
                            attached_capabilities: attached,
                            disabled_capabilities,
                            cancel: cancel.clone(),
                        },
                        |event| {
                            if let Ok(mut state) = shared.lock() {
                                if state.meta.current_turn.as_deref() != Some(&turn_id) {
                                    return;
                                }
                                let omit_conversation = match &event {
                                    AgentEvent::ModelAttemptStarted { .. }
                                    | AgentEvent::ModelAttemptFinished(..)
                                    | AgentEvent::AuxiliaryUsage(_) => true,
                                    AgentEvent::TextDelta(_) => {
                                        state.state.active_assistant_entry_id.is_some()
                                    }
                                    AgentEvent::ReasoningDelta(_)
                                    | AgentEvent::ReasoningSummaryDelta(_) => {
                                        state.state.active_reasoning_entry_id.is_some()
                                    }
                                    _ => false,
                                };
                                if let Some(id) = &state.state.current_session_id
                                    && let Some(log_event) = agent_event_log_value(&event)
                                    && let Err(error) =
                                        store.append_event(id, Some(&turn_id), &log_event)
                                {
                                    state.state.error_message =
                                        Some(format!("Event log write failed: {error}"));
                                }
                                apply_agent_event(&mut state, event);
                                state.state.pending_shell_permission = permission.pending_shell();
                                state.state.pending_native_app_permission =
                                    permission.pending_native_app();
                                let envelope = if omit_conversation {
                                    state_envelope_without_conversation(&state.state)
                                } else {
                                    state_envelope(&state.state)
                                };
                                let _ = tx.send(BackendEvent::Envelope(envelope));
                            }
                        },
                    )
                });
            let mut title_request = None;
            if let Ok(mut state) = shared.lock()
                && state.meta.current_turn.as_deref() == Some(&turn_id)
            {
                let (run_status, run_error) = match result {
                    Err(error) if cancel.load(Ordering::Acquire) => {
                        state.set_activity("interrupted", "Interrupted", None);
                        (RunStatus::Interrupted, Some(error.to_string()))
                    }
                    Err(error) => {
                        let description = error.to_string();
                        state.state.error_message = Some(description.clone());
                        if state.state.active_assistant_text.is_empty() {
                            state.append_assistant_text(&format!("**Error:** {description}"));
                        }
                        state.set_activity("failed", "Failed", Some(description.clone()));
                        (RunStatus::Failed, Some(description))
                    }
                    Ok(AgentRunOutcome::Completed) => {
                        state.set_activity("done", "Done", None);
                        (RunStatus::Completed, None)
                    }
                    Ok(AgentRunOutcome::CompletedUnverified { reason }) => {
                        state.set_activity("done", "Done · Unverified", Some(reason.clone()));
                        (RunStatus::CompletedUnverified, Some(reason))
                    }
                };
                state.seal_assistant();
                state.state.active_assistant_entry_id = None;
                state.state.active_assistant_text.clear();
                state.state.active_reasoning_entry_id = None;
                state.state.active_reasoning_text.clear();
                state.state.active_reasoning_summary.clear();
                state.settle_pending_tool_calls(ToolCallStatus::Failed);
                state.state.pending_shell_permission = None;
                state.state.pending_native_app_permission = None;
                state.state.is_streaming = false;
                state.finish_run(run_status, run_error);
                state.meta.current_turn = None;
                if state.meta.pending_compaction
                    && let Ok(mut coordinator) = coordinator.lock()
                {
                    let compaction = coordinator.compact_model_history();
                    if let Ok((before, after)) = compaction {
                        state.meta.pending_compaction = false;
                        state.append(ConversationKind::System {
                            content: format!(
                                "New context window: {before} working messages to {after}. Original history is retrievable."
                            ),
                        });
                    } else if let Err(error) = compaction {
                        state.state.error_message = Some(error.to_string());
                    }
                }
                if let Ok(coordinator) = coordinator.lock() {
                    let _ =
                        persist_locked(&mut state, &store, &workspace, coordinator.model_history());
                }
                title_request = prepare_title_request(&mut state);
                let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
            }
            if let Some(request) = title_request
                && let Ok(title) = generate_session_title(&bridge, &request)
                && let Ok(mut state) = shared.lock()
                && state.state.current_session_id.as_deref() == Some(request.session_id.as_str())
            {
                state.meta.title = Some(title);
                if let Ok(coordinator) = coordinator.lock() {
                    let _ =
                        persist_locked(&mut state, &store, &workspace, coordinator.model_history());
                }
                let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
            }
            clear_matching_cancel(&active_cancel, &cancel);
        });
        Ok(())
    }

    fn run_command(&mut self, input: &str) -> Result<()> {
        let mut parts = input.split_whitespace();
        let command = parts.next().unwrap_or_default();
        let arguments: Vec<_> = parts.collect();
        match command {
            "/debate" => return self.start_debate(arguments.join(" "), None),
            "/help" => self.append_system("/new  /model  /login  /provider  /settings  /sessions  /capabilities  /image PATH|clear  /compact  /context [LENGTH|auto]  /status  /attach ID  /detach ID  /allow  /deny  /clear"),
            "/new" => self.new_session(),
            "/clear" => {
                let mut shared = self.shared.lock().unwrap();
                if let Some(entries) = shared.state.conversation.as_mut()
                    && let Some(index) = entries
                        .iter()
                        .rposition(|entry| matches!(entry.kind, ConversationKind::System { .. }))
                {
                    entries.remove(index);
                    shared.state.conversation_revision =
                        shared.state.conversation_revision.wrapping_add(1);
                }
                drop(shared);
                self.publish_state();
            }
            "/allow" => { self.permission.resolve(true); self.publish_state(); }
            "/deny" => { self.permission.resolve(false); self.publish_state(); }
            "/compact" => {
                let streaming = self.shared.lock().unwrap().state.is_streaming;
                if streaming {
                    self.shared.lock().unwrap().meta.pending_compaction = true;
                    self.append_system("Context compaction queued for the end of the current response.");
                } else {
                    self.compact_context()?;
                }
            }
            "/context" => {
                let model = self.shared.lock().unwrap().state.active_model.clone();
                if model.is_empty() {
                    self.append_error("No model is selected.".into());
                    return Ok(());
                }
                match arguments.first().copied() {
                    None => {
                        let override_length = self.config.context_length_override(&model)?;
                        let effective = self.shared.lock().unwrap().state.active_model_context_length;
                        let message = match (override_length, effective) {
                            (Some(value), _) => format!("Context length for {model}: {value} tokens (manual override)."),
                            (None, Some(value)) => format!("Context length for {model}: {value} tokens (automatic)."),
                            (None, None) => format!("Context length for {model}: unknown."),
                        };
                        self.append_system(&message);
                    }
                    Some("auto") | Some("reset") => {
                        self.config.set_context_length_override(&model, None)?;
                        self.refresh_context_length();
                        self.append_system(&format!("Context length override cleared for {model}."));
                    }
                    Some(value) => {
                        let length = parse_context_length(value)?;
                        self.config.set_context_length_override(&model, Some(length))?;
                        self.shared.lock().unwrap().state.active_model_context_length = Some(length);
                        self.append_system(&format!("Context length for {model} set to {length} tokens."));
                    }
                }
            }
            "/status" => self.append_system(&self.status_report()),
            "/image" => {
                let Some(argument) = arguments.first().copied() else {
                    let count = self.shared.lock().unwrap().meta.pending_images.len();
                    self.append_system(&format!("{count} image{} queued for the next turn. Use /image PATH to add one or /image clear to remove them.", if count == 1 { "" } else { "s" }));
                    return Ok(());
                };
                if argument == "clear" {
                    self.shared.lock().unwrap().meta.pending_images.clear();
                    self.append_system("Cleared queued images.");
                    return Ok(());
                }
                let path_text = arguments.join(" ");
                let path = PathBuf::from(&path_text);
                let path = if path.is_absolute() { path } else { self.workspace_root.join(path) };
                let image = ImageAttachment::from_file(&path)?;
                let name = image.name.clone().unwrap_or_else(|| path.display().to_string());
                let mut shared = self.shared.lock().unwrap();
                shared.meta.pending_images.push(image);
                let count = shared.meta.pending_images.len();
                drop(shared);
                self.append_system(&format!("Queued {name} for the next turn ({count} total)."));
            }
            "/capabilities" => {
                self.reload_project_capabilities()?;
                let capabilities = self.bridge.list_harness_capabilities()?;
                let attached = self.shared.lock().unwrap().meta.attached_capabilities.clone();
                let effective: Vec<String> =
                    attached.unwrap_or_else(|| default_attached_harness(&capabilities));
                let mut lines = capabilities.into_iter().map(|capability| format!("{} [{}] — {}", capability.id, if effective.contains(&capability.id) { "attached" } else { "detached" }, capability.description)).collect::<Vec<_>>();
                lines.push(format!("web-search [{}] — Default-attached live web search through Agent-Reach/Exa with managed SearXNG fallback.", if effective.iter().any(|value| value == "web-search") { "attached" } else { "detached" }));
                let text = lines.join("\n");
                self.append_system(if text.is_empty() { "No harness capabilities are available." } else { &text });
            }
            "/attach" => {
                let Some(id) = arguments.first() else { self.append_system("Usage: /attach capability-id"); return Ok(()); };
                self.reload_project_capabilities()?;
                let capabilities = self.bridge.list_harness_capabilities()?;
                if *id != "web-search" && !capabilities.iter().any(|capability| capability.id == *id) { self.append_error(format!("Unknown capability: {id}")); return Ok(()); }
                let mut shared = self.shared.lock().unwrap();
                let mut values = shared.meta.attached_capabilities.clone().unwrap_or_else(|| default_attached_harness(&capabilities));
                if !values.iter().any(|value| value == id) { values.push((*id).to_owned()); values.sort(); }
                shared.meta.attached_capabilities = Some(values); drop(shared);
                self.save_project_capabilities()?;
                self.append_system(&format!("Attached {id}."));
            }
            "/detach" => {
                let Some(id) = arguments.first() else { self.append_system("Usage: /detach capability-id"); return Ok(()); };
                self.reload_project_capabilities()?;
                let capabilities = self.bridge.list_harness_capabilities()?;
                let mut shared = self.shared.lock().unwrap();
                let mut values = shared.meta.attached_capabilities.clone().unwrap_or_else(|| default_attached_harness(&capabilities));
                values.retain(|value| value != id); shared.meta.attached_capabilities = Some(values); drop(shared);
                self.save_project_capabilities()?;
                self.append_system(&format!("Detached {id}."));
            }
            "/provider" => self.append_system("Use `yeet provider ...` for custom API endpoints."),
            "/login" => self.append_system("Use `yeet auth ...` for provider authentication."),
            "/settings" => self.append_system(
                "Open /settings in the interactive TUI to manage runtime and sandbox settings.",
            ),
            "/model" | "/reasoning" | "/sessions" => {},
            _ => self.append_error(format!("Unknown command: {command}. Type /help for commands.")),
        }
        Ok(())
    }

    fn status_report(&self) -> String {
        let state = self.shared.lock().unwrap().state.without_conversation();
        let current_context = state.current_context_tokens.unwrap_or(0);
        let context = match state.active_model_context_length {
            Some(total) if total > 0 => format!(
                "Context: {current_context}/{total} tokens ({:.1}% used)",
                current_context as f64 * 100.0 / total as f64
            ),
            Some(total) => format!("Context: {current_context}/{total} tokens"),
            None => format!("Context: {current_context} tokens / unknown limit"),
        };
        let usage = &state.token_usage;
        let input = usage.input_tokens.unwrap_or(0);
        let output = usage.output_tokens.unwrap_or(0);
        let cached = usage.cached_input_tokens.unwrap_or(0).min(input);
        let cache_write = usage.cache_write_input_tokens.unwrap_or(0);
        let reasoning_tokens = usage.reasoning_tokens.unwrap_or(0);
        let reasoning_mode = if state.active_reasoning_level.is_empty() {
            "auto"
        } else {
            state.active_reasoning_level.as_str()
        };
        let (permission, sandbox_detail) = state
            .sandbox_settings
            .as_ref()
            .map(|settings| {
                (
                    settings.permission_mode(),
                    format!(
                        "preset {} · execution {} · auto-approve {}",
                        settings.preset, settings.execution_mode, settings.auto_approve
                    ),
                )
            })
            .unwrap_or(("unknown", "sandbox state unavailable".to_owned()));
        let cache_line = if input > 0 {
            format!(
                "Cache: read {cached} tokens ({:.0}% hit) · write {cache_write} tokens",
                cached as f64 * 100.0 / input as f64
            )
        } else {
            format!("Cache: read {cached} tokens · write {cache_write} tokens")
        };
        let mut lines = vec![
            format!(
                "Model: {}",
                if state.active_model.is_empty() {
                    "not selected"
                } else {
                    &state.active_model
                }
            ),
            context,
            format!("Tokens: input {input} · output {output} · reasoning {reasoning_tokens}"),
            cache_line,
            format!("Reasoning mode: {reasoning_mode}"),
            format!("Permission: {permission} · {sandbox_detail}"),
            format!(
                "Runtime: {}",
                if state.is_streaming {
                    "executing"
                } else {
                    "idle"
                }
            ),
        ];
        if let Some(cost) = usage.estimated_cost_usd {
            lines.push(format!("Estimated cost: ${cost:.4}"));
        }
        if state.credit_usage > 0 {
            lines.push(format!("Provider calls / credits: {}", state.credit_usage));
        }
        if let Some(session_id) = state.current_session_id.as_deref() {
            lines.push(format!("Session: {session_id}"));
        }
        if let Some(run_id) = state.active_run_id.as_deref() {
            lines.push(format!("Run: {run_id}"));
        }
        if let Some(error) = state.error_message.as_deref() {
            lines.push(format!("Last error: {error}"));
        }
        if let Some((provider, _)) = state.active_model.split_once('/')
            && let Ok(provider_usage) = self.bridge.provider_usage(provider)
            && provider_usage.source != "none"
        {
            if provider_usage.windows.is_empty() {
                lines.push(format!(
                    "Quota: {} · {}",
                    provider_usage.source,
                    provider_usage.message.as_deref().unwrap_or("unavailable")
                ));
            } else {
                let windows = provider_usage
                    .windows
                    .iter()
                    .map(|window| {
                        let reset = window
                            .resets_at
                            .as_deref()
                            .map(|value| format!(" · resets {value}"))
                            .unwrap_or_default();
                        format!("{} {}% left{reset}", window.label, window.remaining_percent)
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                lines.push(format!("Quota: {} · {windows}", provider_usage.source));
            }
        }
        lines.join("\n")
    }

    fn request_models(&self) {
        {
            let mut shared = self.shared.lock().unwrap();
            shared.state.is_loading_models = true;
            shared.state.error_message = None;
        }
        self.publish_state();
        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        let config = self.config.clone();
        thread::spawn(move || {
            let result = (|| -> Result<(Vec<String>, HashMap<String, u64>)> {
                let mut providers = bridge.list_providers()?;
                providers.sort();
                let mut models = Vec::new();
                let mut lengths = HashMap::new();
                for provider in providers {
                    if let Ok(info) = bridge.list_model_info(&provider) {
                        for model in info {
                            let full = format!("{provider}/{}", model.id);
                            if let Some(length) = model.context_length {
                                lengths.insert(full.clone(), length);
                            }
                            models.push(full);
                        }
                    }
                }
                models.sort_by_key(|value| value.to_ascii_lowercase());
                models.dedup();
                Ok((models, lengths))
            })();
            if let Ok(mut state) = shared.lock() {
                match result {
                    Ok((models, lengths)) => {
                        for (model, length) in lengths {
                            let _ = config.set_context_length(&model, Some(length));
                        }
                        state.state.available_models = models;
                        if state.state.available_models.is_empty() {
                            state.state.error_message = Some(
                                "No available models could be loaded. Check provider credentials."
                                    .into(),
                            );
                        }
                    }
                    Err(error) => {
                        state.state.error_message = Some(format!("Unable to load models: {error}"))
                    }
                }
                state.state.is_loading_models = false;
                let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
            }
        });
    }

    fn select_model(&self, model: String) -> Result<()> {
        let model = self.config.set_model(&model)?;
        {
            let mut shared = self.shared.lock().unwrap();
            shared.state.active_model = model.clone();
            shared.state.active_model_context_length = None;
            shared.state.current_context_tokens = None;
            shared.append(ConversationKind::System {
                content: format!("Model set to {model}"),
            });
        }
        self.refresh_context_length();
        self.publish_state();
        Ok(())
    }

    fn select_reasoning(&self, level: String) -> Result<()> {
        let level = self.config.set_reasoning_level(&level)?;
        self.shared.lock().unwrap().state.active_reasoning_level = level;
        self.publish_state();
        Ok(())
    }

    fn refresh_context_length(&self) {
        let model = self.shared.lock().unwrap().state.active_model.clone();
        if model.is_empty() {
            return;
        }
        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        let config = self.config.clone();
        thread::spawn(move || {
            let length = config
                .context_length_override(&model)
                .ok()
                .flatten()
                .or_else(|| bridge.context_length(&model).ok().flatten())
                .or_else(|| config.context_length(&model).ok().flatten());
            if let Ok(mut state) = shared.lock()
                && state.state.active_model == model
            {
                state.state.active_model_context_length = length;
                let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
            }
        });
    }

    fn request_sessions(&self) {
        let result = self.store.list(&self.workspace_root);
        let mut shared = self.shared.lock().unwrap();
        match result {
            Ok(sessions) => shared.state.saved_sessions = sessions,
            Err(error) => {
                shared.state.error_message = Some(format!("Unable to list sessions: {error}"))
            }
        }
        drop(shared);
        self.publish_state();
    }

    fn request_capabilities(&self) {
        if let Err(error) = self.reload_project_capabilities() {
            self.append_error(format!("Unable to load project settings: {error}"));
            return;
        }
        {
            let mut shared = self.shared.lock().unwrap();
            shared.state.is_loading_capabilities = true;
            shared.state.error_message = None;
        }
        self.publish_state();

        let bridge = self.bridge.clone();
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = (|| -> Result<Vec<CapabilityToggleItem>> {
                let harness = bridge.list_harness_capabilities()?;
                let skills = bridge.list_skills().unwrap_or_default();
                let mcp = bridge.list_mcp_servers().unwrap_or_default();

                let state = shared
                    .lock()
                    .map_err(|_| anyhow!("session lock poisoned"))?;
                let effective_harness = state
                    .meta
                    .attached_capabilities
                    .clone()
                    .unwrap_or_else(|| default_attached_harness(&harness));
                let disabled = state.meta.disabled_capabilities.clone();
                drop(state);

                let mut items = harness
                    .into_iter()
                    .map(|capability| CapabilityToggleItem {
                        id: capability.id.clone(),
                        kind: "capability".into(),
                        name: capability.name,
                        description: capability.description,
                        enabled: effective_harness.contains(&capability.id),
                    })
                    .collect::<Vec<_>>();
                items.push(CapabilityToggleItem {
                    id: "web-search".into(),
                    kind: "capability".into(),
                    name: "Web Search".into(),
                    description:
                        "Default-attached live web search through Agent-Reach/Exa with managed SearXNG fallback."
                            .into(),
                    enabled: effective_harness.iter().any(|value| value == "web-search"),
                });
                items.extend(
                    crate::tools::builtin_capabilities()
                        .iter()
                        .map(|capability| CapabilityToggleItem {
                            id: capability.id.into(),
                            kind: "builtin".into(),
                            name: capability.name.into(),
                            description: capability.description.into(),
                            enabled: !disabled.iter().any(|value| value == capability.id),
                        }),
                );
                items.extend(skills.into_iter().map(|skill| {
                    let id = format!("skill:{}", skill.name);
                    CapabilityToggleItem {
                        enabled: !disabled.contains(&id),
                        id,
                        kind: "skill".into(),
                        name: skill.name,
                        description: skill.description,
                    }
                }));
                items.extend(mcp.into_iter().map(|server| {
                    let id = format!("mcp:{}", server.name);
                    CapabilityToggleItem {
                        enabled: !disabled.contains(&id),
                        id,
                        kind: "mcp".into(),
                        name: server.name,
                        description: format!(
                            "{} MCP server{}",
                            server.transport,
                            if server.connected {
                                " · connected"
                            } else {
                                ""
                            }
                        ),
                    }
                }));
                items.sort_by(|a, b| {
                    a.kind.cmp(&b.kind).then_with(|| {
                        a.name
                            .to_ascii_lowercase()
                            .cmp(&b.name.to_ascii_lowercase())
                    })
                });
                Ok(items)
            })();

            if let Ok(mut state) = shared.lock() {
                state.state.is_loading_capabilities = false;
                match result {
                    Ok(items) => state.state.available_capabilities = items,
                    Err(error) => {
                        state.state.error_message =
                            Some(format!("Unable to load capabilities: {error}"))
                    }
                }
                let _ = tx.send(BackendEvent::Envelope(state_envelope(&state.state)));
            }
        });
    }

    fn toggle_capability(&self, id: &str) -> Result<()> {
        self.reload_project_capabilities()?;
        let harness = self.bridge.list_harness_capabilities()?;
        let is_harness = id == "web-search" || harness.iter().any(|capability| capability.id == id);
        let is_lazy = id.starts_with("skill:") || id.starts_with("mcp:");
        let is_builtin = crate::tools::builtin_capabilities()
            .iter()
            .any(|capability| capability.id == id);
        if !is_harness && !is_lazy && !is_builtin {
            return Err(anyhow!("Unknown capability: {id}"));
        }

        {
            let mut shared = self.shared.lock().unwrap();
            if shared.state.is_streaming {
                shared.state.error_message =
                    Some("Capabilities cannot be changed while a response is running.".into());
                drop(shared);
                self.publish_state();
                return Ok(());
            }
            if is_harness {
                let mut values = shared
                    .meta
                    .attached_capabilities
                    .clone()
                    .unwrap_or_else(|| default_attached_harness(&harness));
                if values.iter().any(|value| value == id) {
                    values.retain(|value| value != id);
                } else {
                    values.push(id.to_owned());
                    values.sort();
                    values.dedup();
                }
                shared.meta.attached_capabilities = Some(values);
            } else {
                if shared
                    .meta
                    .disabled_capabilities
                    .iter()
                    .any(|value| value == id)
                {
                    shared
                        .meta
                        .disabled_capabilities
                        .retain(|value| value != id);
                } else {
                    shared.meta.disabled_capabilities.push(id.to_owned());
                    shared.meta.disabled_capabilities.sort();
                    shared.meta.disabled_capabilities.dedup();
                }
            }
        }
        self.save_project_capabilities()?;
        self.request_capabilities();
        Ok(())
    }

    fn save_project_capabilities(&self) -> Result<()> {
        let shared = self.shared.lock().unwrap();
        self.project_settings.save_capabilities(
            shared.meta.attached_capabilities.clone(),
            shared.meta.disabled_capabilities.clone(),
        )
    }

    fn reload_project_capabilities(&self) -> Result<()> {
        let project = self.project_settings.load()?;
        let mut shared = self.shared.lock().unwrap();
        shared.meta.attached_capabilities = project.capabilities.attached;
        shared.meta.disabled_capabilities = project.capabilities.disabled;
        Ok(())
    }
}

impl Drop for BackendService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn clear_matching_cancel(slot: &Arc<Mutex<Option<Arc<AtomicBool>>>>, completed: &Arc<AtomicBool>) {
    if let Ok(mut active) = slot.lock()
        && active
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, completed))
    {
        *active = None;
    }
}

pub fn forward_cli(arguments: &[String]) -> Result<i32> {
    crate::cli::run(arguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_search_is_part_of_default_attached_harness() {
        let harness = vec![
            HarnessCapabilityDescriptor {
                id: "vision".into(),
                name: "Vision".into(),
                description: "images".into(),
                default_attached: true,
            },
            HarnessCapabilityDescriptor {
                id: "lead".into(),
                name: "Lead Agent".into(),
                description: "primary agent marker".into(),
                default_attached: false,
            },
        ];

        let attached = default_attached_harness(&harness);

        assert!(attached.iter().any(|value| value == "vision"));
        assert!(attached.iter().any(|value| value == "web-search"));
        assert!(!attached.iter().any(|value| value == "lead"));
    }

    #[test]
    fn implementation_topics_bind_broad_current_logic_references_to_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("src")).unwrap();
        for topic in [
            "Assess the current deorbit logic",
            "Review the existing low-level control loop",
            "현재 착륙 로직의 안정성을 검토해",
        ] {
            let subject = discover_debate_subject(topic, workspace.path());
            assert!(subject.workspace_bound, "topic was not bound: {topic}");
            assert!(
                subject
                    .workspace_root
                    .contains(workspace.path().file_name().unwrap().to_str().unwrap())
            );
        }
        assert!(
            !discover_debate_subject("Assess the current geopolitical climate", workspace.path())
                .workspace_bound
        );
    }

    #[test]
    fn named_project_topics_bind_to_the_matching_workspace() {
        let parent = tempfile::tempdir().unwrap();
        let workspace = parent.path().join("Yeet");
        std::fs::create_dir_all(workspace.join("src")).unwrap();

        let subject = discover_debate_subject(
            "Is current direction of yeet having tons of tools right?",
            &workspace,
        );
        assert!(subject.workspace_bound);
        assert!(subject.workspace_root.ends_with("Yeet"));
        assert!(!topic_mentions_identifier("street", "tree"));
    }

    #[test]
    fn debate_project_brief_captures_manifest_readme_and_structure() {
        let parent = tempfile::tempdir().unwrap();
        let workspace = parent.path().join("Yeet");
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::create_dir_all(workspace.join("RuntimeSource")).unwrap();
        std::fs::create_dir_all(workspace.join("target")).unwrap();
        std::fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"yeet\"\ndescription = \"A general-purpose agent TUI\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            workspace.join("README.md"),
            "# Yeet\n\nYeet is a Rust-native general-purpose agent TUI with an isolated coding mode.\n\n## Build\n\nMore details follow.\n",
        )
        .unwrap();

        let brief = debate_project_brief(&workspace);

        assert!(brief.contains("Project name: Yeet"));
        assert!(brief.contains("Rust crate yeet: A general-purpose agent TUI"));
        assert!(brief.contains("RuntimeSource/"));
        assert!(brief.contains("src/"));
        assert!(!brief.contains("target/"));
        assert!(brief.contains("Rust-native general-purpose agent TUI"));
        assert!(brief.chars().count() <= DEBATE_PROJECT_BRIEF_CHARS);
    }

    #[test]
    fn persisted_model_history_drops_internal_profile_system_prompt() {
        let stored = storage_model_history(vec![
            Message::system(crate::agent::SYSTEM_INSTRUCTION),
            Message::user("hello"),
            Message::assistant("world", None),
        ]);
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].role, crate::core::MessageRole::User);
        assert!(
            !serde_json::to_string(&stored)
                .unwrap()
                .contains("You are Yeet's agent")
        );

        let custom = storage_model_history(vec![
            Message::system("project-specific user system context"),
            Message::user("hello"),
        ]);
        assert_eq!(custom.len(), 2);
        assert_eq!(custom[0].role, crate::core::MessageRole::System);
    }

    #[test]
    fn generated_session_titles_are_normalized() {
        assert_eq!(
            normalize_generated_title("\"Fix Yeet Scrolling.\"\nextra"),
            Some("Fix Yeet Scrolling".into())
        );
        assert_eq!(
            normalize_generated_title("Title: Rust TUI Migration"),
            Some("Rust TUI Migration".into())
        );
        assert_eq!(normalize_generated_title("   "), None);
        let long = normalize_generated_title("This title is intentionally much longer than the persisted session title limit for Yeet").unwrap();
        assert!(long.chars().count() <= 56);
    }

    #[test]
    fn title_prompt_excerpt_bounds_large_requests_and_keeps_both_ends() {
        let input = format!("BEGIN-{}-END", "x".repeat(10_000));
        let excerpt = title_prompt_excerpt(&input);

        assert!(excerpt.starts_with("BEGIN-"));
        assert!(excerpt.ends_with("-END"));
        assert!(excerpt.contains("title input omitted"));
        assert!(excerpt.chars().count() <= TITLE_INPUT_HEAD_CHARS + TITLE_INPUT_TAIL_CHARS + 32);
    }

    #[test]
    fn consecutive_text_deltas_stay_in_one_assistant_entry() {
        let mut state = SharedSession::new("test/model".into(), "medium".into());
        state.set_activity("thinking", "Thinking", None);

        apply_agent_event(&mut state, AgentEvent::TextDelta("Hel".into()));
        let assistant_id = state.state.active_assistant_entry_id.clone().unwrap();
        apply_agent_event(&mut state, AgentEvent::TextDelta("lo".into()));

        assert_eq!(
            state.state.active_assistant_entry_id.as_deref(),
            Some(assistant_id.as_str())
        );
        assert_eq!(state.state.active_assistant_text, "Hello");
        assert_eq!(
            state
                .state
                .conversation
                .as_ref()
                .unwrap()
                .iter()
                .filter(|entry| matches!(entry.kind, ConversationKind::Assistant { .. }))
                .count(),
            1
        );
        assert_eq!(
            state
                .state
                .conversation
                .as_ref()
                .unwrap()
                .iter()
                .filter(|entry| matches!(
                    &entry.kind,
                    ConversationKind::Activity { activity }
                        if activity.phase.as_str() == Some("responding")
                ))
                .count(),
            1
        );
    }

    #[test]
    fn turn_activity_updates_in_place_and_settles_last() {
        let mut state = SharedSession::new("test/model".into(), "medium".into());
        state.set_activity("thinking", "Thinking", None);
        let activity_id = state.state.active_activity_entry_id.clone().unwrap();

        state.set_activity("reasoning", "Reasoning", None);
        state.set_activity("tool", "Reading", Some("src/backend.rs".into()));

        assert_eq!(
            state.state.active_activity_entry_id.as_deref(),
            Some(activity_id.as_str())
        );
        assert_eq!(
            state
                .state
                .conversation
                .as_ref()
                .unwrap()
                .iter()
                .filter(|entry| matches!(entry.kind, ConversationKind::Activity { .. }))
                .count(),
            1
        );

        state.append_assistant_text("done");
        state.set_activity("done", "Done", None);
        let entries = state.state.conversation.as_ref().unwrap();
        assert!(matches!(
            entries.last().map(|entry| &entry.kind),
            Some(ConversationKind::Activity { activity })
                if activity.phase.as_str() == Some("done") && activity.title == "Done"
        ));
    }

    #[test]
    fn later_run_activity_cannot_overwrite_an_earlier_completed_run() {
        let mut state = SharedSession::new("test/model".into(), "medium".into());
        state.start_run("run-a".into(), "debate", "test/model".into());
        state.set_activity("thinking", "Debating", None);
        state.set_activity("done", "Debate completed", None);
        state.finish_run(RunStatus::Completed, None);

        state.start_run("run-b".into(), "agent", "test/model".into());
        state.set_activity("failed", "Failed", Some("provider error".into()));
        state.finish_run(RunStatus::Failed, Some("provider error".into()));

        let activities = state
            .state
            .conversation
            .as_ref()
            .unwrap()
            .iter()
            .filter_map(|entry| match &entry.kind {
                ConversationKind::Activity { activity } => Some(activity),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(activities.len(), 2);
        assert_eq!(activities[0].run_id.as_deref(), Some("run-a"));
        assert_eq!(activities[0].phase.as_str(), Some("done"));
        assert_eq!(activities[1].run_id.as_deref(), Some("run-b"));
        assert_eq!(activities[1].phase.as_str(), Some("failed"));
        assert_eq!(state.meta.runs[0].status, RunStatus::Completed);
        assert_eq!(state.meta.runs[1].status, RunStatus::Failed);
    }

    #[test]
    fn tool_activity_details_are_task_specific() {
        let edit = crate::core::ToolCall {
            id: "1".into(),
            name: "apply_file_edits".into(),
            arguments: json!({
                "changes": [
                    {"path": "src/app.rs"},
                    {"path": "src/ui.rs"}
                ]
            }),
        };
        assert_eq!(tool_activity_title(&edit.name), "Editing");
        assert_eq!(tool_detail(&edit).as_deref(), Some("src/app.rs +1 file"));

        let search = crate::core::ToolCall {
            id: "2".into(),
            name: "search_workspace".into(),
            arguments: json!({"query": "active_activity", "path": "src"}),
        };
        assert_eq!(tool_activity_title(&search.name), "Searching");
        assert_eq!(
            tool_detail(&search).as_deref(),
            Some("active_activity · src")
        );
    }

    #[test]
    fn tool_calls_are_persisted_and_finish_with_execution_status() {
        let mut state = SharedSession::new("test/model".into(), "medium".into());
        let call = crate::core::ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: json!({"path": "src/ui.rs", "startLine": 10, "endLine": 20}),
        };

        apply_agent_event(
            &mut state,
            AgentEvent::ToolCall {
                index: 0,
                call: call.clone(),
            },
        );

        let entries = state.state.conversation.as_ref().unwrap();
        assert!(entries.iter().any(|entry| matches!(
            &entry.kind,
            ConversationKind::ToolCall { tool_call }
                if tool_call.call_id.as_deref() == Some("call-1")
                    && matches!(tool_call.status, ToolCallStatus::Streaming)
        )));

        apply_agent_event(
            &mut state,
            AgentEvent::ToolExecutionFinished {
                call: call.clone(),
                succeeded: true,
                result: "ok".into(),
            },
        );

        let entries = state.state.conversation.as_ref().unwrap();
        assert!(entries.iter().any(|entry| matches!(
            &entry.kind,
            ConversationKind::ToolCall { tool_call }
                if tool_call.call_id.as_deref() == Some("call-1")
                    && matches!(tool_call.status, ToolCallStatus::Completed)
        )));
        assert!(state.meta.pending_tool_calls.is_empty());
    }

    #[test]
    fn sandbox_presets_are_complete_policies_and_custom_changes_are_detected() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().to_path_buf();

        let balanced = apply_sandbox_action(
            &workspace,
            SandboxAction::ApplyPreset {
                preset: "balanced".into(),
            },
        )
        .unwrap();
        assert_eq!(detect_sandbox_preset(&balanced), "balanced");
        assert_eq!(balanced.workspace_read, WorkspaceRead::All);
        assert!(!balanced.auto_approve);

        let custom = apply_sandbox_action(
            &workspace,
            SandboxAction::SetScratchWritable { enabled: false },
        )
        .unwrap();
        assert_eq!(detect_sandbox_preset(&custom), "custom");

        let safe = apply_sandbox_action(
            &workspace,
            SandboxAction::ApplyPreset {
                preset: "safe".into(),
            },
        )
        .unwrap();
        assert_eq!(detect_sandbox_preset(&safe), "safe");
        assert_eq!(safe, SandboxPolicy::default());

        let unlimited = apply_sandbox_action(
            &workspace,
            SandboxAction::ApplyPreset {
                preset: "unlimited".into(),
            },
        )
        .unwrap();
        assert_eq!(detect_sandbox_preset(&unlimited), "unlimited");
        assert_eq!(unlimited.mode, SandboxMode::Unlimited);
        assert!(unlimited.auto_approve);
        assert_eq!(unlimited.workspace_read, WorkspaceRead::All);
    }
}
