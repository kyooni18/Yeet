//! Admission policy: decides which proposed tasks may start under the group's
//! concurrency, total, budget, and write limits. It never runs agents itself.

use crate::agents::task::{AgentTask, AgentTaskId, ProposedTask};

use super::{AgentLimits, WritePolicy, state::AgentGroupState};

/// A task the scheduler accepted, bound to the generation that admitted it.
pub(super) struct Admission {
    pub generation: u64,
    pub task_id: AgentTaskId,
    pub task: ProposedTask,
}

/// Admits a prefix of `proposed`, recording each admitted task in `group`.
pub(super) fn admit(
    limits: &AgentLimits,
    group: &mut AgentGroupState,
    proposed: Vec<ProposedTask>,
) -> Vec<Admission> {
    if limits.budget_exhausted(&group.usage) {
        return Vec::new();
    }

    let remaining_total = limits.max_total.saturating_sub(group.total_started);
    let remaining_concurrent = limits
        .max_concurrent
        .saturating_sub(group.active_task_count());
    let limit = proposed
        .len()
        .min(remaining_total)
        .min(remaining_concurrent);
    if limit == 0 {
        return Vec::new();
    }

    let mut writer_admitted = group.has_active_writer();
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
        let record = AgentTask::admitted(&task);
        group.total_started += 1;
        admitted.push(Admission {
            generation: group.generation,
            task_id: record.id,
            task,
        });
        group.tasks.push(record);
    }
    admitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{member::AgentRole, task::AgentTaskStatus};

    fn task(role: AgentRole) -> ProposedTask {
        ProposedTask {
            role,
            objective: "work".into(),
        }
    }

    #[test]
    fn single_writer_admits_one_implementer_across_calls() {
        let limits = AgentLimits::default();
        let mut group = AgentGroupState::new(1, None);
        let first = admit(
            &limits,
            &mut group,
            vec![task(AgentRole::Implementer), task(AgentRole::Implementer)],
        );
        assert_eq!(first.len(), 1);
        let second = admit(&limits, &mut group, vec![task(AgentRole::Implementer)]);
        assert!(second.is_empty());
        let reader = admit(&limits, &mut group, vec![task(AgentRole::Researcher)]);
        assert_eq!(reader.len(), 1);
    }

    #[test]
    fn concurrency_and_total_limits_bound_admission() {
        let limits = AgentLimits {
            max_concurrent: 2,
            max_total: 3,
            ..AgentLimits::default()
        };
        let mut group = AgentGroupState::new(1, None);
        let researchers = || vec![task(AgentRole::Researcher); 4];
        assert_eq!(admit(&limits, &mut group, researchers()).len(), 2);
        assert!(admit(&limits, &mut group, researchers()).is_empty());
        for task in &mut group.tasks {
            task.status = AgentTaskStatus::Reported;
        }
        assert_eq!(admit(&limits, &mut group, researchers()).len(), 1);
    }
}
