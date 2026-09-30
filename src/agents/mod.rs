//! Process-wide runtime agent state, independent of providers and frontends.
//! Identity is per coordinator, not per model attempt or conversation message.
/// Agent-owned intents, independent of Skyline deployment and orchestration.
pub mod actions;
pub(crate) mod group;
pub(crate) mod member;
mod registry;
pub(crate) mod runtime;
mod state;
pub(crate) mod task;

pub use registry::{AgentRegistry, RunGuard, global};
pub use state::{AgentDecision, AgentId, AgentState, AgentStatus};

#[cfg(test)]
mod tests;
