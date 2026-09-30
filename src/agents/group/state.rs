//! Agent Group state: serializable membership, tasks, and usage, kept apart
//! from the live runtime resources (cancellation flags) that execute them.
//!
//! Tasks and members are stored in admission order so frontends render a
//! stable list.

use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
};

use serde::Serialize;
use uuid::Uuid;

use crate::{
    agents::{
        AgentId,
        member::{AgentMember, MemberStatus},
        task::{AgentTask, AgentTaskId},
    },
    core::Usage,
};

pub(crate) type AgentGroupId = Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentGroupState {
    pub id: AgentGroupId,
    /// Bumped per primary run; late updates from older generations are ignored.
    pub generation: u64,
    pub primary_agent: Option<AgentId>,
    pub members: Vec<AgentMember>,
    pub tasks: Vec<AgentTask>,
    pub total_started: usize,
    pub usage: Usage,
}

impl AgentGroupState {
    pub(crate) fn new(generation: u64, primary_agent: Option<AgentId>) -> Self {
        Self {
            id: AgentGroupId::new_v4(),
            generation,
            primary_agent,
            members: Vec::new(),
            tasks: Vec::new(),
            total_started: 0,
            usage: Usage::default(),
        }
    }

    pub(crate) fn active_task_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.status.is_active())
            .count()
    }

    pub(crate) fn has_active_writer(&self) -> bool {
        self.tasks
            .iter()
            .any(|task| task.status.is_active() && task.role.writes_workspace())
    }

    pub(crate) fn task_mut(&mut self, id: AgentTaskId) -> Option<&mut AgentTask> {
        self.tasks.iter_mut().find(|task| task.id == id)
    }

    /// Adds a running member and makes it the assignee of `task_id`.
    pub(crate) fn assign(&mut self, task_id: AgentTaskId, member: AgentMember) {
        if let Some(task) = self.task_mut(task_id) {
            task.assignee = Some(member.id);
        }
        self.members.push(AgentMember {
            status: MemberStatus::Running,
            current_task: Some(task_id),
            ..member
        });
    }

    /// Stops whichever member is currently working on `task_id`.
    pub(crate) fn release_task(&mut self, task_id: AgentTaskId) {
        for member in &mut self.members {
            if member.current_task == Some(task_id) {
                member.status = MemberStatus::Stopped;
                member.current_task = None;
            }
        }
    }
}

/// Live group state plus the runtime-only cancellation handles of its workers.
pub(super) struct GroupRuntimeState {
    pub group: AgentGroupState,
    pub cancels: HashMap<AgentTaskId, Arc<AtomicBool>>,
}

impl Default for GroupRuntimeState {
    fn default() -> Self {
        Self {
            group: AgentGroupState::new(0, None),
            cancels: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{
        member::AgentRole,
        task::{AgentTaskStatus, ProposedTask},
    };

    #[test]
    fn member_assignment_links_task_and_release_stops_member() {
        let mut group = AgentGroupState::new(1, None);
        let task = AgentTask::admitted(&ProposedTask {
            role: AgentRole::Implementer,
            objective: "fix".into(),
        });
        let task_id = task.id;
        group.tasks.push(task);
        assert!(group.has_active_writer());

        let member_id = AgentId::new_v4();
        group.assign(
            task_id,
            AgentMember {
                id: member_id,
                role: AgentRole::Implementer,
                model: "m".into(),
                parent: None,
                status: MemberStatus::Stopped,
                current_task: None,
            },
        );
        assert_eq!(group.tasks[0].assignee, Some(member_id));
        assert_eq!(group.members[0].status, MemberStatus::Running);

        group.task_mut(task_id).unwrap().status = AgentTaskStatus::NeedsVerification;
        group.release_task(task_id);
        assert_eq!(group.members[0].status, MemberStatus::Stopped);
        assert_eq!(group.members[0].current_task, None);
        assert_eq!(group.active_task_count(), 0);
        assert!(!group.has_active_writer());
    }
}
