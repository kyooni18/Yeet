//! Backend-facing owner of Agent Group runtimes.
//!
//! The backend opens a budget window per turn, replaces the group when the
//! session or agent mode changes, hands tools an `AgentGroupHandle`, and
//! reads frontend projections and pending notifications. It never sees
//! scheduler state or member coordinators.

use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::model::{AgentGroupItem, AgentTaskItem};

use super::{
    group::{AgentGroupRuntime, AgentLimits, AgentNotification, ChangeListener},
    handle::AgentGroupHandle,
    member::CoordinatorLauncher,
    runtime::AgentRuntimeFactory,
};

#[derive(Clone)]
pub(crate) struct AgentGroupSupervisor {
    active: AgentGroupRuntime,
}

impl AgentGroupSupervisor {
    pub(crate) fn new(factory: AgentRuntimeFactory, limits: AgentLimits) -> Self {
        Self {
            active: AgentGroupRuntime::new(Arc::new(CoordinatorLauncher::new(factory)), limits),
        }
    }

    /// Called on every group change, from member threads included.
    pub(crate) fn set_change_listener(&self, listener: ChangeListener) {
        self.active.set_change_listener(listener);
    }

    /// Applies swarm limits to the active group and every later one.
    pub(crate) fn set_limits(&self, limits: AgentLimits) {
        self.active.set_limits(limits);
    }

    /// Stops every member and starts an empty group, so a new session or
    /// agent mode never inherits stale agents.
    pub(crate) fn replace_active_group(&self) {
        self.active.replace_group();
    }

    /// Opens a budget window for a new primary turn. Background agents keep
    /// running; foreground agents from an earlier turn are cancelled.
    pub(crate) fn begin_turn(&self) {
        self.active.begin_turn();
    }

    /// Cancels agents the primary is blocked on; background agents survive.
    pub(crate) fn cancel_foreground(&self) {
        self.active.cancel_foreground();
    }

    pub(crate) fn has_notifications(&self) -> bool {
        self.active.has_notifications()
    }

    pub(crate) fn take_notifications(&self) -> Vec<AgentNotification> {
        self.active.take_notifications()
    }

    pub(crate) fn handle(&self) -> AgentGroupHandle {
        AgentGroupHandle::new(self.active.clone())
    }

    pub(crate) fn task_items(&self) -> Vec<AgentTaskItem> {
        self.active.task_items()
    }

    pub(crate) fn group_item(&self) -> AgentGroupItem {
        self.active.group_item()
    }

    /// Queues a message the user typed to one member.
    pub(crate) fn message(&self, agent_id: &str, message: String) -> Result<()> {
        self.active.steer(parse_id(agent_id)?, message).map(|_| ())
    }

    /// Launches a background member the user asked for from a frontend.
    pub(crate) fn spawn_for_user(
        &self,
        role: &str,
        description: String,
        prompt: String,
        model: &str,
        session: Option<String>,
    ) -> Result<()> {
        let request = super::task::SpawnRequest {
            role: super::member::AgentRole::parse(role)?,
            description,
            prompt,
            background: true,
        };
        self.active
            .spawn_for_user(request, model, session)
            .map(|_| ())
    }

    /// Removes one member (stopping it first), or every stopped member when
    /// `agent_id` is `None`.
    pub(crate) fn remove(&self, agent_id: Option<&str>) -> Result<()> {
        match agent_id {
            Some(id) => self.active.remove(parse_id(id)?),
            None => {
                self.active.remove_stopped();
                Ok(())
            }
        }
    }

    /// Stops one member, or every member when `agent_id` is `None`.
    pub(crate) fn stop(&self, agent_id: Option<&str>) -> Result<()> {
        match agent_id {
            Some(id) => self.active.stop(parse_id(id)?),
            None => {
                self.active.stop_all();
                Ok(())
            }
        }
    }
}

fn parse_id(value: &str) -> Result<super::AgentId> {
    value
        .parse()
        .map_err(|_| anyhow!("invalid agent id: {value}"))
}
