//! Narrow access to either Group Agent lifecycle controls or coordinator-only
//! member delegation. The Main Agent receives lifecycle controls; members
//! never receive a handle that can create another group.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{
    agent::{AgentCoordinator, AgentEvent, AgentRunOutcome, AgentRunRequest},
    agents::{runtime::AgentRuntimeFactory, task::AgentTaskId},
    core::{ToolCall, ToolDefinition},
};

use super::{
    AgentId,
    group::{
        AgentGroupRuntime,
        commands::{self, AGENT_TOOL, LEGACY_PROPOSE_TOOL, TOOL_NAMES},
        control_commands, task_result,
    },
};

type Launch = std::result::Result<(AgentId, AgentTaskId), String>;

#[derive(Default)]
struct GroupCoordinatorSlot {
    group_id: Option<String>,
    coordinator: Option<AgentCoordinator>,
    run_sequence: u64,
}

#[derive(Clone)]
enum Surface {
    Main {
        factory: AgentRuntimeFactory,
        coordinator: Arc<Mutex<GroupCoordinatorSlot>>,
    },
    Members,
}

#[derive(Clone)]
pub(crate) struct AgentGroupHandle {
    runtime: AgentGroupRuntime,
    surface: Surface,
    /// Member tasks from one model response are started before waiting on any
    /// individual result, so independent work genuinely overlaps.
    prestarted: Arc<Mutex<HashMap<String, Launch>>>,
}

impl AgentGroupHandle {
    pub(super) fn new(runtime: AgentGroupRuntime, factory: AgentRuntimeFactory) -> Self {
        Self {
            runtime,
            surface: Surface::Main {
                factory,
                coordinator: Arc::new(Mutex::new(GroupCoordinatorSlot::default())),
            },
            prestarted: Arc::default(),
        }
    }

    pub(crate) fn members(runtime: AgentGroupRuntime) -> Self {
        Self {
            runtime,
            surface: Surface::Members,
            prestarted: Arc::default(),
        }
    }

    pub(crate) fn tool_definitions(&self) -> Vec<ToolDefinition> {
        match self.surface {
            Surface::Main { .. } => control_commands::tool_definitions(),
            Surface::Members => commands::tool_definitions(),
        }
    }

