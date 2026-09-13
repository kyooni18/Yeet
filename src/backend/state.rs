//! Mutable conversation state owned by a backend runtime.
//!
//! This module contains the small state machine that turns streaming model and
//! tool events into durable conversation entries. Keeping it separate makes
//! backend command dispatch independent from transcript bookkeeping.

use super::*;

/// Runtime-only metadata that accompanies the serializable bridge state.
#[derive(Default)]
pub(super) struct SessionMeta {
    pub(super) created_at: Option<DateTime<Utc>>,
    pub(super) title: Option<String>,
    pub(super) title_generation_attempted: bool,
    pub(super) attached_capabilities: Option<Vec<String>>,
    pub(super) disabled_capabilities: Vec<String>,
    pub(super) pending_tool_calls: HashMap<usize, ConversationToolCall>,
    pub(super) current_turn: Option<String>,
    pub(super) pending_compaction: bool,
    pub(super) pending_images: Vec<ImageAttachment>,
    pub(super) runs: Vec<StoredRun>,
    pub(super) retained_debate_knowledge: Vec<crate::debate::RetainedDebateKnowledge>,
}

/// Shared state for one live backend session.
pub(super) struct SharedSession {
    pub(super) state: BridgeState,
    pub(super) meta: SessionMeta,
}

impl SharedSession {
    /// Creates an empty session with the selected model and reasoning policy.
    pub(super) fn new(model: String, reasoning_level: String) -> Self {
        let state = BridgeState {
            active_model: model,
            active_reasoning_level: reasoning_level,
            conversation: Some(Vec::new()),
            ..BridgeState::default()
        };
        Self {
            state,
            meta: SessionMeta::default(),
        }
    }

    /// Returns the mutable transcript, creating it when a compact state omitted it.
    pub(super) fn conversation_mut(&mut self) -> &mut Vec<ConversationEntry> {
        self.state.conversation.get_or_insert_with(Vec::new)
    }

    /// Appends one transcript entry and advances the conversation revision.
    pub(super) fn append(&mut self, kind: ConversationKind) -> String {
        let id = Uuid::new_v4().to_string();
        self.conversation_mut().push(ConversationEntry {
            id: id.clone(),
            kind,
        });
        self.state.conversation_revision = self.state.conversation_revision.wrapping_add(1);
        id
    }

    /// Commits the active assistant text buffer into its transcript entry.
    pub(super) fn seal_assistant(&mut self) {
        let Some(id) = self.state.active_assistant_entry_id.clone() else {
            return;
        };
        let content = self.state.active_assistant_text.clone();
        if let Some(entry) = self
            .conversation_mut()
            .iter_mut()
            .find(|entry| entry.id == id)
            && let ConversationKind::Assistant { tool_calls, .. } = &mut entry.kind
        {
            let calls = std::mem::take(tool_calls);
            entry.kind = ConversationKind::Assistant {
                content,
                tool_calls: calls,
            };
        }
        self.state.conversation_revision = self.state.conversation_revision.wrapping_add(1);
    }

    /// Ends the current assistant streaming segment without ending the run.
    pub(super) fn seal_assistant_segment(&mut self) {
        if self.state.active_assistant_entry_id.is_some() {
            if !self.state.active_assistant_text.is_empty() {
                self.seal_assistant();
            }
            self.state.active_assistant_entry_id = None;
            self.state.active_assistant_text.clear();
        }
    }

    /// Ends the current reasoning segment and clears live-only buffers.
    pub(super) fn seal_reasoning_segment(&mut self) {
        self.state.active_reasoning_entry_id = None;
        self.state.active_reasoning_text.clear();
        self.state.active_reasoning_summary.clear();
    }

