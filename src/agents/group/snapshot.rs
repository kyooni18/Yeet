//! Frontend-facing projections of group task state.

use serde::Serialize;

use crate::{
    agents::{member::AgentRole, task::AgentTaskStatus},
    model::AgentTaskItem,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentTaskSnapshot {
    pub id: String,
    pub role: AgentRole,
    pub objective: String,
    pub status: AgentTaskStatus,
    pub summary: Option<String>,
}

impl AgentTaskSnapshot {
    pub(super) fn to_item(&self) -> AgentTaskItem {
        AgentTaskItem {
            id: self.id.clone(),
            role: self.role.as_str().into(),
            objective: self.objective.clone(),
            status: self.status.as_str().into(),
            summary: self.summary.clone(),
        }
    }
}
