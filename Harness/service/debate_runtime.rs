//! Backend orchestration for the multi-stage debate workflow.
//!
//! Keeping debate execution here prevents the general backend dispatcher from
//! accumulating research, jury, and retained-knowledge state-machine details.
//! Stage prompts, evidence policy and scoring live in `crate::debate`; this
//! module only sequences them over the Harness bridge, tool registry, session
//! store and run lifecycle.

use super::*;
use std::sync::MutexGuard;

/// Handles shared by one debate run's worker thread.
struct DebateRun {
    turn: String,
    cancel: Arc<AtomicBool>,
    shared: Arc<Mutex<SharedSession>>,
    bridge: BridgeHandle,
    store: SessionStore,
    workspace: PathBuf,
    tx: EventSender,
    history: Vec<crate::core::Message>,
}

impl DebateRun {
    fn interrupted(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    /// Locks the session, failing once this run is cancelled or superseded.
    fn live(&self) -> Result<MutexGuard<'_, SharedSession>> {
        let s = self.shared.lock_or_recover();
        anyhow::ensure!(
            s.meta.current_turn.as_ref() == Some(&self.turn),
            "Session changed"
        );
        anyhow::ensure!(!self.interrupted(), "Debate interrupted");
        Ok(s)
    }

    /// Persists a durable state change and publishes the full state.
    fn commit(&self, s: &mut SharedSession) -> Result<()> {
        persist_locked(s, &self.store, &self.workspace, self.history.clone())?;
        let _ = self
            .tx
            .send(ServiceEvent::Envelope(state_envelope(&s.state)));
        Ok(())
    }

    /// Publishes an activity-only change without re-sending the transcript.
    fn publish_activity(&self, s: &SharedSession) {
        let _ = self
            .tx
            .send(ServiceEvent::Envelope(state_envelope_without_conversation(
                &s.state,
            )));
    }

    /// Sets the debate status and activity line, then commits.
    fn begin(&self, status: &str, kind: &str, activity: &str) -> Result<()> {
        let mut s = self.live()?;
        s.state.debate.as_mut().unwrap().status = status.into();
        s.set_activity(kind, activity, None);
        self.commit(&mut s)
    }

    fn append(&self, s: &mut SharedSession, kind: ConversationKind) -> Result<()> {
        s.append(kind);
        self.commit(s)
    }

    fn heading(&self, s: &mut SharedSession, title: &str, body: &str) -> Result<()> {
        self.append(
            s,
            ConversationKind::Assistant {
                content: format!("### {title}\n{body}"),
                tool_calls: vec![],
            },
        )
    }

    /// Appends a debate event to the session event log. A failed write is
    /// surfaced once as a non-fatal warning instead of aborting the debate.
    fn log_event(&self, s: &mut SharedSession, event: Value) {
        let Some(id) = s.state.current_session_id.clone() else {
            return;
        };
        if let Err(error) = self.store.append_event(&id, Some(&self.turn), &event) {
            let warning = format!("Debate event log write failed: {error}");
            if s.state.error_message.as_deref() != Some(warning.as_str()) {
                s.state.error_message = Some(warning);
            }
        }
    }

    /// Runs one model call, recording usage and standard cache diagnostics.
    /// The outer error means the run was interrupted; the inner one is the
    /// provider outcome, which each stage handles according to its policy.
    fn call(&self, phase: &str, request: &CallRequest) -> Result<Result<crate::core::CallResult>> {
        let response = self.bridge.complete_cancellable(request, &self.cancel);
        let mut s = self.live()?;
        let usage = response.as_ref().ok().and_then(|r| r.usage.as_ref());
        if let Some(usage) = usage {
            record_usage(&mut s.state, usage, 0);
        }
        let event = json!({
            "type": "debate-model-call",
            "phase": phase,
            "model": request.model,
            "succeeded": response.is_ok(),
            "cacheDiagnostics": crate::agent::model_call_cache_diagnostics(request, usage),
        });
        self.log_event(&mut s, event);
        Ok(response)
    }

