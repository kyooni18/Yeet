//! Bridges Agent Group state into live client snapshots and the durable
//! session event/checkpoint streams.

use super::*;
use crate::{agents::group::AgentGroupCheckpoint, model::AgentTaskItem};

impl BackendService {
    /// Publishes the task list on every group change, including changes
    /// made by member threads after the primary turn has ended.
    pub(super) fn install_agent_group_listener(&self) {
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        let store = self.store.clone();
        let cursor = Arc::new(Mutex::new(None::<(String, String, u64, u64)>));
        self.agent_groups.set_change_listener(Arc::new(
            move |items: Vec<AgentTaskItem>, group, checkpoint: AgentGroupCheckpoint| {
                let session_id = {
                    let mut state = shared.lock_or_recover();
                    state.state.agent_tasks = items;
                    state.state.agent_group = group.clone();
                    let session_id = state.state.current_session_id.clone();
                    let _ = tx.send(BackendEvent::Envelope(state_envelope_without_conversation(
                        &state.state,
                    )));
                    session_id
                };
                let Some(session_id) = session_id else {
                    return;
                };
                if group.objective.is_none() && group.status == "idle" {
                    return;
                }

                let mut cursor = cursor
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if cursor
                    .as_ref()
                    .is_none_or(|(known_session, known_group, _, _)| {
                        known_session != &session_id || known_group != &group.group_id
                    })
                {
                    // The checkpoint sidecar already contains the complete
                    // bounded ring. On restoration only append the new marker.
                    let restored = checkpoint
                        .group
                        .events
                        .back()
                        .is_some_and(|event| event.kind == "group_restored");
                    let last = if restored {
                        checkpoint.group.event_sequence.saturating_sub(1)
                    } else {
                        0
                    };
                    *cursor = Some((
                        session_id.clone(),
                        group.group_id.clone(),
                        last,
                        checkpoint.revision.saturating_sub(1),
                    ));
                }
                let Some((_, _, last_sequence, last_revision)) = cursor.as_mut() else {
                    return;
                };
                let pending = checkpoint
                    .group
                    .events
                    .iter()
                    .filter(|event| event.sequence > *last_sequence)
                    .cloned()
                    .collect::<Vec<_>>();
                for event in pending {
                    let payload = serde_json::json!({
                        "type":"agent_group_event",
                        "groupId":event.group_id.clone(),
                        "groupSequence":event.sequence,
                        "memberId":event.member_id.clone(),
                        "taskId":event.task_id.clone(),
                        "event":&event,
                    });
                    if let Err(error) = store.append_event(&session_id, None, &payload) {
                        let mut state = shared.lock_or_recover();
                        state.state.error_message =
                            Some(format!("Group event save failed: {error}"));
                        break;
                    }
                    *last_sequence = event.sequence;
                }
                // A different member thread can reach this listener first
                // with a newer snapshot. Never let a delayed callback replace
                // its checkpoint with an older group event sequence.
                if checkpoint.group.event_sequence < *last_sequence
                    || checkpoint.revision < *last_revision
                {
                    return;
                }
                if let Err(error) = store.save_group_checkpoint(&session_id, &checkpoint) {
                    let mut state = shared.lock_or_recover();
                    state.state.error_message =
                        Some(format!("Group checkpoint save failed: {error}"));
                } else {
                    *last_revision = checkpoint.revision;
                }
            },
        ));
    }
}
