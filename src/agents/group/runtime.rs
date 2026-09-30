//! Executes admitted group tasks on child coordinators.
//!
//! Group lifetime is still scoped to one delegation call: admitted tasks run
//! on scoped threads and the call returns after every child finishes.

use std::{
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{
    agent::{AgentEvent, AgentRunOutcome, AgentRunRequest},
    agents::{
        AgentId,
        member::{AgentMember, AgentRole, MemberStatus, worker_prompt, worker_reasoning_level},
        runtime::{AgentRuntimeFactory, RunManager},
        task::{AgentTaskId, AgentTaskStatus},
    },
    core::{MessageRole, ToolDefinition, Usage},
    model::AgentTaskItem,
};

use super::{
    AgentLimits, commands,
    scheduler::{self, Admission},
    state::{AgentGroupState, GroupRuntimeState},
};

#[derive(Clone)]
pub(crate) struct AdaptiveAgentOrchestrator {
    factory: AgentRuntimeFactory,
    run_manager: RunManager,
    limits: AgentLimits,
    state: Arc<Mutex<GroupRuntimeState>>,
}

impl AdaptiveAgentOrchestrator {
    pub(crate) fn bind_parent_agent(&self, id: AgentId) {
        self.lock_state().group.primary_agent = Some(id);
    }

    pub(crate) fn new(
        factory: AgentRuntimeFactory,
        run_manager: RunManager,
        limits: AgentLimits,
    ) -> Self {
        Self {
            factory,
            run_manager,
            limits,
            state: Arc::new(Mutex::new(GroupRuntimeState::default())),
        }
    }

    pub(crate) fn tool_definition() -> ToolDefinition {
        commands::tool_definition()
    }

    pub(crate) fn begin_run(&self) {
        let mut state = self.lock_state();
        for cancel in state.cancels.values() {
            cancel.store(true, Ordering::Release);
        }
        let generation = state.group.generation.wrapping_add(1);
        let primary_agent = state.group.primary_agent;
        state.group = AgentGroupState::new(generation, primary_agent);
        state.cancels.clear();
    }

    pub(crate) fn execute(
        &self,
        arguments: &Map<String, Value>,
        model: &str,
        active_session_id: Option<String>,
        parent_cancel: &AtomicBool,
    ) -> Result<String> {
        let proposed = commands::parse_proposed_tasks(arguments)?;
        if parent_cancel.load(Ordering::Acquire) {
            bail!("parent run is cancelled");
        }
        let admitted = scheduler::admit(&self.limits, &mut self.lock_state().group, proposed);
        if admitted.is_empty() {
            return Ok(json!({
                "admitted":0,
                "findings":[],
                "reason":"scheduler rejected all proposed tasks under the current concurrency, budget, or write policy"
            })
            .to_string());
        }

        let findings = thread::scope(|scope| {
            let handles = admitted
                .into_iter()
                .map(|admission| {
                    let orchestrator = self.clone();
                    let session_id = active_session_id.clone();
                    scope.spawn(move || orchestrator.run_task(admission, model, session_id))
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err(anyhow!("agent worker panicked")))
                })
                .collect::<Vec<_>>()
        });

        let values = findings
            .into_iter()
            .map(|result| match result {
                Ok(value) => value,
                Err(error) => json!({"status":"failed","error":error.to_string()}),
            })
            .collect::<Vec<_>>();
        Ok(json!({"admitted":values.len(),"findings":values}).to_string())
    }

    pub(crate) fn snapshots(&self) -> Vec<AgentTaskItem> {
        self.lock_state().group.task_items()
    }

    fn run_task(
        &self,
        admission: Admission,
        model: &str,
        active_session_id: Option<String>,
    ) -> Result<Value> {
        let Admission {
            generation,
            task_id,
            task,
        } = admission;
        self.update_task(generation, task_id, AgentTaskStatus::Running, None);
        let run_id = task_id.to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        self.run_manager.register(run_id.clone(), cancel.clone());
        if !self.register_worker_cancel(generation, task_id, cancel.clone()) {
            cancel.store(true, Ordering::Release);
            self.run_manager.remove_matching(&run_id, &cancel);
            bail!("agent worker belongs to a stale primary run");
        }
        let result = (|| -> Result<Value> {
            let mut coordinator = self.factory.build(active_session_id)?;
            let parent_agent = {
                let state = self.lock_state();
                if state.group.generation != generation {
                    bail!("agent worker belongs to a stale primary run");
                }
                state.group.primary_agent
            };
            let child_id =
                coordinator.register_runtime_agent(task.role.as_str(), model, parent_agent)?;
            if let Some(parent) = parent_agent {
                crate::agents::global().connect(parent, child_id)?;
            }
            self.with_group(generation, |group| {
                group.assign(
                    task_id,
                    AgentMember {
                        id: child_id,
                        role: task.role,
                        model: model.to_owned(),
                        parent: parent_agent,
                        status: MemberStatus::Running,
                        current_task: None,
                    },
                )
            });
            let reasoning_level = worker_reasoning_level(model);
            let goal_mode = Arc::new(AtomicBool::new(true));
            let prompt = worker_prompt(task.role, &task.objective);
            let local_usage = Arc::new(Mutex::new(Usage::default()));
            let usage_for_events = local_usage.clone();
            let budget_cancel = cancel.clone();
            let orchestrator = self.clone();
            let outcome = coordinator.run(
                AgentRunRequest {
                    input: &prompt,
                    images: Vec::new(),
                    model,
                    reasoning_level,
                    attached_capabilities: None,
                    disabled_capabilities: task.role.disabled_capabilities(),
                    goal_mode,
                    cancel: cancel.clone(),
                    continuation: false,
                },
                |event| {
                    let usage = match event {
                        AgentEvent::ModelAttemptFinished(_, Some(usage)) => Some(usage),
                        AgentEvent::AuxiliaryUsage { usage, .. } => Some(usage),
                        _ => None,
                    };
                    if let Some(usage) = usage {
                        usage_for_events
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .accumulate(&usage);
                        if orchestrator.record_usage_and_budget_exhausted(generation, &usage) {
                            budget_cancel.store(true, Ordering::Release);
                        }
                    }
                },
            )?;
            let summary = coordinator
                .model_history()
                .iter()
                .rev()
                .find(|message| {
                    message.role == MessageRole::Assistant
                        && message
                            .content
                            .as_deref()
                            .is_some_and(|text| !text.trim().is_empty())
                })
                .and_then(|message| message.content.clone())
                .unwrap_or_default();
            let usage = usage_for_events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let (status, outcome_name) = match outcome {
                AgentRunOutcome::Completed => {
                    let status = if task.role == AgentRole::Verifier {
                        AgentTaskStatus::Verified
                    } else if task.role == AgentRole::Implementer {
                        AgentTaskStatus::NeedsVerification
                    } else {
                        AgentTaskStatus::Reported
                    };
                    (status, "completed")
                }
                AgentRunOutcome::CompletedUnverified { .. } => {
                    (AgentTaskStatus::NeedsVerification, "completed_unverified")
                }
                AgentRunOutcome::GoalPaused { .. } | AgentRunOutcome::AutonomousIdle { .. } => {
                    (AgentTaskStatus::Reported, "paused")
                }
            };
            self.update_task(generation, task_id, status, Some(summary.clone()));
            Ok(json!({
                "taskId":task_id,
                "role":task.role.as_str(),
                "status":status.as_str(),
                "outcome":outcome_name,
                "summary":summary,
                "usage":usage,
            }))
        })();
        self.run_manager.remove_matching(&run_id, &cancel);
        self.finish_worker(generation, task_id);
        if let Err(error) = &result {
            let status = if cancel.load(Ordering::Acquire) {
                AgentTaskStatus::Cancelled
            } else {
                AgentTaskStatus::Failed
            };
            self.update_task(generation, task_id, status, Some(error.to_string()));
        }
        result
    }

    fn register_worker_cancel(
        &self,
        generation: u64,
        id: AgentTaskId,
        cancel: Arc<AtomicBool>,
    ) -> bool {
        let mut state = self.lock_state();
        if state.group.generation != generation {
            return false;
        }
        state.cancels.insert(id, cancel);
        true
    }

    /// Detaches a finished worker. Callers set the task's terminal status.
    fn finish_worker(&self, generation: u64, id: AgentTaskId) {
        let mut state = self.lock_state();
        if state.group.generation == generation {
            state.cancels.remove(&id);
            state.group.release_task(id);
        }
    }

    fn record_usage_and_budget_exhausted(&self, generation: u64, usage: &Usage) -> bool {
        let mut state = self.lock_state();
        if state.group.generation != generation {
            return true;
        }
        state.group.usage.accumulate(usage);
        let exhausted = self.limits.budget_exhausted(&state.group.usage);
        if exhausted {
            for cancel in state.cancels.values() {
                cancel.store(true, Ordering::Release);
            }
        }
        exhausted
    }

    fn update_task(
        &self,
        generation: u64,
        id: AgentTaskId,
        status: AgentTaskStatus,
        summary: Option<String>,
    ) {
        self.with_group(generation, |group| {
            if let Some(task) = group.task_mut(id) {
                task.status = status;
                if summary.is_some() {
                    task.summary = summary;
                }
            }
        });
    }

    /// Applies `update` only while `generation` is still the live group.
    fn with_group(&self, generation: u64, update: impl FnOnce(&mut AgentGroupState)) {
        let mut state = self.lock_state();
        if state.group.generation == generation {
            update(&mut state.group);
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, GroupRuntimeState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
