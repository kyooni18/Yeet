//! Task identity, lifecycle states, and requested (not yet admitted) work.
//!
//! A task is one unit of work: a member's initial prompt or a later message
//! to it. The member that performs it is referenced only through `assignee`,
//! so task and member lifetimes stay independent.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    agents::{AgentId, member::AgentRole},
    core::Usage,
};

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

    /// Admitted work that is queued or executing.
    pub(crate) fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

/// A request to launch a new member, before admission.
#[derive(Debug, Clone)]
pub(crate) struct SpawnRequest {
    pub role: AgentRole,
    pub description: String,
    pub prompt: String,
    /// Background work is announced through a notification instead of
    /// being awaited by the caller.
    pub background: bool,
}

/// Admitted work owned by an Agent Group.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentTask {
    pub id: AgentTaskId,
    pub assignee: AgentId,
    pub role: AgentRole,
    pub description: String,
    pub status: AgentTaskStatus,
    pub background: bool,
    pub outcome: Option<String>,
    pub summary: Option<String>,
    pub usage: Usage,
}

impl AgentTask {
    pub(crate) fn queued(
        assignee: AgentId,
        role: AgentRole,
        description: impl Into<String>,
        background: bool,
    ) -> Self {
        Self {
            id: AgentTaskId::new_v4(),
            assignee,
            role,
            description: description.into(),
            status: AgentTaskStatus::Pending,
            background,
            outcome: None,
            summary: None,
            usage: Usage::default(),
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
