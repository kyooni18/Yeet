//! Group-wide limits on live members, concurrent work, tokens, and cost.
//!
//! Token and cost limits apply to a budget window that restarts with each
//! primary turn; members and their contexts outlive the window.

use crate::core::Usage;

use super::WritePolicy;

#[derive(Debug, Clone)]
pub(crate) struct AgentLimits {
    /// Members that may run or have queued work at once.
    pub max_concurrent: usize,
    /// Members kept alive (idle members hold their context for follow-ups).
    pub max_members: usize,
    pub max_tokens: u64,
    pub max_cost_usd: f64,
    pub write_policy: WritePolicy,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            max_members: 8,
            max_tokens: 200_000,
            max_cost_usd: 2.0,
            write_policy: WritePolicy::SingleWriter,
        }
    }
}

impl AgentLimits {
    /// True once usage in the current window reaches the token or cost ceiling.
    pub(crate) fn budget_exhausted(&self, usage: &Usage) -> bool {
        usage_tokens(usage) >= self.max_tokens
            || usage.estimated_cost_usd.unwrap_or(0.0) >= self.max_cost_usd
    }
}

fn usage_tokens(usage: &Usage) -> u64 {
    usage.total_tokens.unwrap_or_else(|| {
        usage
            .input_tokens
            .unwrap_or(0)
            .saturating_add(usage.output_tokens.unwrap_or(0))
    })
}
