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
pub(super) const GOAL_CONTINUATION_PROMPT: &str =
    "Resume the existing working state after an interruption. Do not restart completed work.";
pub(super) const GOAL_JOB_INSTRUCTION: &str = "Goal mode is a continuous job, not a sequence of normal chat completions. Work toward the user's goal using concrete actions and verification. Preserve completed work and choose the next unfinished requirement. Do not produce a normal final-answer summary after each work unit: a tool-free response is a checkpoint for the independent goal judge, not job completion. At checkpoints report only new evidence or a concrete blocker. The runtime owns completion; do not repeat unchanged work or invent additional scope. Permission, cancellation, and safety limits still apply.";
