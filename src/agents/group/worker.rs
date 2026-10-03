//! The member thread: owns one runner, executes inbox messages one at a
//! time, records results, and queues notifications for background work.
//!
//! The thread idles between messages so the member keeps its context, and
//! exits when the member is stopped or its group generation is replaced.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::anyhow;

use crate::{
    agents::{
        AgentId,
        member::{AgentRole, MemberProgress, MemberRunner, MemberStatus, RunOutcome, RunReport},
        task::{AgentTaskId, AgentTaskStatus},
    },
    core::Usage,
    model::AgentActivityKind,
};

use super::{runtime::GroupShared, snapshot::notification, state::GroupRuntimeState};

const IDLE_RECHECK: Duration = Duration::from_secs(1);

pub(super) struct MemberThread {
    pub shared: Arc<GroupShared>,
    pub generation: u64,
    pub member: AgentId,
    pub runner: Box<dyn MemberRunner>,
}

impl MemberThread {
    pub(super) fn run(mut self) {
        while let Some((task_id, input, cancel)) = self.next_message() {
            self.shared.changed();
            let shared = self.shared.clone();
            let generation = self.generation;
            let member = self.member;
            let (result, panicked) = match catch_unwind(AssertUnwindSafe(|| {
                self.runner
                    .run(&input, cancel.clone(), &mut |progress| match progress {
                        MemberProgress::Usage(usage) => {
                            record_usage(&shared, generation, task_id, usage)
                        }
                        MemberProgress::Tool { name, detail } => {
                            record_tool(&shared, generation, member, name, &detail)
                        }
                    })
            })) {
                Ok(result) => (result, false),
                // A runner that unwound may hold broken state: retire it.
                Err(_) => (Err(anyhow!("agent worker panicked")), true),
            };
            if !self.finish(task_id, &cancel, result, panicked) {
                return;
            }
            self.shared.changed();
            if panicked {
                return;
            }
        }
    }

    /// Blocks until a message is queued. Returns `None` when this thread
    /// should exit.
    fn next_message(&self) -> Option<(AgentTaskId, String, Arc<AtomicBool>)> {
        let mut state = self.shared.lock();
        loop {
            if !self.owns(&state) {
                return None;
            }
            let slot = state.slots.get_mut(&self.member)?;
            if slot.stop {
                return None;
            }
            if let Some((task_id, input)) = slot.inbox.pop_front() {
                let cancel = Arc::new(AtomicBool::new(false));
                slot.running = Some((task_id, cancel.clone()));
                if let Some(task) = state.group.task_mut(task_id) {
                    task.status = AgentTaskStatus::Running;
                }
                if let Some(member) = state.group.member_mut(self.member) {
                    member.status = MemberStatus::Running;
                    member.current_task = Some(task_id);
                }
                return Some((task_id, input, cancel));
            }
            state = self.shared.wait(state, IDLE_RECHECK);
        }
    }

    /// Records a run result. Returns false when the thread must exit.
    fn finish(
        &self,
        task_id: AgentTaskId,
        cancel: &AtomicBool,
        result: anyhow::Result<RunReport>,
        panicked: bool,
    ) -> bool {
        let mut state = self.shared.lock();
        if !self.owns(&state) {
            return false;
        }
        let queued = state.slots.get_mut(&self.member).map(|slot| {
            slot.running = None;
            !slot.inbox.is_empty() && !slot.stop
        });
        let Some(member) = state.group.member_mut(self.member) else {
            return false;
        };
        let role = member.role;
        member.current_task = None;
        if member.status != MemberStatus::Stopped {
            member.status = if panicked {
                MemberStatus::Stopped
            } else if queued == Some(true) {
                MemberStatus::Running
            } else {
                MemberStatus::Idle
            };
        }
        let Some(task) = state.group.task_mut(task_id) else {
            return true;
        };
        // Stop and cancelled waits settle the task themselves; their late
        // results are dropped and never announced.
        if task.status != AgentTaskStatus::Running {
            return true;
        }
        match result {
            Ok(report) => {
                task.status = completed_status(role, report.outcome);
                task.outcome = Some(report.outcome.as_str().into());
                task.summary = Some(report.summary);
            }
            Err(error) => {
                let cancelled = cancel.load(Ordering::Acquire);
                task.status = if cancelled {
                    AgentTaskStatus::Cancelled
                } else {
                    AgentTaskStatus::Failed
                };
                task.outcome = Some(task.status.as_str().into());
                task.summary = Some(if cancelled {
                    "cancelled (budget exhausted or interrupted)".into()
                } else {
                    error.to_string()
                });
            }
        }
        let task = task.clone();
        let kind = if matches!(
            task.status,
            AgentTaskStatus::Failed | AgentTaskStatus::Cancelled
        ) {
            AgentActivityKind::Failed
        } else {
            AgentActivityKind::Finished
        };
        state.group.record(
            Some(self.member),
            None,
            kind,
            None,
            task.summary.as_deref().unwrap_or(task.status.as_str()),
        );
        if task.background {
            if let Some(member) = state.group.member(self.member) {
                let notice = notification(member, &task);
                state.notifications.push(notice);
            }
        }
        true
    }

    fn owns(&self, state: &GroupRuntimeState) -> bool {
        state.group.generation == self.generation
    }
}

fn completed_status(role: AgentRole, outcome: RunOutcome) -> AgentTaskStatus {
    match (outcome, role) {
        (RunOutcome::Completed, AgentRole::Verifier) => AgentTaskStatus::Verified,
        (RunOutcome::Completed, AgentRole::Implementer) => AgentTaskStatus::NeedsVerification,
        (RunOutcome::Completed, AgentRole::Researcher) => AgentTaskStatus::Reported,
        (RunOutcome::CompletedUnverified, _) => AgentTaskStatus::NeedsVerification,
        (RunOutcome::Paused, _) => AgentTaskStatus::Reported,
    }
}

fn record_tool(shared: &GroupShared, generation: u64, member: AgentId, name: &str, detail: &str) {
    {
        let mut state = shared.lock();
        if state.group.generation != generation {
            return;
        }
        let text = if detail.is_empty() { name } else { detail };
        state.group.record(
            Some(member),
            None,
            AgentActivityKind::Tool,
            Some(name.to_owned()),
            text,
        );
    }
    shared.changed();
}

/// Accrues usage and cancels every running member once the window budget
/// is spent.
fn record_usage(shared: &GroupShared, generation: u64, task_id: AgentTaskId, usage: &Usage) {
    let mut state = shared.lock();
    if state.group.generation != generation {
        return;
    }
    state.group.usage.accumulate(usage);
    state.group.window_usage.accumulate(usage);
    if let Some(task) = state.group.task_mut(task_id) {
        task.usage.accumulate(usage);
    }
    if shared.limits().budget_exhausted(&state.group.window_usage) {
        for slot in state.slots.values() {
            if let Some((_, cancel)) = &slot.running {
                cancel.store(true, Ordering::Release);
            }
        }
    }
}
