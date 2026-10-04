//! The live Agent Group: launches members on dedicated threads, routes
//! messages to them, and settles, waits for, and cancels their tasks.
//!
//! Only `AgentGroupSupervisor` constructs or replaces a runtime; the tool
//! layer reaches it through `AgentGroupHandle`. Members outlive the calls and
//! turns that created them until they are stopped, retired, or the group is
//! replaced.

use std::{
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Result, anyhow, bail};

use crate::{
    agents::{
        AgentId,
        member::{AgentMember, MemberLauncher, MemberSpec, MemberStatus},
        task::{AgentTask, AgentTaskId, AgentTaskStatus, SpawnRequest},
    },
    model::{AgentActivityKind, AgentGroupItem, AgentTaskItem},
};

use super::{
    AgentLimits, scheduler,
    snapshot::AgentNotification,
    state::{GroupRuntimeState, MemberSlot},
    worker::MemberThread,
};

const WAIT_CANCEL_CHECK: Duration = Duration::from_millis(250);

/// Receives the current task list and group projection after every change.
pub(crate) type ChangeListener = Arc<dyn Fn(Vec<AgentTaskItem>, AgentGroupItem) + Send + Sync>;

/// State shared between the runtime, its handles, and member threads.
pub(super) struct GroupShared {
    state: Mutex<GroupRuntimeState>,
    signal: Condvar,
    limits: Mutex<AgentLimits>,
    listener: Mutex<Option<ChangeListener>>,
}

impl GroupShared {
    pub(super) fn lock(&self) -> MutexGuard<'_, GroupRuntimeState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn limits(&self) -> AgentLimits {
        self.limits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn wait<'a>(
        &self,
        guard: MutexGuard<'a, GroupRuntimeState>,
        timeout: Duration,
    ) -> MutexGuard<'a, GroupRuntimeState> {
        self.signal
            .wait_timeout(guard, timeout)
            .map(|(guard, _)| guard)
            .unwrap_or_else(|poisoned| poisoned.into_inner().0)
    }

    /// Wakes waiters and tells the frontend listener. Never call while
    /// holding the state lock: the listener reads group state.
    pub(super) fn changed(&self) {
        self.signal.notify_all();
        let listener = self
            .listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(listener) = listener {
            let (items, group) = {
                let state = self.lock();
                (state.group.task_items(), state.group.group_item())
            };
            listener(items, group);
        }
    }
}

#[derive(Clone)]
pub(crate) struct AgentGroupRuntime {
    launcher: Arc<dyn MemberLauncher>,
    shared: Arc<GroupShared>,
    /// Serializes admission with launch so concurrent spawns cannot both
    /// pass the same limit check.
    spawn_lock: Arc<Mutex<()>>,
}

