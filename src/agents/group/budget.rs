//! Group-wide limits on live members, concurrent work, tokens, and cost.
//!
//! Token and cost limits apply to a budget window that restarts with each
//! primary turn; members and their contexts outlive the window.

use crate::{core::Usage, model::SwarmSettings};

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
    /// The primary is told to fan out parallelizable work on its own.
    pub auto_deploy: bool,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self::from(&SwarmSettings::default())
    }
}

impl From<&SwarmSettings> for AgentLimits {
    fn from(settings: &SwarmSettings) -> Self {
        let settings = settings.clone().normalized();
        Self {
            max_concurrent: settings.max_concurrent as usize,
            max_members: settings.max_members as usize,
            max_tokens: settings.max_tokens,
            max_cost_usd: settings.max_cost_cents as f64 / 100.0,
            write_policy: if settings.write_policy == "primary_only" {
                WritePolicy::PrimaryOnly
            } else {
                WritePolicy::SingleWriter
            },
            auto_deploy: settings.auto_deploy,
        }
    }
}

impl AgentLimits {
    /// Primary-agent guidance for auto-deploy, or `None` when it is off.
    /// Depends only on the limits so it stays byte-stable across turns.
    pub(crate) fn deploy_guidance(&self) -> Option<String> {
        if !self.auto_deploy {
            return None;
        }
        let writers = match self.write_policy {
            WritePolicy::PrimaryOnly => {
                "Delegated agents may not write; keep every edit with yourself."
            }
            _ => {
                "At most one implementer agent may write at a time; give it a disjoint, bounded change."
            }
        };
        Some(format!(
            "Agent swarm auto-deploy is on. When a task splits into independent parts (separate files, modules, questions, or checks), launch agents for them with the agent tool instead of doing them one by one: up to {} at once, several agent calls in one response, run_in_background=true when you can keep working meanwhile. Use researcher agents for parallel investigation and verifier agents for independent checks. {writers} Do the work yourself when it is small, sequential, or needs this conversation's context. Each agent prompt must be self-contained.",
            self.max_concurrent
        ))
    }

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