    fn framing(&self) -> Result<()> {
        let (grounding, request) = {
            let mut s = self.live()?;
            let debate = s.state.debate.as_mut().unwrap();
            let project_brief = debate.framing_project_brief.as_deref().unwrap_or(
                "Project briefing omitted because this proposition is not workspace-bound.",
            );
            let grounding = format!(
                "{}\nProject briefing collected locally before framing:\n{}\nThe project briefing is orientation context, not proof of contested implementation details. The advocates will inspect the exact workspace during research. Framing must use this briefing to understand what project is being discussed, while leaving disputed technical claims for evidence collection. It must not replace a workspace-bound referent with a generic technology, theory, industry, or unrelated implementation debate.",
                debate.subject.render(),
                project_brief
            );
            debate.status = "Framing debate".into();
            let request = debate.contract_request_with_grounding(Some(&grounding));
            s.set_activity("thinking", "Framing debate", None);
            self.commit(&mut s)?;
            (grounding, request)
        };
        let response = self.call("framing", &request)?;
        let mut s = self.live()?;
        let topic = s.state.debate.as_ref().unwrap().topic.clone();
        let parsed = response.and_then(|response| {
            crate::debate::DebateContract::parse_response(&response)
                .map_err(|error| anyhow!("Neutral framing output was invalid: {error}"))
        });
        let (contract, warning) = match parsed {
            Ok(contract) => (contract, None),
            Err(error) => (
                crate::debate::DebateContract::minimal_with_grounding(&topic, Some(&grounding)),
                Some(format!(
                    "Neutral framing failed; using the conservative fallback contract: {error}"
                )),
            ),
        };
        s.state.debate.as_mut().unwrap().contract = Some(contract.clone());
        s.append(ConversationKind::Assistant {
            content: format!("### Debate contract\n{}", contract.render()),
            tool_calls: vec![],
        });
        if let Some(warning) = warning {
            s.append(ConversationKind::System { content: warning });
        }
        self.commit(&mut s)
    }

    /// Collects both dossiers before either advocate sees this stage's evidence.
    fn research_stage(&self, stage: usize) -> Result<()> {
        for pro in [true, false] {
            let label = format!(
                "{} research · {}",
                if stage == 0 { "Initial" } else { "Follow-up" },
                if pro { "Pro" } else { "Con" }
            );
            self.begin(&label, "research", &label)?;
            let snapshot = self.live()?.state.debate.clone().unwrap();
            let dossier = match self.research_side(&snapshot, stage, pro, &label) {
                Ok(dossier) => dossier,
                Err(error) => {
                    let mut s = self.live()?;
                    let message = format!(
                        "Research incomplete for {} at stage {stage} because a provider/tool capability failed: {error}. The debate will continue using only evidence already retained.",
                        if pro { "PRO" } else { "CON" },
                    );
                    s.append(ConversationKind::System {
                        content: message.clone(),
                    });
                    let mut partial = s
                        .state
                        .debate
                        .as_ref()
                        .and_then(|debate| debate.research_record(stage, pro).cloned())
                        .unwrap_or(crate::debate::ResearchRecord {
                            stage,
                            pro,
                            status: crate::debate::ResearchStatus::SynthesisFailed,
                            notes: String::new(),
                            history: Vec::new(),
                            successful_reads: 0,
                            evidence: Vec::new(),
                        });
                    partial.history.clear();
                    partial.status = crate::debate::ResearchStatus::SynthesisFailed;
                    partial.notes = message;
                    partial
                }
            };
            let mut s = self.live()?;
            s.append(ConversationKind::Assistant {
                content: format!("### {label}\n{}", dossier.notes),
                tool_calls: vec![],
            });
            s.state
                .debate
                .as_mut()
                .unwrap()
                .upsert_research_record(dossier);
            self.commit(&mut s)?;
        }
        Ok(())
    }

