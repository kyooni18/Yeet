//! Agent Group: the live set of delegated members, their tasks, and the
//! workspace write policy that governs them.
//!
//! Ownership: `agents::AgentGroupSupervisor` owns the `AgentGroupRuntime`;
//! tools reach it only through `agents::AgentGroupHandle`.

pub(crate) mod commands;
pub(crate) mod control_commands;
mod policy;
mod runtime;
mod scheduler;
mod snapshot;
mod state;
mod worker;
mod workspace;

#[cfg(test)]
mod tests;

pub(crate) use policy::AgentGroupPolicy;
pub(crate) use runtime::{AgentGroupRuntime, ChangeListener};
pub(crate) use snapshot::task_result;
pub(crate) use state::AgentGroupCheckpoint;
pub(crate) use workspace::WritePolicy;
