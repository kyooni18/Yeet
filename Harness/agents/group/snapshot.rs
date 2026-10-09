//! Projections of group state for frontends, persistence, and tool results.

use serde_json::{Value, json};

use crate::{
    agents::{member::AgentMember, task::AgentTask},
    core::Usage,
    model::{AgentGroupItem, AgentMemberItem, AgentTaskItem},
};

use super::state::AgentGroupState;

impl AgentGroupState {
    /// The flat task list the bridge currently exposes to frontends.
    pub(crate) fn task_items(&self) -> Vec<AgentTaskItem> {
        self.tasks.iter().map(task_item).collect()
    }

    /// Members, recent activity, and usage for the frontend Agent view.
    pub(crate) fn group_item(&self) -> AgentGroupItem {
        AgentGroupItem {
            group_id: self.id.to_string(),
            objective: self.objective.clone(),
            status: self.status.clone(),
            final_result: self.final_result.clone(),
            checkpoint_summary: self.checkpoint_summary.clone(),
            members: self
                .members
                .iter()
                .map(|member| self.member_item(member))
                .collect(),
            activity: self.activity.iter().cloned().collect(),
            started_at: self.members.first().map(|member| member.started_at.clone()),
            events: self.events.iter().cloned().collect(),
            shared_findings: self.shared_findings.iter().cloned().collect(),
        }
    }

    fn member_item(&self, member: &AgentMember) -> AgentMemberItem {
        let mut usage = Usage::default();
        let mut latest = None;
        for task in self.tasks.iter().filter(|task| task.assignee == member.id) {
            usage.accumulate(&task.usage);
            latest = Some(task);
        }
        AgentMemberItem {
            id: member.id.to_string(),
            description: member.description.clone(),
            role: member.role.as_str().into(),
            model: member.model.clone(),
            status: member.status.as_str().into(),
            activity_state: member.activity_state.clone(),
            task_status: latest
                .map(|task| task.status.as_str().to_owned())
                .unwrap_or_default(),
            summary: latest.and_then(|task| task.summary.clone()),
            started_at: member.started_at.clone(),
        }
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
