//! Runtime-initiated verification commands.
//!
//! The coordinator may run a detected project check after an edit, but only
//! when `run_shell` would run the same command without asking the user: the
//! shell capability is enabled, the command is not a restricted operation, and
//! the workspace root is an active context root. Anything else is refused
//! here so an automatic check can never surface an approval prompt.

use super::*;

const AUTO_CHECK_TIMEOUT_SECONDS: u64 = 300;
const AUTO_CHECK_CAPTURE_BYTES: usize = 256 * 1024;

impl ToolRegistry {
    pub fn can_auto_check(&self, command: &str) -> bool {
        self.tool_enabled("run_shell")
            && restricted_operation(command).is_none()
            && self.context_root_for_path(&self.workspace_root).is_some()
    }

    /// Runs `command` from the workspace root under the session's sandbox
    /// policy. Returns `None` when the command is not auto-runnable.
    pub fn run_auto_check(
        &mut self,
        command: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<crate::shell::ShellResult>> {
        if !self.can_auto_check(command) {
            return Ok(None);
        }
        let policy = SandboxStore::new(&self.workspace_root)?.load()?;
        let hard_confined = self.hard_access_root.is_some();
        let working_directory = self.workspace_root.to_string_lossy().into_owned();
        let output = run_shell_cancellable(ShellExecutionRequest {
            command,
            workspace_root: &self.workspace_root,
            working_directory: Some(&working_directory),
            timeout_seconds: AUTO_CHECK_TIMEOUT_SECONDS,
            capture_bytes: AUTO_CHECK_CAPTURE_BYTES,
            allow_write: false,
            unrestricted: !hard_confined && policy.mode == SandboxMode::Unlimited,
            force_sandboxed: hard_confined,
            cancel: Some(cancel),
        })?;
        Ok(Some(output))
    }
}
