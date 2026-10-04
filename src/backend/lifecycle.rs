//! Session replacement, interruption, compaction, and backend shutdown.
//!
//! Lifecycle transitions are isolated from command routing so cancellation and
//! persistence invariants stay easy to audit.

use super::*;

impl BackendService {
    /// Restores a persisted session and rebinds coordinator runtime state.
    pub(super) fn load_session(&mut self, id: &str) -> Result<()> {
        self.interrupt();
        let stored = self.store.load(id)?;
        let group_checkpoint = self
            .store
            .load_group_checkpoint::<crate::agents::group::AgentGroupCheckpoint>(id)?;
        self.agent_groups.replace_active_group();
        let rebase = |path: &String| {
            crate::session_store::resolve_workspace_path(
                &self.workspace_root,
                &stored.workspace_root,
                path,
            )
        };
        let stored_working_directory = stored.working_directory.as_ref().map(rebase);
        let stored_context_roots = stored.context_roots.iter().map(rebase).collect::<Vec<_>>();
        let persisted_goal = self.store.goal_mode(id).unwrap_or(false);
        let restored_agent_mode = stored.agent_mode;
        let restored_autonomy = if stored.autonomy_mode == AutonomyMode::Manual && persisted_goal {
            // Sessions written before autonomy policy existed only persisted the
            // Goal sidecar. Preserve their old resume semantics.
            AutonomyMode::Goal
        } else {
            stored.autonomy_mode
        };
        let resume_goal = persisted_goal
            && stored
                .runs
                .last()
                .is_none_or(|run| run.status == RunStatus::Running)
            && stored
                .conversation
                .iter()
                .any(|entry| matches!(entry.kind, ConversationKind::User { .. }));
        let stored_history = stored.model_history.clone();
        let sandbox_settings = SandboxStore::new(&self.workspace_root)
            .and_then(|store| store.load())
            .ok()
            .map(|policy| sandbox_settings_state(&policy));
        self.invalidate_active_turn_for_replacement()?;
        self.coordinator
            .lock()
            .map_err(|_| anyhow!("coordinator lock poisoned"))?
            .replace_model_history(stored_history.clone());
        let mut shared = self.shared.lock_or_recover();
        shared.meta.current_turn = None;
        shared.state.is_streaming = false;
        shared.state.active_run_id = None;
        shared.state.active_assistant_entry_id = None;
        shared.state.active_assistant_text.clear();
        shared.state.active_activity_entry_id = None;
        shared.state.active_reasoning_entry_id = None;
        shared.state.active_reasoning_text.clear();
        shared.state.active_reasoning_summary.clear();
        shared.meta.pending_tool_calls.clear();
        shared.state.debate = stored.debate.map(|mut debate| {
            if debate.verdict.is_none() && debate.status != "Failed" {
                debate.status = "Interrupted · Restored saved record".into();
            }
            debate
        });
        shared.state.conversation = Some(stored.conversation);
        shared.state.conversation_revision = shared.state.conversation_revision.wrapping_add(1);
        shared.state.active_model = stored.model;
        shared.state.token_usage = stored.token_usage;
        shared.state.goal_mode = persisted_goal;
        shared.state.agent_mode = restored_agent_mode;
        shared.state.autonomy_mode = restored_autonomy;
        shared.state.current_context_tokens = None;
        shared.state.sandbox_settings = sandbox_settings;
        shared.state.credit_usage = stored.credit_usage;
        shared.state.current_session_id = Some(stored.id.clone());
        shared.meta.created_at = Some(stored.created_at);
        shared.meta.title = Some(stored.title);
        shared.meta.title_generation_attempted = true;
        shared.meta.runs = stored.runs;
        shared.meta.retained_debate_knowledge = stored.retained_debate_knowledge;
        shared.meta.attached_capabilities = stored.attached_harness_capabilities;
        shared.meta.disabled_capabilities = stored.disabled_capabilities;
        shared.meta.working_directory = stored_working_directory.clone();
        shared.meta.context_roots = stored_context_roots.clone();
        shared.state.error_message = None;
        let reconciled = shared.reconcile_orphaned_runs(
            "Persisted run had no live runtime when this session was restored.",
        );
        if reconciled {
            persist_locked(
                &mut shared,
                &self.store,
                &self.workspace_root,
                stored_history.clone(),
            )?;
        }
        let protected = self.store.directory.join(&stored.id);
        let retained_knowledge = shared.meta.retained_debate_knowledge.clone();
        drop(shared);
        if let Some(checkpoint) = group_checkpoint {
            self.agent_groups.restore_checkpoint(checkpoint)?;
        }
        self.goal_mode.store(persisted_goal, Ordering::Release);
        let (working_directory, context_roots) = {
            let mut coordinator = self
                .coordinator
                .lock()
                .map_err(|_| anyhow!("coordinator lock poisoned"))?;
            coordinator.set_protected_write_paths([protected]);
            coordinator.set_session_runtime(self.store.clone(), Some(stored.id.clone()));
            coordinator.set_retained_debate_knowledge(retained_knowledge);
            coordinator.set_agent_group(match restored_agent_mode {
                AgentMode::Single => None,
                AgentMode::Adaptive => Some(self.agent_groups.handle()),
            });
            coordinator.restore_session_environment(
                stored_working_directory.as_deref(),
                &stored_context_roots,
            )?;
            coordinator.session_environment()
        };
        {
            let mut shared = self.shared.lock_or_recover();
            shared.meta.working_directory = Some(working_directory);
            shared.meta.context_roots = context_roots;
        }
        self.reload_project_capabilities()?;
        self.refresh_context_length();
        self.request_sessions();
        self.publish_state();
        if resume_goal {
            self.submit_agent(GOAL_RESUME_PROMPT.to_owned(), false, "goal-resume", true)?;
        }
        Ok(())
    }

