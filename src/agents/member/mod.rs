//! Group member identity: roles, capabilities, role-specific prompting, and
//! the runner that executes a member's work. Members are workers; the work
//! they perform lives in `agents::task`.

mod model;
mod prompt;
mod runtime;

pub(crate) use model::{AgentMember, AgentRole, MemberStatus};
pub(crate) use prompt::{worker_prompt, worker_reasoning_level};
pub(crate) use runtime::{
    CoordinatorLauncher, MemberLauncher, MemberRunner, MemberSpec, RunOutcome, RunReport,
};
