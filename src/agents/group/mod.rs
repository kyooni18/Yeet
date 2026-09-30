//! Agent Group: the live set of delegated members, their tasks, and the
//! policies (budget, scheduling, workspace writes) that govern them.
//!
//! Ownership: `agents::AgentGroupSupervisor` owns the `AgentGroupRuntime`;
//! tools reach it only through `agents::AgentGroupHandle`.

mod budget;
pub(crate) mod commands;
mod runtime;
mod scheduler;
mod snapshot;
mod state;
mod workspace;

pub(crate) use budget::AgentLimits;
pub(crate) use runtime::AgentGroupRuntime;
pub(crate) use workspace::WritePolicy;