    /// Updates or appends the activity row associated with the current run.
    pub(super) fn set_activity(&mut self, phase: &str, title: &str, detail: Option<String>) {
        let run_id = self.meta.current_turn.clone();
        let is_same_activity = self
            .state
            .active_activity_entry_id
            .as_deref()
            .and_then(|id| {
                self.state
                    .conversation
                    .as_ref()?
                    .iter()
                    .find(|entry| entry.id == id)
            })
            .is_some_and(|entry| {
                matches!(
                    &entry.kind,
                    ConversationKind::Activity { activity }
                        if activity.phase.as_str() == Some(phase)
                            && activity.title == title
                            && activity.detail.as_deref() == detail.as_deref()
                            && activity.run_id == run_id
                )
            });
        if is_same_activity {
            return;
        }

        self.seal_reasoning_segment();
        self.seal_assistant_segment();
        if let Some(id) = self.state.active_activity_entry_id.clone() {
            let terminal = matches!(phase, "done" | "failed" | "interrupted");
            let updated = {
                let entries = self.conversation_mut();
                if let Some(index) = entries.iter().position(|entry| entry.id == id) {
                    let same_run = matches!(
                        &entries[index].kind,
                        ConversationKind::Activity { activity } if activity.run_id == run_id
                    );
                    if same_run {
                        entries[index].kind = ConversationKind::Activity {
                            activity: ModelActivity {
                                phase: json!(phase),
                                title: title.into(),
                                detail: detail.clone(),
                                run_id: run_id.clone(),
                            },
                        };
                        if terminal && index + 1 != entries.len() {
                            let entry = entries.remove(index);
                            entries.push(entry);
                        }
                    }
                    same_run
                } else {
                    false
                }
            };
            if updated {
                self.state.conversation_revision = self.state.conversation_revision.wrapping_add(1);
                return;
            }
        }

        let id = self.append(ConversationKind::Activity {
            activity: ModelActivity {
                phase: json!(phase),
                title: title.into(),
                detail,
                run_id,
            },
        });
        self.state.active_activity_entry_id = Some(id);
    }

    /// Marks persisted running records terminal when no live runtime owns them.
    pub(super) fn reconcile_orphaned_runs(&mut self, reason: &str) -> bool {
        let now = Utc::now();
        let orphaned = self
            .meta
            .runs
            .iter_mut()
            .filter_map(|run| {
                if run.status != RunStatus::Running {
                    return None;
                }
                run.status = RunStatus::Interrupted;
                run.finished_at = Some(now);
                run.error = Some(reason.to_owned());
                Some(run.id.clone())
            })
            .collect::<Vec<_>>();
        if orphaned.is_empty() {
            return false;
        }

        let is_orphaned = |id: &str| orphaned.iter().any(|candidate| candidate == id);
        let mut transcript_changed = false;
        if let Some(entries) = self.state.conversation.as_mut() {
            for entry in entries {
                let ConversationKind::Activity { activity } = &mut entry.kind else {
                    continue;
                };
                let Some(run_id) = activity.run_id.as_deref() else {
                    continue;
                };
                if !is_orphaned(run_id)
                    || matches!(
                        activity.phase.as_str(),
                        Some("done" | "failed" | "interrupted")
                    )
                {
                    continue;
                }
                activity.phase = json!("interrupted");
                activity.title = "Interrupted · Recovered stale run".into();
                activity.detail = Some(reason.to_owned());
                transcript_changed = true;
            }
        }
        if transcript_changed {
            self.state.conversation_revision = self.state.conversation_revision.wrapping_add(1);
        }
        if self.meta.current_turn.as_deref().is_some_and(&is_orphaned) {
            self.meta.current_turn = None;
        }
        if self.state.active_run_id.as_deref().is_some_and(is_orphaned) {
            self.state.active_run_id = None;
        }
        true
    }

    /// Registers a new persisted run and marks it active in the UI state.
    pub(super) fn start_run(&mut self, id: String, kind: &str, model: String) {
        self.reconcile_orphaned_runs(
            "A newer run started before the previous persisted run reached a terminal state.",
        );
        self.meta.current_turn = Some(id.clone());
        self.state.active_run_id = Some(id.clone());
        self.meta.runs.push(StoredRun {
            id,
            kind: kind.into(),
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            model,
            error: None,
        });
    }

    /// Finalizes the current persisted run with a terminal status.
    pub(super) fn finish_run(&mut self, status: RunStatus, error: Option<String>) {
        let Some(id) = self.meta.current_turn.as_deref() else {
            return;
        };
        if let Some(run) = self.meta.runs.iter_mut().rev().find(|run| run.id == id) {
            run.status = status;
            run.finished_at = Some(Utc::now());
            run.error = error;
        }
        self.state.active_run_id = None;
    }

    /// Extends the active assistant response, creating its entry on first delta.
    pub(super) fn append_assistant_text(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        if self.state.active_assistant_entry_id.is_none() {
            let id = self.append(ConversationKind::Assistant {
                content: String::new(),
                tool_calls: Vec::new(),
            });
            self.state.active_assistant_entry_id = Some(id);
        }
        self.state.active_assistant_text.push_str(delta);
        self.state.conversation_revision = self.state.conversation_revision.wrapping_add(1);
    }

