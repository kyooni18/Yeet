//! Narrow, cheaply cloned access to the active Agent Group for the tool layer.
//!
//! The handle submits commands; it cannot replace the group or inspect
//! scheduler and member runtime internals.

use std::sync::atomic::AtomicBool;

use anyhow::Result;
use serde_json::{Map, Value};

use crate::core::ToolDefinition;

use super::{
    AgentId,
    group::{AgentGroupRuntime, commands},
};

#[derive(Clone)]
pub(crate) struct AgentGroupHandle {
    runtime: AgentGroupRuntime,
}

impl AgentGroupHandle {
    pub(super) fn new(runtime: AgentGroupRuntime) -> Self {
        Self { runtime }
    }

    pub(crate) fn tool_definition() -> ToolDefinition {
        commands::tool_definition()
    }

    /// Records the coordinator that delegates into this group.
    pub(crate) fn bind_primary_agent(&self, id: AgentId) {
        self.runtime.bind_primary_agent(id);
    }

    /// Submits proposed tasks and returns the tool result.
    pub(crate) fn delegate_tasks(
        &self,
        arguments: &Map<String, Value>,
        model: &str,
        active_session_id: Option<String>,
        parent_cancel: &AtomicBool,
    ) -> Result<String> {
        self.runtime
            .delegate(arguments, model, active_session_id, parent_cancel)
    }
}