    fn research_side(
        &self,
        snapshot: &crate::debate::DebateState,
        stage: usize,
        pro: bool,
        label: &str,
    ) -> Result<crate::debate::ResearchRecord> {
        let mut registry = ToolRegistry::new(
            self.bridge.client()?,
            self.workspace.clone(),
            WorkerRegistry::new(Vec::new())?,
            PermissionBroker::default(),
        )?;
        registry.restrict_execution(true, true);
        let result = registry.attach_enabled_mcp_servers().and_then(|_| {
            crate::debate::research::run(
                snapshot,
                stage,
                pro,
                &mut registry,
                &self.cancel,
                |kind, detail| self.research_event(stage, pro, label, kind, &detail),
                |request| self.call("research", request)?,
            )
        });
        registry.shutdown();
        result
    }

    fn research_event(
        &self,
        stage: usize,
        pro: bool,
        label: &str,
        kind: &str,
        detail: &Value,
    ) -> Result<()> {
        let mut s = self.live()?;
        let record = (kind == "checkpoint")
            .then(|| detail.get("record"))
            .flatten();
        // Checkpoints carry the full evidence record, which is persisted in
        // debate state; the event log keeps only its provenance summary.
        let logged_detail = match record {
            Some(record) => json!({
                "status": record.get("status"),
                "successfulReads": record.get("successful_reads"),
                "sourceIds": record
                    .get("evidence")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .flat_map(|item| item.get("source_ids").and_then(Value::as_array).into_iter().flatten())
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>(),
            }),
            None => detail.clone(),
        };
        self.log_event(
            &mut s,
            json!({"type":"debate-research", "stage":stage, "pro":pro, "event":kind, "detail":logged_detail}),
        );
        let tool = detail
            .get("call")
            .and_then(|call| call.get("name"))
            .and_then(Value::as_str)
            .unwrap_or(kind);
        s.set_activity("research", label, Some(tool.into()));
        if let Some(record) = record {
            let record = serde_json::from_value::<crate::debate::ResearchRecord>(record.clone())?;
            s.state
                .debate
                .as_mut()
                .unwrap()
                .upsert_research_record(record);
            persist_locked(&mut s, &self.store, &self.workspace, self.history.clone())?;
        }
        self.publish_activity(&s);
        Ok(())
    }

    /// Delivers the advocate speech or jury ballot for `step`.
    fn speech_or_ballot(&self, step: usize, judging: bool) -> Result<()> {
        let stage = step / 2;
        let pro = step.is_multiple_of(2);
        let (reversed, request, status) = {
            let mut s = self.live()?;
            let d = s.state.debate.as_mut().unwrap();
            let reversed = judging && d.next_jury_reversed();
            d.status = if judging {
                let target = if d.ballots.len() < crate::debate::JURY_EARLY_BALLOTS {
                    crate::debate::JURY_EARLY_BALLOTS
                } else {
                    crate::debate::JURY_TARGET_BALLOTS
                };
                format!(
                    "Jury ballot {}/{target} · attempt {}",
                    d.ballots.len() + 1,
                    d.jury_attempts.len() + 1,
                )
            } else {
                format!(
                    "{} · {}",
                    d.stage_label(stage),
                    if pro { "Pro" } else { "Con" }
                )
            };
            let request = if judging {
                d.jury_request(reversed)
            } else {
                d.advocate_request(stage, pro)
            };
            let status = d.status.clone();
            s.set_activity("thinking", &status, None);
            self.commit(&mut s)?;
            (reversed, request, status)
        };
        let response = self.call(if judging { "jury" } else { "advocate" }, &request)?;
        let mut s = self.live()?;
        if judging {
            match response {
                Ok(response) => {
                    let display = if !response.text.trim().is_empty() {
                        response.text.clone()
                    } else if let Some(call) = response
                        .tool_calls
                        .iter()
                        .find(|call| call.name == "submit_debate_ballot")
                    {
                        serde_json::to_string_pretty(&call.arguments)
                            .unwrap_or_else(|_| "Structured jury ballot".into())
                    } else {
                        "Invalid empty jury response".into()
                    };
                    s.state
                        .debate
                        .as_mut()
                        .unwrap()
                        .record_jury_response(reversed, &response);
                    self.heading(&mut s, &status, &display)
                }
                Err(error) => {
                    let message = format!("Jury attempt failed: {error}");
                    s.state
                        .debate
                        .as_mut()
                        .unwrap()
                        .record_jury_error(reversed, message.clone());
                    self.heading(&mut s, "Jury attempt rejected", &message)
                }
            }
        } else {
            let response = response?;
            anyhow::ensure!(
                !response.text.trim().is_empty() && response.finish_reason != "length",
                "The response was empty or truncated. No verdict will be issued."
            );
            let speech = crate::debate::Speech::from_response(stage, pro, response.text);
            s.state
                .debate
                .as_mut()
                .unwrap()
                .record_speech(speech.clone())?;
            self.heading(&mut s, &status, &speech.text)
        }
    }

