//! Backend orchestration for the multi-stage debate workflow.
//!
//! Keeping debate execution here prevents the general backend dispatcher from
//! accumulating research, jury, and retained-knowledge state-machine details.

use super::*;

impl BackendService {
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
            !self.shared.lock().unwrap().state.is_streaming,
            "Stop the current task before starting a debate."
        );
        let history = self.coordinator.lock().unwrap().model_history();
        let turn = Uuid::new_v4().to_string();
        let subject = discover_debate_subject(&topic, &self.workspace_root);
        let framing_project_brief = subject
            .workspace_bound
            .then(|| debate_project_brief(&self.workspace_root));
        {
            let mut s = self.shared.lock().unwrap();
            anyhow::ensure!(
                !s.state.is_streaming,
                "Stop the current task before starting a debate."
            );
            let models =
                models.unwrap_or_else(|| crate::debate::DebateModels::same(&s.state.active_model));
            models.validate()?;
            let run_model = format!("pro={} con={} jury={}", models.pro, models.con, models.jury);
            s.start_run(turn.clone(), "debate", run_model);
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
            let shared = self.shared.lock().unwrap();
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
        *self.active_cancel.lock().unwrap() = Some(cancel.clone());
        self.publish_state();
        let shared = self.shared.clone();
        let coordinator = self.coordinator.clone();
        let bridge = self.bridge.clone();
        let foundation_capability = format!(
            "mcp:{}",
            self.project_settings
                .load()
                .ok()
                .map(|project| project.foundation_memory.server)
                .unwrap_or_else(|| "foundation".into())
        );
        let store = self.store.clone();
        let workspace = self.workspace_root.clone();
        let tx = self.tx.clone();
        let active_cancel = self.active_cancel.clone();
        thread::spawn(move || {
            let result = (|| -> Result<()> {
                let framing_grounding = {
                    let s = shared.lock().unwrap();
                    let debate = s.state.debate.as_ref().unwrap();
                    let project_brief = debate.framing_project_brief.as_deref().unwrap_or(
                        "Project briefing omitted because this proposition is not workspace-bound.",
                    );
                    format!(
                        "{}\nProject briefing collected locally before framing:\n{}\nThe project briefing is orientation context, not proof of contested implementation details. The advocates will inspect the exact workspace during research. Framing must use this briefing to understand what project is being discussed, while leaving disputed technical claims for evidence collection. It must not replace a workspace-bound referent with a generic technology, theory, industry, or unrelated implementation debate.",
                        debate.subject.render(),
                        project_brief
                    )
                };
                let contract_request = {
                    let mut s = shared.lock().unwrap();
                    anyhow::ensure!(
                        s.meta.current_turn.as_ref() == Some(&turn),
                        "Session changed"
                    );
                    let request = {
                        let debate = s.state.debate.as_mut().unwrap();
                        debate.status = "Framing debate".into();
                        debate.contract_request_with_grounding(Some(&framing_grounding))
                    };
                    s.set_activity("thinking", "Framing debate", None);
                    persist_locked(&mut s, &store, &workspace, history.clone())?;
                    let _ = tx.send(BackendEvent::Envelope(state_envelope(&s.state)));
                    request
                };
                let contract_response = bridge.complete_cancellable(&contract_request, &cancel);
                {
                    let mut s = shared.lock().unwrap();
                    anyhow::ensure!(
                        s.meta.current_turn.as_ref() == Some(&turn)
                            && !cancel.load(Ordering::Acquire),
                        "Debate interrupted"
                    );
                    let topic = s.state.debate.as_ref().unwrap().topic.clone();
                    let mut warning = None;
                    let contract = match contract_response {
                        Ok(response) => {
                            if let Some(usage) = &response.usage {
                                record_usage(&mut s.state, usage, 0);
                            }
                            match crate::debate::DebateContract::parse_response(&response) {
                                Ok(contract) => contract,
                                Err(error) => {
                                    warning = Some(format!(
                                        "Neutral framing output was invalid; using the conservative fallback contract: {error}"
                                    ));
                                    crate::debate::DebateContract::minimal_with_grounding(
                                        &topic,
                                        Some(&framing_grounding),
                                    )
                                }
                            }
                        }
                        Err(error) => {
                            warning = Some(format!(
                                "Neutral framing call failed; using the conservative fallback contract: {error}"
                            ));
                            crate::debate::DebateContract::minimal_with_grounding(
                                &topic,
                                Some(&framing_grounding),
                            )
                        }
                    };
                    s.state.debate.as_mut().unwrap().contract = Some(contract.clone());
                    s.append(ConversationKind::Assistant {
                        content: format!("### Debate contract\n{}", contract.render()),
                        tool_calls: vec![],
                    });
                    if let Some(warning) = warning {
                        s.append(ConversationKind::System { content: warning });
                    }
                    persist_locked(&mut s, &store, &workspace, history.clone())?;
                    let _ = tx.send(BackendEvent::Envelope(state_envelope(&s.state)));
                }
                let mut step = 0usize;
                loop {
                    anyhow::ensure!(!cancel.load(Ordering::Acquire), "Debate interrupted");
                    let (closing_stage, jury_done) = {
                        let s = shared.lock().unwrap();
                        let debate = s.state.debate.as_ref().unwrap();
                        (debate.closing_stage, debate.jury_done())
                    };
                    let jury_start = closing_stage.map(|stage| (stage + 1) * 2);
                    if jury_start.is_some_and(|start| step >= start) && jury_done {
                        break;
                    }
                    let judging = jury_start.is_some_and(|start| step >= start);
                    let reversed = if judging {
                        shared
                            .lock()
                            .unwrap()
                            .state
                            .debate
                            .as_ref()
                            .unwrap()
                            .next_jury_reversed()
                    } else {
                        false
                    };
                    if !judging
                        && step.is_multiple_of(2)
                        && closing_stage != Some(step / 2)
                        && (step == 0 || step >= 4)
                    {
                        // Collect both dossiers before either advocate sees this stage's evidence.
                        for pro in [true, false] {
                            let stage = step / 2;
                            let label = format!(
                                "{} research · {}",
                                if stage == 0 { "Initial" } else { "Follow-up" },
                                if pro { "Pro" } else { "Con" }
                            );
                            let snapshot = {
                                let mut s = shared.lock().unwrap();
                                anyhow::ensure!(
                                    s.meta.current_turn.as_ref() == Some(&turn),
                                    "Session changed"
                                );
                                s.state.debate.as_mut().unwrap().status = label.clone();
                                s.set_activity("research", &label, None);
                                persist_locked(&mut s, &store, &workspace, history.clone())?;
                                let _ = tx.send(BackendEvent::Envelope(state_envelope(&s.state)));
                                s.state.debate.clone().unwrap()
                            };
                            let research = (|| -> Result<crate::debate::ResearchRecord> {
                                let mut registry = ToolRegistry::new(
                                    bridge.clone(),
                                    workspace.clone(),
                                    WorkerRegistry::new(Vec::new())?,
                                    PermissionBroker::default(),
                                )?;
                                registry.set_disabled_capabilities([
                                    "builtin:file-write".into(),
                                    "builtin:shell".into(),
                                    foundation_capability.clone(),
                                ]);
                                let result = registry.attach_enabled_mcp_servers().and_then(|_| {
                                    crate::debate::research::run(&snapshot, stage, pro, &bridge, &mut registry, &cancel, |kind, detail| {
                                        let mut s = shared.lock().unwrap();
                                        anyhow::ensure!(s.meta.current_turn.as_ref() == Some(&turn) && !cancel.load(Ordering::Acquire), "Research interrupted");
                                        if let Some(id) = &s.state.current_session_id {
                                            let logged_detail = if kind == "checkpoint" {
                                                let record = detail.get("record");
                                                let source_ids = record
                                                    .and_then(|record| record.get("evidence"))
                                                    .and_then(Value::as_array)
                                                    .into_iter()
                                                    .flatten()
                                                    .flat_map(|item| {
                                                        item.get("source_ids")
                                                            .and_then(Value::as_array)
                                                            .into_iter()
                                                            .flatten()
                                                    })
                                                    .filter_map(Value::as_str)
                                                    .map(str::to_owned)
                                                    .collect::<Vec<_>>();
                                                json!({
                                                    "status": record.and_then(|record| record.get("status")),
                                                    "successfulReads": record.and_then(|record| record.get("successful_reads")),
                                                    "sourceIds": source_ids,
                                                })
                                            } else {
                                                detail.clone()
                                            };
                                            if let Err(error) = store.append_event(
                                                id,
                                                Some(&turn),
                                                &json!({"type":"debate-research", "stage":stage, "pro":pro, "event":kind, "detail":logged_detail}),
                                            ) {
                                                let warning = format!("Debate event log write failed: {error}");
                                                if s.state.error_message.as_deref() != Some(warning.as_str()) {
                                                    s.state.error_message = Some(warning);
                                                }
                                            }
                                        }
                                        if kind == "checkpoint"
                                            && let Some(record) = detail.get("record")
                                        {
                                            let record = serde_json::from_value::<crate::debate::ResearchRecord>(record.clone())?;
                                            s.state.debate.as_mut().unwrap().upsert_research_record(record);
                                            persist_locked(&mut s, &store, &workspace, history.clone())?;
                                        }
                                        if kind == "response"
                                            && let Some(usage) = detail
                                                .get("response")
                                                .and_then(|r| r.get("usage"))
                                                .filter(|u| !u.is_null())
                                        {
                                            record_usage(
                                                &mut s.state,
                                                &serde_json::from_value::<Usage>(usage.clone())?,
                                                0,
                                            );
                                        }
                                        let tool = detail.get("call").and_then(|c| c.get("name")).and_then(Value::as_str).unwrap_or(kind);
                                        s.set_activity("research", &label, Some(tool.into()));
                                        let _ = tx.send(BackendEvent::Envelope(state_envelope(&s.state)));
                                        Ok(())
                                    })
                                });
                                registry.shutdown();
                                result
                            })();
                            let dossier = match research {
                                Ok(dossier) => dossier,
                                Err(error) => {
                                    let mut s = shared.lock().unwrap();
                                    anyhow::ensure!(
                                        s.meta.current_turn.as_ref() == Some(&turn)
                                            && !cancel.load(Ordering::Acquire),
                                        "Research interrupted"
                                    );
                                    let message = format!(
                                        "Research incomplete for {} at stage {} because a provider/tool capability failed: {error}. The debate will continue using only evidence already retained.",
                                        if pro { "PRO" } else { "CON" },
                                        stage,
                                    );
                                    s.append(ConversationKind::System {
                                        content: message.clone(),
                                    });
                                    let mut partial = s
                                        .state
                                        .debate
                                        .as_ref()
                                        .and_then(|debate| {
                                            debate.research_record(stage, pro).cloned()
                                        })
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
                            let mut s = shared.lock().unwrap();
                            anyhow::ensure!(
                                s.meta.current_turn.as_ref() == Some(&turn)
                                    && !cancel.load(Ordering::Acquire),
                                "Research interrupted"
                            );
                            s.append(ConversationKind::Assistant {
                                content: format!("### {label}\n{}", dossier.notes),
                                tool_calls: vec![],
                            });
                            s.state
                                .debate
                                .as_mut()
                                .unwrap()
                                .upsert_research_record(dossier);
                            persist_locked(&mut s, &store, &workspace, history.clone())?;
                            let _ = tx.send(BackendEvent::Envelope(state_envelope(&s.state)));
                        }

                        if step == 0 {
                            let grounding_gap =
                                shared.lock().unwrap().state.debate.as_ref().and_then(
                                    crate::debate::DebateState::initial_workspace_grounding_gap,
                                );
                            if let Some(gap) = grounding_gap {
                                anyhow::bail!(
                                    "Workspace-bound debate stopped before opening: {gap}. No verdict will be issued; fix research/provider access and retry."
                                );
                            }
                        }
                    }
                    let request = {
                        let mut s = shared.lock().unwrap();
                        anyhow::ensure!(
                            s.meta.current_turn.as_ref() == Some(&turn),
                            "Session changed"
                        );
                        let d = s.state.debate.as_mut().unwrap();
                        d.status = if !judging {
                            format!(
                                "{} · {}",
                                d.stage_label(step / 2),
                                if step.is_multiple_of(2) { "Pro" } else { "Con" }
                            )
                        } else {
                            let jury_target = if d.ballots.len() < crate::debate::JURY_EARLY_BALLOTS
                            {
                                crate::debate::JURY_EARLY_BALLOTS
                            } else {
                                crate::debate::JURY_TARGET_BALLOTS
                            };
                            format!(
                                "Jury ballot {}/{} · attempt {}/{}",
                                d.ballots.len() + 1,
                                jury_target,
                                d.jury_attempts.len() + 1,
                                crate::debate::JURY_MAX_ATTEMPTS,
                            )
                        };
                        let request = if !judging {
                            d.advocate_request(step / 2, step.is_multiple_of(2))
                        } else {
                            d.jury_request(reversed)
                        };
                        let status = d.status.clone();
                        s.set_activity("thinking", &status, None);
                        persist_locked(&mut s, &store, &workspace, history.clone())?;
                        let _ = tx.send(BackendEvent::Envelope(state_envelope(&s.state)));
                        request
                    };
                    let response = match bridge.complete_cancellable(&request, &cancel) {
                        Ok(response) => response,
                        Err(error) if judging => {
                            let mut s = shared.lock().unwrap();
                            anyhow::ensure!(
                                s.meta.current_turn.as_ref() == Some(&turn)
                                    && !cancel.load(Ordering::Acquire),
                                "Debate interrupted"
                            );
                            let message = format!("Jury attempt failed: {error}");
                            s.state
                                .debate
                                .as_mut()
                                .unwrap()
                                .record_jury_error(reversed, message.clone());
                            s.append(ConversationKind::Assistant {
                                content: format!("### Jury attempt rejected\n{message}"),
                                tool_calls: vec![],
                            });
                            persist_locked(&mut s, &store, &workspace, history.clone())?;
                            let _ = tx.send(BackendEvent::Envelope(state_envelope(&s.state)));
                            step += 1;
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    let mut s = shared.lock().unwrap();
                    anyhow::ensure!(
                        s.meta.current_turn.as_ref() == Some(&turn)
                            && !cancel.load(Ordering::Acquire),
                        "Debate interrupted"
                    );
                    if let Some(usage) = &response.usage {
                        record_usage(&mut s.state, usage, 0);
                    }
                    if !judging {
                        anyhow::ensure!(
                            !response.text.trim().is_empty() && response.finish_reason != "length",
                            "The response was empty or truncated. No verdict will be issued."
                        );
                    }
                    let status = s.state.debate.as_ref().unwrap().status.clone();
                    let jury_display = if judging {
                        if !response.text.trim().is_empty() {
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
                        }
                    } else {
                        String::new()
                    };
                    s.append(ConversationKind::Assistant {
                        content: format!(
                            "### {}\n{}",
                            status,
                            if judging {
                                jury_display
                            } else {
                                crate::debate::Speech::from_response(
                                    step / 2,
                                    step.is_multiple_of(2),
                                    response.text.clone(),
                                )
                                .text
                            }
                        ),
                        tool_calls: vec![],
                    });
                    persist_locked(&mut s, &store, &workspace, history.clone())?;
                    let d = s.state.debate.as_mut().unwrap();
                    if !judging {
                        d.record_speech(crate::debate::Speech::from_response(
                            step / 2,
                            step.is_multiple_of(2),
                            response.text,
                        ))?;
                    } else {
                        d.record_jury_response(reversed, &response);
                    }
                    persist_locked(&mut s, &store, &workspace, history.clone())?;
                    let _ = tx.send(BackendEvent::Envelope(state_envelope(&s.state)));
                    step += 1;
                    let completed_strengthening_stage = (!judging && step.is_multiple_of(2))
                        .then(|| step / 2 - 1)
                        .filter(|stage| *stage >= 2);
                    drop(s);

                    if let Some(stage) = completed_strengthening_stage {
                        let checkpoint_request = {
                            let mut s = shared.lock().unwrap();
                            anyhow::ensure!(
                                s.meta.current_turn.as_ref() == Some(&turn)
                                    && !cancel.load(Ordering::Acquire),
                                "Debate interrupted"
                            );
                            let request = {
                                let debate = s.state.debate.as_mut().unwrap();
                                debate.status =
                                    format!("Progress checkpoint · {}", debate.stage_label(stage));
                                debate.checkpoint_request(stage)
                            };
                            s.set_activity("thinking", "Checking debate progress", None);
                            persist_locked(&mut s, &store, &workspace, history.clone())?;
                            let _ = tx.send(BackendEvent::Envelope(state_envelope(&s.state)));
                            request
                        };

                        match bridge.complete_cancellable(&checkpoint_request, &cancel) {
                            Ok(response) => {
                                let mut s = shared.lock().unwrap();
                                anyhow::ensure!(
                                    s.meta.current_turn.as_ref() == Some(&turn)
                                        && !cancel.load(Ordering::Acquire),
                                    "Debate interrupted"
                                );
                                if let Some(usage) = &response.usage {
                                    record_usage(&mut s.state, usage, 0);
                                }
                                match crate::debate::DebateCheckpoint::parse_response(
                                    stage, &response,
                                ) {
                                    Ok(checkpoint) => {
                                        let rendered = checkpoint.render();
                                        let stage_label =
                                            s.state.debate.as_ref().unwrap().stage_label(stage);
                                        s.append(ConversationKind::Assistant {
                                            content: format!(
                                                "### Debate progress checkpoint · {}\n{}",
                                                stage_label, rendered
                                            ),
                                            tool_calls: vec![],
                                        });
                                        s.state
                                            .debate
                                            .as_mut()
                                            .unwrap()
                                            .record_checkpoint(checkpoint.clone())?;
                                        if let Some(id) = &s.state.current_session_id {
                                            store.append_event(
                                                id,
                                                Some(&turn),
                                                &json!({
                                                    "type":"debate-checkpoint",
                                                    "stage":stage,
                                                    "checkpoint":checkpoint,
                                                }),
                                            )?;
                                        }
                                    }
                                    Err(error) => {
                                        s.append(ConversationKind::System {
                                            content: format!(
                                                "Neutral progress checkpoint was invalid; keeping the bounded fallback debate loop: {error}"
                                            ),
                                        });
                                    }
                                }
                                persist_locked(&mut s, &store, &workspace, history.clone())?;
                                let _ = tx.send(BackendEvent::Envelope(state_envelope(&s.state)));
                            }
                            Err(error) if cancel.load(Ordering::Acquire) => return Err(error),
                            Err(error) => {
                                let mut s = shared.lock().unwrap();
                                anyhow::ensure!(
                                    s.meta.current_turn.as_ref() == Some(&turn),
                                    "Session changed"
                                );
                                s.append(ConversationKind::System {
                                    content: format!(
                                        "Neutral progress checkpoint failed; keeping the bounded fallback debate loop: {error}"
                                    ),
                                });
                                persist_locked(&mut s, &store, &workspace, history.clone())?;
                                let _ = tx.send(BackendEvent::Envelope(state_envelope(&s.state)));
                            }
                        }
                    }
                }
                Ok(())
            })();
            let outcome = result.and_then(|_| {
                let mut s = shared.lock().unwrap();
                anyhow::ensure!(
                    s.meta.current_turn.as_ref() == Some(&turn),
                    "Session changed"
                );
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
                && shared
                    .lock()
                    .ok()
                    .and_then(|s| {
                        s.state
                            .debate
                            .as_ref()
                            .map(crate::debate::DebateState::has_reusable_evidence)
                    })
                    .unwrap_or(false);
            let knowledge = if knowledge_allowed {
                let request = {
                    let mut s = shared.lock().unwrap();
                    if s.meta.current_turn.as_ref() != Some(&turn) {
                        return;
                    }
                    let debate = s.state.debate.as_mut().unwrap();
                    debate.status = "Synthesizing learned facts".into();
                    let request = debate.knowledge_summary_request();
                    s.set_activity("thinking", "Synthesizing learned facts", None);
                    let _ = persist_locked(
                        &mut s,
                        &store,
                        &workspace,
                        coordinator
                            .lock()
                            .map(|coordinator| coordinator.model_history())
                            .unwrap_or_else(|_| history.clone()),
                    );
                    let _ = tx.send(BackendEvent::Envelope(state_envelope(&s.state)));
                    request
                };
                match bridge.complete_cancellable(&request, &cancel) {
                    Ok(response) => {
                        if let Some(usage) = &response.usage
                            && let Ok(mut s) = shared.lock()
                            && s.meta.current_turn.as_ref() == Some(&turn)
                        {
                            record_usage(&mut s.state, usage, 0);
                        }
                        Some(crate::debate::DebateKnowledgeSummary::parse_response(
                            &response,
                        ))
                    }
                    Err(error) => Some(Err(error)),
                }
            } else {
                None
            };

            let mut retained_for_coordinator = None;
            let mut s = shared.lock().unwrap();
            if s.meta.current_turn.as_ref() != Some(&turn) {
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
                                turn.clone(),
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
                            retained_for_coordinator =
                                Some(s.meta.retained_debate_knowledge.clone());
                        }
                        Some(Err(error)) => {
                            s.append(ConversationKind::System {
                                content: format!(
                                    "Debate completed, but reusable knowledge synthesis failed: {error}"
                                ),
                            });
                        }
                        None => {
                            if !knowledge_allowed {
                                s.append(ConversationKind::System {
                                    content: "Debate completed, but reusable knowledge was not retained because no provenance-bearing evidence packet was available.".into(),
                                });
                            }
                        }
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
                    let interrupted = cancel.load(Ordering::Acquire);
                    s.state.debate.as_mut().unwrap().status =
                        if interrupted { "Interrupted" } else { "Failed" }.into();
                    s.state.error_message = Some(message.clone());
                    s.set_activity("failed", "Debate ended · No verdict", Some(message));
                    let run_error = s.state.error_message.clone();
                    s.finish_run(
                        if interrupted {
                            RunStatus::Interrupted
                        } else {
                            RunStatus::Failed
                        },
                        run_error,
                    );
                }
            }
            s.state.is_streaming = false;
            s.meta.current_turn = None;
            let final_history = coordinator
                .lock()
                .map(|coordinator| coordinator.model_history())
                .unwrap_or(history);
            if let Err(error) = persist_locked(&mut s, &store, &workspace, final_history) {
                s.state.error_message = Some(format!("Save failed: {error}"));
            }
            let _ = tx.send(BackendEvent::Envelope(state_envelope(&s.state)));
            drop(s);
            if let Some(knowledge) = retained_for_coordinator
                && let Ok(mut coordinator) = coordinator.lock()
            {
                coordinator.set_retained_debate_knowledge(knowledge);
            }
            clear_matching_cancel(&active_cancel, &cancel);
        });
        Ok(())
    }
}