    /// Extends live reasoning text or its summary and mirrors it into the transcript.
    pub(super) fn append_reasoning(&mut self, delta: &str, summary: bool) {
        if delta.is_empty() {
            return;
        }
        if self.state.active_reasoning_entry_id.is_none() {
            let id = self.append(ConversationKind::Reasoning {
                content: String::new(),
                summary: None,
            });
            self.state.active_reasoning_entry_id = Some(id);
        }
        if summary {
            self.state.active_reasoning_summary.push_str(delta);
        } else {
            self.state.active_reasoning_text.push_str(delta);
        }
        let id = self.state.active_reasoning_entry_id.clone().unwrap();
        let content = self.state.active_reasoning_text.clone();
        let summary = (!self.state.active_reasoning_summary.is_empty())
            .then(|| self.state.active_reasoning_summary.clone());
        if let Some(entry) = self
            .conversation_mut()
            .iter_mut()
            .find(|entry| entry.id == id)
        {
            entry.kind = ConversationKind::Reasoning { content, summary };
        }
        self.state.conversation_revision = self.state.conversation_revision.wrapping_add(1);
    }

    /// Removes a just-emitted assistant suffix when a retry invalidates it.
    pub(super) fn discard_assistant_text(&mut self, text: &str) {
        if self.state.active_assistant_text.ends_with(text) {
            let keep = self
                .state
                .active_assistant_text
                .len()
                .saturating_sub(text.len());
            self.state.active_assistant_text.truncate(keep);
            if self.state.active_assistant_text.is_empty()
                && let Some(id) = self.state.active_assistant_entry_id.take()
            {
                self.conversation_mut().retain(|entry| entry.id != id);
            }
            self.state.conversation_revision = self.state.conversation_revision.wrapping_add(1);
        }
    }

    /// Inserts or refreshes the transcript row for one tool call.
    pub(super) fn sync_tool_call(&mut self, call: ConversationToolCall) {
        let mut updated = false;
        {
            let entries = self.conversation_mut();
            if let Some(entry) = entries.iter_mut().rev().find(|entry| {
                matches!(
                    &entry.kind,
                    ConversationKind::ToolCall { tool_call } if tool_call.id == call.id
                )
            }) {
                entry.kind = ConversationKind::ToolCall {
                    tool_call: call.clone(),
                };
                updated = true;
            }
        }

        if updated {
            self.state.conversation_revision = self.state.conversation_revision.wrapping_add(1);
        } else {
            self.append(ConversationKind::ToolCall { tool_call: call });
        }
    }

    /// Updates execution status for a tool call, creating a row if needed.
    pub(super) fn set_tool_call_execution_status(
        &mut self,
        call: &crate::core::ToolCall,
        status: ToolCallStatus,
    ) {
        let existing_index = self
            .meta
            .pending_tool_calls
            .iter()
            .find_map(|(index, pending)| {
                (pending.call_id.as_deref() == Some(call.id.as_str())).then_some(*index)
            });

        let rendered = if let Some(index) = existing_index {
            let pending = self.meta.pending_tool_calls.get_mut(&index).unwrap();
            pending.call_id = Some(call.id.clone());
            pending.name = call.name.clone();
            pending.arguments = pretty_json(&call.arguments);
            pending.status = status;
            pending.clone()
        } else {
            ConversationToolCall {
                id: Uuid::new_v4().to_string(),
                index: None,
                call_id: Some(call.id.clone()),
                name: call.name.clone(),
                arguments: pretty_json(&call.arguments),
                status,
            }
        };

        self.sync_tool_call(rendered);

        if !matches!(status, ToolCallStatus::Streaming) {
            self.meta
                .pending_tool_calls
                .retain(|_, pending| pending.call_id.as_deref() != Some(call.id.as_str()));
        }
    }

    /// Forces all still-streaming tools into a terminal status at run shutdown.
    pub(super) fn settle_pending_tool_calls(&mut self, status: ToolCallStatus) {
        let calls = self
            .meta
            .pending_tool_calls
            .values_mut()
            .map(|call| {
                call.status = status;
                call.clone()
            })
            .collect::<Vec<_>>();
        self.meta.pending_tool_calls.clear();
        for call in calls {
            self.sync_tool_call(call);
        }
    }
}
