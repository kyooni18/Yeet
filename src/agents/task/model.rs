//! Task identity, lifecycle states, and proposed (not yet admitted) work.
//!
//! A task is work; the member that performs it is referenced only through
//! `assignee`, so task and member lifetimes stay independent.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agents::{AgentId, member::AgentRole};

pub(crate) type AgentTaskId = Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentTaskStatus {
    Pending,
    Running,
    Reported,
    NeedsVerification,
    Verified,
    Done,
    Failed,
    Cancelled,
}

impl AgentTaskStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Reported => "reported",
            Self::NeedsVerification => "needs_verification",
            Self::Verified => "verified",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Admitted work that still occupies a concurrency slot.
    pub(crate) fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

/// Work requested by an agent before the scheduler has admitted it.
#[derive(Debug, Clone)]
pub(crate) struct ProposedTask {
    pub role: AgentRole,
    pub objective: String,
}

/// Admitted work owned by an Agent Group.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentTask {
    pub id: AgentTaskId,
    /// The member role required to perform this task.
    pub role: AgentRole,
    pub objective: String,
    pub status: AgentTaskStatus,
    pub assignee: Option<AgentId>,
    pub summary: Option<String>,
}

impl AgentTask {
    pub(crate) fn admitted(proposed: &ProposedTask) -> Self {
        Self {
            id: AgentTaskId::new_v4(),
            role: proposed.role,
            objective: proposed.objective.clone(),
            status: AgentTaskStatus::Pending,
            assignee: None,
            summary: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_uses_stable_wire_names() {
        assert_eq!(
            AgentTaskStatus::NeedsVerification.as_str(),
            "needs_verification"
        );
        assert_eq!(AgentTaskStatus::Verified.as_str(), "verified");
    }
}
