//! Projections of group state for frontends, tool results, and the primary
//! agent's completion notifications.

use serde_json::{Value, json};

use crate::{
    agents::{member::AgentMember, task::AgentTask},
    model::AgentTaskItem,
};

use super::state::AgentGroupState;

/// Summaries longer than this are truncated in notifications; the agent can
/// be messaged for the rest.
const NOTIFICATION_SUMMARY_CHARS: usize = 16_000;

/// A finished background task, rendered for the primary agent.
#[derive(Debug, Clone)]
pub(crate) struct AgentNotification {
    /// One line for the transcript.
    pub headline: String,
    /// The model-facing message.
    pub message: String,
}

impl AgentGroupState {
    /// The flat task list the bridge currently exposes to frontends.
    pub(crate) fn task_items(&self) -> Vec<AgentTaskItem> {
        self.tasks.iter().map(task_item).collect()
    }
}

fn task_item(task: &AgentTask) -> AgentTaskItem {
    AgentTaskItem {
        id: task.id.to_string(),
        role: task.role.as_str().into(),
        objective: task.description.clone(),
        status: task.status.as_str().into(),
        summary: task.summary.clone(),
    }
}

/// The tool-result form of a task.
pub(crate) fn task_result(task: &AgentTask) -> Value {
    json!({
        "agentId": task.assignee,
        "taskId": task.id,
        "role": task.role.as_str(),
        "status": task.status.as_str(),
        "outcome": task.outcome,
        "summary": task.summary,
        "usage": task.usage,
    })
}

pub(super) fn notification(member: &AgentMember, task: &AgentTask) -> AgentNotification {
    let summary = task.summary.as_deref().unwrap_or_default();
    let summary = match summary.char_indices().nth(NOTIFICATION_SUMMARY_CHARS) {
        Some((cut, _)) => format!(
            "{}\n[truncated; message the agent for the rest]",
            &summary[..cut]
        ),
        None => summary.to_owned(),
    };
    AgentNotification {
        headline: format!(
            "Background agent \"{}\" finished: {}",
            member.description,
            task.status.as_str()
        ),
        message: format!(
            "<agent-notification>\n<agent-id>{}</agent-id>\n<task-id>{}</task-id>\n<description>{}</description>\n<status>{}</status>\n<summary>\n{}\n</summary>\n</agent-notification>\nThis is an automated notice, not a user message. Continue the agent with send_agent_message using its agent-id if needed.",
            member.id,
            task.id,
            member.description,
            task.status.as_str(),
            summary
        ),
    }
}
