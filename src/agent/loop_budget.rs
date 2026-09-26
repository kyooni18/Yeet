//! Structural context-window lifecycle and turn telemetry.
//!
//! Long-running work is allowed to continue indefinitely. This module never
//! stops, finalizes, or narrows a task because of aggregate token usage. It
//! watches only the active model window and requests a compact rollover before
//! replay or instruction history becomes expensive.

use crate::core::Usage;

const PROACTIVE_ROLLOVER_PERCENT: u64 = 80;
const ROLLOVER_CUMULATIVE_INPUT_TOKENS: u64 = 96_000;

#[derive(Debug, Default)]
pub(super) struct LoopBudget {
    cumulative_input_tokens: u64,
    cumulative_cost_equivalent_input_tokens: u64,
    cumulative_sent_request_chars: u64,
    cumulative_estimated_cost_microusd: u64,
    peak_request_chars: usize,
    window_min_request_chars: Option<usize>,
    last_request_chars: usize,
    last_rollover_input_tokens: u64,
    rollovers: usize,
    window_model_calls: usize,
}

impl LoopBudget {
    pub(super) fn observe_request(&mut self, request_chars: usize) {
        self.peak_request_chars = self.peak_request_chars.max(request_chars);
        self.last_request_chars = request_chars;
        self.window_min_request_chars = Some(
            self.window_min_request_chars
                .map(|current| current.min(request_chars))
                .unwrap_or(request_chars),
        );
    }

    pub(super) fn record_sent_request(&mut self, request_chars: usize) {
        self.cumulative_sent_request_chars = self
            .cumulative_sent_request_chars
            .saturating_add(request_chars as u64);
    }

    pub(super) fn cumulative_sent_request_chars(&self) -> u64 {
        self.cumulative_sent_request_chars
    }

    pub(super) fn observe_usage(&mut self, usage: Option<&Usage>) {
        // A model response happened even when a provider omitted usage telemetry.
        self.window_model_calls = self.window_model_calls.saturating_add(1);
        let Some(usage) = usage else {
            return;
        };
        let input = usage.input_tokens.unwrap_or(0);
        self.cumulative_input_tokens = self.cumulative_input_tokens.saturating_add(input);
        self.cumulative_cost_equivalent_input_tokens =
            self.cumulative_cost_equivalent_input_tokens.saturating_add(
                usage
                    .cost_equivalent_input_tokens
                    .unwrap_or_else(|| cost_equivalent_input_tokens(usage)),
            );
        if let Some(cost) = usage.estimated_cost_usd
            && cost.is_finite()
            && cost > 0.0
        {
            let micros = (cost * 1_000_000.0).round().max(0.0) as u64;
            self.cumulative_estimated_cost_microusd = self
                .cumulative_estimated_cost_microusd
                .saturating_add(micros);
        }
    }

    pub(super) fn cumulative_input_tokens(&self) -> u64 {
        self.cumulative_input_tokens
    }

    pub(super) fn cumulative_cost_equivalent_input_tokens(&self) -> u64 {
        self.cumulative_cost_equivalent_input_tokens
    }

    pub(super) fn input_tokens_since_rollover(&self) -> u64 {
        self.cumulative_input_tokens
            .saturating_sub(self.last_rollover_input_tokens)
    }

    pub(super) fn rollover_count(&self) -> usize {
        self.rollovers
    }

    pub(super) fn window_model_calls(&self) -> usize {
        self.window_model_calls
    }

    pub(super) fn stage_label(&self) -> &'static str {
        let input = self.input_tokens_since_rollover();
        if input >= ROLLOVER_CUMULATIVE_INPUT_TOKENS {
            "rollover-due"
        } else if input >= ROLLOVER_CUMULATIVE_INPUT_TOKENS * 3 / 4 {
            "window-growing"
        } else {
            "normal"
        }
    }

    pub(super) fn estimated_cost_usd(&self) -> f64 {
        self.cumulative_estimated_cost_microusd as f64 / 1_000_000.0
    }

    pub(super) fn peak_request_chars(&self) -> usize {
        self.peak_request_chars
    }

    /// Returns a structural rollover reason for the active model window.
    ///
    /// Aggregate turn usage is intentionally ignored. A long task may create as
    /// many immutable windows as it needs; only the currently replayed window is
    /// bounded.
    pub(super) fn proactive_rollover_reason(
        &self,
        current_request_chars: usize,
        estimated_context_tokens: u64,
        rollover_budget: u64,
    ) -> Option<String> {
        let baseline = self
            .window_min_request_chars
            .unwrap_or(current_request_chars);
        let context_growth = current_request_chars.saturating_sub(baseline);
        let input_since_rollover = self.input_tokens_since_rollover();

        // The context estimator includes the full request prefix, tool schemas,
        // and request-only overlays. Fixed character or model-call thresholds
        // reset otherwise cacheable prefixes far below the model's real window.
        // Leave headroom for the rollover handoff, but let the configured model
        // budget—not an arbitrary transcript size—decide when to compact.
        let proactive_budget = rollover_budget
            .saturating_mul(PROACTIVE_ROLLOVER_PERCENT)
            .div_ceil(100)
            .max(1);
        if estimated_context_tokens < proactive_budget {
            return None;
        }

        Some(format!(
            "Coordinator structural rollover state: the context reached the proactive rollover threshold, so the runtime is carrying the same task into a fresh working window. estimatedContextTokens={estimated_context_tokens}; proactiveBudgetTokens={proactive_budget}; rolloverBudgetTokens={rollover_budget}; contextGrowthChars={context_growth}; inputTokensSinceRollover={input_since_rollover}; modelCallsInWindow={}. The compact working-evidence index is preserved and archived windows remain unchanged.",
            self.window_model_calls
        ))
    }

    pub(super) fn record_rollover(&mut self) {
        self.last_rollover_input_tokens = self.cumulative_input_tokens;
        self.rollovers = self.rollovers.saturating_add(1);
        self.window_min_request_chars = None;
        self.last_request_chars = 0;
        self.window_model_calls = 0;
    }
}