impl AgentGroupRuntime {
    pub(crate) fn new(launcher: Arc<dyn MemberLauncher>, limits: AgentLimits) -> Self {
        Self {
            launcher,
            shared: Arc::new(GroupShared {
                state: Mutex::new(GroupRuntimeState::new(0, None)),
                signal: Condvar::new(),
                limits: Mutex::new(limits),
                listener: Mutex::new(None),
            }),
            spawn_lock: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) fn set_change_listener(&self, listener: ChangeListener) {
        *self
            .shared
            .listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(listener);
    }

    /// Applies new limits to later admissions; running work is not cut short.
    pub(crate) fn set_limits(&self, limits: AgentLimits) {
        *self
            .shared
            .limits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = limits;
        self.shared.changed();
    }

    pub(crate) fn deploy_guidance(&self) -> Option<String> {
        self.shared.limits().deploy_guidance()
    }

    pub(crate) fn bind_primary_agent(&self, id: AgentId) {
        self.shared.lock().group.primary_agent = Some(id);
    }

    /// Launches a member for the primary agent and queues its first task.
    pub(crate) fn spawn(
        &self,
        request: SpawnRequest,
        model: &str,
        active_session_id: Option<String>,
    ) -> Result<(AgentId, AgentTaskId)> {
        self.launch(
            request,
            model,
            active_session_id,
            AgentActivityKind::Message,
        )
    }

    /// Launches a member the user created directly.
    pub(crate) fn spawn_for_user(
        &self,
        request: SpawnRequest,
        model: &str,
        active_session_id: Option<String>,
    ) -> Result<(AgentId, AgentTaskId)> {
        self.launch(request, model, active_session_id, AgentActivityKind::Steer)
    }

    fn launch(
        &self,
        request: SpawnRequest,
        model: &str,
        active_session_id: Option<String>,
        kind: AgentActivityKind,
    ) -> Result<(AgentId, AgentTaskId)> {
        let _admission = self
            .spawn_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (generation, spawned_by, retire) = {
            let state = self.shared.lock();
            let retire = scheduler::admit_spawn(&self.shared.limits(), &state.group, request.role)?;
            (state.group.generation, state.group.primary_agent, retire)
        };
        if let Some(idle) = retire {
            self.retire(idle, "retired to make room for a new agent");
        }
        let (member_id, runner) = self.launcher.launch(&MemberSpec {
            role: request.role,
            description: request.description.clone(),
            model: model.to_owned(),
            spawned_by,
            active_session_id,
        })?;

        let task = AgentTask::queued(
            member_id,
            request.role,
            request.description.clone(),
            request.background,
        );
        let task_id = task.id;
        {
            let mut state = self.shared.lock();
            if state.group.generation != generation {
                bail!("the agent group was replaced while launching this agent");
            }
            state.group.members.push(AgentMember {
                id: member_id,
                description: request.description,
                role: request.role,
                model: model.to_owned(),
                spawned_by,
                status: MemberStatus::Running,
                current_task: None,
                started_at: chrono::Utc::now().to_rfc3339(),
            });
            state
                .group
                .record(None, Some(member_id), kind, None, &request.prompt);
            state.group.tasks.push(task);
            let mut slot = MemberSlot::default();
            slot.inbox.push_back((task_id, request.prompt));
            state.slots.insert(member_id, slot);
        }
        let member_thread = MemberThread {
            shared: self.shared.clone(),
            generation,
            member: member_id,
            runner,
        };
        let spawned = thread::Builder::new()
            .name(format!("agent-{member_id}"))
            .spawn(move || crate::core::run_detached(|| member_thread.run()));
        if let Err(error) = spawned {
            self.retire(member_id, "agent thread could not start");
            return Err(error.into());
        }
        self.shared.changed();
        Ok((member_id, task_id))
    }

    /// Queues a follow-up from the primary agent; it runs in the background.
    pub(crate) fn send(&self, to: AgentId, message: String) -> Result<AgentTaskId> {
        self.queue_message(to, message, AgentActivityKind::Message)
    }

    /// Queues a message the user typed to a member directly.
    pub(crate) fn steer(&self, to: AgentId, message: String) -> Result<AgentTaskId> {
        self.queue_message(to, message, AgentActivityKind::Steer)
    }

    fn queue_message(
        &self,
        to: AgentId,
        message: String,
        kind: AgentActivityKind,
    ) -> Result<AgentTaskId> {
        let task_id = {
            let mut state = self.shared.lock();
            let member = state
                .group
                .member(to)
                .ok_or_else(|| anyhow!("unknown agent: {to}"))?
                .clone();
            scheduler::admit_message(&self.shared.limits(), &state.group, &member)?;
            if !state.slots.contains_key(&to) {
                bail!("agent {to} is no longer running");
            }
            let task = AgentTask::queued(
                to,
                member.role,
                format!("Follow-up: {}", member.description),
                true,
            );
            let task_id = task.id;
            state.group.record(None, Some(to), kind, None, &message);
            let slot = state
                .slots
                .get_mut(&to)
                .ok_or_else(|| anyhow!("agent {to} is no longer running"))?;
            slot.inbox.push_back((task_id, message));
            state.group.tasks.push(task);
            if let Some(member) = state.group.member_mut(to) {
                member.status = MemberStatus::Running;
            }
            task_id
        };
        self.shared.changed();
        Ok(task_id)
    }

    /// Blocks until `task_id` settles. Setting `cancel` cancels the task.
    pub(crate) fn wait(&self, task_id: AgentTaskId, cancel: &AtomicBool) -> Result<AgentTask> {
        let mut state = self.shared.lock();
        loop {
            let task = state.group.task(task_id).ok_or_else(|| {
                anyhow!("agent task {task_id} no longer exists; the agent group was replaced")
            })?;
            if !task.status.is_active() {
                return Ok(task.clone());
            }
            if cancel.load(Ordering::Acquire) {
                drop(state);
                self.cancel_task(task_id);
                bail!("cancelled");
            }
            state = self.shared.wait(state, WAIT_CANCEL_CHECK);
        }
    }

    /// Cancels the member's work and retires it; its context is dropped.
    pub(crate) fn stop(&self, id: AgentId) -> Result<()> {
        match self.shared.lock().group.member(id) {
            None => bail!("unknown agent: {id}"),
            Some(member) if member.status == MemberStatus::Stopped => {
                bail!("agent {id} is already stopped")
            }
            Some(_) => {}
        }
        self.retire(id, "stopped");
        Ok(())
    }

    /// Drops a member and its history from the group, stopping it first.
    pub(crate) fn remove(&self, id: AgentId) -> Result<()> {
        let live = match self.shared.lock().group.member(id) {
            None => bail!("unknown agent: {id}"),
            Some(member) => member.status != MemberStatus::Stopped,
        };
        if live {
            self.retire(id, "removed");
        }
        {
            let mut state = self.shared.lock();
            let id_text = id.to_string();
            state.slots.remove(&id);
            let group = &mut state.group;
            group.members.retain(|member| member.id != id);
            group
                .tasks
                .retain(|task| task.assignee != id || task.status.is_active());
            group.activity.retain(|entry| {
                entry.from.as_deref() != Some(id_text.as_str())
                    && entry.to.as_deref() != Some(id_text.as_str())
            });
        }
        self.shared.changed();
        Ok(())
    }

    /// Removes every stopped member.
    pub(crate) fn remove_stopped(&self) {
        let stopped = self
            .shared
            .lock()
            .group
            .members
            .iter()
            .filter(|member| member.status == MemberStatus::Stopped)
            .map(|member| member.id)
            .collect::<Vec<_>>();
        for id in stopped {
            let _ = self.remove(id);
        }
    }

    /// Stops every live member.
    pub(crate) fn stop_all(&self) {
        let live = self
            .shared
            .lock()
            .group
            .members
            .iter()
            .filter(|member| member.status != MemberStatus::Stopped)
            .map(|member| member.id)
            .collect::<Vec<_>>();
        for id in live {
            self.retire(id, "stopped");
        }
    }

    /// Cancels foreground work left over from an earlier primary turn or
    /// interrupted by the user. Background members keep running.
    pub(crate) fn cancel_foreground(&self) {
        let foreground = self
            .shared
            .lock()
            .group
            .tasks
            .iter()
            .filter(|task| !task.background && task.status.is_active())
            .map(|task| task.id)
            .collect::<Vec<_>>();
        for task_id in foreground {
            self.cancel_task(task_id);
        }
    }

    /// Starts a new budget window for a primary turn.
    pub(crate) fn begin_turn(&self) {
        self.cancel_foreground();
        self.shared.lock().group.window_usage = Default::default();
    }

    /// Stops every member and starts an empty group generation, keeping the
    /// bound primary agent. Undelivered notifications are dropped.
    pub(crate) fn replace_group(&self) {
        {
            let mut state = self.shared.lock();
            for slot in state.slots.values() {
                if let Some((_, cancel)) = &slot.running {
                    cancel.store(true, Ordering::Release);
                }
            }
            let generation = state.group.generation.wrapping_add(1);
            let primary_agent = state.group.primary_agent;
            *state = GroupRuntimeState::new(generation, primary_agent);
        }
        self.shared.changed();
    }

    pub(crate) fn take_notifications(&self) -> Vec<AgentNotification> {
        std::mem::take(&mut self.shared.lock().notifications)
    }

    pub(crate) fn has_notifications(&self) -> bool {
        !self.shared.lock().notifications.is_empty()
    }

    pub(crate) fn task_items(&self) -> Vec<AgentTaskItem> {
        self.shared.lock().group.task_items()
    }

    pub(crate) fn group_item(&self) -> AgentGroupItem {
        self.shared.lock().group.group_item()
    }

    /// Settles one task as cancelled without announcing it.
    fn cancel_task(&self, task_id: AgentTaskId) {
        {
            let mut state = self.shared.lock();
            let Some(assignee) = state.group.task(task_id).map(|task| task.assignee) else {
                return;
            };
            if let Some(slot) = state.slots.get_mut(&assignee) {
                slot.inbox.retain(|(queued, _)| *queued != task_id);
                if let Some((running, cancel)) = &slot.running
                    && *running == task_id
                {
                    cancel.store(true, Ordering::Release);
                }
            }
            settle_cancelled(&mut state, task_id, "cancelled");
        }
        self.shared.changed();
    }

    fn retire(&self, id: AgentId, reason: &str) {
        {
            let mut state = self.shared.lock();
            let mut settled = Vec::new();
            if let Some(slot) = state.slots.get_mut(&id) {
                slot.stop = true;
                settled.extend(slot.inbox.drain(..).map(|(task_id, _)| task_id));
                if let Some((task_id, cancel)) = &slot.running {
                    cancel.store(true, Ordering::Release);
                    settled.push(*task_id);
                }
            }
            for task_id in settled {
                settle_cancelled(&mut state, task_id, reason);
            }
            if let Some(member) = state.group.member_mut(id) {
                member.status = MemberStatus::Stopped;
                member.current_task = None;
                state
                    .group
                    .record(Some(id), None, AgentActivityKind::Stopped, None, reason);
            }
        }
        self.shared.changed();
    }
}

fn settle_cancelled(state: &mut GroupRuntimeState, task_id: AgentTaskId, reason: &str) {
    if let Some(task) = state.group.task_mut(task_id)
        && task.status.is_active()
    {
        task.status = AgentTaskStatus::Cancelled;
        task.outcome = Some("cancelled".into());
        task.summary = Some(reason.into());
    }
}
