//! Agent Group: the live set of delegated members, their tasks, and the
//! policies (budget, scheduling, workspace writes) that govern them.
//!
//! `AdaptiveAgentOrchestrator` remains the external facade while group
//! lifetime is still scoped to one delegation tool call.

mod budget;
mod commands;
mod runtime;
mod scheduler;
mod snapshot;
mod state;
mod workspace;

pub(crate) use budget::AgentLimits;
pub(crate) use runtime::AdaptiveAgentOrchestrator;
pub(crate) use workspace::WritePolicy;
