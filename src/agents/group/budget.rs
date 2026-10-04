//! Group-wide limits on live members, concurrent work, tokens, and cost.
//!
//! Token and cost limits apply to a budget window that restarts with each
//! primary turn; members and their contexts outlive the window.

use crate::{core::Usage, model::AgentGroupSettings};

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
        Self::from(&AgentGroupSettings::default())
    }
}

impl From<&AgentGroupSettings> for AgentLimits {
    fn from(settings: &AgentGroupSettings) -> Self {
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
            "Automatic Group Agent delegation is enabled. When a task has independent parts, create one group for the shared objective and let its coordinator assign bounded researcher, implementer, and verifier tasks. The group may run up to {} members concurrently. {writers} Handle small or sequential work directly. Give the group the objective and relevant context; its coordinator will scope each member assignment.",
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
