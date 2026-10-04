use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
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
    agents::{
        AgentGroupSupervisor,
        group::AgentLimits,
        runtime::{AgentRuntimeFactory, RunManager},
    },
    config::{ConfigStore, parse_context_length},
    core::{
        BridgeEvent, CallRequest, HarnessCapabilityDescriptor, ImageAttachment, Message, Usage,
    },
    model::{
        AgentMode, AuthProviderItem, AutonomyMode, BridgeEnvelope, BridgeState,
        CapabilityToggleItem, ConversationEntry, ConversationKind, ConversationToolCall,
        FrontendCommand, ModelActivity, ModelCatalogItem, NativeAppPermission,
        ProviderConfigurationItem, RuntimeSettingsState, SandboxAction, SandboxEnvironmentItem,
        SandboxLimitsState, SandboxNetworkItem, SandboxSettingsState, SessionSummary,
        ToolCallStatus, WorkspaceSessionGroup, WorkspaceSummary, normalize_reasoning_level,
        reasoning_levels_for_model,
    },
    permission::PermissionBroker,
    project_settings::{ProjectSettingsStore, ServiceBackend},
    sandbox::{
        NetworkEndpoint, SandboxMode, SandboxPolicy, SandboxStore, WorkspaceRead,
        validate_environment, validate_relative_path, validate_secret_id,
    },
    session_store::{RunStatus, SessionStore, StoredRun, StoredSession},
    tools::{BridgeHandle, ToolRegistry},
    workers::WorkerRegistry,
};

mod agent_notifications;
mod capabilities;
mod commands;
mod debate_context;
mod debate_runtime;
mod events;
mod input_context;
mod lifecycle;
mod run;
mod service_methods;
mod settings;
mod settings_support;
mod state;
mod sync;
mod titles;
mod transport;

use debate_context::{debate_project_brief, discover_debate_subject};
use events::{
    PreparedSessionWrite, agent_event_log_value, apply_agent_event, commit_session_write,
    persist_locked, prepare_session_write_locked, record_usage,
};
use input_context::{
    REMOTE_FILE_CONTEXT_CLOSE, REMOTE_FILE_CONTEXT_OPEN, split_remote_file_context,
    visible_user_content,
};
use settings_support::{
    apply_sandbox_action, auth_login_options, finish_auth_action, finish_provider_action,
    foundation_server_ready, load_auth_providers, load_provider_configurations,
    runtime_settings_state, sandbox_settings_state,
};
use state::SharedSession;
pub(crate) use sync::{EventSender, SessionCatalog};
use sync::{LockExt, apply_session_catalog_locked};
use titles::{fallback_title, generate_session_title, prepare_title_request};
pub use transport::Backend;
pub(crate) use transport::tool_detail;
use transport::{
    cache_status_line, pretty_json, state_envelope, state_envelope_without_conversation,
    tool_activity_title,
};

use crate::background::Wake;
const GOAL_RESUME_PROMPT: &str = "Continue the current goal from the existing working state. Do not restart completed work. Make useful forward progress toward satisfying every requirement and do not stop until the strict goal judge can accept concrete evidence.";

const AUTONOMOUS_NEXT_OBJECTIVE_PROMPT: &str = "Autonomous cycle: inspect the current conversation, workspace, working state, and completed work. Choose and complete exactly one concrete, useful, safe next objective that advances the user's established intent. Prefer unfinished work, verification, integration, or cleanup that materially improves the result. Do not invent busywork or repeat completed work. If there is no meaningful safe work left, respond with exactly AUTONOMOUS_IDLE and do not call tools.";
const AUTONOMOUS_IDLE_MARKER: &str = "AUTONOMOUS_IDLE";
const SKYLINE_CAPABILITY_ID: &str = crate::skyline::CAPABILITY_ID;
const CAPABILITY_STREAMING_LOCK_ERROR: &str =
    "Capabilities cannot be changed while a response is running.";
const SESSION_ENVIRONMENT_STREAMING_LOCK_ERROR: &str =
    "Session environment cannot be changed while a response is running.";

fn session_only_capability(id: &str) -> bool {
    id.starts_with("skill:") || id == SKYLINE_CAPABILITY_ID
}

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

fn model_selection_changes(current: &str, next: &str) -> bool {
    current != next
}

