//! Bridges the Agent Group to the session: live task-list publishing and
//! delivery of background-agent notifications while the primary is idle.
//!
//! A running primary turn drains notifications itself between model
//! requests; this path only starts a turn when nothing else is running.

use super::*;
use crate::model::AgentTaskItem;

impl BackendService {
    /// Publishes the task list on every group change, including changes
    /// made by member threads after the primary turn has ended.
    pub(super) fn install_agent_group_listener(&self) {
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        self.agent_groups
            .set_change_listener(Arc::new(move |items: Vec<AgentTaskItem>| {
                let mut state = shared.lock_or_recover();
                state.state.agent_tasks = items;
                let _ = tx.send(BackendEvent::Envelope(state_envelope_without_conversation(
                    &state.state,
                )));
            }));
    }

    /// Wakes the idle primary agent with finished background work.
    pub(super) fn deliver_agent_notifications(&mut self) {
        if !self.agent_groups.has_notifications() {
            return;
        }
        {
            let shared = self.shared.lock_or_recover();
            if shared.state.is_streaming || shared.meta.current_turn.is_some() {
                return;
            }
        }
        let notices = self.agent_groups.take_notifications();
        if notices.is_empty() {
            return;
        }
        {
            let mut shared = self.shared.lock_or_recover();
            for notice in &notices {
                shared.append(ConversationKind::System {
                    content: notice.headline.clone(),
                });
            }
        }
        let input = notices
            .iter()
            .map(|notice| notice.message.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        if let Err(error) = self.submit_agent(input, false, "agent-notification", false) {
            self.append_error(format!("Could not deliver agent results: {error}"));
        }
    }
}
