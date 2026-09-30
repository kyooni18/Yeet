use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type AgentId = uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Running,
}

/// Explicit decisions, not private model reasoning or streaming deltas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDecision {
    pub summary: String,
}

/// An owned snapshot: callers cannot mutate registry invariants through it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub id: AgentId,
    pub name: String,
    pub model: String,
    pub workspace: PathBuf,
    pub decisions: Vec<AgentDecision>,
    pub coworkers: Vec<AgentId>,
    pub parent_agent: Option<AgentId>,
    pub status: AgentStatus,
}