fn session_environment_mutation_allowed(is_streaming: bool) -> bool {
    !is_streaming
}

fn clear_resolved_streaming_lock_error(error_message: &mut Option<String>) {
    if matches!(
        error_message.as_deref(),
        Some(CAPABILITY_STREAMING_LOCK_ERROR | SESSION_ENVIRONMENT_STREAMING_LOCK_ERROR)
    ) {
        *error_message = None;
    }
}

fn native_app_approval_is_automatic(policy: &SandboxPolicy) -> bool {
    policy.mode == SandboxMode::Unlimited || policy.auto_approve
}

pub enum BackendEvent {
    Envelope(BridgeEnvelope),
}

pub(crate) struct BackendService {
    shared: Arc<Mutex<SharedSession>>,
    coordinator: Arc<Mutex<AgentCoordinator>>,
    bridge: BridgeHandle,
    config: ConfigStore,
    project_settings: ProjectSettingsStore,
    project_identity: String,
    store: SessionStore,
    workspace_root: PathBuf,
    permission: PermissionBroker,
    run_manager: RunManager,
    agent_groups: AgentGroupSupervisor,
    goal_mode: Arc<AtomicBool>,
    events: Receiver<BackendEvent>,
    tx: EventSender,
    closed: bool,
}

enum DebateTerminalOutcome {
    Completed,
    JuryUnavailable(String),
}

