//! Member roles and the tool capabilities each role is denied.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

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
