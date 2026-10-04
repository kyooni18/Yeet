//! Top-level run lifecycle: admission, execution, settlement and release.
//!
//! A `BackendService` executes at most one top-level run at a time. This is a
//! structural property rather than a scheduling choice: the service owns one
//! `AgentCoordinator` (held for the whole run) and the live `SharedSession`
//! carries singular run state — `current_turn`, `active_run_id`, one streaming
//! assistant/reasoning buffer and one pending tool-call projection. Admission
//! is therefore gated on `is_streaming`, and every projection from a worker is
//! fenced by `current_turn == run.id` so a replaced or abandoned run can never
//! write into its successor's state.
//!
//! `RunManager` is keyed by run ID because a run that has already *settled*
//! (released `is_streaming`) may still be registered while it finishes post-run
//! persistence and title generation. That overlap is the only time two run IDs
//! coexist; it is not concurrent top-level execution.
//!
//! The lifecycle is independent of storage: settlement only mutates live state
//! and yields a prepared snapshot, and `SessionWriter` commits it afterwards
//! without holding the live-state mutex.

use super::*;

/// Identity and request of one admitted top-level run.
pub(super) struct TopLevelRun {
    pub(super) id: String,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) input: String,
    pub(super) images: Vec<ImageAttachment>,
    pub(super) continuation: bool,
    pub(super) model: String,
    pub(super) reasoning_level: String,
}

/// Commits prepared session snapshots and refreshes the workspace catalog.
///
/// Session-store writes take cross-process file locks and may block on
/// filesystem I/O, so callers prepare a snapshot under the live-state mutex and
/// hand it here only after releasing that mutex: Remote reconnects and the
/// background daemon need the mutex to observe semantic state.
#[derive(Clone)]
pub(super) struct SessionWriter {
    pub(super) store: SessionStore,
    pub(super) workspace: PathBuf,
    pub(super) shared: Arc<Mutex<SharedSession>>,
    pub(super) tx: EventSender,
}

impl SessionWriter {
    /// Commits a prepared snapshot; a failure is surfaced as a session error
    /// instead of failing the run that produced it.
    pub(super) fn commit_or_report(&self, prepared: PreparedSessionWrite) {
        if let Err(error) = commit_session_write(&self.store, prepared) {
            let mut state = self.shared.lock_or_recover();
            state.state.error_message = Some(format!("Save failed: {error}"));
            self.publish_compact(&state);
        }
    }

    /// Re-reads the workspace session catalog and publishes it.
    pub(super) fn refresh_catalog(&self) {
        if let Ok(catalog) = SessionCatalog::read_from_store(&self.store, &self.workspace) {
            let mut state = self.shared.lock_or_recover();
            apply_session_catalog_locked(&mut state, &catalog);
            self.publish_compact(&state);
        }
    }

    fn publish_compact(&self, state: &SharedSession) {
        let _ = self
            .tx
            .send(BackendEvent::Envelope(state_envelope_without_conversation(
                &state.state,
            )));
    }
}