/// Conservative input-cost normalization for provider cache telemetry. Cache
/// reads are usually around 0.1x base input; cache writes range from about 1.25x
/// to 2x. Using the expensive 2x write case keeps telemetry conservative.
fn cost_equivalent_input_tokens(usage: &Usage) -> u64 {
    let input = usage.input_tokens.unwrap_or(0);
    let cached = usage.cached_input_tokens.unwrap_or(0).min(input);
    let cache_write = usage
        .cache_write_input_tokens
        .unwrap_or(0)
        .min(input.saturating_sub(cached));
    let ordinary = input.saturating_sub(cached).saturating_sub(cache_write);
    ordinary
        .saturating_add(cache_write.saturating_mul(2))
        .saturating_add(cached.div_ceil(10))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input_tokens: u64, cached_input_tokens: u64) -> Usage {
        Usage {
            input_tokens: Some(input_tokens),
            cached_input_tokens: Some(cached_input_tokens),
            ..Usage::default()
        }
    }

    #[test]
    fn rollover_tracks_trace_growth_not_large_immutable_prompt() {
        let mut budget = LoopBudget::default();
        budget.observe_request(80_000);
        assert!(
            budget
                .proactive_rollover_reason(80_000, 20_000, 100_000)
                .is_none()
        );

        budget.observe_request(150_000);
        assert!(
            budget
                .proactive_rollover_reason(150_000, 85_000, 100_000)
                .is_some()
        );

        budget.record_rollover();
        budget.observe_request(82_000);
        assert!(
            budget
                .proactive_rollover_reason(82_000, 20_000, 100_000)
                .is_none()
        );
    }

    #[test]
    fn repeated_medium_replay_rolls_the_window() {
        let mut budget = LoopBudget::default();
        budget.observe_request(30_000);
        budget.observe_usage(Some(&usage(100_000, 90_000)));
        budget.observe_request(50_000);
        assert!(
            budget
                .proactive_rollover_reason(50_000, 80_000, 100_000)
                .is_some()
        );
    }

    #[test]
    fn many_small_model_calls_do_not_force_a_small_window_rollover() {
        let mut budget = LoopBudget::default();
        budget.observe_request(12_000);
        for _ in 0..8 {
            budget.observe_usage(Some(&usage(2_000, 1_800)));
        }
        assert!(
            budget
                .proactive_rollover_reason(12_500, 12_500, 100_000)
                .is_none()
        );

        budget.record_rollover();
        assert_eq!(budget.window_model_calls(), 0);
        assert!(
            budget
                .proactive_rollover_reason(12_500, 12_500, 100_000)
                .is_none()
        );
        assert_eq!(budget.cumulative_input_tokens(), 8 * 2_000);
    }

    #[test]
    fn aggregate_megatoken_work_continues_via_many_compact_windows() {
        let mut budget = LoopBudget::default();
        let mut rollovers = 0;
        for _ in 0..128 {
            budget.observe_request(24_000);
            budget.observe_usage(Some(&usage(10_000, 9_000)));
            if budget
                .proactive_rollover_reason(24_000, 26_000, 32_000)
                .is_some()
            {
                budget.record_rollover();
                rollovers += 1;
            }
        }

        assert_eq!(budget.cumulative_input_tokens(), 1_280_000);
        assert!(rollovers >= 16);
        assert_eq!(budget.stage_label(), "normal");
    }

    #[test]
    fn missing_provider_usage_does_not_force_a_small_window_rollover() {
        let mut budget = LoopBudget::default();
        budget.observe_request(8_000);
        for _ in 0..8 {
            budget.observe_usage(None);
        }
        assert!(
            budget
                .proactive_rollover_reason(8_000, 8_000, 100_000)
                .is_none()
        );
    }

    #[test]
    fn cached_input_cost_equivalent_is_discounted_but_raw_telemetry_is_preserved() {
        let mut budget = LoopBudget::default();
        budget.observe_usage(Some(&usage(100_000, 90_000)));
        assert_eq!(budget.cumulative_input_tokens(), 100_000);
        assert_eq!(budget.cumulative_cost_equivalent_input_tokens(), 19_000);
    }
}
