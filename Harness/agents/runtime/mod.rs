//! Execution resources shared by primary and delegated agents: coordinator
//! construction and run-keyed cancellation ownership.

mod factory;
mod run_manager;

pub(crate) use factory::AgentRuntimeFactory;
pub(crate) use run_manager::RunManager;
