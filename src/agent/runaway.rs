//! Adaptive detection for repetitive agent/tool behavior.
//!
//! A detected loop never ends the task. Instead it requests a fresh model window
//! with a compact evidence handoff so execution can continue with a different
//! strategy without replaying the same accumulated trace.

use std::collections::{HashSet, VecDeque};

const RUNAWAY_WINDOW: usize = 6;
const RUNAWAY_WARN_SCORE: i32 = 6;
const RUNAWAY_ROLLOVER_SCORE: i32 = 12;
const RUNAWAY_CONTEXT_CHARS: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub(super) struct RunawayRound {
    pub(super) progressed: bool,
    pub(super) mutated: bool,
    pub(super) duplicate_inspection: bool,
    pub(super) inspection_only: bool,
    pub(super) semantic_fingerprint: String,
    pub(super) failure_fingerprints: Vec<String>,
    pub(super) output_fingerprints: Vec<u64>,
    pub(super) request_context_chars: usize,
    pub(super) fresh_calls: usize,
    pub(super) repeated_calls: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RunawayDecision {
    Continue,
    Warn(String),
    Rollover(String),
}

#[derive(Debug, Default)]
pub(super) struct RunawayDetector {
    pub(super) score: i32,
    pub(super) recent: VecDeque<RunawayRound>,
}

impl RunawayDetector {
    pub(super) fn observe(&mut self, round: RunawayRound) -> RunawayDecision {
        if round.mutated {
            self.reset();
            self.recent.push_back(round);
            return RunawayDecision::Continue;
        }

        let repeated_pattern = self
            .recent
            .iter()
            .rev()
            .take(4)
            .filter(|previous| previous.semantic_fingerprint == round.semantic_fingerprint)
            .count()
            >= 2
            && (!round.progressed || round.duplicate_inspection || round.repeated_calls > 0);
        let repeated_failure = round.failure_fingerprints.iter().any(|failure| {
            self.recent
                .iter()
                .rev()
                .take(4)
                .any(|previous| previous.failure_fingerprints.contains(failure))
        });
        let repeated_output = round.output_fingerprints.iter().any(|fingerprint| {
            self.recent
                .iter()
                .rev()
                .take(4)
                .any(|previous| previous.output_fingerprints.contains(fingerprint))
        });
        let previous_context = self
            .recent
            .back()
            .map(|previous| previous.request_context_chars);
        let window_start_context = self
            .recent
            .front()
            .map(|previous| previous.request_context_chars);
        let context_blowup = round.request_context_chars >= RUNAWAY_CONTEXT_CHARS
            && (previous_context.is_some_and(|previous| {
                previous > 0 && round.request_context_chars >= previous.saturating_mul(5) / 4
            }) || window_start_context.is_some_and(|start| {
                start > 0
                    && self.recent.len() >= 3
                    && round.request_context_chars >= start.saturating_mul(3) / 2
            }));
        let inspection_spiral = {
            let mut window = self
                .recent
                .iter()
                .rev()
                .take(RUNAWAY_WINDOW - 1)
                .collect::<Vec<_>>();
            window.push(&round);
            window.len() >= 4
                && window
                    .iter()
                    .all(|item| item.inspection_only && !item.mutated)
                && window.iter().filter(|item| item.progressed).count() >= 3
                && window
                    .iter()
                    .map(|item| item.semantic_fingerprint.as_str())
                    .collect::<HashSet<_>>()
                    .len()
                    <= 2
                && window.iter().any(|item| {
                    !item.progressed || item.duplicate_inspection || item.repeated_calls > 0
                })
        };
        let low_novelty = round.repeated_calls > 0
            && round.repeated_calls >= round.fresh_calls
            && !round.progressed;

        if !round.progressed {
            self.score += 3;
        } else {
            self.score = (self.score - 2).max(0);
        }
        if round.duplicate_inspection || low_novelty {
            self.score += 2;
        }
        if repeated_pattern {
            self.score += 3;
        }
        if repeated_failure {
            self.score += 4;
        }
        if repeated_output {
            self.score += 3;
        }
        if context_blowup {
            self.score += 3;
        }
        if inspection_spiral {
            self.score += 3;
        }

        let novelty_signal = !round.progressed || low_novelty || repeated_output;
        let repetition_signal = round.duplicate_inspection || repeated_pattern || inspection_spiral;
        let signal_families = [
            novelty_signal,
            repetition_signal,
            repeated_failure,
            context_blowup,
        ]
        .into_iter()
        .filter(|active| *active)
        .count();

        self.recent.push_back(round.clone());
        while self.recent.len() > RUNAWAY_WINDOW {
            self.recent.pop_front();
        }

        let established_low_context_loop = self.recent.len() >= RUNAWAY_WINDOW
            && (repeated_pattern || inspection_spiral)
            && (repeated_failure || repeated_output || low_novelty);
        if self.score >= RUNAWAY_ROLLOVER_SCORE
            && signal_families >= 2
            && (context_blowup || established_low_context_loop)
        {
            let message = format!(
                "Structural loop rollover triggered after multiple independent repetition signals were detected (score={}): {}. The runtime is carrying the same task into a fresh working window with compact workspace-evidence state preserved.",
                self.score,
                signal_summary(
                    !round.progressed,
                    round.duplicate_inspection || low_novelty,
                    repeated_pattern,
                    repeated_failure,
                    repeated_output,
                    context_blowup,
                    inspection_spiral,
                )
            );
            self.reset();
            return RunawayDecision::Rollover(message);
        }

        if self.score >= RUNAWAY_WARN_SCORE {
            return RunawayDecision::Warn(format!(
                "Possible repetitive tool pattern detected (score={}): {}. If repetition persists, the runtime may rotate to a fresh working window without ending the task.",
                self.score,
                signal_summary(
                    !round.progressed,
                    round.duplicate_inspection || low_novelty,
                    repeated_pattern,
                    repeated_failure,
                    repeated_output,
                    context_blowup,
                    inspection_spiral,
                )
            ));
        }

        RunawayDecision::Continue
    }

    fn reset(&mut self) {
        self.score = 0;
        self.recent.clear();
    }
}

fn signal_summary(
    no_progress: bool,
    low_novelty: bool,
    repeated_pattern: bool,
    repeated_failure: bool,
    repeated_output: bool,
    context_blowup: bool,
    inspection_spiral: bool,
) -> String {
    let mut signals = Vec::new();
    if no_progress {
        signals.push("no fresh progress");
    }
    if low_novelty {
        signals.push("duplicate/low-novelty calls");
    }
    if repeated_pattern {
        signals.push("repeated tool pattern");
    }
    if repeated_failure {
        signals.push("same failure repeating");
    }
    if repeated_output {
        signals.push("equivalent output repeating");
    }
    if context_blowup {
        signals.push("request context rapidly expanding");
    }
    if inspection_spiral {
        signals.push("inspection-only spiral");
    }
    if signals.is_empty() {
        "weak anomaly signal".into()
    } else {
        signals.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stalled_inspection() -> RunawayRound {
        RunawayRound {
            progressed: false,
            mutated: false,
            duplicate_inspection: true,
            inspection_only: true,
            semantic_fingerprint: "read_file:src/remote.rs".into(),
            failure_fingerprints: Vec::new(),
            output_fingerprints: vec![7],
            request_context_chars: 70_000,
            fresh_calls: 0,
            repeated_calls: 1,
        }
    }

    #[test]
    fn repeated_stall_rotates_context_instead_of_finalizing() {
        let mut detector = RunawayDetector::default();
        let mut rolled = false;
        for _ in 0..8 {
            if matches!(
                detector.observe(stalled_inspection()),
                RunawayDecision::Rollover(_)
            ) {
                rolled = true;
                break;
            }
        }

        assert!(rolled);
        assert_eq!(detector.score, 0);
        assert!(detector.recent.is_empty());
    }

    #[test]
    fn ordinary_early_failures_do_not_destroy_a_small_context_window() {
        let mut detector = RunawayDetector::default();
        let failures = ["glob", "timeout", "different-timeout", "permission"];
        for (index, failure) in failures.into_iter().enumerate() {
            let decision = detector.observe(RunawayRound {
                progressed: index == 2,
                mutated: false,
                duplicate_inspection: false,
                inspection_only: false,
                semantic_fingerprint: format!("shell-attempt-{index}"),
                failure_fingerprints: vec![failure.into()],
                output_fingerprints: vec![index as u64 + 100],
                request_context_chars: 20_000 + index * 2_000,
                fresh_calls: 1,
                repeated_calls: 0,
            });
            assert!(!matches!(decision, RunawayDecision::Rollover(_)));
        }
    }

    #[test]
    fn mutation_resets_loop_state() {
        let mut detector = RunawayDetector::default();
        for _ in 0..3 {
            let _ = detector.observe(stalled_inspection());
        }

        let mut mutation = stalled_inspection();
        mutation.mutated = true;
        mutation.progressed = true;
        assert_eq!(detector.observe(mutation), RunawayDecision::Continue);
        assert_eq!(detector.score, 0);
        assert_eq!(detector.recent.len(), 1);
    }

    #[test]
    fn rollover_reset_allows_the_task_to_continue() {
        let mut detector = RunawayDetector::default();
        for _ in 0..8 {
            if matches!(
                detector.observe(stalled_inspection()),
                RunawayDecision::Rollover(_)
            ) {
                break;
            }
        }

        let mut fresh = stalled_inspection();
        fresh.duplicate_inspection = false;
        fresh.repeated_calls = 0;
        fresh.fresh_calls = 1;
        fresh.progressed = true;
        fresh.semantic_fingerprint = "read_file:src/new.rs".into();
        assert_eq!(detector.observe(fresh), RunawayDecision::Continue);
    }
}