/// Maps a finished coordinator result onto live session state.
///
/// This is the single terminal transition for a top-level run: it seals the
/// streaming buffers, settles pending tool-call projections according to the
/// terminal status, releases `is_streaming` and clears `current_turn`. It does
/// not persist anything.
pub(super) fn settle_run_state(
    state: &mut SharedSession,
    result: Result<AgentRunOutcome>,
    cancelled: bool,
    goal_mode: bool,
) -> RunStatus {
    state.state.goal_mode = goal_mode;
    let (run_status, run_error) = match result {
        Err(error) if cancelled => {
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
        Ok(AgentRunOutcome::GoalPaused { reason }) => {
            state.set_activity("paused", "Goal · Paused", Some(reason.clone()));
            (RunStatus::Paused, Some(reason))
        }
        Ok(AgentRunOutcome::AutonomousIdle { reason }) => {
            state.set_activity("paused", "Autonomous · Idle", Some(reason.clone()));
            (RunStatus::Paused, Some(reason))
        }
        Ok(AgentRunOutcome::CompletedUnverified { reason }) => {
            state.set_activity("done", "Done · Unverified", Some(reason.clone()));
            (RunStatus::CompletedUnverified, Some(reason))
        }
    };
    if run_status == RunStatus::Completed && state.state.autonomy_mode == AutonomyMode::Goal {
        state.state.autonomy_mode = AutonomyMode::Manual;
        state.state.goal_mode = false;
    }
    state.seal_assistant();
    state.state.active_assistant_entry_id = None;
    state.state.active_assistant_text.clear();
    state.state.active_reasoning_entry_id = None;
    state.state.active_reasoning_text.clear();
    state.state.active_reasoning_summary.clear();
    let pending_tool_status = match run_status {
        RunStatus::Completed | RunStatus::CompletedUnverified | RunStatus::Paused => {
            ToolCallStatus::Suppressed
        }
        RunStatus::Interrupted => ToolCallStatus::Interrupted,
        RunStatus::Running | RunStatus::Failed => ToolCallStatus::Failed,
    };
    state.settle_pending_tool_calls(pending_tool_status);
    state.state.is_streaming = false;
    clear_resolved_streaming_lock_error(&mut state.state.error_message);
    state.finish_run(run_status, run_error);
    state.meta.current_turn = None;
    run_status
}

/// Worker-side dependencies of one top-level run.
struct RunWorker {
    shared: Arc<Mutex<SharedSession>>,
    coordinator: Arc<Mutex<AgentCoordinator>>,
    tx: EventSender,
    writer: SessionWriter,
    run_manager: RunManager,
    goal_mode: Arc<AtomicBool>,
    permission: PermissionBroker,
    bridge: BridgeHandle,
    agent_groups: AgentGroupSupervisor,
}

impl BackendService {
    pub(super) fn session_writer(&self) -> SessionWriter {
        SessionWriter {
            store: self.store.clone(),
            workspace: self.workspace_root.clone(),
            shared: self.shared.clone(),
            tx: self.tx.clone(),
        }
    }

    /// Admits a top-level run and starts its worker thread.
    ///
    /// Returns `Ok(())` without starting anything when another top-level run
    /// is still streaming; see the module docs for the single-run invariant.
    pub(super) fn submit_agent_with_images(
        &mut self,
        text: String,
        mut images: Vec<ImageAttachment>,
        visible_user: bool,
        run_kind: &str,
        continuation: bool,
    ) -> Result<()> {
        let input = text.trim().to_owned();
        if visible_user && input.starts_with('/') {
            if !images.is_empty() {
                return Err(anyhow!("Images cannot be attached to slash commands"));
            }
            return self.run_command(&input);
        }
        self.reload_project_capabilities()?;
        let turn_id = Uuid::new_v4().to_string();
        let images = {
            let mut shared = self.shared.lock_or_recover();
            if shared.state.is_streaming {
                return Ok(());
            }
            if !visible_user {
                Vec::new()
            } else {
                images.extend(std::mem::take(&mut shared.meta.pending_images));
                images
            }
        };
        if input.is_empty() && images.is_empty() {
            return Ok(());
        }
        let autonomy_mode = self.shared.lock_or_recover().state.autonomy_mode;
        let goal_enabled = autonomy_mode != AutonomyMode::Manual;
        self.goal_mode.store(goal_enabled, Ordering::Release);
        self.agent_groups.begin_turn();
        let history_start = self
            .coordinator
            .lock_or_recover()
            .prepare_turn_history_checkpoint();
        {
            let mut shared = self.shared.lock_or_recover();
            shared.state.goal_mode = goal_enabled;
            shared.state.agent_tasks = self.agent_groups.task_items();
            shared.state.agent_group = self.agent_groups.group_item();
            shared.state.error_message = None;
            shared.state.is_streaming = true;
            shared.state.active_assistant_entry_id = None;
            shared.state.active_assistant_text.clear();
            shared.state.active_activity_entry_id = None;
            shared.meta.pending_tool_calls.clear();
            let run_model = shared.state.active_model.clone();
            let conversation_start = shared.state.conversation.as_ref().map_or(0, Vec::len);
            shared.start_run(
                turn_id.clone(),
                run_kind,
                run_model,
                history_start,
                conversation_start,
            );
            if visible_user {
                let visible_input = visible_user_content(&input, &images);
                let user_entry_id = shared.append(ConversationKind::User {
                    content: visible_input,
                });
                shared.bind_run_user_entry(&turn_id, user_entry_id);
            }
            shared.set_activity(
                "thinking",
                if continuation {
                    "Goal · resuming"
                } else {
                    "Thinking"
                },
                continuation.then(|| "Resuming persisted Goal execution".to_owned()),
            );
        }
        self.publish_state();

        let (model, reasoning_level) = {
            let shared = self.shared.lock_or_recover();
            (
                shared.state.active_model.trim().to_owned(),
                shared.state.active_reasoning_level.clone(),
            )
        };
        if model.is_empty() {
            let mut shared = self.shared.lock_or_recover();
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
        self.run_manager.register(turn_id.clone(), cancel.clone());
        // The run record must be durable before any provider call so crash
        // recovery can find and reconcile it.
        if let Err(error) = self.persist_run_start() {
            let mut state = self.shared.lock_or_recover();
            state.state.is_streaming = false;
            state.finish_run(RunStatus::Failed, Some(format!("Save failed: {error}")));
            state.meta.current_turn = None;
            state.state.error_message = Some(format!("Save failed: {error}"));
            drop(state);
            self.run_manager.remove_matching(&turn_id, &cancel);
            self.publish_state();
            return Ok(());
        }
        if let Some(id) = self
            .shared
            .lock_or_recover()
            .state
            .current_session_id
            .clone()
            && let Ok(mut coordinator) = self.coordinator.lock()
        {
            coordinator.set_protected_write_paths([self.store.directory.join(&id)]);
            coordinator.set_session_runtime(self.store.clone(), Some(id));
        }

        let worker = RunWorker {
            shared: self.shared.clone(),
            coordinator: self.coordinator.clone(),
            tx: self.tx.clone(),
            writer: self.session_writer(),
            run_manager: self.run_manager.clone(),
            goal_mode: self.goal_mode.clone(),
            permission: self.permission.clone(),
            bridge: self.bridge.clone(),
            agent_groups: self.agent_groups.clone(),
        };
        let run = TopLevelRun {
            id: turn_id,
            cancel,
            input,
            images,
            continuation,
            model,
            reasoning_level,
        };
        thread::spawn(move || worker.execute(run));
        Ok(())
    }

    fn persist_run_start(&self) -> Result<()> {
        let history = self.coordinator.lock_or_recover().model_history();
        let workspace_id = crate::session_store::ensure_workspace_id(&self.workspace_root)?;
        let prepared = {
            let mut state = self.shared.lock_or_recover();
            prepare_session_write_locked(&mut state, &self.workspace_root, workspace_id, history)
        };
        match prepared {
            Some(prepared) => commit_session_write(&self.store, prepared),
            None => Ok(()),
        }
    }
}

impl RunWorker {
    /// Runs the full worker lifecycle: drive, settle, persist, title, release.
    fn execute(self, mut run: TopLevelRun) {
        // A panic in provider/tool execution must not leave the shared session
        // marked streaming forever. Convert it into a normal failed run so the
        // completion path below always clears the activity and publishes state.
        let result = catch_unwind(AssertUnwindSafe(|| self.drive(&mut run)))
            .unwrap_or_else(|_| Err(anyhow!("agent run panicked")));
        let workspace_id = crate::session_store::ensure_workspace_id(&self.writer.workspace).ok();
        let (final_write, title_request) = self.settle(&run, result, workspace_id.clone());

        if let Some(prepared) = final_write {
            self.writer.commit_or_report(prepared);
        }
        self.writer.refresh_catalog();
        if let Some(request) = title_request {
            self.apply_generated_title(request, workspace_id);
        }
        self.run_manager.remove_matching(&run.id, &run.cancel);
    }

    /// Drives the coordinator, chaining autonomous cycles while the session
    /// stays in Autonomous mode and each cycle completes.
    fn drive(&self, run: &mut TopLevelRun) -> Result<AgentRunOutcome> {
        let (attached, disabled_capabilities) = {
            let value = self.shared.lock_or_recover();
            (
                value.meta.attached_capabilities.clone(),
                value.meta.disabled_capabilities.clone(),
            )
        };
        let mut coordinator = self.coordinator.lock_or_recover();
        let mut cycle_input = std::mem::take(&mut run.input);
        let mut cycle_images = std::mem::take(&mut run.images);
        let mut cycle_continuation = run.continuation;
        loop {
            let self_directed_cycle = cycle_input == AUTONOMOUS_NEXT_OBJECTIVE_PROMPT;
            let outcome = coordinator.run(
                AgentRunRequest {
                    input: &cycle_input,
                    images: std::mem::take(&mut cycle_images),
                    model: &run.model,
                    reasoning_level: &run.reasoning_level,
                    attached_capabilities: attached.clone(),
                    disabled_capabilities: disabled_capabilities.clone(),
                    cancel: run.cancel.clone(),
                    goal_mode: self.goal_mode.clone(),
                    continuation: cycle_continuation,
                },
                |event| self.project_event(&run.id, event),
            )?;

            let decision = {
                let state = self.shared.lock_or_recover();
                autonomy::next_cycle(
                    state.state.autonomy_mode,
                    &outcome,
                    run.cancel.load(Ordering::Acquire),
                    self_directed_cycle,
                    &state.state.active_assistant_text,
                )
            };
            match decision {
                autonomy::CycleDecision::Finish => return Ok(outcome),
                autonomy::CycleDecision::Idle => {
                    self.shared
                        .lock_or_recover()
                        .discard_assistant_text(AUTONOMOUS_IDLE_MARKER);
                    return Ok(AgentRunOutcome::AutonomousIdle {
                        reason: "No meaningful safe next objective is available.".into(),
                    });
                }
                autonomy::CycleDecision::Continue => {}
            }
            self.goal_mode.store(true, Ordering::Release);
            {
                let mut state = self.shared.lock_or_recover();
                state.state.goal_mode = true;
                state.set_activity(
                    "thinking",
                    "Autonomous - next objective",
                    Some("Selecting the next useful objective".into()),
                );
                let _ = self
                    .tx
                    .send(BackendEvent::Envelope(state_envelope_without_conversation(
                        &state.state,
                    )));
            }
            cycle_input = AUTONOMOUS_NEXT_OBJECTIVE_PROMPT.to_owned();
            cycle_continuation = false;
        }
    }

    /// Projects one coordinator event into live state, publishes it, and
    /// appends its durable evidence to the session event log.
    fn project_event(&self, run_id: &str, event: AgentEvent) {
        let log_event = agent_event_log_value(&event);
        let projected = {
            let mut state = self.shared.lock_or_recover();
            if state.meta.current_turn.as_deref() != Some(run_id) {
                return;
            }
            let omit_conversation = match &event {
                AgentEvent::ModelAttemptStarted { .. }
                | AgentEvent::ModelAttemptFinished(..)
                | AgentEvent::AuxiliaryUsage { .. }
                | AgentEvent::GoalCheckpoint { .. }
                | AgentEvent::GoalJudge { .. }
                | AgentEvent::GoalRetry { .. } => true,
                AgentEvent::TextDelta(_) => state.state.active_assistant_entry_id.is_some(),
                AgentEvent::ReasoningDelta(_) | AgentEvent::ReasoningSummaryDelta(_) => {
                    state.state.active_reasoning_entry_id.is_some()
                }
                _ => false,
            };
            let session_id = state.state.current_session_id.clone();
            apply_agent_event(&mut state, event);
            state.state.agent_tasks = self.agent_groups.task_items();
            state.state.agent_group = self.agent_groups.group_item();
            state.state.pending_shell_permission = self.permission.pending_shell();
            state.state.pending_native_app_permission = self.permission.pending_native_app();
            let envelope = if omit_conversation {
                state_envelope_without_conversation(&state.state)
            } else {
                state_envelope(&state.state)
            };
            (session_id, envelope)
        };

        let (session_id, envelope) = projected;
        let _ = self.tx.send(BackendEvent::Envelope(envelope));

        // Event-log writes happen outside the live-state mutex (see
        // `SessionWriter`).
        if let (Some(id), Some(log_event)) = (session_id, log_event)
            && let Err(error) = self
                .writer
                .store
                .append_event(&id, Some(run_id), &log_event)
        {
            let mut state = self.shared.lock_or_recover();
            if state.meta.current_turn.as_deref() == Some(run_id) {
                state.state.error_message = Some(format!("Event log write failed: {error}"));
                let _ = self
                    .tx
                    .send(BackendEvent::Envelope(state_envelope_without_conversation(
                        &state.state,
                    )));
            }
        }
    }

    /// Applies the terminal transition if this run still owns the session,
    /// runs deferred compaction, and prepares (but does not commit) the final
    /// snapshot.
    fn settle(
        &self,
        run: &TopLevelRun,
        result: Result<AgentRunOutcome>,
        workspace_id: Option<String>,
    ) -> (Option<PreparedSessionWrite>, Option<titles::TitleRequest>) {
        let mut state = self.shared.lock_or_recover();
        if state.meta.current_turn.as_deref() != Some(&run.id) {
            return (None, None);
        }
        settle_run_state(
            &mut state,
            result,
            run.cancel.load(Ordering::Acquire),
            self.goal_mode.load(Ordering::Acquire),
        );

        if state.meta.pending_compaction {
            let mut coordinator = self.coordinator.lock_or_recover();
            match coordinator.compact_model_history() {
                Ok((before, after)) => {
                    state.meta.pending_compaction = false;
                    state.append(ConversationKind::System {
                        content: format!(
                            "New context window: {before} working messages to {after}. Original history is retrievable."
                        ),
                    });
                }
                Err(error) => state.state.error_message = Some(error.to_string()),
            }
        }

        let final_write = workspace_id.and_then(|workspace_id| {
            let coordinator = self.coordinator.lock_or_recover();
            prepare_session_write_locked(
                &mut state,
                &self.writer.workspace,
                workspace_id,
                coordinator.model_history(),
            )
        });
        let title_request = prepare_title_request(&mut state);
        // Publish completion immediately. Persistence and the workspace-wide
        // catalog scan happen after releasing the live-state mutex.
        let _ = self
            .tx
            .send(BackendEvent::Envelope(state_envelope(&state.state)));
        (final_write, title_request)
    }

    /// Generates a session title after the run and persists it if the same
    /// session is still loaded.
    fn apply_generated_title(&self, request: titles::TitleRequest, workspace_id: Option<String>) {
        let Ok(title) = generate_session_title(&self.bridge, &request) else {
            return;
        };
        let history = self.coordinator.lock_or_recover().model_history();
        let title_write = workspace_id.and_then(|workspace_id| {
            let mut state = self.shared.lock_or_recover();
            if state.state.current_session_id.as_deref() != Some(request.session_id.as_str()) {
                return None;
            }
            state.meta.title = Some(title);
            let prepared = prepare_session_write_locked(
                &mut state,
                &self.writer.workspace,
                workspace_id,
                history,
            );
            let _ = self
                .tx
                .send(BackendEvent::Envelope(state_envelope_without_conversation(
                    &state.state,
                )));
            prepared
        });
        if let Some(prepared) = title_write {
            self.writer.commit_or_report(prepared);
        }
        self.writer.refresh_catalog();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streaming_session() -> SharedSession {
        let mut session = SharedSession::new("openai/test".into(), "auto".into());
        session.state.is_streaming = true;
        session.start_run("run-1".into(), "agent", "openai/test".into(), 0, 0);
        session.append_assistant_text("partial");
        session
    }

    #[test]
    fn settlement_releases_singular_run_state_on_every_outcome() {
        let outcomes: Vec<(Result<AgentRunOutcome>, bool, RunStatus)> = vec![
            (Ok(AgentRunOutcome::Completed), false, RunStatus::Completed),
            (Err(anyhow!("boom")), false, RunStatus::Failed),
            (Err(anyhow!("cancelled")), true, RunStatus::Interrupted),
            (
                Ok(AgentRunOutcome::GoalPaused {
                    reason: "judge".into(),
                }),
                false,
                RunStatus::Paused,
            ),
        ];
        for (result, cancelled, expected) in outcomes {
            let mut session = streaming_session();
            let status = settle_run_state(&mut session, result, cancelled, false);
            assert_eq!(status, expected);
            assert!(!session.state.is_streaming);
            assert!(session.meta.current_turn.is_none());
            assert!(session.state.active_run_id.is_none());
            assert!(session.state.active_assistant_entry_id.is_none());
            assert!(session.state.active_assistant_text.is_empty());
            let run = session.meta.runs.last().expect("run recorded");
            assert_eq!(run.status, expected);
            assert!(run.finished_at.is_some());
        }
    }

    #[test]
    fn completed_goal_returns_session_to_manual() {
        let mut session = streaming_session();
        session.state.autonomy_mode = AutonomyMode::Goal;
        settle_run_state(&mut session, Ok(AgentRunOutcome::Completed), false, true);
        assert_eq!(session.state.autonomy_mode, AutonomyMode::Manual);
        assert!(!session.state.goal_mode);

        let mut paused = streaming_session();
        paused.state.autonomy_mode = AutonomyMode::Goal;
        settle_run_state(
            &mut paused,
            Ok(AgentRunOutcome::GoalPaused {
                reason: "needs evidence".into(),
            }),
            false,
            true,
        );
        assert_eq!(paused.state.autonomy_mode, AutonomyMode::Goal);
    }
}
