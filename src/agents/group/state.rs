//! Agent Group state: serializable membership, tasks, and usage, kept apart
//! from the live runtime resources (inboxes, cancellation flags) that drive
//! member threads.
//!
//! Tasks and members are stored in creation order so frontends render a
//! stable list.

use std::{
    collections::{HashMap, VecDeque},
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

use super::snapshot::AgentNotification;

pub(crate) type AgentGroupId = Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentGroupState {
    pub id: AgentGroupId,
    /// Bumped when the group is replaced; late updates from member threads
    /// of an older generation are ignored.
    pub generation: u64,
    pub primary_agent: Option<AgentId>,
    pub members: Vec<AgentMember>,
    pub tasks: Vec<AgentTask>,
    /// Usage since the group was created.
    pub usage: Usage,
    /// Usage in the current budget window (one primary turn).
    pub window_usage: Usage,
}

impl AgentGroupState {
    pub(crate) fn new(generation: u64, primary_agent: Option<AgentId>) -> Self {
        Self {
            id: AgentGroupId::new_v4(),
            generation,
            primary_agent,
            members: Vec::new(),
            tasks: Vec::new(),
            usage: Usage::default(),
            window_usage: Usage::default(),
        }
    }

    pub(crate) fn member(&self, id: AgentId) -> Option<&AgentMember> {
        self.members.iter().find(|member| member.id == id)
    }

    pub(crate) fn member_mut(&mut self, id: AgentId) -> Option<&mut AgentMember> {
        self.members.iter_mut().find(|member| member.id == id)
    }

    pub(crate) fn task(&self, id: AgentTaskId) -> Option<&AgentTask> {
        self.tasks.iter().find(|task| task.id == id)
    }

    pub(crate) fn task_mut(&mut self, id: AgentTaskId) -> Option<&mut AgentTask> {
        self.tasks.iter_mut().find(|task| task.id == id)
    }

    /// Members running or holding queued work.
    pub(crate) fn busy_member_count(&self) -> usize {
        self.members
            .iter()
            .filter(|member| member.status == MemberStatus::Running)
            .count()
    }

    pub(crate) fn live_member_count(&self) -> usize {
        self.members
            .iter()
            .filter(|member| member.status != MemberStatus::Stopped)
            .count()
    }

    pub(crate) fn has_busy_writer(&self) -> bool {
        self.members
            .iter()
            .any(|member| member.status == MemberStatus::Running && member.role.writes_workspace())
    }
}

/// Runtime-only resources for one member thread.
#[derive(Default)]
pub(super) struct MemberSlot {
    pub inbox: VecDeque<(AgentTaskId, String)>,
    pub running: Option<(AgentTaskId, Arc<AtomicBool>)>,
    pub stop: bool,
}

/// Live group state plus the runtime-only resources of its member threads.
pub(super) struct GroupRuntimeState {
    pub group: AgentGroupState,
    pub slots: HashMap<AgentId, MemberSlot>,
    /// Background completions not yet delivered to the primary agent.
    pub notifications: Vec<AgentNotification>,
}

impl GroupRuntimeState {
    pub(super) fn new(generation: u64, primary_agent: Option<AgentId>) -> Self {
        Self {
            group: AgentGroupState::new(generation, primary_agent),
            slots: HashMap::new(),
            notifications: Vec::new(),
        }
    }
}
