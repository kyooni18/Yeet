//! First-party Codex Computer Use execution for the Yeet tool registry.
//!
//! The provider bridge owns discovery and lifetime of Codex's bundled CUA
//! runtime. This module keeps native UI dispatch out of the central registry
//! coordinator and refreshes workspace evidence after UI automation.

use std::sync::atomic::AtomicBool;

use anyhow::Result;
use serde_json::{Map, Value};

use super::ToolRegistry;

impl ToolRegistry {
    pub(super) fn computer_use_tool(
        &mut self,
        object: &Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let result = self
            .bridge
            .call_computer_use_cancellable("js", object, cancel)?;
        // UI automation can mutate project files indirectly through editors,
        // terminals, or IDEs. Refresh cached source evidence, but do not report
        // every observation/click as a source-code mutation to verification.
        self.invalidate_workspace_cache();
        Ok(result.to_string())
    }

    pub(super) fn computer_use_reset_tool(
        &self,
        object: &Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<String> {
        Ok(self
            .bridge
            .call_computer_use_cancellable("js_reset", object, cancel)?
            .to_string())
    }
}
