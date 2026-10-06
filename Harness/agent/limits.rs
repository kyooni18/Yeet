//! Coordinator repair, timeout, and Goal continuation limits shared across turn modules.

use std::time::Duration;

pub(super) const MALFORMED_TOOL_REPAIR_LIMIT: usize = 1;
pub(super) const EMPTY_RESPONSE_REPAIR_LIMIT: usize = 1;
pub(super) const LENGTH_CONTINUATION_LIMIT: usize = 1;
pub(super) const IMPLEMENTATION_REPAIR_LIMIT: usize = 1;
pub(super) const NO_PROGRESS_CORRECTION_THRESHOLD: usize = 1;
pub(super) const NO_PROGRESS_DECISION_THRESHOLD: usize = 2;
pub(super) const IMPLEMENTATION_INSPECTION_CHECKPOINT: usize = 3;
pub(super) const COMPLETION_GATE_REPAIR_LIMIT: usize = 2;
pub(super) const MODEL_ATTEMPT_TIMEOUT_MS: u64 = 15 * 60 * 1_000;
pub(super) const GOAL_CONTINUATION_DELAY: Duration = Duration::from_secs(1);
pub(super) const GOAL_CONTINUATION_PROMPT: &str = "Goal runtime state: an interrupted continuous job is resuming with its existing working state; previously completed work remains part of that state.";
pub(super) const GOAL_JOB_INSTRUCTION: &str = "Goal runtime state: this request is running as a continuous job. Tool-free responses are checkpoints evaluated by the independent goal judge, and runtime completion depends on concrete evidence for the user's requirements. Permission, cancellation, and safety limits remain in force.";
