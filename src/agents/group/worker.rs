//! The member thread: owns one runner, executes inbox messages one at a
//! time, and promotes results into shared group state.
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

use super::{runtime::GroupShared, state::GroupRuntimeState};

const IDLE_RECHECK: Duration = Duration::from_secs(1);

pub(super) struct MemberThread {
    pub shared: Arc<GroupShared>,
    pub generation: u64,
    pub member: AgentId,
    pub runner: Box<dyn MemberRunner>,
}

impl MemberThread {
    pub(super) fn run(mut self) {
        while let Some((task_id, input, cancel, output_cap)) = self.next_message() {
            self.runner.set_output_cap(output_cap);
            self.shared.changed();
            let shared = self.shared.clone();
            let generation = self.generation;
            let member = self.member;
            let (result, panicked) = match catch_unwind(AssertUnwindSafe(|| {
                self.runner
                    .run(&input, cancel.clone(), &mut |progress| match progress {
                        MemberProgress::Usage { usage, event_key } => {
                            record_usage(&shared, generation, member, task_id, event_key, usage)
                        }
                        MemberProgress::Event { kind, detail } => {
                            record_member_event(&shared, generation, member, task_id, kind, &detail)
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
    fn next_message(
        &self,
    ) -> Option<(
        AgentTaskId,
        String,
        Arc<AtomicBool>,
        Arc<std::sync::atomic::AtomicU64>,
    )> {
        let mut state = self.shared.lock();
        loop {
            if !self.owns(&state) {
                return None;
            }
            let message = {
                let slot = state.slots.get_mut(&self.member)?;
                if slot.stop {
                    return None;
                }
                slot.inbox.pop_front()
            };
            if let Some((task_id, input)) = message {
                let cancel = Arc::new(AtomicBool::new(false));
                let output_cap = state
                    .task_caps
                    .get(&task_id)
                    .cloned()
                    .unwrap_or_else(|| Arc::new(std::sync::atomic::AtomicU64::new(0)));
                if let Some(slot) = state.slots.get_mut(&self.member) {
                    slot.running = Some((task_id, cancel.clone()));
                }
                if let Some(task) = state.group.task_mut(task_id) {
                    task.status = AgentTaskStatus::Running;
                }
                if let Some(member) = state.group.member_mut(self.member) {
                    member.status = MemberStatus::Running;
                    member.activity_state = "reasoning".into();
                    member.current_task = Some(task_id);
                }
                state.group.record_event(
                    Some(self.member),
                    Some(task_id),
                    "task_started",
                    "Member began its assigned task",
                );
                return Some((task_id, input, cancel, output_cap));
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
        if let Some(member) = state.group.member_mut(self.member) {
            member.activity_state = match task.status {
                AgentTaskStatus::Failed => "failed",
                AgentTaskStatus::Cancelled => "cancelled",
                _ if task.outcome.as_deref() == Some("paused") => "waiting_for_input",
                _ if queued == Some(true) => "queued",
                _ => "completed",
            }
            .into();
        }
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
        if matches!(
            task.status,
            AgentTaskStatus::Reported | AgentTaskStatus::Verified | AgentTaskStatus::Done
        ) && let Some(summary) = task.summary.as_deref()
        {
            state.group.promote_finding(self.member, task_id, summary);
        }
        state.group.record_event(
            Some(self.member),
            Some(task_id),
            match task.status {
                AgentTaskStatus::Failed => "task_failed",
                AgentTaskStatus::Cancelled => "task_cancelled",
                _ => "task_completed",
            },
            task.summary.as_deref().unwrap_or(task.status.as_str()),
        );
        state.budget.complete_task(&task_id.to_string());
        state.sync_budget_projection();
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
        let task = state
            .group
            .member(member)
            .and_then(|entry| entry.current_task);
        state
            .group
            .record_event(Some(member), task, "tool_started", text);
    }
    shared.changed();
}

/// Accrues the provider's canonical usage report exactly once.
fn record_usage(
    shared: &GroupShared,
    generation: u64,
    member: AgentId,
    task_id: AgentTaskId,
    event_key: &str,
    usage: &Usage,
) {
    let mut state = shared.lock();
    if state.group.generation != generation {
        return;
    }
    let ledger_key = format!("{member}:{task_id}:{event_key}");
    if !state
        .budget
        .account(&task_id.to_string(), &ledger_key, usage)
    {
        return;
    }
    state.group.usage.accumulate(usage);
    if let Some(task) = state.group.task_mut(task_id) {
        task.usage.accumulate(usage);
    }
    state.sync_budget_projection();
    state.group.record_event(
        Some(member),
        Some(task_id),
        "usage",
        &format!(
            "{event_key}: {} output tokens",
            usage.output_tokens.unwrap_or(0)
        ),
    );
    drop(state);
    shared.changed();
}

fn record_member_event(
    shared: &GroupShared,
    generation: u64,
    member: AgentId,
    task_id: AgentTaskId,
    kind: &str,
    detail: &str,
) {
    let mut state = shared.lock();
    if state.group.generation != generation {
        return;
    }
    if let Some(member_state) = state.group.member_mut(member) {
        member_state.activity_state = match kind {
            "reasoning_started" | "tool_finished" | "tool_failed" | "tool_suppressed" => {
                "reasoning"
            }
            "tool_started" => "tool_call",
            "provider_activity" => "provider_activity",
            _ => kind,
        }
        .into();
    }
    state
        .group
        .record_event(Some(member), Some(task_id), kind, detail);
    drop(state);
    shared.changed();
}
