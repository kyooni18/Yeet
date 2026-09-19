//! Cost-aware guardrails for long model/tool loops.
//!
//! This is deliberately separate from `RunawayDetector`: fresh work can be
//! behaviorally legitimate and still be too expensive to continue unchanged.

use crate::core::Usage;

const SOFT_INPUT_TOKENS: u64 = 200_000;
const SOFT_REQUEST_CHARS: usize = 90_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LoopBudgetDecision {
    Continue,
    Checkpoint(String),
}

#[derive(Debug, Default)]
pub(super) struct LoopBudget {
    cumulative_input_tokens: u64,
    cumulative_cost_equivalent_input_tokens: u64,
    cumulative_estimated_cost_microusd: u64,
    peak_request_chars: usize,
    checkpoint_issued: bool,
}

impl LoopBudget {
    pub(super) fn observe_request(&mut self, request_chars: usize) {
        self.peak_request_chars = self.peak_request_chars.max(request_chars);
    }

    pub(super) fn observe_usage(&mut self, usage: Option<&Usage>) {
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

    pub(super) fn estimated_cost_usd(&self) -> f64 {
        self.cumulative_estimated_cost_microusd as f64 / 1_000_000.0
    }

    pub(super) fn peak_request_chars(&self) -> usize {
        self.peak_request_chars
    }

    /// Returns advisory cost guidance. Cost observations never withdraw tools or
    /// force finalization; correctness and user cancellation control termination.
    pub(super) fn decide(
        &mut self,
        root_calls: usize,
        implementation_requested: bool,
        unresolved_failed_mutation: bool,
        successful_mutations: usize,
        verification_succeeded: bool,
    ) -> LoopBudgetDecision {
        // Model/tool round count is diagnostic only. Long-lived Computer Use and
        // other interactive workflows can require many cheap rounds, so a fixed
        // attempt count must never withdraw tools by itself.
        let soft = self.cumulative_cost_equivalent_input_tokens >= SOFT_INPUT_TOKENS
            || self.peak_request_chars >= SOFT_REQUEST_CHARS;

        let unresolved_implementation = implementation_requested
            && (unresolved_failed_mutation || successful_mutations == 0 || !verification_succeeded);
        if soft && !self.checkpoint_issued {
            self.checkpoint_issued = true;
            return LoopBudgetDecision::Checkpoint(self.message(
                root_calls,
                if unresolved_implementation {
                    "cost checkpoint reached; stop broad discovery and complete only the smallest remaining mutation/verification path"
                } else {
                    "cost checkpoint reached; reuse existing evidence and finish unless one concrete gap remains"
                },
            ));
        }
        LoopBudgetDecision::Continue
    }

    fn message(&self, root_calls: usize, action: &str) -> String {
        format!(
            "Internal loop budget: {action}. rootCalls={root_calls}; cumulativeInputTokens={}; costEquivalentInputTokens={}; peakRequestChars={}; estimatedCostUsd={:.6}.",
            self.cumulative_input_tokens,
            self.cumulative_cost_equivalent_input_tokens,
            self.peak_request_chars,
            self.estimated_cost_usd(),
        )
    }
}

/// Conservative input-cost normalization for provider cache telemetry. Cache
/// reads are usually around 0.1x base input; cache writes range from about 1.25x
/// to 2x. Using the expensive 2x write case keeps the loop guard conservative
/// while no longer treating a large cache hit as full-price replay.
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
