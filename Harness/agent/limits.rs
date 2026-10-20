//! Retry limits, timeouts, and Goal continuation constants.

use std::time::Duration;

pub(super) const MALFORMED_TOOL_REPAIR_LIMIT: usize = 1;
pub(super) const LENGTH_CONTINUATION_LIMIT: usize = 1;
pub(super) const MODEL_ATTEMPT_TIMEOUT_MS: u64 = 15 * 60 * 1_000;
pub(super) const GOAL_CONTINUATION_DELAY: Duration = Duration::from_secs(1);
pub(super) const GOAL_CONTINUATION_PROMPT: &str = "Resume the goal job from where it stopped.";
pub(super) const GOAL_JOB_INSTRUCTION: &str = "This is a goal job: keep working until the goal is met. When you stop calling tools, an independent judge checks the result against the goal. The goal is the user's own request, not a fixed rule: their newer messages take precedence over it.";
