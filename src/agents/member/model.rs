//! Member identity and roles, plus the tool capabilities each role is denied.
//!
//! `AgentMember` is identity only; live execution resources (the coordinator
//! and its cancellation) are owned by the member's worker thread.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::agents::{AgentId, task::AgentTaskId};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MemberStatus {
    /// Has a run in progress or queued messages.
    Running,
    /// Alive with its context intact, waiting for a message.
    Idle,
    /// Retired; its context is gone and it cannot be messaged.
    Stopped,
}

impl MemberStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Stopped => "stopped",
        }
    }
}

/// A group member. Its `id` is the process-wide registry identity.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentMember {
    pub id: AgentId,
    pub description: String,
    pub role: AgentRole,
    pub model: String,
    pub parent: Option<AgentId>,
    pub status: MemberStatus,
    pub current_task: Option<AgentTaskId>,
    /// RFC 3339 launch time.
    pub started_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentRole {
    Researcher,
    Implementer,
    Verifier,
}

impl AgentRole {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "researcher" | "research" => Ok(Self::Researcher),
            "implementer" | "implementation" | "writer" => Ok(Self::Implementer),
            "verifier" | "verification" | "verify" => Ok(Self::Verifier),
            other => bail!("unknown agent role: {other}"),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Researcher => "researcher",
            Self::Implementer => "implementer",
            Self::Verifier => "verifier",
        }
    }

    /// Whether this role mutates the workspace and is subject to write policy.
    pub(crate) fn writes_workspace(self) -> bool {
        self == Self::Implementer
    }

    pub(crate) fn disabled_capabilities(self) -> Vec<String> {
        match self {
            Self::Researcher => vec![
                "builtin:file-write".into(),
                "builtin:shell".into(),
                "builtin:computer-use".into(),
            ],
            Self::Implementer => Vec::new(),
            Self::Verifier => vec!["builtin:file-write".into(), "builtin:computer-use".into()],
        }
    }
}
