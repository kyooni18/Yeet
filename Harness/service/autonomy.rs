//! Autonomy and goal execution policy.
//!
//! Owns the session-level autonomy mode (Manual / Goal / Autonomous), the
//! hidden prompts that resume or chain self-directed work, and the decision
//! of whether a completed top-level run continues into another autonomous
//! cycle. The run lifecycle (`run.rs`) asks this module what to do next; it
//! does not encode autonomy rules itself.

use super::*;

pub(super) const GOAL_RESUME_PROMPT: &str = "Continue the current goal from the existing working state. Do not restart completed work. Make useful forward progress toward satisfying every requirement and do not stop until the strict goal judge can accept concrete evidence.";

pub(super) const AUTONOMOUS_NEXT_OBJECTIVE_PROMPT: &str = "Autonomous cycle: inspect the current conversation, workspace, working state, and completed work. Choose and complete exactly one concrete, useful, safe next objective that advances the user's established intent. Prefer unfinished work, verification, integration, or cleanup that materially improves the result. Do not invent busywork or repeat completed work. If there is no meaningful safe work left, respond with exactly AUTONOMOUS_IDLE and do not call tools.";
pub(super) const AUTONOMOUS_IDLE_MARKER: &str = "AUTONOMOUS_IDLE";

/// A hidden run that resumes self-directed work after autonomy is enabled.
pub(super) struct ResumeRun {
    pub(super) prompt: &'static str,
    pub(super) run_kind: &'static str,
    pub(super) continuation: bool,
}

impl ResumeRun {
    pub(super) fn for_mode(mode: AutonomyMode) -> Option<Self> {
        match mode {
            AutonomyMode::Manual => None,
            AutonomyMode::Goal => Some(Self {
                prompt: GOAL_RESUME_PROMPT,
                run_kind: "goal-resume",
                continuation: true,
            }),
            AutonomyMode::Autonomous => Some(Self {
                prompt: AUTONOMOUS_NEXT_OBJECTIVE_PROMPT,
                run_kind: "autonomous-resume",
                continuation: false,
            }),
        }
    }
}

/// What a top-level run does after one coordinator cycle returns.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CycleDecision {
    /// Return this cycle's outcome as the run outcome.
    Finish,
    /// The self-directed cycle reported no remaining work.
    Idle,
    /// Chain another self-directed cycle in the same run.
    Continue,
}

/// Decides whether an Autonomous-mode run chains another cycle.
///
/// Only a successfully completed, uncancelled cycle in Autonomous mode can
/// continue, and a self-directed cycle that answered exactly with the idle
/// marker ends the run as idle instead of looping.
pub(super) fn next_cycle(
    mode: AutonomyMode,
    outcome: &AgentRunOutcome,
    cancelled: bool,
    self_directed_cycle: bool,
    assistant_text: &str,
) -> CycleDecision {
    if mode != AutonomyMode::Autonomous
        || !matches!(outcome, AgentRunOutcome::Completed)
        || cancelled
    {
        return CycleDecision::Finish;
    }
    if self_directed_cycle && assistant_text.trim() == AUTONOMOUS_IDLE_MARKER {
        return CycleDecision::Idle;
    }
    CycleDecision::Continue
}

impl HarnessService {
    pub(super) fn set_goal_enabled(&mut self, enabled: bool) -> Result<()> {
        self.set_autonomy_mode(if enabled {
            AutonomyMode::Goal
        } else {
            AutonomyMode::Manual
        })
    }

    pub(super) fn set_autonomy_mode(&mut self, mode: AutonomyMode) -> Result<()> {
        let is_streaming = self.shared.lock_or_recover().state.is_streaming;
        if is_streaming && mode != AutonomyMode::Manual {
            return Err(anyhow!(
                "Autonomy mode cannot be enabled while a response is running."
            ));
        }

        let goal_enabled = mode != AutonomyMode::Manual;
        self.goal_mode.store(goal_enabled, Ordering::Release);
        let (session_id, resume) = {
            let mut shared = self.shared.lock_or_recover();
            shared.state.goal_mode = goal_enabled;
            shared.state.autonomy_mode = mode;
            let resume = goal_enabled
                && !shared.state.is_streaming
                && shared
                    .state
                    .conversation
                    .iter()
                    .flatten()
                    .any(|entry| matches!(entry.kind, ConversationKind::User { .. }));
            (shared.state.current_session_id.clone(), resume)
        };

        if let Some(session_id) = session_id.as_deref() {
            self.store.set_goal_mode(session_id, goal_enabled)?;
            if !is_streaming {
                let history = self.coordinator.lock_or_recover().model_history();
                let mut shared = self.shared.lock_or_recover();
                persist_locked(&mut shared, &self.store, &self.workspace_root, history)?;
            }
        }

        self.publish_state();
        if resume && let Some(resume) = ResumeRun::for_mode(mode) {
            self.submit_agent(
                resume.prompt.to_owned(),
                false,
                resume.run_kind,
                resume.continuation,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_completed_autonomous_cycles_chain() {
        let done = AgentRunOutcome::Completed;
        assert_eq!(
            next_cycle(AutonomyMode::Goal, &done, false, false, ""),
            CycleDecision::Finish
        );
        assert_eq!(
            next_cycle(AutonomyMode::Autonomous, &done, true, false, ""),
            CycleDecision::Finish
        );
        let paused = AgentRunOutcome::GoalPaused { reason: "x".into() };
        assert_eq!(
            next_cycle(AutonomyMode::Autonomous, &paused, false, false, ""),
            CycleDecision::Finish
        );
        assert_eq!(
            next_cycle(
                AutonomyMode::Autonomous,
                &done,
                false,
                false,
                "AUTONOMOUS_IDLE"
            ),
            CycleDecision::Continue,
            "the idle marker only counts on a self-directed cycle"
        );
        assert_eq!(
            next_cycle(
                AutonomyMode::Autonomous,
                &done,
                false,
                true,
                " AUTONOMOUS_IDLE\n"
            ),
            CycleDecision::Idle
        );
    }
}
