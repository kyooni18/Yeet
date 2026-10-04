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
    model::{AgentActivityKind, AgentGroupItem, AgentTaskItem, Usage},
};

use super::{
    AgentLimits,
    budget_ledger::{BudgetLedger, TaskNeed},
    scheduler,
    state::{AgentGroupCheckpoint, GroupRuntimeState, MemberSlot},
    worker::MemberThread,
};

const WAIT_CANCEL_CHECK: Duration = Duration::from_millis(250);

/// Receives the current task list and group projection after every change.
pub(crate) type ChangeListener =
    Arc<dyn Fn(Vec<AgentTaskItem>, AgentGroupItem, AgentGroupCheckpoint) + Send + Sync>;

/// State shared between the runtime, its handles, and member threads.
pub(super) struct GroupShared {
    state: Mutex<GroupRuntimeState>,
    signal: Condvar,
    limits: Mutex<AgentLimits>,
    listener: Mutex<Option<ChangeListener>>,
    coordinator_output_cap: Arc<std::sync::atomic::AtomicU64>,
    coordinator_cancel: Arc<AtomicBool>,
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
        let snapshot = {
            let mut state = self.lock();
            state.checkpoint_revision = state.checkpoint_revision.saturating_add(1);
            listener.as_ref().map(|_| {
                (
                    state.group.task_items(),
                    state.group.group_item(),
                    state.checkpoint(),
                )
            })
        };
        if let (Some(listener), Some((items, group, checkpoint))) = (listener, snapshot) {
            listener(items, group, checkpoint);
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
        let budget = BudgetLedger::new(limits.budget_limits());
        let coordinator_output_cap = Arc::new(std::sync::atomic::AtomicU64::new(
            limits.budget_limits().coordination_output_reserve,
        ));
        Self {
            launcher,
            shared: Arc::new(GroupShared {
                state: Mutex::new(GroupRuntimeState::new(0, None, budget)),
                signal: Condvar::new(),
                limits: Mutex::new(limits),
                listener: Mutex::new(None),
                coordinator_output_cap,
                coordinator_cancel: Arc::new(AtomicBool::new(false)),
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
        {
            let mut state = self.shared.lock();
            state.budget.set_limits(limits.budget_limits());
            state.sync_budget_projection();
        }
        *self
            .shared
            .limits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = limits;
        self.shared.changed();
    }

    pub(crate) fn group_guidance(&self) -> Option<String> {
        self.shared.limits().group_guidance()
    }

    pub(crate) fn restore_checkpoint(&self, mut checkpoint: AgentGroupCheckpoint) -> Result<()> {
        if checkpoint.schema_version != 1 {
            bail!(
                "unsupported Group Agent checkpoint schema version {}",
                checkpoint.schema_version
            );
        }
        {
            let mut state = self.shared.lock();
            let generation = state.group.generation.wrapping_add(1);
            checkpoint.group.generation = generation;
            for member in &mut checkpoint.group.members {
                member.status = MemberStatus::Stopped;
                member.activity_state = "interrupted".into();
                member.current_task = None;
            }
            for task in &mut checkpoint.group.tasks {
                if task.status.is_active() {
                    task.status = AgentTaskStatus::Cancelled;
                    task.outcome = Some("interrupted".into());
                    if task.summary.is_none() {
                        task.summary = Some(
                            "Interrupted when the session was restored; resume from the group checkpoint.".into(),
                        );
                    }
                    checkpoint.budget.complete_task(&task.id.to_string());
                }
            }
            if checkpoint.group.status == "running" {
                checkpoint.group.status = "paused".into();
            }
            checkpoint.group.record_event(
                None,
                None,
                "group_restored",
                "Restored from a durable session checkpoint",
            );
            *state = GroupRuntimeState {
                group: checkpoint.group,
                checkpoint_revision: checkpoint.revision,
                slots: Default::default(),
                task_caps: Default::default(),
                budget: checkpoint.budget,
            };
            state.sync_budget_projection();
        }
        self.shared
            .coordinator_cancel
            .store(false, Ordering::Release);
        let limits = self.shared.limits();
        self.shared.coordinator_output_cap.store(
            limits.budget_limits().coordination_output_reserve,
            Ordering::Release,
        );
        self.shared.changed();
        Ok(())
    }

    pub(crate) fn resume_context(&self) -> String {
        let state = self.shared.lock();
        let mut context = format!(
            "GROUP OBJECTIVE:\n{}",
            state.group.objective.as_deref().unwrap_or_default()
        );
        if let Some(checkpoint) = &state.group.checkpoint_summary {
            context.push_str("\n\nPREVIOUS COORDINATOR CHECKPOINT:\n");
            context.push_str(checkpoint);
        }
        let findings = state.group.shared_findings.iter().collect::<Vec<_>>();
        if !findings.is_empty() {
            context.push_str("\n\nPERSISTED SHARED FINDINGS:\n");
            for finding in findings {
                context.push_str("- ");
                context.push_str(&finding.summary);
                context.push('\n');
            }
        }
        let interrupted = state
            .group
            .tasks
            .iter()
            .filter(|task| task.status == AgentTaskStatus::Cancelled);
        let interrupted = interrupted.collect::<Vec<_>>();
        if !interrupted.is_empty() {
            context.push_str("\n\nINTERRUPTED MEMBER TASKS:\n");
            for task in interrupted {
                context.push_str("- ");
                context.push_str(&task.description);
                if let Some(summary) = &task.summary {
                    context.push_str(": ");
                    context.push_str(summary);
                }
                context.push('\n');
            }
        }
        context.push_str("\nContinue the shared objective, using saved findings and checkpoints. Reassign incomplete work as needed and integrate one coherent group result.");
        context
    }

    #[cfg(test)]
    pub(crate) fn checkpoint(&self) -> AgentGroupCheckpoint {
        self.shared.lock().checkpoint()
    }

    pub(crate) fn has_active_tasks(&self) -> bool {
        self.shared
            .lock()
            .group
            .tasks
            .iter()
            .any(|task| task.status.is_active())
    }

    pub(crate) fn bind_primary_agent(&self, id: AgentId) {
        self.shared.lock().group.primary_agent = Some(id);
    }

    pub(crate) fn create_group(&self, objective: String) -> Result<String> {
        if self.shared.lock().group.status == "running" {
            bail!("cannot replace a running Agent Group; cancel or stop it first");
        }
        self.replace_group();
        self.shared
            .coordinator_cancel
            .store(false, Ordering::Release);
        let group_id = {
            let mut state = self.shared.lock();
            state.group.set_objective(objective);
            state.group.id.to_string()
        };
        self.shared.changed();
        Ok(group_id)
    }

    pub(crate) fn begin_group(&self, group_id: &str) -> Result<()> {
        {
            let mut state = self.shared.lock();
            if state.group.id.to_string() != group_id {
                bail!("unknown Agent Group: {group_id}");
            }
            if !matches!(
                state.group.status.as_str(),
                "created" | "paused" | "completed" | "failed"
            ) {
                bail!(
                    "Agent Group cannot start from status {}",
                    state.group.status
                );
            }
            state.group.status = "running".into();
            state
                .group
                .record_event(None, None, "group_started", "Coordinator started");
            let phase = if state.group.synthesis_started {
                "synthesis"
            } else {
                "coordination"
            };
            self.shared.coordinator_output_cap.store(
                state.budget.remaining_phase_output(phase),
                Ordering::Release,
            );
        }
        self.shared.changed();
        Ok(())
    }

    pub(crate) fn record_coordinator_event(&self, kind: &str, detail: &str) {
        {
            let mut state = self.shared.lock();
            state.group.record_event(None, None, kind, detail);
        }
        self.shared.changed();
    }

    pub(crate) fn record_coordinator_usage(&self, event_key: &str, usage: &Usage) {
        {
            let mut state = self.shared.lock();
            let phase = if state.group.synthesis_started {
                "__synthesis"
            } else {
                "__coordination"
            };
            if !state.budget.account(phase, event_key, usage) {
                return;
            }
            state.group.usage.accumulate(usage);
            state.group.record_event(
                None,
                None,
                "usage",
                &format!(
                    "{event_key}: {} output tokens",
                    usage.output_tokens.unwrap_or(0)
                ),
            );
            state.sync_budget_projection();
            let phase = if state.group.synthesis_started {
                "synthesis"
            } else {
                "coordination"
            };
            self.shared.coordinator_output_cap.store(
                state.budget.remaining_phase_output(phase),
                Ordering::Release,
            );
        }
        self.shared.changed();
    }

    pub(crate) fn begin_synthesis(&self) {
        {
            let mut state = self.shared.lock();
            state.group.synthesis_started = true;
            state.group.record_event(
                None,
                None,
                "synthesis_started",
                "Reserved synthesis budget activated",
            );
            self.shared.coordinator_output_cap.store(
                state.budget.remaining_phase_output("synthesis"),
                Ordering::Release,
            );
        }
        self.shared.changed();
    }

    pub(crate) fn coordinator_output_cap(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.shared.coordinator_output_cap.clone()
    }

    pub(crate) fn coordinator_cost_budget(&self) -> f64 {
        let state = self.shared.lock();
        let phase = if state.group.synthesis_started {
            "synthesis"
        } else {
            "coordination"
        };
        state.budget.remaining_phase_cost(phase)
    }

    pub(crate) fn coordinator_cancel(&self) -> Arc<AtomicBool> {
        self.shared.coordinator_cancel.clone()
    }

    pub(crate) fn complete_group(&self, result: String) {
        {
            let mut state = self.shared.lock();
            state.group.final_result = Some(result.clone());
            state.group.checkpoint_summary = None;
            state.group.status = "completed".into();
            state
                .group
                .record_event(None, None, "group_completed", &result);
        }
        self.shared.changed();
    }

    pub(crate) fn fail_group(&self, error: &str) {
        {
            let mut state = self.shared.lock();
            state.group.status = "failed".into();
            state.group.final_result = Some(error.to_owned());
            state.group.record_event(None, None, "group_failed", error);
        }
        self.shared.changed();
    }

    pub(crate) fn pause_group(&self, checkpoint: &str) {
        {
            let mut state = self.shared.lock();
            state.group.status = "paused".into();
            state.group.final_result = None;
            state.group.checkpoint_summary = Some(checkpoint.to_owned());
            state
                .group
                .record_event(None, None, "group_paused", checkpoint);
        }
        self.shared.changed();
    }

    pub(crate) fn cancel_group(&self) {
        self.shared
            .coordinator_cancel
            .store(true, Ordering::Release);
        self.stop_all();
        {
            let mut state = self.shared.lock();
            state.group.status = "cancelled".into();
            state
                .group
                .record_event(None, None, "group_cancelled", "Group was cancelled");
        }
        self.shared.changed();
    }

    pub(crate) fn stop_group(&self) {
        self.shared
            .coordinator_cancel
            .store(true, Ordering::Release);
        self.stop_all();
        {
            let mut state = self.shared.lock();
            state.group.status = "stopped".into();
            state
                .group
                .record_event(None, None, "group_stopped", "Group stopped by its owner");
        }
        self.shared.changed();
    }

    /// Launches a subordinate member for the active group and queues its first task.
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
        let cost_budget_usd = {
            let state = self.shared.lock();
            state
                .budget
                .prospective_cost_allocation(request.role, request.weight)
        };
        let selected_model = self
            .launcher
            .model_for_budget(model, request.role, cost_budget_usd);
        let context_window_tokens = self
            .launcher
            .context_window_tokens(&selected_model)
            .unwrap_or_default();
        let (generation, spawned_by, retire, scoped_prompt) = {
            let state = self.shared.lock();
            let retire = scheduler::admit_spawn(
                &self.shared.limits(),
                &state.group,
                &state.budget,
                request.role,
                request.weight,
            )?;
            (
                state.group.generation,
                state.group.primary_agent,
                retire,
                state.group.task_context(&request.prompt),
            )
        };
        if let Some(idle) = retire {
            self.retire(idle, "retired to make room for a new agent");
        }
        let (member_id, runner) = self.launcher.launch(&MemberSpec {
            role: request.role,
            description: request.description.clone(),
            model: selected_model.clone(),
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
                description: request.description.clone(),
                role: request.role,
                model: selected_model,
                spawned_by,
                status: MemberStatus::Running,
                activity_state: "queued".into(),
                current_task: None,
                started_at: chrono::Utc::now().to_rfc3339(),
            });
            state
                .group
                .record(None, Some(member_id), kind, None, &request.prompt);
            state.group.record_event(
                Some(member_id),
                Some(task_id),
                "task_assigned",
                &request.description,
            );
            state.group.tasks.push(task);
            state.budget.add_task(TaskNeed {
                task_id: task_id.to_string(),
                role: request.role,
                weight: request.weight,
                context_window_tokens,
            });
            state
                .task_caps
                .insert(task_id, Arc::new(std::sync::atomic::AtomicU64::new(0)));
            state.sync_budget_projection();
            let mut slot = MemberSlot::default();
            slot.inbox.push_back((task_id, scoped_prompt));
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
            scheduler::admit_message(&self.shared.limits(), &state.group, &state.budget, &member)?;
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
            let scoped_prompt = state.group.task_context(&message);
            state.group.record(None, Some(to), kind, None, &message);
            let slot = state
                .slots
                .get_mut(&to)
                .ok_or_else(|| anyhow!("agent {to} is no longer running"))?;
            slot.inbox.push_back((task_id, scoped_prompt));
            state.group.tasks.push(task);
            state.budget.add_task(TaskNeed {
                task_id: task_id.to_string(),
                role: member.role,
                weight: 1.0,
                context_window_tokens: self
                    .launcher
                    .context_window_tokens(&member.model)
                    .unwrap_or_default(),
            });
            state
                .task_caps
                .insert(task_id, Arc::new(std::sync::atomic::AtomicU64::new(0)));
            state.sync_budget_projection();
            if let Some(member) = state.group.member_mut(to) {
                member.status = MemberStatus::Running;
                member.activity_state = "queued".into();
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

    /// Stops every member and starts an empty group generation, keeping the
    /// bound primary agent.
    pub(crate) fn replace_group(&self) {
        self.shared
            .coordinator_cancel
            .store(true, Ordering::Release);
        {
            let mut state = self.shared.lock();
            for slot in state.slots.values() {
                if let Some((_, cancel)) = &slot.running {
                    cancel.store(true, Ordering::Release);
                }
            }
            let generation = state.group.generation.wrapping_add(1);
            let primary_agent = state.group.primary_agent;
            let limits = self.shared.limits();
            let budget = BudgetLedger::new(limits.budget_limits());
            self.shared.coordinator_output_cap.store(
                limits.budget_limits().coordination_output_reserve,
                Ordering::Release,
            );
            *state = GroupRuntimeState::new(generation, primary_agent, budget);
        }
        self.shared.changed();
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