impl BackendService {
    pub(crate) fn spawn(workspace_root: PathBuf, wake: Option<Wake>) -> Result<Self> {
        let workspace_root = workspace_root.canonicalize().unwrap_or(workspace_root);
        let config = ConfigStore::default();
        config.ensure()?;
        let model = config.model()?.unwrap_or_default();
        let mut reasoning_level = config.reasoning_level()?.unwrap_or_else(|| "auto".into());
        if !model.is_empty()
            && !reasoning_levels_for_model(&model).contains(&reasoning_level.as_str())
        {
            reasoning_level = config.set_reasoning_level("high")?;
        }
        let project_settings = ProjectSettingsStore::new(&workspace_root)?;
        project_settings.ensure()?;
        let project = project_settings.load()?;
        let project_identity = project_settings.project_identity()?;
        let bridge = BridgeHandle::lazy_for_workspace(workspace_root.clone());
        bridge.set_openai_flex(project.openai_flex);
        let permission = PermissionBroker::default();
        let workers = WorkerRegistry::new(Vec::new())?;
        let store = SessionStore::new(&config.directory);
        store.prepare()?;
        let runtime_factory = AgentRuntimeFactory::new(
            bridge.clone(),
            workspace_root.clone(),
            workers,
            permission.clone(),
            project_settings.clone(),
            project_identity.clone(),
            store.clone(),
        );
        let run_manager = RunManager::default();
        let swarm = config.swarm_settings().unwrap_or_default();
        let agent_groups =
            AgentGroupSupervisor::new(runtime_factory.clone(), AgentLimits::from(&swarm));
        let coordinator = Arc::new(Mutex::new(runtime_factory.build(None)?));
        let mut session = SharedSession::new(model, reasoning_level);
        if swarm.auto_deploy {
            session.state.agent_mode = AgentMode::Adaptive;
            coordinator
                .lock_or_recover()
                .set_agent_group(Some(agent_groups.handle()));
        }
        session.meta.working_directory = Some(workspace_root.display().to_string());
        session.meta.context_roots = vec![workspace_root.display().to_string()];
        if let Ok(catalog) = config.model_catalog_cache() {
            session.state.available_models = catalog.iter().map(|item| item.id.clone()).collect();
            session.state.model_catalog = catalog;
        }
        if let Ok((workspaces, session_groups)) = store.list_workspace_catalog(&workspace_root) {
            let current_workspace_id = workspaces
                .iter()
                .find(|workspace| workspace.is_current)
                .map(|workspace| workspace.id.as_str());
            session.state.saved_sessions = current_workspace_id
                .and_then(|id| {
                    session_groups
                        .iter()
                        .find(|group| group.workspace_id == id)
                        .map(|group| group.sessions.clone())
                })
                .unwrap_or_default();
            session.state.known_workspaces = workspaces;
            session.state.workspace_session_groups = session_groups;
        } else {
            // Keep startup resilient if the cross-workspace catalog cannot be built.
            // This fallback repeats the scan only on the exceptional path.
            session.state.saved_sessions = store.list(&workspace_root).unwrap_or_default();
        }
        session.state.sandbox_settings = SandboxStore::new(&workspace_root)
            .and_then(|store| store.load())
            .ok()
            .map(|policy| sandbox_settings_state(&policy));
        session.state.openai_flex = project.openai_flex;
        session.state.foundation_memory_enabled = project.foundation_memory.enabled;
        session.state.foundation_memory_backend = project.foundation_memory.backend.as_str().into();
        session.state.foundation_memory_server = project.foundation_memory.server.clone();
        session.state.web_backend = project.web.backend.as_str().into();
        session.state.web_server = project.web.server.clone();
        session.meta.attached_capabilities = project.capabilities.attached.map(|values| {
            values
                .into_iter()
                .filter(|value| !session_only_capability(value))
                .collect()
        });
        session.meta.disabled_capabilities = project
            .capabilities
            .disabled
            .into_iter()
            .filter(|value| !session_only_capability(value))
            .collect();
        let shared = Arc::new(Mutex::new(session));
        let (tx, events) = mpsc::channel();
        let tx = EventSender::new(tx, wake);
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
            run_manager,
            agent_groups,
            goal_mode: Arc::new(AtomicBool::new(false)),
            events,
            tx,
            closed: false,
        };
        backend.install_agent_group_listener();
        backend.refresh_context_length();
        backend.publish_state();
        Ok(backend)
    }

    pub(crate) fn send(&mut self, command: FrontendCommand) -> Result<()> {
        match command {
            FrontendCommand::Submit {
                text,
                images,
                attachment_ids,
            } => {
                if !attachment_ids.is_empty() {
                    return Err(anyhow!(
                        "Remote attachment IDs must be resolved before backend submission"
                    ));
                }
                self.submit(text, images)
            }
            FrontendCommand::StartDebate { topic, models } => self.start_debate(topic, models),
            FrontendCommand::Interrupt => {
                self.set_goal_enabled(false)?;
                self.interrupt();
                Ok(())
            }
            FrontendCommand::RegenerateLast => self.regenerate_last(),
            FrontendCommand::EditLast { text } => self.edit_last(text),
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
            FrontendCommand::SetGoal { enabled } => self.set_goal_enabled(enabled),
            FrontendCommand::SetAgentMode { mode } => self.set_agent_mode(mode),
            FrontendCommand::SetAutonomyMode { mode } => self.set_autonomy_mode(mode),
            FrontendCommand::MessageAgent { agent_id, message } => {
                self.agent_groups.message(&agent_id, message)
            }
            FrontendCommand::StopAgent { agent_id } => self.agent_groups.stop(agent_id.as_deref()),
            FrontendCommand::RemoveAgent { agent_id } => {
                self.agent_groups.remove(agent_id.as_deref())
            }
            FrontendCommand::SpawnAgent {
                role,
                description,
                prompt,
            } => {
                let (model, session) = {
                    let shared = self.shared.lock_or_recover();
                    (
                        shared.state.active_model.clone(),
                        shared.state.current_session_id.clone(),
                    )
                };
                self.agent_groups
                    .spawn_for_user(&role, description, prompt, &model, session)
            }
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
            FrontendCommand::ToggleCapability { id } => self.toggle_capability(&id).map(|_| ()),
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
            FrontendCommand::SetAppearance { appearance } => {
                self.set_appearance(appearance);
                Ok(())
            }
            FrontendCommand::SetTheme { mode, value } => {
                self.set_theme(mode, value);
                Ok(())
            }
            FrontendCommand::SetContextLength { length } => {
                self.set_context_length(length);
                Ok(())
            }
            FrontendCommand::SetJevLoopMode { mode } => {
                self.set_jev_loop_mode(mode);
                Ok(())
            }
            FrontendCommand::SetSwarmSettings { settings } => {
                self.set_swarm_settings(settings);
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
            FrontendCommand::SetServiceBackend {
                service,
                backend,
                server,
            } => {
                self.set_service_backend(service, backend, server);
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
            FrontendCommand::ExtensionCommand { .. } => Ok(()),
            FrontendCommand::Shutdown => {
                self.shutdown();
                Ok(())
            }
        }
    }

    pub(crate) fn try_recv(&mut self) -> Option<BackendEvent> {
        self.deliver_agent_notifications();
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

        // Native Computer Use elicitations must respect the same workspace
        // approval policy as shell/file operations. In auto-approve or unlimited
        // mode, surfacing every low-level click/type/scroll as a modal defeats
        // the policy and makes Computer Use effectively unusable.
        let auto_approve = SandboxStore::new(&self.workspace_root)
            .and_then(|store| store.load())
            .map(|policy| native_app_approval_is_automatic(&policy))
            .unwrap_or(false);
        if auto_approve {
            let _ = self
                .bridge
                .send_native_app_approval_decision(&request.request_id, true);
            return;
        }

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

    pub(super) fn set_goal_enabled(&mut self, enabled: bool) -> Result<()> {
        self.set_autonomy_mode(if enabled {
            AutonomyMode::Goal
        } else {
            AutonomyMode::Manual
        })
    }

    fn set_autonomy_mode(&mut self, mode: AutonomyMode) -> Result<()> {
        let is_streaming = self.shared.lock_or_recover().state.is_streaming;
        if is_streaming && mode != AutonomyMode::Manual {
            return Err(anyhow!(
                "Autonomy mode cannot be enabled while a response is running."
            ));
        }

        let goal_enabled = mode != AutonomyMode::Manual;
        self.goal_mode.store(goal_enabled, Ordering::Release);
        let (session_id, resume) = {
            let mut shared = self.shared.lock_or_recover();
            shared.state.goal_mode = goal_enabled;
            shared.state.autonomy_mode = mode;
            let resume = goal_enabled
                && !shared.state.is_streaming
                && shared
                    .state
                    .conversation
                    .iter()
                    .flatten()
                    .any(|entry| matches!(entry.kind, ConversationKind::User { .. }));
            (shared.state.current_session_id.clone(), resume)
        };

        if let Some(session_id) = session_id.as_deref() {
            self.store.set_goal_mode(session_id, goal_enabled)?;
            if !is_streaming {
                let history = self.coordinator.lock_or_recover().model_history();
                let mut shared = self.shared.lock_or_recover();
                persist_locked(&mut shared, &self.store, &self.workspace_root, history)?;
            }
        }

        self.publish_state();
        if resume {
            match mode {
                AutonomyMode::Manual => {}
                AutonomyMode::Goal => {
                    self.submit_agent(GOAL_RESUME_PROMPT.to_owned(), false, "goal-resume", true)?
                }
                AutonomyMode::Autonomous => self.submit_agent(
                    AUTONOMOUS_NEXT_OBJECTIVE_PROMPT.to_owned(),
                    false,
                    "autonomous-resume",
                    false,
                )?,
            }
        }
        Ok(())
    }

    pub(super) fn set_agent_mode(&mut self, mode: AgentMode) -> Result<()> {
        if self.shared.lock_or_recover().state.is_streaming {
            return Err(anyhow!(
                "Agent mode cannot be changed while a response is running."
            ));
        }
        self.agent_groups.replace_active_group();
        {
            let mut coordinator = self.coordinator.lock_or_recover();
            coordinator.set_agent_group(match mode {
                AgentMode::Single => None,
                AgentMode::Adaptive => Some(self.agent_groups.handle()),
            });
        }
        let history = self
            .coordinator
            .lock()
            .map_err(|_| anyhow!("coordinator lock poisoned"))?
            .model_history();
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.agent_mode = mode;
            shared.state.agent_tasks = self.agent_groups.task_items();
            shared.state.agent_group = self.agent_groups.group_item();
            if shared.state.current_session_id.is_some() {
                persist_locked(&mut shared, &self.store, &self.workspace_root, history)?;
            }
        }
        self.publish_state();
        Ok(())
    }

    fn regenerate_last(&mut self) -> Result<()> {
        self.replay_last_visible_turn(None)
    }

    fn edit_last(&mut self, text: String) -> Result<()> {
        self.replay_last_visible_turn(Some(text.trim().to_owned()))
    }

    fn replay_last_visible_turn(&mut self, replacement_text: Option<String>) -> Result<()> {
        if self.shared.lock_or_recover().state.is_streaming {
            return Err(anyhow!("cannot replay while a response is streaming"));
        }

        let (history_start, conversation_start, run_index, expected_visible_content) = {
            let shared = self.shared.lock_or_recover();
            let Some((index, run)) = shared.meta.runs.iter().enumerate().rev().find(|(_, run)| {
                run.kind == "agent"
                    && run.user_entry_id.is_some()
                    && run.history_start.is_some()
                    && run.conversation_start.is_some()
            }) else {
                return Err(anyhow!("the latest visible turn has no replay checkpoint"));
            };
            if shared.meta.runs[index + 1..]
                .iter()
                .any(|later| later.kind != "goal-resume")
            {
                return Err(anyhow!(
                    "the latest visible turn is followed by another top-level run and cannot be replayed"
                ));
            }
            let conversation_start = run.conversation_start.expect("checked");
            let conversation = shared
                .state
                .conversation
                .as_ref()
                .ok_or_else(|| anyhow!("replay checkpoint has no transcript"))?;
            let entry = conversation
                .get(conversation_start)
                .ok_or_else(|| anyhow!("replay checkpoint no longer matches transcript"))?;
            if entry.id != run.user_entry_id.as_deref().expect("checked") {
                return Err(anyhow!(
                    "replay checkpoint user identity no longer matches transcript"
                ));
            }
            let ConversationKind::User { content } = &entry.kind else {
                return Err(anyhow!(
                    "replay checkpoint does not point to a visible user entry"
                ));
            };
            (
                run.history_start.expect("checked"),
                conversation_start,
                index,
                content.clone(),
            )
        };

        let original = {
            let mut coordinator = self.coordinator.lock_or_recover();
            let history = coordinator.model_history();
            let Some(message) = history.get(history_start).cloned() else {
                return Err(anyhow!("replay checkpoint no longer matches model history"));
            };
            if message.role != crate::core::MessageRole::User || message.request_only == Some(true)
            {
                return Err(anyhow!(
                    "replay checkpoint does not point to a user request"
                ));
            }
            let images = message.images.as_deref().unwrap_or_default();
            if visible_user_content(message.content.as_deref().unwrap_or_default(), images)
                != expected_visible_content
            {
                return Err(anyhow!(
                    "replay checkpoint content no longer matches the visible user turn"
                ));
            }
            coordinator.rewind_model_history(history_start)?;
            message
        };

        let images = original.images.unwrap_or_default();
        let original_content = original.content.unwrap_or_default();
        let (_, remote_file_payload) = split_remote_file_context(&original_content);
        let remote_file_payload = remote_file_payload.map(str::to_owned);
        let mut text = replacement_text.unwrap_or(original_content);
        if let Some(payload) = remote_file_payload
            && split_remote_file_context(&text).1.is_none()
        {
            text.push_str(REMOTE_FILE_CONTEXT_OPEN);
            text.push_str(&payload);
            text.push_str(REMOTE_FILE_CONTEXT_CLOSE);
        }
        if text.is_empty() && images.is_empty() {
            return Err(anyhow!("edited message cannot be empty"));
        }

        {
            let mut shared = self.shared.lock_or_recover();
            if conversation_start > shared.conversation_mut().len() {
                return Err(anyhow!("replay checkpoint no longer matches transcript"));
            }
            shared.conversation_mut().truncate(conversation_start);
            shared.meta.runs.truncate(run_index);
            shared.state.conversation_revision = shared.state.conversation_revision.wrapping_add(1);
            shared.state.active_assistant_entry_id = None;
            shared.state.active_assistant_text.clear();
            shared.state.active_reasoning_entry_id = None;
            shared.state.active_reasoning_text.clear();
            shared.state.active_reasoning_summary.clear();
            shared.state.active_activity_entry_id = None;
            shared.state.error_message = None;
        }
        self.publish_state();

        self.submit_agent_with_images(text, images, true, "agent", false)
    }

    fn submit(&mut self, text: String, images: Vec<ImageAttachment>) -> Result<()> {
        self.submit_agent_with_images(text, images, true, "agent", false)
    }

    pub(super) fn submit_agent(
        &mut self,
        text: String,
        visible_user: bool,
        run_kind: &str,
        continuation: bool,
    ) -> Result<()> {
        self.submit_agent_with_images(text, Vec::new(), visible_user, run_kind, continuation)
    }
}

impl Drop for BackendService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn forward_cli(arguments: &[String]) -> Result<i32> {
    crate::cli::run(arguments)
}
