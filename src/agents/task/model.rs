//! Task lifecycle states and proposed (not yet admitted) work.

use serde::{Deserialize, Serialize};

use crate::agents::member::AgentRole;

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
}

/// Work requested by an agent before the scheduler has admitted it.
#[derive(Debug, Clone)]
pub(crate) struct ProposedTask {
    pub role: AgentRole,
    pub objective: String,
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
