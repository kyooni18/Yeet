//! Workspace write policy for group members.
//!
//! Enforcement is currently admission-time only; a runtime write lease at the
//! mutating-tool boundary is a later stage.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WritePolicy {
    PrimaryOnly,
    SingleWriter,
    IsolatedWorktree,
}
