//! Narrow, cheaply cloned access to the active Agent Group for the tool layer.
//!
//! The handle executes the agent tools and hands background notifications to
//! the primary coordinator; it cannot replace the group or reach scheduler
//! and member runtime internals.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use crate::core::{ToolCall, ToolDefinition};

use super::{
    AgentId,
    group::{
        AgentGroupRuntime, AgentNotification,
        commands::{self, AGENT_TOOL, LEGACY_PROPOSE_TOOL, SEND_TOOL, STOP_TOOL, TOOL_NAMES},
        task_result,
    },
    task::AgentTaskId,
};

type Launch = std::result::Result<(AgentId, AgentTaskId), String>;

#[derive(Clone)]
pub(crate) struct AgentGroupHandle {
    runtime: AgentGroupRuntime,
    /// Foreground `agent` calls launched before their turn in a tool batch,
    /// keyed by tool call id, so one response's agents run concurrently.
    prestarted: Arc<Mutex<HashMap<String, Launch>>>,
}

impl AgentGroupHandle {
    pub(super) fn new(runtime: AgentGroupRuntime) -> Self {
        Self {
            runtime,
            prestarted: Arc::default(),
        }
    }

    pub(crate) fn tool_definitions() -> Vec<ToolDefinition> {
        commands::tool_definitions()
    }

    pub(crate) fn tool_names() -> [&'static str; 3] {
        TOOL_NAMES
    }

    /// Records the coordinator that delegates into this group.
    pub(crate) fn bind_primary_agent(&self, id: AgentId) {
        self.runtime.bind_primary_agent(id);
    }

    /// Background completions not yet delivered to the primary agent.
    pub(crate) fn take_notifications(&self) -> Vec<AgentNotification> {
        self.runtime.take_notifications()
    }

    /// Launches every foreground `agent` call of a batch up front; each call
    /// then only waits for its own agent when the batch reaches it.
    pub(crate) fn prestart(&self, calls: &[ToolCall], model: &str, session: Option<String>) {
        let foreground = calls
            .iter()
            .filter(|call| call.name == AGENT_TOOL)
            .filter_map(|call| {
                let request = commands::parse_spawn(call.arguments.as_object()?).ok()?;
                (!request.background).then_some((call.id.clone(), request))
            })
            .collect::<Vec<_>>();
        if foreground.len() < 2 {
            return;
        }
        for (call_id, request) in foreground {
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
        let value = match call.name.as_str() {
            AGENT_TOOL => self.agent(call, arguments, model, session, cancel)?,
            SEND_TOOL => {
                let (to, message) = commands::parse_send(arguments)?;
                let task_id = self.runtime.send(to, message)?;
                json!({
                    "agentId": to,
                    "taskId": task_id,
                    "status": "queued",
                    "note": "The agent continues in the background. You will be notified when it finishes; do not poll or wait for it."
                })
            }
            STOP_TOOL => {
                let id = commands::parse_stop(arguments)?;
                self.runtime.stop(id)?;
                json!({"agentId": id, "status": "stopped"})
            }
            LEGACY_PROPOSE_TOOL => self.legacy_propose(arguments, model, session, cancel)?,
            other => return Err(anyhow!("unknown agent tool: {other}")),
        };
        Ok(value.to_string())
    }

    fn agent(
        &self,
        call: &ToolCall,
        arguments: &Map<String, Value>,
        model: &str,
        session: Option<String>,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let request = commands::parse_spawn(arguments)?;
        let background = request.background;
        let prestarted = self.lock_prestarted().remove(&call.id);
        let (agent_id, task_id) = match prestarted {
            Some(launched) => launched.map_err(|error| anyhow!(error))?,
            None => self.runtime.spawn(request, model, session)?,
        };
        if background {
            return Ok(json!({
                "agentId": agent_id,
                "taskId": task_id,
                "status": "running",
                "note": "The agent is working in the background. You will be notified when it finishes; do not poll or wait for it."
            }));
        }
        let task = self.runtime.wait(task_id, cancel)?;
        let mut value = task_result(&task);
        value["note"] = json!("Continue this agent with send_agent_message using its agentId.");
        Ok(value)
    }

    /// The pre-`agent` batch tool: launches every proposal in the foreground
    /// and returns all findings together.
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