    /// Replaces the active session with a fresh empty transcript.
    pub(super) fn new_session(&mut self) {
        let _ = self.set_goal_enabled(false);
        self.interrupt();
        self.agent_groups.replace_active_group();
        let _ = self.invalidate_active_turn_for_replacement();
        if let Ok(mut coordinator) = self.coordinator.lock() {
            coordinator.replace_model_history(Vec::new());
        }
        let previous = self.shared.lock_or_recover().state.without_conversation();
        let project = self.project_settings.load().unwrap_or_default();
        let mut session = SharedSession::new(
            previous.active_model.clone(),
            previous.active_reasoning_level.clone(),
        );
        // Workspace-wide state is not part of a session. Resetting it would
        // blank the client's session list and model picker until they are
        // re-requested, and publish an empty catalog to other runtimes.
        session.state.saved_sessions = previous.saved_sessions;
        session.state.known_workspaces = previous.known_workspaces;
        session.state.workspace_session_groups = previous.workspace_session_groups;
        session.state.available_models = previous.available_models;
        session.state.model_catalog = previous.model_catalog;
        session.state.active_model_context_length = previous.active_model_context_length;
        session.state.openai_flex = previous.openai_flex;
        session.state.foundation_memory_enabled = previous.foundation_memory_enabled;
        session.state.foundation_memory_backend = previous.foundation_memory_backend;
        session.state.foundation_memory_server = previous.foundation_memory_server;
        session.state.web_backend = previous.web_backend;
        session.state.web_server = previous.web_server;
        session.meta.working_directory = Some(self.workspace_root.display().to_string());
        session.meta.context_roots = vec![self.workspace_root.display().to_string()];
        session.state.sandbox_settings = SandboxStore::new(&self.workspace_root)
            .and_then(|store| store.load())
            .ok()
            .map(|policy| sandbox_settings_state(&policy));
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
        *self.shared.lock_or_recover() = session;
        if let Ok(mut coordinator) = self.coordinator.lock() {
            coordinator.set_protected_write_paths(Vec::<PathBuf>::new());
            coordinator.set_session_runtime(self.store.clone(), None);
            coordinator.set_retained_debate_knowledge(Vec::new());
            coordinator.set_agent_group(None);
            let _ = coordinator.restore_session_environment(None, &[]);
        }
        let auto_deploy = self
            .config
            .agent_group_settings()
            .is_ok_and(|agent_group| agent_group.auto_deploy);
        if !(auto_deploy && self.set_agent_mode(AgentMode::Adaptive).is_ok()) {
            self.publish_state();
        }
    }

    /// Compacts coordinator history and records the transition in the transcript.
    pub(super) fn compact_context(&self) -> Result<()> {
        let (before, after) = self
            .coordinator
            .lock()
            .map_err(|_| anyhow!("coordinator lock poisoned"))?
            .compact_model_history()?;
        {
            let mut shared = self.shared.lock_or_recover();
            shared.meta.pending_compaction = false;
            shared.state.current_context_tokens = None;
            shared.append(ConversationKind::System {
                content: format!("New context window: {before} working messages to {after}. Original history is retrievable."),
            });
            let history = self
                .coordinator
                .lock()
                .map_err(|_| anyhow!("coordinator lock poisoned"))?
                .model_history();
            let _ = persist_locked(&mut shared, &self.store, &self.workspace_root, history);
        }
        self.publish_state();
        Ok(())
    }

