//! Group-wide limits on live members, concurrent work, output, and cost.

use crate::model::AgentGroupSettings;

use super::{WritePolicy, budget_ledger::BudgetLimits};

#[derive(Debug, Clone)]
pub(crate) struct AgentLimits {
    /// Members that may run or have queued work at once.
    pub max_concurrent: usize,
    /// Members kept alive (idle members hold their context for follow-ups).
    pub max_members: usize,
    pub max_tokens: u64,
    pub max_cost_usd: f64,
    pub write_policy: WritePolicy,
    /// The Main Agent is advised when group delegation fits the budget.
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
    pub(crate) fn budget_limits(&self) -> BudgetLimits {
        // Keep both coordinator and final synthesis capacity reserved while
        // delegated tasks share and rebalance the remainder.
        let coordination_output_reserve = self.max_tokens / 10;
        let synthesis_output_reserve = self.max_tokens / 10;
        let coordination_cost_reserve_usd = self.max_cost_usd * 0.1;
        let synthesis_cost_reserve_usd = self.max_cost_usd * 0.1;
        BudgetLimits {
            output_tokens: self.max_tokens,
            estimated_cost_usd: self.max_cost_usd,
            coordination_output_reserve,
            synthesis_output_reserve,
            coordination_cost_reserve_usd,
            synthesis_cost_reserve_usd,
        }
    }

    /// Main-Agent group guidance, or `None` when delegation is off.
    /// Depends only on the limits so it stays byte-stable across turns.
    pub(crate) fn group_guidance(&self) -> Option<String> {
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
}
