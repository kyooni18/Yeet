//! Frontend-facing projections of group state.

use crate::{agents::task::AgentTask, model::AgentTaskItem};

use super::state::AgentGroupState;

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
        objective: task.objective.clone(),
        status: task.status.as_str().into(),
        summary: task.summary.clone(),
    }
}
