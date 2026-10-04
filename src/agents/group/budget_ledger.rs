//! Shared, group-owned budget with task allocations and deduplicated usage.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::{agents::member::AgentRole, core::Usage};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct BudgetLimits {
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
    pub coordination_output_reserve: u64,
    pub synthesis_output_reserve: u64,
    pub coordination_cost_reserve_usd: f64,
    pub synthesis_cost_reserve_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TaskNeed {
    pub task_id: String,
    pub role: AgentRole,
    /// Relative requested effort; normalized against the other active tasks.
    pub weight: f64,
    /// Input/context capacity for a task, independent from output spend.
    pub context_window_tokens: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Allocation {
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub context_window_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GroupUsage {
    pub output_tokens: u64,
    /// `None` means at least one accounted usage report had unknown cost.
    pub estimated_cost_usd: Option<f64>,
    pub provider_usage: Usage,
}

impl Default for GroupUsage {
    fn default() -> Self {
        Self {
            output_tokens: 0,
            // With no reports, known cost is zero. Once any report omits cost,
            // the aggregate becomes unknown rather than silently staying zero.
            estimated_cost_usd: Some(0.0),
            provider_usage: Usage::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BudgetLedger {
    limits: BudgetLimits,
    allocations: HashMap<String, Allocation>,
    needs: HashMap<String, TaskNeed>,
    completed: HashSet<String>,
    event_keys: HashSet<String>,
    task_usage: HashMap<String, GroupUsage>,
    usage: GroupUsage,
    member_usage: GroupUsage,
}

impl BudgetLedger {
    pub(crate) fn new(limits: BudgetLimits) -> Self {
        Self {
            limits,
            allocations: HashMap::new(),
            needs: HashMap::new(),
            completed: HashSet::new(),
            event_keys: HashSet::new(),
            task_usage: HashMap::new(),
            usage: GroupUsage::default(),
            member_usage: GroupUsage::default(),
        }
    }

    pub(crate) fn checkpoint_clone(&self) -> Self {
        let mut checkpoint = self.clone();
        // Checkpoints contain already-aggregated totals. Old event keys are
        // unnecessary after restore; new run keys remain unique by sequence.
        checkpoint.event_keys.clear();
        checkpoint
    }

    pub(crate) fn set_limits(&mut self, limits: BudgetLimits) {
        self.limits = limits;
        self.rebalance();
    }

    pub(crate) fn add_task(&mut self, need: TaskNeed) {
        self.completed.remove(&need.task_id);
        self.needs.insert(need.task_id.clone(), need);
        self.rebalance();
    }

    /// Rebalance unspent ceilings among active tasks. Spent amounts remain charged to the group.
    pub(crate) fn complete_task(&mut self, task_id: &str) {
        self.completed.insert(task_id.to_owned());
        self.rebalance();
    }

    pub(crate) fn rebalance(&mut self) {
        let out_pool = self.limits.output_tokens.saturating_sub(
            self.limits
                .coordination_output_reserve
                .saturating_add(self.limits.synthesis_output_reserve),
        );
        let cost_pool = (self.limits.estimated_cost_usd
            - self.limits.coordination_cost_reserve_usd
            - self.limits.synthesis_cost_reserve_usd)
            .max(0.0);
        let active: Vec<_> = self
            .needs
            .values()
            .filter(|n| !self.completed.contains(&n.task_id))
            .collect();
        let total_weight: f64 = active
            .iter()
            .map(|n| role_weight(n.role) * n.weight.max(0.0))
            .sum();
        let spent_out = self.member_usage.output_tokens;
        let out_remaining = out_pool.saturating_sub(spent_out);
        // An unknown provider cost cannot safely be treated as zero spend.
        // Preserve output capacity for useful work, but issue no additional
        // cost allocation until pricing/accounting can be determined.
        let cost_remaining = self
            .member_usage
            .estimated_cost_usd
            .map_or(0.0, |spent| (cost_pool - spent).max(0.0));
        self.allocations.clear();
        if total_weight <= 0.0 {
            return;
        }
        for need in active {
            let share = role_weight(need.role) * need.weight.max(0.0) / total_weight;
            let task_spent = self
                .task_usage
                .get(&need.task_id)
                .map_or(0, |u| u.output_tokens);
            let task_cost = self
                .task_usage
                .get(&need.task_id)
                .and_then(|u| u.estimated_cost_usd)
                .unwrap_or(0.0);
            self.allocations.insert(
                need.task_id.clone(),
                Allocation {
                    output_tokens: task_spent
                        .saturating_add((out_remaining as f64 * share).floor() as u64),
                    cost_usd: task_cost + cost_remaining * share,
                    context_window_tokens: need.context_window_tokens,
                },
            );
        }
    }

    /// Account each provider report once. `Usage` is retained verbatim at both task and group levels.
    pub(crate) fn account(&mut self, task_id: &str, event_key: &str, usage: &Usage) -> bool {
        if !self.event_keys.insert(event_key.to_owned()) {
            return false;
        }
        let output = usage.output_tokens.unwrap_or(0);
        self.usage.output_tokens = self.usage.output_tokens.saturating_add(output);
        self.usage.estimated_cost_usd =
            match (self.usage.estimated_cost_usd, usage.estimated_cost_usd) {
                (Some(a), Some(b)) => Some(a + b),
                _ => None,
            };
        self.usage.provider_usage.accumulate(usage);
        if self.needs.contains_key(task_id) {
            self.member_usage.output_tokens =
                self.member_usage.output_tokens.saturating_add(output);
            self.member_usage.estimated_cost_usd = match (
                self.member_usage.estimated_cost_usd,
                usage.estimated_cost_usd,
            ) {
                (Some(a), Some(b)) => Some(a + b),
                _ => None,
            };
            self.member_usage.provider_usage.accumulate(usage);
        }
        let tu = self.task_usage.entry(task_id.to_owned()).or_default();
        tu.output_tokens = tu.output_tokens.saturating_add(output);
        tu.estimated_cost_usd = match (tu.estimated_cost_usd, usage.estimated_cost_usd) {
            (Some(a), Some(b)) => Some(a + b),
            _ => None,
        };
        tu.provider_usage.accumulate(usage);
        self.rebalance();
        true
    }

    pub(crate) fn allocation(&self, task_id: &str) -> Option<Allocation> {
        self.allocations.get(task_id).copied()
    }
    /// Remaining output cap suitable for a provider's `max_tokens` parameter.
    pub(crate) fn remaining_output_cap(&self, task_id: &str) -> Option<u64> {
        let a = self.allocation(task_id)?;
        let spent = self.task_usage.get(task_id).map_or(0, |u| u.output_tokens);
        Some(a.output_tokens.saturating_sub(spent))
    }
    pub(crate) fn usage(&self) -> &GroupUsage {
        &self.usage
    }
    pub(crate) fn task_usage(&self, task_id: &str) -> Option<&GroupUsage> {
        self.task_usage.get(task_id)
    }
    pub(crate) fn remaining_phase_output(&self, phase: &str) -> u64 {
        if phase == "synthesis" {
            // Once member work settles, unused task and coordination capacity
            // rolls into the group's final integration phase.
            self.limits
                .output_tokens
                .saturating_sub(self.usage.output_tokens)
        } else {
            let spent = self
                .task_usage
                .get("__coordination")
                .map_or(0, |usage| usage.output_tokens);
            self.limits
                .coordination_output_reserve
                .saturating_sub(spent)
        }
    }

    pub(crate) fn remaining_phase_cost(&self, phase: &str) -> f64 {
        if phase == "synthesis" {
            self.usage
                .estimated_cost_usd
                .map_or(0.0, |used| (self.limits.estimated_cost_usd - used).max(0.0))
        } else {
            self.task_usage
                .get("__coordination")
                .and_then(|usage| usage.estimated_cost_usd)
                .map_or(0.0, |used| {
                    (self.limits.coordination_cost_reserve_usd - used).max(0.0)
                })
        }
    }

    pub(crate) fn has_remaining_capacity(&self, role: AgentRole, weight: f64) -> bool {
        let output_pool = self
            .limits
            .output_tokens
            .saturating_sub(self.reserves().output_tokens);
        output_pool.saturating_sub(self.member_usage.output_tokens) > 0
            && self.prospective_cost_allocation(role, weight) > 0.0
    }

    pub(crate) fn reserves(&self) -> Allocation {
        Allocation {
            output_tokens: self
                .limits
                .coordination_output_reserve
                .saturating_add(self.limits.synthesis_output_reserve),
            cost_usd: self.limits.coordination_cost_reserve_usd
                + self.limits.synthesis_cost_reserve_usd,
            context_window_tokens: 0,
        }
    }

    pub(crate) fn limits(&self) -> BudgetLimits {
        self.limits
    }

    pub(crate) fn prospective_cost_allocation(&self, role: AgentRole, weight: f64) -> f64 {
        let cost_pool = (self.limits.estimated_cost_usd
            - self.limits.coordination_cost_reserve_usd
            - self.limits.synthesis_cost_reserve_usd)
            .max(0.0);
        let remaining = self
            .member_usage
            .estimated_cost_usd
            .map_or(0.0, |spent| (cost_pool - spent).max(0.0));
        let new_weight = role_weight(role) * weight.max(0.0);
        let active_weight: f64 = self
            .needs
            .values()
            .filter(|need| !self.completed.contains(&need.task_id))
            .map(|need| role_weight(need.role) * need.weight.max(0.0))
            .sum();
        let total = active_weight + new_weight;
        if total <= 0.0 {
            0.0
        } else {
            remaining * new_weight / total
        }
    }
}

fn role_weight(role: AgentRole) -> f64 {
    match role {
        AgentRole::Researcher => 1.0,
        AgentRole::Implementer => 1.25,
        AgentRole::Verifier => 0.7,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> BudgetLedger {
        BudgetLedger::new(BudgetLimits {
            output_tokens: 1000,
            estimated_cost_usd: 10.0,
            coordination_output_reserve: 100,
            synthesis_output_reserve: 100,
            coordination_cost_reserve_usd: 1.0,
            synthesis_cost_reserve_usd: 1.0,
        })
    }
    fn task(id: &str, role: AgentRole, weight: f64, ctx: u64) -> TaskNeed {
        TaskNeed {
            task_id: id.into(),
            role,
            weight,
            context_window_tokens: ctx,
        }
    }
    fn usage(input: u64, output: u64, cost: Option<f64>) -> Usage {
        Usage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            estimated_cost_usd: cost,
            ..Usage::default()
        }
    }

    #[test]
    fn concurrent_allocations_rebalance_unspent_budget() {
        let mut l = ledger();
        l.add_task(task("a", AgentRole::Researcher, 1.0, 32000));
        let a = l.allocation("a").unwrap().output_tokens;
        l.add_task(task("b", AgentRole::Implementer, 1.0, 64000));
        assert!(l.allocation("a").unwrap().output_tokens < a);
        let before = l.allocation("a").unwrap().output_tokens;
        l.complete_task("b");
        assert!(l.allocation("a").unwrap().output_tokens > before);
    }

    #[test]
    fn reserves_and_context_output_cost_are_separate() {
        let mut l = ledger();
        l.add_task(task("a", AgentRole::Researcher, 1.0, 48000));
        assert_eq!(l.reserves().output_tokens, 200);
        assert_eq!(l.allocation("a").unwrap().context_window_tokens, 48000);
        l.account("a", "event-1", &usage(40000, 25, Some(0.25)));
        assert_eq!(l.usage().output_tokens, 25);
        assert_eq!(l.usage().estimated_cost_usd, Some(0.25));
        assert_eq!(
            l.remaining_output_cap("a"),
            Some(l.allocation("a").unwrap().output_tokens - 25)
        );
    }

    #[test]
    fn duplicate_event_is_ignored_and_unknown_cost_is_not_zero() {
        let mut l = ledger();
        l.add_task(task("a", AgentRole::Researcher, 1.0, 1000));
        assert!(l.account("a", "same", &usage(2, 7, None)));
        assert!(!l.account("a", "same", &usage(2, 7, Some(0.0))));
        assert_eq!(l.usage().output_tokens, 7);
        assert_eq!(l.usage().estimated_cost_usd, None);
        assert_eq!(l.usage().provider_usage.output_tokens, Some(7));
    }

    #[test]
    fn coordinator_and_member_usage_are_distinct_and_duplicates_do_not_recount() {
        let mut l = ledger();
        l.add_task(task("member-a", AgentRole::Researcher, 1.0, 32000));
        assert!(l.account("__coordination", "group:1", &usage(10, 3, Some(0.1))));
        assert!(l.account("member-a", "member-a:1", &usage(20, 7, Some(0.2))));
        assert!(!l.account("member-a", "member-a:1", &usage(20, 7, Some(0.2))));
        assert_eq!(l.usage().provider_usage.input_tokens, Some(30));
        assert_eq!(l.usage().provider_usage.output_tokens, Some(10));
        assert!((l.usage().estimated_cost_usd.unwrap() - 0.3).abs() < 1e-9);
        assert_eq!(l.task_usage("__coordination").unwrap().output_tokens, 3);
        assert_eq!(l.task_usage("member-a").unwrap().output_tokens, 7);
    }
}
