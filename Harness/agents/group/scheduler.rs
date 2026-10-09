//! Enforces workspace write-safety policy for Agent Group members.

use anyhow::{Result, bail};

use crate::agents::member::{AgentMember, AgentRole, MemberStatus};

use super::{AgentGroupPolicy, WritePolicy, state::AgentGroupState};

pub(super) fn check_spawn(
    policy: &AgentGroupPolicy,
    group: &AgentGroupState,
    role: AgentRole,
) -> Result<()> {
    if role.writes_workspace() {
        match policy.write_policy {
            WritePolicy::PrimaryOnly => {
                bail!("implementer agents are disabled; only the primary agent may write")
            }
            WritePolicy::SingleWriter if group.has_busy_writer() => bail!(
                "another implementer agent is working; only one may write to the workspace at a time"
            ),
            WritePolicy::SingleWriter => {}
            WritePolicy::IsolatedWorktree => {
                bail!("isolated worktrees are not available yet")
            }
        }
    }
    Ok(())
}

pub(super) fn check_message(
    policy: &AgentGroupPolicy,
    group: &AgentGroupState,
    member: &AgentMember,
) -> Result<()> {
    match member.status {
        MemberStatus::Stopped => bail!("agent {} was stopped and cannot be messaged", member.id),
        MemberStatus::Running => Ok(()),
        MemberStatus::Idle => check_spawn(policy, group, member.role),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentId;

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
    fn single_writer_allows_only_one_busy_implementer() {
        let policy = AgentGroupPolicy::default();
        let mut group = AgentGroupState::new(1, None);
        assert!(check_spawn(&policy, &group, AgentRole::Implementer).is_ok());
        group
            .members
            .push(member(AgentRole::Implementer, MemberStatus::Running));
        assert!(check_spawn(&policy, &group, AgentRole::Implementer).is_err());
        assert!(check_spawn(&policy, &group, AgentRole::Researcher).is_ok());

        let idle_writer = member(AgentRole::Implementer, MemberStatus::Idle);
        assert!(check_message(&policy, &group, &idle_writer).is_err());
        group.members[0].status = MemberStatus::Idle;
        assert!(check_message(&policy, &group, &idle_writer).is_ok());
    }
}