    fn checkpoint(&self, stage: usize) -> Result<()> {
        let request = {
            let mut s = self.live()?;
            let debate = s.state.debate.as_mut().unwrap();
            debate.status = format!("Progress checkpoint · {}", debate.stage_label(stage));
            let request = debate.checkpoint_request(stage);
            s.set_activity("thinking", "Checking debate progress", None);
            self.commit(&mut s)?;
            request
        };
        let response = self.call("checkpoint", &request)?;
        let mut s = self.live()?;
        let parsed = response.and_then(|response| {
            crate::debate::DebateCheckpoint::parse_response(stage, &response)
                .map_err(|error| anyhow!("invalid output: {error}"))
        });
        match parsed {
            Ok(checkpoint) => {
                let stage_label = s.state.debate.as_ref().unwrap().stage_label(stage);
                s.append(ConversationKind::Assistant {
                    content: format!(
                        "### Debate progress checkpoint · {stage_label}\n{}",
                        checkpoint.render()
                    ),
                    tool_calls: vec![],
                });
                s.state
                    .debate
                    .as_mut()
                    .unwrap()
                    .record_checkpoint(checkpoint.clone())?;
                self.log_event(
                    &mut s,
                    json!({"type":"debate-checkpoint", "stage":stage, "checkpoint":checkpoint}),
                );
            }
            Err(error) => {
                s.append(ConversationKind::System {
                    content: format!(
                        "Neutral progress checkpoint failed; keeping the bounded fallback debate loop: {error}"
                    ),
                });
            }
        }
        self.commit(&mut s)
    }

    /// Framing, research, advocacy, checkpoints and jury, in that order.
    fn run_stages(&self) -> Result<()> {
        self.framing()?;
        let mut step = 0usize;
        loop {
            let (closing_stage, jury_settled) = {
                let s = self.live()?;
                let debate = s.state.debate.as_ref().unwrap();
                (debate.closing_stage, debate.jury_settled())
            };
            let judging = closing_stage.is_some_and(|stage| step >= (stage + 1) * 2);
            if judging && jury_settled {
                return Ok(());
            }
            let stage = step / 2;
            if !judging
                && step.is_multiple_of(2)
                && closing_stage != Some(stage)
                && (step == 0 || step >= 4)
            {
                self.research_stage(stage)?;
                if step == 0
                    && let Some(gap) = self
                        .live()?
                        .state
                        .debate
                        .as_ref()
                        .and_then(crate::debate::DebateState::initial_workspace_grounding_gap)
                {
                    anyhow::bail!(
                        "Workspace-bound debate stopped before opening: {gap}. No verdict will be issued; fix research/provider access and retry."
                    );
                }
            }
            self.speech_or_ballot(step, judging)?;
            step += 1;
            if !judging && step.is_multiple_of(2) && stage >= 2 {
                self.checkpoint(stage)?;
            }
        }
    }

    fn knowledge_summary(&self) -> Result<crate::debate::DebateKnowledgeSummary> {
        let request = {
            let mut s = self.live()?;
            let debate = s.state.debate.as_mut().unwrap();
            debate.status = "Synthesizing learned facts".into();
            let request = debate.knowledge_summary_request();
            s.set_activity("thinking", "Synthesizing learned facts", None);
            self.commit(&mut s)?;
            request
        };
        crate::debate::DebateKnowledgeSummary::parse_response(&self.call("knowledge", &request)??)
    }
}

