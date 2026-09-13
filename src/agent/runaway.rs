//! Adaptive detection for runaway agent/tool loops.
//!
//! The detector scores several independent failure signals instead of relying
//! on a hard round cap, preserving recovery attempts while stopping genuine
//! repeated-work spirals.

use std::collections::{HashSet, VecDeque};

pub(super) const RUNAWAY_FINALIZATION_RETRY_LIMIT: usize = 1;
const RUNAWAY_WINDOW: usize = 6;
const RUNAWAY_WARN_SCORE: i32 = 6;
pub(super) const RUNAWAY_FINALIZE_SCORE: i32 = 12;
const RUNAWAY_CONTEXT_CHARS: usize = 64 * 1024;

/// One completed tool round summarized into progress and repetition signals.
#[derive(Debug, Clone)]
pub(super) struct RunawayRound {
    pub(super) progressed: bool,
    pub(super) mutated: bool,
    pub(super) failed_mutation: bool,
    pub(super) duplicate_inspection: bool,
    pub(super) inspection_only: bool,
    pub(super) semantic_fingerprint: String,
    pub(super) failure_fingerprints: Vec<String>,
    pub(super) output_fingerprints: Vec<u64>,
    pub(super) request_context_chars: usize,
    pub(super) fresh_calls: usize,
    pub(super) repeated_calls: usize,
}

/// Action recommended by the runaway detector after observing a round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RunawayDecision {
    Continue,
    Warn(String),
    Finalize(String),
}

/// Sliding-window detector that accumulates independent runaway signals.
#[derive(Debug, Default)]
pub(super) struct RunawayDetector {
    pub(super) score: i32,
    pub(super) recent: VecDeque<RunawayRound>,
}

impl RunawayDetector {
    /// Observes a tool round and decides whether to continue, warn, or finalize.
    pub(super) fn observe(
        &mut self,
        round: RunawayRound,
        implementation_requested: bool,
        implementation_incomplete: bool,
    ) -> RunawayDecision {
        if round.mutated {
            self.score = 0;
            self.recent.clear();
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
            >= 2;
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

        let repeated_mutation_failure =
            implementation_requested && round.failed_mutation && repeated_failure;
        let implementation_recovery_required =
            implementation_incomplete && !repeated_mutation_failure;
        if self.score >= RUNAWAY_FINALIZE_SCORE && signal_families >= 2 {
            if implementation_recovery_required {
                return RunawayDecision::Warn(format!(
                    "Runaway pattern detected from multiple independent signals (score={}): {}. Implementation work is still unresolved, so tool access remains available. Stop broad inspection and either perform the smallest justified mutation/verification action now or state a concrete blocker without more inspection.",
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
            return RunawayDecision::Finalize(format!(
                "Runaway pattern detected from multiple independent signals (score={}): {}. Stop tool execution and answer from already collected evidence.",
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
        if self.score >= RUNAWAY_WARN_SCORE {
            return RunawayDecision::Warn(format!(
                "Possible tool-loop pattern detected (score={}): {}. Change strategy, reuse existing evidence, and avoid repeating equivalent inspection. Tools remain available because the guard has not established a runaway loop.",
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
}

/// Renders active runaway signals into one compact diagnostic sentence.
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

    fn inspection_round() -> RunawayRound {
        RunawayRound {
            progressed: false,
            mutated: false,
            failed_mutation: false,
            duplicate_inspection: true,
            inspection_only: true,
            semantic_fingerprint: "read_file:src/ui".into(),
            failure_fingerprints: Vec::new(),
            output_fingerprints: vec![7],
            request_context_chars: RUNAWAY_CONTEXT_CHARS,
            fresh_calls: 0,
            repeated_calls: 1,
        }
    }

    #[test]
    fn unresolved_implementation_keeps_tools_after_inspection_runaway() {
        let mut detector = RunawayDetector::default();
        let mut decision = RunawayDecision::Continue;
        for _ in 0..6 {
            decision = detector.observe(inspection_round(), true, true);
        }
        assert!(detector.score >= RUNAWAY_FINALIZE_SCORE);
        assert!(matches!(decision, RunawayDecision::Warn(_)));
    }

    #[test]
    fn repeated_real_mutation_failure_can_still_hard_stop() {
        let mut detector = RunawayDetector::default();
        let failed_edit = || RunawayRound {
            progressed: false,
            mutated: false,
            failed_mutation: true,
            duplicate_inspection: false,
            inspection_only: false,
            semantic_fingerprint: "apply_file_edits:src/ui.rs".into(),
            failure_fingerprints: vec!["apply_file_edits:stale_anchor".into()],
            output_fingerprints: Vec::new(),
            request_context_chars: 1_000,
            fresh_calls: 1,
            repeated_calls: 0,
        };
        let mut decision = RunawayDecision::Continue;
        for _ in 0..4 {
            decision = detector.observe(failed_edit(), true, true);
        }
        assert!(matches!(decision, RunawayDecision::Finalize(_)));
    }
}