    /// Invalidates a running turn without blocking on its coordinator mutex.
    fn invalidate_active_turn_for_replacement(&self) -> Result<()> {
        let replaced_session_id = {
            let mut shared = self.shared.lock_or_recover();
            if shared.meta.current_turn.is_none() {
                None
            } else {
                shared.set_activity("interrupted", "Interrupted · Session replaced", None);
                shared.settle_pending_tool_calls(ToolCallStatus::Failed);
                shared.finish_run(
                    RunStatus::Interrupted,
                    Some("Session was replaced while this run was active.".into()),
                );
                shared.meta.current_turn = None;
                shared.state.is_streaming = false;
                shared.state.pending_shell_permission = None;
                shared.state.pending_native_app_permission = None;
                shared.state.current_session_id.clone()
            }
        };
        self.run_manager.detach_all();
        let Some(session_id) = replaced_session_id else {
            return Ok(());
        };

        let history = if let Ok(coordinator) = self.coordinator.try_lock() {
            coordinator.model_history()
        } else {
            self.store
                .load(&session_id)
                .map(|session| session.model_history)
                .unwrap_or_default()
        };
        let mut shared = self.shared.lock_or_recover();
        persist_locked(&mut shared, &self.store, &self.workspace_root, history)?;
        Ok(())
    }

    /// Requests cancellation for the active run and pending permission prompt.
    pub(super) fn interrupt(&self) {
        self.run_manager.cancel_all();
        if self.agent_groups.group_item().status == "running" {
            self.agent_groups.cancel_group();
        }
        self.bridge.interrupt_active_requests();
        if self.permission.pending_shell().is_some()
            || self.permission.pending_native_app().is_some()
        {
            self.permission.resolve(false);
        }
    }

    /// Marks an unresponsive interrupted run terminal without waiting on the
    /// coordinator mutex. The background daemon uses this before discarding a
    /// runtime whose worker did not settle after cancellation.
    pub(crate) fn abandon_stuck_run(&self, reason: &str) -> Result<()> {
        self.interrupt();
        let session_id = {
            let mut shared = self.shared.lock_or_recover();
            if shared.meta.current_turn.is_none() {
                shared.state.is_streaming = false;
                None
            } else {
                shared.set_activity(
                    "interrupted",
                    "Interrupted · Runtime recovered",
                    Some(reason.to_owned()),
                );
                shared.seal_assistant();
                shared.settle_pending_tool_calls(ToolCallStatus::Failed);
                shared.finish_run(RunStatus::Interrupted, Some(reason.to_owned()));
                shared.meta.current_turn = None;
                shared.state.is_streaming = false;
                shared.state.active_assistant_entry_id = None;
                shared.state.active_assistant_text.clear();
                shared.state.active_reasoning_entry_id = None;
                shared.state.active_reasoning_text.clear();
                shared.state.active_reasoning_summary.clear();
                shared.state.pending_shell_permission = None;
                shared.state.pending_native_app_permission = None;
                shared.append(ConversationKind::System {
                    content: reason.to_owned(),
                });
                shared.state.current_session_id.clone()
            }
        };
        self.run_manager.detach_all();

        if let Some(session_id) = session_id {
            let history = if let Ok(coordinator) = self.coordinator.try_lock() {
                coordinator.model_history()
            } else {
                self.store
                    .load(&session_id)
                    .map(|session| session.model_history)
                    .unwrap_or_default()
            };
            let mut shared = self.shared.lock_or_recover();
            persist_locked(&mut shared, &self.store, &self.workspace_root, history)?;
        }
        self.publish_state();
        Ok(())
    }

    /// Appends a system notice to the active transcript.
    pub(super) fn append_system(&self, text: &str) {
        self.shared
            .lock_or_recover()
            .append(ConversationKind::System {
                content: text.into(),
            });
        self.publish_state();
    }

    /// Publishes a backend error without mutating model history.
    pub(super) fn append_error(&self, text: String) {
        self.shared.lock_or_recover().state.error_message = Some(text);
        self.publish_state();
    }

    /// Emits the latest bridge state with current permission prompts attached.
    pub(super) fn publish_state(&self) {
        let mut state = self.shared.lock_or_recover();
        state.state.agent_tasks = self.agent_groups.task_items();
        state.state.agent_group = self.agent_groups.group_item();
        state.state.pending_shell_permission = self.permission.pending_shell();
        state.state.pending_native_app_permission = self.permission.pending_native_app();
        let _ = self
            .tx
            .send(BackendEvent::Envelope(state_envelope(&state.state)));
    }

    /// Stops active work and tears down the runtime bridge without deadlocking.
    pub(super) fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.interrupt();
        // interrupt() spares background agents; shutdown must not.
        self.agent_groups.replace_active_group();
        self.permission.close();
        self.bridge.shutdown();

        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(500) {
            if let Ok(coordinator) = self.coordinator.try_lock() {
                coordinator.shutdown();
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
