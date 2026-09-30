//! Group member identity: roles, capabilities, and role-specific prompting.
//! Members are workers; the work they perform lives in `agents::task`.

mod model;
mod prompt;

pub(crate) use model::AgentRole;
pub(crate) use prompt::{worker_prompt, worker_reasoning_level};
