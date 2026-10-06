//! Shared execution context for tool calls.
//!
//! `ToolExecutionContext` is the identity and authority every tool call runs
//! under: the workspace and session it belongs to, the session working
//! directory and context roots, the optional hard access root (MCP clients),
//! protected runtime-state paths, and the permission broker used for
//! approvals. Domain executors take filesystem scope and approval decisions
//! from here instead of re-deriving them, so a scope rule changes in one place.
//!
//! Per-call cancellation stays an explicit `&AtomicBool` argument and the
//! sandbox policy is loaded from `.yeet/sandbox.json` at decision time, so a
//! policy change applies to the very next call.

use super::*;

pub(super) struct ToolExecutionContext {
    /// Canonical primary workspace root; never removable from context roots.
    pub(super) workspace_root: PathBuf,
    pub(super) working_directory: PathBuf,
    pub(super) context_roots: Vec<PathBuf>,
    /// When set, file access outside this root is refused outright.
    pub(super) hard_access_root: Option<PathBuf>,
    /// Active Yeet session/runtime state that no tool may mutate, even in
    /// unlimited sandbox mode.
    pub(super) protected_write_paths: Vec<PathBuf>,
    pub(super) session_store: Option<SessionStore>,
    pub(super) active_session_id: Option<String>,
    pub(super) active_task_id: Option<String>,
    pub(super) permission: PermissionBroker,
    /// Identifies a delegated agent in permission prompts it raises.
    pub(super) permission_label: Option<String>,
}

impl ToolExecutionContext {
    pub(super) fn new(workspace_root: PathBuf, permission: PermissionBroker) -> Self {
        Self {
            working_directory: workspace_root.clone(),
            context_roots: vec![workspace_root.clone()],
            workspace_root,
            hard_access_root: None,
            protected_write_paths: Vec::new(),
            session_store: None,
            active_session_id: None,
            active_task_id: None,
            permission,
            permission_label: None,
        }
    }

    pub(super) fn resolve_session_path(&self, path: &str) -> Result<PathBuf> {
        if path.trim().is_empty() || path.contains('\0') {
            bail!("invalid file path");
        }
        let input = Path::new(path);
        let joined = if input.is_absolute() {
            input.to_path_buf()
        } else {
            self.working_directory.join(input)
        };
        canonicalize_existing_ancestor(&joined)
    }

    pub(super) fn path_in_context_roots(&self, path: &Path) -> bool {
        self.context_roots
            .iter()
            .any(|root| path == root || path.starts_with(root))
    }

    pub(super) fn context_root_for_path(&self, path: &Path) -> Option<&PathBuf> {
        self.context_roots
            .iter()
            .filter(|root| path == root.as_path() || path.starts_with(root.as_path()))
            .max_by_key(|root| root.components().count())
    }

    pub(super) fn path_outside_hard_access_root(&self, path: &str) -> Result<bool> {
        let Some(root) = self.hard_access_root.as_ref() else {
            return Ok(false);
        };
        let resolved = self.resolve_session_path(path)?;
        Ok(!(resolved == *root || resolved.starts_with(root)))
    }

    pub(super) fn request_approval(
        &self,
        kind: &str,
        target: &str,
        operation: &str,
        reason: &str,
    ) -> Result<bool> {
        let policy = SandboxStore::new(&self.workspace_root)?.load()?;
        if policy.mode == SandboxMode::Unlimited || policy.auto_approve {
            return Ok(true);
        }
        let reason = match &self.permission_label {
            Some(label) => format!("[{label}] {reason}"),
            None => reason.into(),
        };
        Ok(self
            .permission
            .request(kind.into(), target.into(), operation.into(), reason))
    }

    pub(super) fn ensure_file_scope(&self, path: &str, write: bool) -> Result<()> {
        if self.path_outside_hard_access_root(path)? {
            let operation = if write { "file write" } else { "file read" };
            let root = self
                .hard_access_root
                .as_ref()
                .expect("hard access root exists when path is outside it");
            bail!(
                "MCP {operation} is restricted to {} and its descendants: {path}",
                root.display()
            );
        }
        let resolved = self.resolve_session_path(path)?;
        if self.path_in_context_roots(&resolved) {
            return Ok(());
        }
        let operation = if write {
            "outside-context file write"
        } else {
            "outside-context file read"
        };
        let reason = if write {
            "The model requested a file change outside the active session context roots."
        } else {
            "The model requested file access outside the active session context roots."
        };
        if self.request_approval("file", path, operation, reason)? {
            return Ok(());
        }
        bail!("User denied {operation}: {path}")
    }

    /// Rejects writes into active Yeet runtime/session state directories.
    pub(super) fn ensure_not_protected_write_path(&self, path: &str) -> Result<()> {
        if self.protected_write_paths.is_empty() {
            return Ok(());
        }
        let candidate = PathBuf::from(path);
        let candidate = if candidate.is_absolute() {
            candidate
        } else {
            self.workspace_root.join(candidate)
        };
        let normalized = canonicalize_existing_ancestor(&candidate)?;
        for protected in &self.protected_write_paths {
            let protected = protected
                .canonicalize()
                .unwrap_or_else(|_| protected.to_path_buf());
            if normalized == protected || normalized.starts_with(&protected) {
                bail!(
                    "Refusing to mutate active Yeet runtime state at {}. Active session state is protected even in unlimited mode.",
                    protected.display()
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_session_state_and_hard_root_are_enforced_by_context() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        let session = root.join(".yeet-session");
        std::fs::create_dir_all(&session).unwrap();
        let mut context = ToolExecutionContext::new(root.clone(), PermissionBroker::default());
        context.protected_write_paths = vec![session.clone()];

        assert!(
            context
                .ensure_not_protected_write_path(".yeet-session/state.json")
                .is_err()
        );
        assert!(
            context
                .ensure_not_protected_write_path(&session.join("nested/new.txt").to_string_lossy())
                .is_err()
        );
        context
            .ensure_not_protected_write_path("src/lib.rs")
            .unwrap();

        context.hard_access_root = Some(root.join("src"));
        assert!(context.path_outside_hard_access_root("README.md").unwrap());
        assert!(!context.path_outside_hard_access_root("src/lib.rs").unwrap());
        assert!(context.ensure_file_scope("README.md", false).is_err());
    }
}
