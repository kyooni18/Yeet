//! Admission policy: decides which proposed tasks may start under the group's
//! concurrency, total, budget, and write limits. It never runs agents itself.

use uuid::Uuid;

use crate::agents::task::{AgentTaskStatus, ProposedTask};

use super::{AgentLimits, WritePolicy, snapshot::AgentTaskSnapshot, state::AdaptiveAgentState};

/// A task the scheduler accepted, bound to the generation that admitted it.
pub(super) struct Admission {
    pub generation: u64,
    pub task_id: String,
    pub task: ProposedTask,
}

/// Admits a prefix of `proposed`, recording each admitted task in `state`.
pub(super) fn admit(
    limits: &AgentLimits,
    state: &mut AdaptiveAgentState,
    proposed: Vec<ProposedTask>,
) -> Vec<Admission> {
    if limits.budget_exhausted(&state.usage) {
        return Vec::new();
    }

    let remaining_total = limits.max_total.saturating_sub(state.total_started);
    let remaining_concurrent = limits.max_concurrent.saturating_sub(state.active.len());
    let limit = proposed
        .len()
        .min(remaining_total)
        .min(remaining_concurrent);
    if limit == 0 {
        return Vec::new();
    }

    let mut writer_admitted = state
        .tasks
        .iter()
        .any(|task| task.role.writes_workspace() && state.active.contains(&task.id));
    let mut admitted = Vec::new();
    for task in proposed.into_iter().take(limit) {
        if task.role.writes_workspace() {
            match limits.write_policy {
                WritePolicy::PrimaryOnly => continue,
                WritePolicy::SingleWriter if writer_admitted => continue,
                WritePolicy::SingleWriter => writer_admitted = true,
                WritePolicy::IsolatedWorktree => {
                    // Worktree isolation is a separate rollout stage. Fail closed
                    // instead of silently sharing a writable checkout.
                    continue;
                }
            }
        }
        let id = Uuid::new_v4().to_string();
        state.total_started += 1;
        state.active.insert(id.clone());
        state.tasks.push(AgentTaskSnapshot {
            id: id.clone(),
            role: task.role,
            objective: task.objective.clone(),
            status: AgentTaskStatus::Pending,
            summary: None,
        });
        admitted.push(Admission {
            generation: state.generation,
            task_id: id,
            task,
        });
    }
    admitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::member::AgentRole;

    fn task(role: AgentRole) -> ProposedTask {
        ProposedTask {
            role,
            objective: "work".into(),
        }
    }

    #[test]
    fn single_writer_admits_one_implementer_across_calls() {
        let limits = AgentLimits::default();
        let mut state = AdaptiveAgentState::default();
        let first = admit(
            &limits,
            &mut state,
            vec![task(AgentRole::Implementer), task(AgentRole::Implementer)],
        );
        assert_eq!(first.len(), 1);
        let second = admit(&limits, &mut state, vec![task(AgentRole::Implementer)]);
        assert!(second.is_empty());
        let reader = admit(&limits, &mut state, vec![task(AgentRole::Researcher)]);
        assert_eq!(reader.len(), 1);
    }

    #[test]
    fn concurrency_and_total_limits_bound_admission() {
        let limits = AgentLimits {
            max_concurrent: 2,
            max_total: 3,
            ..AgentLimits::default()
        };
        let mut state = AdaptiveAgentState::default();
        let researchers = || vec![task(AgentRole::Researcher); 4];
        assert_eq!(admit(&limits, &mut state, researchers()).len(), 2);
        assert!(admit(&limits, &mut state, researchers()).is_empty());
        state.active.clear();
        assert_eq!(admit(&limits, &mut state, researchers()).len(), 1);
    }
}
