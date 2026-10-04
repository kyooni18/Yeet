//! Admission policy: decides whether new work may start under the group's
//! budget, concurrency, membership, and write limits. It never runs agents.
//!
//! Rejections are returned as errors whose text is shown to the model, so
//! they say what to do instead.

use anyhow::{Result, bail};

use crate::agents::{
    AgentId,
    member::{AgentMember, AgentRole, MemberStatus},
};

use super::{AgentLimits, WritePolicy, budget_ledger::BudgetLedger, state::AgentGroupState};

/// Admits a new member. Returns an idle member to retire when the group is
/// at its membership cap.
pub(super) fn admit_spawn(
    limits: &AgentLimits,
    group: &AgentGroupState,
    budget: &BudgetLedger,
    role: AgentRole,
    weight: f64,
) -> Result<Option<AgentId>> {
    admit_work(limits, group, budget, role, weight)?;
    if group.live_member_count() < limits.max_members {
        return Ok(None);
    }
    match group
        .members
        .iter()
        .find(|member| member.status == MemberStatus::Idle)
    {
        Some(oldest_idle) => Ok(Some(oldest_idle.id)),
        None => bail!(
            "agent limit reached ({} live agents); stop an agent before launching another",
            limits.max_members
        ),
    }
}

/// Admits a follow-up message. Busy members queue it without taking a new
/// concurrency slot.
pub(super) fn admit_message(
    limits: &AgentLimits,
    group: &AgentGroupState,
    budget: &BudgetLedger,
    member: &AgentMember,
) -> Result<()> {
    match member.status {
        MemberStatus::Stopped => bail!("agent {} was stopped and cannot be messaged", member.id),
        MemberStatus::Running => Ok(()),
        MemberStatus::Idle => admit_work(limits, group, budget, member.role, 1.0),
    }
}

fn admit_work(
    limits: &AgentLimits,
    group: &AgentGroupState,
    budget: &BudgetLedger,
    role: AgentRole,
    weight: f64,
) -> Result<()> {
    if !budget.has_remaining_capacity(role, weight) {
        bail!(
            "the Agent Group has no remaining task budget; checkpoint or rebalance member work before delegating more"
        );
    }
    if group.busy_member_count() >= limits.max_concurrent {
        bail!(
            "{} members are already working; let current assignments progress before starting more",
            limits.max_concurrent
        );
    }
    if role.writes_workspace() {
        match limits.write_policy {
            WritePolicy::PrimaryOnly => {
                bail!("implementer agents are disabled; only the primary agent may write")
            }
            WritePolicy::SingleWriter if group.has_busy_writer() => bail!(
                "another implementer agent is working; only one may write to the workspace at a time"
            ),
            WritePolicy::SingleWriter => {}
            // Worktree isolation is a separate rollout stage. Fail closed
            // instead of silently sharing a writable checkout.
            WritePolicy::IsolatedWorktree => {
                bail!("isolated worktrees are not available yet")
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(role: AgentRole, status: MemberStatus) -> AgentMember {
        AgentMember {
            id: AgentId::new_v4(),
            description: "work".into(),
            role,
            model: "m".into(),
            spawned_by: None,
            status,
            activity_state: "waiting_for_input".into(),
            current_task: None,
            started_at: String::new(),
        }
    }

    #[test]
    fn single_writer_allows_one_busy_implementer() {
        let limits = AgentLimits::default();
        let mut group = AgentGroupState::new(1, None);
        let budget = BudgetLedger::new(limits.budget_limits());
        assert!(admit_spawn(&limits, &group, &budget, AgentRole::Implementer, 1.0).is_ok());
        group
            .members
            .push(member(AgentRole::Implementer, MemberStatus::Running));
        assert!(admit_spawn(&limits, &group, &budget, AgentRole::Implementer, 1.0).is_err());
        assert!(admit_spawn(&limits, &group, &budget, AgentRole::Researcher, 1.0).is_ok());

        let idle_writer = member(AgentRole::Implementer, MemberStatus::Idle);
        assert!(admit_message(&limits, &group, &budget, &idle_writer).is_err());
        group.members[0].status = MemberStatus::Idle;
        assert!(admit_message(&limits, &group, &budget, &idle_writer).is_ok());
    }

    #[test]
    fn membership_cap_retires_oldest_idle_member() {
        let limits = AgentLimits {
            max_concurrent: 2,
            max_members: 2,
            ..AgentLimits::default()
        };
        let budget = BudgetLedger::new(limits.budget_limits());
        let mut group = AgentGroupState::new(1, None);
        group
            .members
            .push(member(AgentRole::Researcher, MemberStatus::Running));
        group
            .members
            .push(member(AgentRole::Researcher, MemberStatus::Idle));
        let idle = group.members[1].id;
        assert_eq!(
            admit_spawn(&limits, &group, &budget, AgentRole::Researcher, 1.0).unwrap(),
            Some(idle)
        );
        group.members[1].status = MemberStatus::Running;
        assert!(admit_spawn(&limits, &group, &budget, AgentRole::Researcher, 1.0).is_err());
    }
}
