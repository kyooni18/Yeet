//! Process-wide runtime agent state, independent of providers and frontends.
//! Identity is per coordinator, not per model attempt or conversation message.
mod registry;
mod state;

pub use registry::{AgentRegistry, RunGuard, global};
pub use state::{AgentDecision, AgentId, AgentState, AgentStatus};

#[cfg(test)]
mod tests;
