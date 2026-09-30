//! Runtime ownership primitives shared by single- and multi-agent execution.
//!
//! The backend currently admits one top-level run at a time, but cancellation
//! and lifecycle ownership are keyed by run ID so additional worker runs can be
//! introduced without returning to a single global cancellation slot.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    agent::{AgentCoordinator, AgentEvent, AgentRunOutcome, AgentRunRequest},
    core::{MessageRole, ToolDefinition, Usage},
    model::AgentTaskItem,
    permission::PermissionBroker,
    project_settings::ProjectSettingsStore,
    session_store::SessionStore,
    tools::{BridgeHandle, ToolRegistry},
    workers::WorkerRegistry,
};

#[derive(Clone)]
pub(crate) struct AgentRuntimeFactory {
    bridge: BridgeHandle,
    workspace_root: PathBuf,
    workers: WorkerRegistry,
    permission: PermissionBroker,
    project_settings: ProjectSettingsStore,
    project_identity: String,
    store: SessionStore,
}

impl AgentRuntimeFactory {
    pub(crate) fn new(
        bridge: BridgeHandle,
        workspace_root: PathBuf,
        workers: WorkerRegistry,
        permission: PermissionBroker,
        project_settings: ProjectSettingsStore,
        project_identity: String,
        store: SessionStore,
    ) -> Self {
        Self {
            bridge,
            workspace_root,
            workers,
            permission,
            project_settings,
            project_identity,
            store,
        }
    }