    pub(crate) fn tool_names(&self) -> &'static [&'static str] {
        match self.surface {
            Surface::Main { .. } => &control_commands::TOOL_NAMES,
            Surface::Members => &TOOL_NAMES,
        }
    }

    /// Guidance for the Main Agent when group delegation is enabled.
    pub(crate) fn group_guidance(&self) -> Option<String> {
        match self.surface {
            Surface::Main { .. } => self.runtime.group_guidance(),
            Surface::Members => None,
        }
    }

    pub(crate) fn bind_primary_agent(&self, id: AgentId) {
        self.runtime.bind_primary_agent(id);
    }

    pub(crate) fn reset_group_context(&self) {
        if let Surface::Main { coordinator, .. } = &self.surface {
            let mut state = lock_slot(coordinator);
            state.group_id = None;
            state.coordinator = None;
            state.run_sequence = 0;
        }
    }

    pub(crate) fn create_group(
        &self,
        objective: String,
        session: Option<String>,
    ) -> Result<String> {
        let Surface::Main {
            factory,
            coordinator,
        } = &self.surface
        else {
            bail!("only the Main Agent can create a Group Agent");
        };
        self.create_group_with(factory, coordinator, objective, session)
    }

    pub(crate) fn run_group_lifecycle(
        &self,
        group_id: &str,
        model: &str,
        session: Option<String>,
        cancel: &AtomicBool,
        resume: bool,
    ) -> Result<String> {
        let Surface::Main {
            factory,
            coordinator,
        } = &self.surface
        else {
            bail!("only the Main Agent can control a Group Agent lifecycle");
        };
        let result = self.run_group(
            factory,
            coordinator,
            group_id,
            model,
            session,
            resume,
            cancel,
        )?;
        Ok(result.to_string())
    }

    pub(crate) fn cancel_group(&self, group_id: &str) -> Result<()> {
        self.validate_runtime_group_id(group_id)?;
        self.runtime.cancel_group();
        Ok(())
    }

    pub(crate) fn stop_group(&self, group_id: &str) -> Result<()> {
        self.validate_runtime_group_id(group_id)?;
        self.runtime.stop_group();
        Ok(())
    }

    pub(crate) fn prestart(&self, calls: &[ToolCall], model: &str, session: Option<String>) {
        if !matches!(self.surface, Surface::Members) {
            return;
        }
        let requests = calls
            .iter()
            .filter(|call| call.name == AGENT_TOOL)
            .filter_map(|call| {
                let request = commands::parse_spawn(call.arguments.as_object()?).ok()?;
                Some((call.id.clone(), request))
            })
            .collect::<Vec<_>>();
        if requests.len() < 2 {
            return;
        }
        for (call_id, request) in requests {
            let launched = self
                .runtime
                .spawn(request, model, session.clone())
                .map_err(|error| error.to_string());
            self.lock_prestarted().insert(call_id, launched);
        }
    }

    pub(crate) fn execute(
        &self,
        call: &ToolCall,
        arguments: &Map<String, Value>,
        model: &str,
        session: Option<String>,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let value = match &self.surface {
            Surface::Main {
                factory,
                coordinator,
            } => self.execute_group_control(
                factory,
                coordinator,
                call,
                arguments,
                model,
                session,
                cancel,
            )?,
            Surface::Members => {
                self.execute_member_delegation(call, arguments, model, session, cancel)?
            }
        };
        Ok(value.to_string())
    }

    fn execute_group_control(
        &self,
        factory: &AgentRuntimeFactory,
        slot: &Arc<Mutex<GroupCoordinatorSlot>>,
        call: &ToolCall,
        arguments: &Map<String, Value>,
        model: &str,
        session: Option<String>,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        match call.name.as_str() {
            control_commands::CREATE_TOOL => {
                let objective = required_str(arguments, "objective")?.to_owned();
                let group_id = self.create_group_with(factory, slot, objective, session)?;
                Ok(json!({"groupId":group_id,"status":"created"}))
            }
            control_commands::START_TOOL => {
                let group_id = required_str(arguments, "group_id")?;
                self.run_group(factory, slot, group_id, model, session, false, cancel)
            }
            control_commands::RESUME_TOOL => {
                let group_id = required_str(arguments, "group_id")?;
                self.run_group(factory, slot, group_id, model, session, true, cancel)
            }
            control_commands::CANCEL_TOOL => {
                let group_id = required_str(arguments, "group_id")?;
                self.cancel_group(group_id)?;
                Ok(json!({"groupId":group_id,"status":"cancelled"}))
            }
            control_commands::STOP_TOOL => {
                let group_id = required_str(arguments, "group_id")?;
                self.stop_group(group_id)?;
                Ok(json!({"groupId":group_id,"status":"stopped"}))
            }
            control_commands::INSPECT_TOOL => {
                let group_id = required_str(arguments, "group_id")?;
                self.validate_runtime_group_id(group_id)?;
                Ok(json!({
                    "group":self.runtime.group_item(),
                    "tasks":self.runtime.task_items(),
                }))
            }
            LEGACY_PROPOSE_TOOL => {
                let objective = serde_json::to_string(arguments)?;
                let objective = format!(
                    "Continue the legacy task proposal as one coordinated Group Agent objective. Requested task set: {objective}"
                );
                let group_id = self.create_group_with(factory, slot, objective, session.clone())?;
                self.run_group(factory, slot, &group_id, model, session, false, cancel)
            }
            other => Err(anyhow!("unknown Group Agent lifecycle operation: {other}")),
        }
    }

    fn run_group(
        &self,
        factory: &AgentRuntimeFactory,
        slot: &Arc<Mutex<GroupCoordinatorSlot>>,
        group_id: &str,
        model: &str,
        session: Option<String>,
        continuation: bool,
        primary_cancel: &AtomicBool,
    ) -> Result<Value> {
        self.validate_runtime_group_id(group_id)?;
        let fresh_coordinator = { lock_slot(slot).coordinator.is_none() };
        if fresh_coordinator {
            let mut coordinator = match factory.build_group_scoped(session) {
                Ok(coordinator) => coordinator,
                Err(error) => {
                    self.runtime.fail_group(&error.to_string());
                    return Err(error);
                }
            };
            coordinator.set_agent_group(Some(Self::members(self.runtime.clone())));
            coordinator.set_permission_label("Group Agent coordinator".into());
            let mut state = lock_slot(slot);
            state.group_id = Some(group_id.to_owned());
            state.coordinator = Some(coordinator);
        } else {
            let mut state = lock_slot(slot);
            if state
                .group_id
                .as_deref()
                .is_some_and(|current| current != group_id)
            {
                bail!("the Group Agent coordinator belongs to a different group");
            }
            state.group_id = Some(group_id.to_owned());
        }
        let objective = self
            .runtime
            .group_item()
            .objective
            .ok_or_else(|| anyhow!("Agent Group has no objective"))?;
        self.runtime.begin_group(group_id)?;
        let group_cancel = self.runtime.coordinator_cancel();
        let (outcome, result_text) = {
            let mut state = lock_slot(slot);
            state.run_sequence = state.run_sequence.saturating_add(1);
            let run_sequence = state.run_sequence;
            let coordinator = state
                .coordinator
                .as_mut()
                .ok_or_else(|| anyhow!("Group Agent coordinator is unavailable"))?;
            let input = if continuation && !fresh_coordinator {
                "Resume the Group Agent objective from its coordinator history and member checkpoints.".to_owned()
            } else if continuation {
                self.runtime.resume_context()
            } else {
                objective
            };
            let goal_mode = Arc::new(AtomicBool::new(false));
            let mut usage_sequence = 0u64;
            let monitor_done = AtomicBool::new(false);
            let monitor_cancel = group_cancel.clone();
            let monitor = thread::scope(|scope| {
                let monitor = scope.spawn(|| {
                    while !monitor_done.load(Ordering::Acquire) {
                        if primary_cancel.load(Ordering::Acquire) {
                            monitor_cancel.store(true, Ordering::Release);
                            break;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                });
                let run = coordinator.run(
                    AgentRunRequest {
                        input: &input,
                        images: Vec::new(),
                        model,
                        reasoning_level: "auto",
                        attached_capabilities: None,
                        disabled_capabilities: Vec::new(),
                        goal_mode,
                        cancel: group_cancel.clone(),
                        continuation: continuation && !fresh_coordinator,
                    },
                    |event| match event {
                        AgentEvent::ModelAttemptFinished(_, Some(usage)) => {
                            usage_sequence = usage_sequence.saturating_add(1);
                            self.runtime.record_coordinator_usage(
                                &format!("{group_id}:{run_sequence}:usage:{usage_sequence}"),
                                &usage,
                            );
                        }
                        AgentEvent::AuxiliaryUsage {
                            usage,
                            already_counted_calls: 0,
                        } => {
                            usage_sequence = usage_sequence.saturating_add(1);
                            self.runtime.record_coordinator_usage(
                                &format!("{group_id}:{run_sequence}:aux:{usage_sequence}"),
                                &usage,
                            );
                        }
                        AgentEvent::Start | AgentEvent::ReasoningStart => {
                            self.runtime.record_coordinator_event(
                                "reasoning_started",
                                "Group coordinator is reasoning",
                            )
                        }
                        AgentEvent::ProviderActivity { title, detail } => {
                            self.runtime.record_coordinator_event(
                                "provider_activity",
                                detail.as_deref().map_or(title.as_str(), |detail| detail),
                            )
                        }
                        AgentEvent::ToolExecutionStarted(call) => {
                            self.runtime.record_coordinator_event(
                                "coordinator_tool_started",
                                &format!("{}", call.name),
                            )
                        }
                        AgentEvent::ToolExecutionFinished {
                            call, succeeded, ..
                        } => self.runtime.record_coordinator_event(
                            if succeeded {
                                "coordinator_tool_finished"
                            } else {
                                "coordinator_tool_failed"
                            },
                            &call.name,
                        ),
                        _ => {}
                    },
                );
                monitor_done.store(true, Ordering::Release);
                let _ = monitor.join();
                run
            });
            let outcome = match monitor {
                Ok(outcome) => outcome,
                Err(error) => {
                    if group_cancel.load(Ordering::Acquire) {
                        self.runtime.cancel_group();
                    } else {
                        self.runtime.fail_group(&error.to_string());
                    }
                    return Err(error);
                }
            };
            let result = coordinator
                .model_history()
                .iter()
                .rev()
                .find(|message| {
                    message.role == crate::core::MessageRole::Assistant
                        && message
                            .content
                            .as_deref()
                            .is_some_and(|text| !text.trim().is_empty())
                })
                .and_then(|message| message.content.clone())
                .unwrap_or_default();
            (outcome, result)
        };
        match outcome {
            AgentRunOutcome::Completed | AgentRunOutcome::CompletedUnverified { .. } => {
                self.runtime.complete_group(result_text.clone());
            }
            AgentRunOutcome::GoalPaused { reason } | AgentRunOutcome::AutonomousIdle { reason } => {
                self.runtime.pause_group(&reason);
            }
        }
        if group_cancel.load(Ordering::Acquire) {
            self.runtime.cancel_group();
        }
        Ok(json!({
            "groupId":group_id,
            "status":self.runtime.group_item().status,
            "result":result_text,
        }))
    }

    fn create_group_with(
        &self,
        factory: &AgentRuntimeFactory,
        slot: &Arc<Mutex<GroupCoordinatorSlot>>,
        objective: String,
        session: Option<String>,
    ) -> Result<String> {
        let group_id = self.runtime.create_group(objective)?;
        let mut coordinator = match factory.build_group_scoped(session) {
            Ok(coordinator) => coordinator,
            Err(error) => {
                self.runtime.fail_group(&error.to_string());
                return Err(error);
            }
        };
        coordinator.set_agent_group(Some(Self::members(self.runtime.clone())));
        coordinator.set_permission_label("Group Agent coordinator".into());
        let mut state = lock_slot(slot);
        state.group_id = Some(group_id.clone());
        state.coordinator = Some(coordinator);
        state.run_sequence = 0;
        Ok(group_id)
    }

    fn validate_runtime_group_id(&self, requested: &str) -> Result<()> {
        if self.runtime.group_item().group_id != requested {
            bail!("unknown Agent Group: {requested}");
        }
        Ok(())
    }

    fn execute_member_delegation(
        &self,
        call: &ToolCall,
        arguments: &Map<String, Value>,
        model: &str,
        session: Option<String>,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        match call.name.as_str() {
            AGENT_TOOL => self.delegate(call, arguments, model, session, cancel),
            LEGACY_PROPOSE_TOOL => self.legacy_propose(arguments, model, session, cancel),
            other => Err(anyhow!("unknown Group Agent coordination tool: {other}")),
        }
    }

    fn delegate(
        &self,
        call: &ToolCall,
        arguments: &Map<String, Value>,
        model: &str,
        session: Option<String>,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let request = commands::parse_spawn(arguments)?;
        let launch = self.lock_prestarted().remove(&call.id);
        let (_, task_id) = match launch {
            Some(launched) => launched.map_err(|error| anyhow!(error))?,
            None => self.runtime.spawn(request, model, session)?,
        };
        let task = self.runtime.wait(task_id, cancel)?;
        if !self.runtime.has_active_tasks() {
            self.runtime.begin_synthesis();
        }
        Ok(task_result(&task))
    }

    fn legacy_propose(
        &self,
        arguments: &Map<String, Value>,
        model: &str,
        session: Option<String>,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let launched = commands::parse_legacy_proposals(arguments)?
            .into_iter()
            .map(|request| self.runtime.spawn(request, model, session.clone()))
            .collect::<Vec<_>>();
        let findings = launched
            .into_iter()
            .map(|launch| match launch {
                Ok((_, task_id)) => match self.runtime.wait(task_id, cancel) {
                    Ok(task) => task_result(&task),
                    Err(error) => json!({"status":"failed","error":error.to_string()}),
                },
                Err(error) => json!({"status":"rejected","error":error.to_string()}),
            })
            .collect::<Vec<_>>();
        if !self.runtime.has_active_tasks() {
            self.runtime.begin_synthesis();
        }
        let admitted = findings
            .iter()
            .filter(|finding| finding["status"] != "rejected")
            .count();
        Ok(json!({"admitted": admitted, "findings": findings}))
    }

    fn lock_prestarted(&self) -> std::sync::MutexGuard<'_, HashMap<String, Launch>> {
        self.prestarted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn lock_slot(
    slot: &Mutex<GroupCoordinatorSlot>,
) -> std::sync::MutexGuard<'_, GroupCoordinatorSlot> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn required_str<'a>(arguments: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    let value = arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if value.is_empty() {
        bail!("{key} is required");
    }
    Ok(value)
}