impl HarnessService {
    /// Starts a debate run and drives research, advocacy, jury, and knowledge retention.
    pub(super) fn start_debate(
        &mut self,
        topic: String,
        models: Option<crate::debate::DebateModels>,
    ) -> Result<()> {
        let topic = topic.trim().to_owned();
        anyhow::ensure!(
            !topic.is_empty() && topic.chars().count() <= 4000,
            "The debate topic must contain 1–4000 characters."
        );
        anyhow::ensure!(
            !self.shared.lock_or_recover().state.is_streaming,
            "Stop the current task before starting a debate."
        );
        let history = self.coordinator.lock_or_recover().model_history();
        let turn = Uuid::new_v4().to_string();
        let subject = discover_debate_subject(&topic, &self.workspace_root);
        let framing_project_brief = subject
            .workspace_bound
            .then(|| debate_project_brief(&self.workspace_root));
        let history_start = history.len();
        {
            let mut s = self.shared.lock_or_recover();
            anyhow::ensure!(
                !s.state.is_streaming,
                "Stop the current task before starting a debate."
            );
            let models =
                models.unwrap_or_else(|| crate::debate::DebateModels::same(&s.state.active_model));
            models.validate()?;
            let run_model = format!("pro={} con={} jury={}", models.pro, models.con, models.jury);
            let conversation_start = s.state.conversation.as_ref().map_or(0, Vec::len);
            s.start_run(
                turn.clone(),
                "debate",
                run_model,
                history_start,
                conversation_start,
            );
            let mut debate = crate::debate::DebateState::default();
            debate.topic = topic.clone();
            debate.models = models;
            debate.subject = subject;
            debate.framing_project_brief = framing_project_brief;
            debate.status = "Preparing".into();
            s.state.debate = Some(debate);
            s.append(ConversationKind::User {
                content: format!("Debate: {topic}"),
            });
            persist_locked(&mut s, &self.store, &self.workspace_root, history.clone())?;
            s.state.is_streaming = true;
            s.state.error_message = None;
        }
        let session_runtime = {
            let shared = self.shared.lock_or_recover();
            shared
                .state
                .current_session_id
                .clone()
                .map(|id| (id, shared.meta.retained_debate_knowledge.clone()))
        };
        if let Some((id, retained_knowledge)) = session_runtime
            && let Ok(mut coordinator) = self.coordinator.lock()
        {
            coordinator.set_protected_write_paths([self.store.directory.join(&id)]);
            coordinator.set_session_runtime(self.store.clone(), Some(id));
            coordinator.set_retained_debate_knowledge(retained_knowledge);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.run_manager.register(turn.clone(), cancel.clone());
        self.publish_state();
        let run = DebateRun {
            turn,
            cancel,
            shared: self.shared.clone(),
            bridge: self.bridge.clone(),
            store: self.store.clone(),
            workspace: self.workspace_root.clone(),
            tx: self.tx.clone(),
            history,
        };
        let coordinator = self.coordinator.clone();
        let run_manager = self.run_manager.clone();
        thread::spawn(move || {
            let outcome = run.run_stages().and_then(|_| {
                let mut s = run.live()?;
                let debate = s.state.debate.as_mut().unwrap();
                if let Some(reason) = debate.jury_unavailable_reason() {
                    debate.status = "JuryUnavailable".into();
                    Ok(DebateTerminalOutcome::JuryUnavailable(reason))
                } else {
                    debate.finish()?;
                    Ok(DebateTerminalOutcome::Completed)
                }
            });
            let knowledge_allowed = matches!(&outcome, Ok(DebateTerminalOutcome::Completed))
                && run
                    .shared
                    .lock_or_recover()
                    .state
                    .debate
                    .as_ref()
                    .is_some_and(crate::debate::DebateState::has_reusable_evidence);
            let knowledge = knowledge_allowed.then(|| run.knowledge_summary());
            run.finish(outcome, knowledge, knowledge_allowed, &coordinator);
            run_manager.remove_matching(&run.turn, &run.cancel);
        });
        Ok(())
    }
}

impl DebateRun {
    /// Records the terminal outcome, retained knowledge and run status.
    fn finish(
        &self,
        outcome: Result<DebateTerminalOutcome>,
        knowledge: Option<Result<crate::debate::DebateKnowledgeSummary>>,
        knowledge_allowed: bool,
        coordinator: &Arc<Mutex<AgentCoordinator>>,
    ) {
        let mut retained_for_coordinator = None;
        let mut s = self.shared.lock_or_recover();
        if s.meta.current_turn.as_ref() != Some(&self.turn) {
            return;
        }
        match outcome {
            Ok(DebateTerminalOutcome::Completed) => {
                let verdict = s.state.debate.as_ref().unwrap().verdict.clone().unwrap();
                s.append(ConversationKind::Assistant {
                    content: verdict,
                    tool_calls: vec![],
                });
                match knowledge {
                    Some(Ok(summary)) => {
                        let rendered = summary.render();
                        let (subject, evidence_ids) = {
                            let debate = s.state.debate.as_ref().unwrap();
                            (debate.subject.clone(), debate.evidence_source_ids())
                        };
                        let retained = crate::debate::RetainedDebateKnowledge::from_summary(
                            subject,
                            self.turn.clone(),
                            &summary,
                            evidence_ids,
                        );
                        s.state.debate.as_mut().unwrap().knowledge_summary = Some(summary);
                        s.append(ConversationKind::Assistant {
                            content: format!("### Debate knowledge retained\n{rendered}"),
                            tool_calls: vec![],
                        });
                        s.meta.retained_debate_knowledge.retain(|memory| {
                            memory.source_debate_run_id != retained.source_debate_run_id
                        });
                        s.meta.retained_debate_knowledge.push(retained);
                        retained_for_coordinator = Some(s.meta.retained_debate_knowledge.clone());
                    }
                    Some(Err(error)) => {
                        s.append(ConversationKind::System {
                            content: format!(
                                "Debate completed, but reusable knowledge synthesis failed: {error}"
                            ),
                        });
                    }
                    None if !knowledge_allowed => {
                        s.append(ConversationKind::System {
                            content: "Debate completed, but reusable knowledge was not retained because no provenance-bearing evidence packet was available.".into(),
                        });
                    }
                    None => {}
                }
                s.state.debate.as_mut().unwrap().status = "Completed".into();
                s.set_activity("done", "Debate completed", None);
                s.finish_run(RunStatus::Completed, None);
            }
            Ok(DebateTerminalOutcome::JuryUnavailable(reason)) => {
                s.append(ConversationKind::Assistant {
                    content: format!("### Jury unavailable\n{reason}"),
                    tool_calls: vec![],
                });
                s.state.debate.as_mut().unwrap().status = "JuryUnavailable".into();
                s.set_activity("done", "Debate completed · Jury unavailable", Some(reason));
                s.finish_run(RunStatus::Completed, None);
            }
            Err(error) => {
                let message = error.to_string();
                let interrupted = self.interrupted();
                s.state.debate.as_mut().unwrap().status =
                    if interrupted { "Interrupted" } else { "Failed" }.into();
                s.state.error_message = Some(message.clone());
                s.set_activity("failed", "Debate ended · No verdict", Some(message.clone()));
                s.finish_run(
                    if interrupted {
                        RunStatus::Interrupted
                    } else {
                        RunStatus::Failed
                    },
                    Some(message),
                );
            }
        }
        s.state.is_streaming = false;
        s.meta.current_turn = None;
        let final_history = coordinator
            .lock()
            .map(|coordinator| coordinator.model_history())
            .unwrap_or_else(|_| self.history.clone());
        if let Err(error) = persist_locked(&mut s, &self.store, &self.workspace, final_history) {
            s.state.error_message = Some(format!("Save failed: {error}"));
        }
        let _ = self
            .tx
            .send(ServiceEvent::Envelope(state_envelope(&s.state)));
        drop(s);
        if let Some(knowledge) = retained_for_coordinator
            && let Ok(mut coordinator) = coordinator.lock()
        {
            coordinator.set_retained_debate_knowledge(knowledge);
        }
    }
}