    /// Builds a fresh coordinator with independent history and tool state.
    ///
    /// Project-backed service settings are loaded for each build so workers
    /// created later do not inherit stale Foundation/Web configuration.
    pub(crate) fn build(&self, active_session_id: Option<String>) -> Result<AgentCoordinator> {
        let project = self.project_settings.load()?;
        self.bridge.set_openai_flex(project.openai_flex);

        let mut registry = ToolRegistry::new_with_bridge_handle(
            self.bridge.clone(),
            self.workspace_root.clone(),
            self.workers.clone(),
            self.permission.clone(),
        )?;
        registry.configure_foundation_memory(
            project.foundation_memory.enabled,
            project.foundation_memory.backend,
            project.foundation_memory.server,
            self.project_identity.clone(),
        );
        registry.configure_web_backend(project.web.backend, project.web.server);

        let mut coordinator = AgentCoordinator::new(self.bridge.clone(), registry);
        if let Some(session_id) = active_session_id.as_ref() {
            coordinator.set_protected_write_paths([self.store.directory.join(session_id)]);
        }
        coordinator.set_session_runtime(self.store.clone(), active_session_id);
        Ok(coordinator)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentRole {
    Researcher,
    Implementer,
    Verifier,
}

impl AgentRole {
    fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "researcher" | "research" => Ok(Self::Researcher),
            "implementer" | "implementation" | "writer" => Ok(Self::Implementer),
            "verifier" | "verification" | "verify" => Ok(Self::Verifier),
            other => bail!("unknown agent role: {other}"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Researcher => "researcher",
            Self::Implementer => "implementer",
            Self::Verifier => "verifier",
        }
    }

    fn disabled_capabilities(self) -> Vec<String> {
        match self {
            Self::Researcher => vec![
                "builtin:file-write".into(),
                "builtin:shell".into(),
                "builtin:computer-use".into(),
            ],
            Self::Implementer => Vec::new(),
            Self::Verifier => vec!["builtin:file-write".into(), "builtin:computer-use".into()],
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentTaskStatus {
    Pending,
    Running,
    Reported,
    NeedsVerification,
    Verified,
    Done,
    Failed,
    Cancelled,
}

impl AgentTaskStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Reported => "reported",
            Self::NeedsVerification => "needs_verification",
            Self::Verified => "verified",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WritePolicy {
    PrimaryOnly,
    SingleWriter,
    IsolatedWorktree,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentTaskSnapshot {
    pub id: String,
    pub role: AgentRole,
    pub objective: String,
    pub status: AgentTaskStatus,
    pub summary: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct AgentLimits {
    pub max_concurrent: usize,
    pub max_total: usize,
    pub max_tokens: u64,
    pub max_cost_usd: f64,
    pub write_policy: WritePolicy,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            max_total: 8,
            max_tokens: 200_000,
            max_cost_usd: 2.0,
            write_policy: WritePolicy::SingleWriter,
        }
    }
}

#[derive(Clone)]
pub(crate) struct AdaptiveAgentOrchestrator {
    factory: AgentRuntimeFactory,
    run_manager: RunManager,
    limits: AgentLimits,
    state: Arc<Mutex<AdaptiveAgentState>>,
}

#[derive(Default)]
struct AdaptiveAgentState {
    generation: u64,
    parent_agent: Option<crate::agents::AgentId>,
    total_started: usize,
    active: HashSet<String>,
    worker_cancels: HashMap<String, Arc<AtomicBool>>,
    tasks: Vec<AgentTaskSnapshot>,
    usage: Usage,
}

#[derive(Clone)]
struct ProposedTask {
    role: AgentRole,
    objective: String,
}

impl AdaptiveAgentOrchestrator {
    pub(crate) fn bind_parent_agent(&self, id: crate::agents::AgentId) {
        self.lock_state().parent_agent = Some(id);
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
            state: Arc::new(Mutex::new(AdaptiveAgentState::default())),
        }
    }

    pub(crate) fn tool_definition() -> ToolDefinition {
        ToolDefinition::new(
            "propose_agent_tasks",
            "Propose 1-4 bounded, independent tasks that could benefit from parallel workers. The runtime scheduler decides admission, enforces concurrency/budget/write policy, and returns structured findings. Use researcher for read-only inspection/research, implementer for a bounded code change, and verifier for tests or independent validation. Workers cannot spawn workers.",
            json!({
                "type":"object",
                "properties":{
                    "tasks":{
                        "type":"array",
                        "minItems":1,
                        "maxItems":4,
                        "items":{
                            "type":"object",
                            "properties":{
                                "role":{"type":"string","enum":["researcher","implementer","verifier"]},
                                "task":{"type":"string","minLength":1,"maxLength":12000}
                            },
                            "required":["role","task"],
                            "additionalProperties":false
                        }
                    }
                },
                "required":["tasks"],
                "additionalProperties":false
            }),
        )
    }

    pub(crate) fn begin_run(&self) {
        let mut state = self.lock_state();
        for cancel in state.worker_cancels.values() {
            cancel.store(true, Ordering::Release);
        }
        state.generation = state.generation.wrapping_add(1);
        state.total_started = 0;
        state.active.clear();
        state.worker_cancels.clear();
        state.tasks.clear();
        state.usage = Usage::default();
    }

    pub(crate) fn execute(
        &self,
        arguments: &Map<String, Value>,
        model: &str,
        active_session_id: Option<String>,
        parent_cancel: &AtomicBool,
    ) -> Result<String> {
        let proposed = parse_proposed_tasks(arguments)?;
        if parent_cancel.load(Ordering::Acquire) {
            bail!("parent run is cancelled");
        }
        let admitted = self.admit(proposed)?;
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
                .map(|(generation, task_id, task)| {
                    let orchestrator = self.clone();
                    let session_id = active_session_id.clone();
                    scope.spawn(move || {
                        orchestrator.run_task(generation, task_id, task, model, session_id)
                    })
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
        self.lock_state()
            .tasks
            .iter()
            .map(|task| AgentTaskItem {
                id: task.id.clone(),
                role: task.role.as_str().into(),
                objective: task.objective.clone(),
                status: task.status.as_str().into(),
                summary: task.summary.clone(),
            })
            .collect()
    }

    fn admit(&self, proposed: Vec<ProposedTask>) -> Result<Vec<(u64, String, ProposedTask)>> {
        let mut state = self.lock_state();
        let generation = state.generation;
        let used_tokens = usage_tokens(&state.usage);
        let used_cost = state.usage.estimated_cost_usd.unwrap_or(0.0);
        if used_tokens >= self.limits.max_tokens || used_cost >= self.limits.max_cost_usd {
            return Ok(Vec::new());
        }

        let remaining_total = self.limits.max_total.saturating_sub(state.total_started);
        let remaining_concurrent = self
            .limits
            .max_concurrent
            .saturating_sub(state.active.len());
        let limit = proposed
            .len()
            .min(remaining_total)
            .min(remaining_concurrent);
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut writer_admitted = state
            .tasks
            .iter()
            .any(|task| task.role == AgentRole::Implementer && state.active.contains(&task.id));
        let mut admitted = Vec::new();
        for task in proposed.into_iter().take(limit) {
            if task.role == AgentRole::Implementer {
                match self.limits.write_policy {
                    WritePolicy::PrimaryOnly => continue,
                    WritePolicy::SingleWriter if writer_admitted => continue,
                    WritePolicy::SingleWriter => writer_admitted = true,
                    WritePolicy::IsolatedWorktree => {
                        // Worktree isolation is a separate rollout stage. Fail closed
                        // instead of silently sharing a writable checkout.
                        continue;
                    }
                }
            }
            let id = Uuid::new_v4().to_string();
            state.total_started += 1;
            state.active.insert(id.clone());
            state.tasks.push(AgentTaskSnapshot {
                id: id.clone(),
                role: task.role,
                objective: task.objective.clone(),
                status: AgentTaskStatus::Pending,
                summary: None,
            });
            admitted.push((generation, id, task));
        }
        Ok(admitted)
    }

    fn run_task(
        &self,
        generation: u64,
        task_id: String,
        task: ProposedTask,
        model: &str,
        active_session_id: Option<String>,
    ) -> Result<Value> {
        self.update_task(generation, &task_id, AgentTaskStatus::Running, None);
        let cancel = Arc::new(AtomicBool::new(false));
        self.run_manager.register(task_id.clone(), cancel.clone());
        if !self.register_worker_cancel(generation, &task_id, cancel.clone()) {
            cancel.store(true, Ordering::Release);
            self.run_manager.remove_matching(&task_id, &cancel);
            bail!("agent worker belongs to a stale primary run");
        }
        let result = (|| -> Result<Value> {
            let mut coordinator = self.factory.build(active_session_id)?;
            let parent_agent = {
                let state = self.lock_state();
                if state.generation != generation {
                    bail!("agent worker belongs to a stale primary run");
                }
                state.parent_agent
            };
            let child_id =
                coordinator.register_runtime_agent(task.role.as_str(), model, parent_agent)?;
            if let Some(parent) = parent_agent {
                crate::agents::global().connect(parent, child_id)?;
            }
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
            self.update_task(generation, &task_id, status, Some(summary.clone()));
            Ok(json!({
                "taskId":task_id,
                "role":task.role.as_str(),
                "status":status.as_str(),
                "outcome":outcome_name,
                "summary":summary,
                "usage":usage,
            }))
        })();
        self.run_manager.remove_matching(&task_id, &cancel);
        self.finish_worker(generation, &task_id);
        if let Err(error) = &result {
            let status = if cancel.load(Ordering::Acquire) {
                AgentTaskStatus::Cancelled
            } else {
                AgentTaskStatus::Failed
            };
            self.update_task(generation, &task_id, status, Some(error.to_string()));
        }
        result
    }

    fn register_worker_cancel(&self, generation: u64, id: &str, cancel: Arc<AtomicBool>) -> bool {
        let mut state = self.lock_state();
        if state.generation != generation {
            return false;
        }
        state.worker_cancels.insert(id.to_owned(), cancel);
        true
    }

    fn finish_worker(&self, generation: u64, id: &str) {
        let mut state = self.lock_state();
        if state.generation == generation {
            state.active.remove(id);
            state.worker_cancels.remove(id);
        }
    }

    fn record_usage_and_budget_exhausted(&self, generation: u64, usage: &Usage) -> bool {
        let mut state = self.lock_state();
        if state.generation != generation {
            return true;
        }
        state.usage.accumulate(usage);
        let exhausted = usage_tokens(&state.usage) >= self.limits.max_tokens
            || state.usage.estimated_cost_usd.unwrap_or(0.0) >= self.limits.max_cost_usd;
        if exhausted {
            for cancel in state.worker_cancels.values() {
                cancel.store(true, Ordering::Release);
            }
        }
        exhausted
    }

    fn update_task(
        &self,
        generation: u64,
        id: &str,
        status: AgentTaskStatus,
        summary: Option<String>,
    ) {
        let mut state = self.lock_state();
        if state.generation != generation {
            return;
        }
        if let Some(task) = state.tasks.iter_mut().find(|task| task.id == id) {
            task.status = status;
            if summary.is_some() {
                task.summary = summary;
            }
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, AdaptiveAgentState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn parse_proposed_tasks(arguments: &Map<String, Value>) -> Result<Vec<ProposedTask>> {
    let tasks = arguments
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("tasks must be an array"))?;
    if tasks.is_empty() || tasks.len() > 4 {
        bail!("tasks must contain between 1 and 4 items");
    }
    tasks
        .iter()
        .map(|value| {
            let object = value
                .as_object()
                .ok_or_else(|| anyhow!("each task must be an object"))?;
            let role = AgentRole::parse(
                object
                    .get("role")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("task role is required"))?,
            )?;
            let objective = object
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned();
            if objective.is_empty() {
                bail!("task objective must not be empty");
            }
            Ok(ProposedTask { role, objective })
        })
        .collect()
}

fn worker_prompt(role: AgentRole, objective: &str) -> String {
    format!(
        "Role: {} worker. Complete this bounded delegated task and return concise findings with concrete evidence for the primary agent.\n\nTask:\n{}",
        role.as_str(),
        objective.trim()
    )
}

fn worker_reasoning_level(model: &str) -> &'static str {
    let lower = model.to_ascii_lowercase();
    if lower.contains("luna") {
        "low"
    } else {
        "medium"
    }
}

fn usage_tokens(usage: &Usage) -> u64 {
    usage.total_tokens.unwrap_or_else(|| {
        usage
            .input_tokens
            .unwrap_or(0)
            .saturating_add(usage.output_tokens.unwrap_or(0))
    })
}

#[derive(Clone, Default)]
pub(crate) struct RunManager {
    inner: Arc<Mutex<RunManagerState>>,
}

#[derive(Default)]
struct RunManagerState {
    active: HashMap<String, Arc<AtomicBool>>,
}

impl RunManager {
    pub(crate) fn register(&self, run_id: impl Into<String>, cancel: Arc<AtomicBool>) {
        self.lock().active.insert(run_id.into(), cancel);
    }

    pub(crate) fn remove_matching(&self, run_id: &str, completed: &Arc<AtomicBool>) {
        let mut state = self.lock();
        if state
            .active
            .get(run_id)
            .is_some_and(|current| Arc::ptr_eq(current, completed))
        {
            state.active.remove(run_id);
        }
    }

    pub(crate) fn cancel_all(&self) -> usize {
        let state = self.lock();
        for cancel in state.active.values() {
            cancel.store(true, Ordering::Release);
        }
        state.active.len()
    }

    /// Detaches all runtime ownership without changing cancellation flags.
    ///
    /// Session replacement historically invalidates the live transcript first
    /// and lets the old worker observe that its run ID no longer owns the
    /// session. Preserve that behavior while making the ownership registry
    /// capable of holding multiple runs.
    pub(crate) fn detach_all(&self) {
        self.lock().active.clear();
    }

    fn lock(&self) -> MutexGuard<'_, RunManagerState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_all_marks_every_registered_run() {
        let manager = RunManager::default();
        let first = Arc::new(AtomicBool::new(false));
        let second = Arc::new(AtomicBool::new(false));
        manager.register("first", first.clone());
        manager.register("second", second.clone());

        assert_eq!(manager.cancel_all(), 2);
        assert!(first.load(Ordering::Acquire));
        assert!(second.load(Ordering::Acquire));
    }

    #[test]
    fn late_completion_cannot_remove_replacement_run() {
        let manager = RunManager::default();
        let original = Arc::new(AtomicBool::new(false));
        let replacement = Arc::new(AtomicBool::new(false));
        manager.register("run", original.clone());
        manager.register("run", replacement.clone());

        manager.remove_matching("run", &original);
        assert_eq!(manager.cancel_all(), 1);
        assert!(replacement.load(Ordering::Acquire));
        assert!(!original.load(Ordering::Acquire));
    }

    #[test]
    fn parses_bounded_agent_task_proposals() {
        let arguments = json!({
            "tasks": [
                {"role":"researcher","task":"  inspect the parser  "},
                {"role":"verifier","task":"run focused tests"}
            ]
        });
        let tasks = parse_proposed_tasks(arguments.as_object().unwrap()).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].role, AgentRole::Researcher);
        assert_eq!(tasks[0].objective, "inspect the parser");
        assert_eq!(tasks[1].role, AgentRole::Verifier);
    }

    #[test]
    fn rejects_recursive_scale_agent_proposals() {
        let arguments = json!({
            "tasks": [
                {"role":"researcher","task":"a"},
                {"role":"researcher","task":"b"},
                {"role":"researcher","task":"c"},
                {"role":"researcher","task":"d"},
                {"role":"researcher","task":"e"}
            ]
        });
        assert!(parse_proposed_tasks(arguments.as_object().unwrap()).is_err());
    }

    #[test]
    fn worker_reasoning_defaults_are_cost_aware() {
        assert_eq!(worker_reasoning_level("openai/gpt-5.6-sol"), "medium");
        assert_eq!(worker_reasoning_level("openai/gpt-5.6-luna"), "low");
    }

    #[test]
    fn task_status_uses_stable_wire_names() {
        assert_eq!(
            AgentTaskStatus::NeedsVerification.as_str(),
            "needs_verification"
        );
        assert_eq!(AgentTaskStatus::Verified.as_str(), "verified");
    }
}
