//! Session working-directory, context-root, and filesystem-scope controls.

use super::*;

impl ToolRegistry {
    pub fn session_environment(&self) -> (String, Vec<String>) {
        (
            self.working_directory.to_string_lossy().into_owned(),
            self.context_roots
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        )
    }

    pub fn set_working_directory(&mut self, path: impl AsRef<Path>) -> Result<PathBuf> {
        let raw = path.as_ref();
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.working_directory.join(raw)
        };
        let resolved = candidate
            .canonicalize()
            .with_context(|| format!("resolve working directory {}", candidate.display()))?;
        if !resolved.is_dir() {
            bail!(
                "working directory is not a directory: {}",
                resolved.display()
            );
        }
        if !self.path_in_context_roots(&resolved) {
            self.context_roots.push(resolved.clone());
        }
        self.working_directory = resolved.clone();
        self.invalidate_workspace_cache();
        self.shell_inspections.clear();
        Ok(resolved)
    }

    pub fn add_context_root(&mut self, path: impl AsRef<Path>) -> Result<PathBuf> {
        let raw = path.as_ref();
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.working_directory.join(raw)
        };
        let resolved = candidate
            .canonicalize()
            .with_context(|| format!("resolve context root {}", candidate.display()))?;
        if !resolved.is_dir() {
            bail!("context root is not a directory: {}", resolved.display());
        }
        if !self.context_roots.iter().any(|root| root == &resolved) {
            self.context_roots.push(resolved.clone());
            self.context_roots.sort();
        }
        Ok(resolved)
    }

    pub fn remove_context_root(&mut self, path: impl AsRef<Path>) -> Result<bool> {
        let raw = path.as_ref();
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.working_directory.join(raw)
        };
        let resolved = candidate.canonicalize().unwrap_or(candidate);
        if resolved == self.workspace_root {
            bail!("the primary workspace root cannot be removed from the session context");
        }
        let Some(index) = self.context_roots.iter().position(|root| root == &resolved) else {
            return Ok(false);
        };
        let mut remaining = self.context_roots.clone();
        remaining.remove(index);
        let cwd_still_covered = remaining.iter().any(|root| {
            self.working_directory == *root || self.working_directory.starts_with(root)
        });
        if !cwd_still_covered {
            bail!(
                "cannot remove context root {} while the session cwd is inside it; change cwd first",
                resolved.display()
            );
        }
        self.context_roots = remaining;
        Ok(true)
    }

    pub fn restore_session_environment(
        &mut self,
        working_directory: Option<&str>,
        context_roots: &[String],
    ) -> Result<()> {
        self.working_directory = self.workspace_root.clone();
        self.context_roots = vec![self.workspace_root.clone()];
        for root in context_roots {
            let path = PathBuf::from(root);
            if let Ok(resolved) = path.canonicalize()
                && resolved.is_dir()
                && !self
                    .context_roots
                    .iter()
                    .any(|existing| existing == &resolved)
            {
                self.context_roots.push(resolved);
            }
        }
        if let Some(cwd) = working_directory {
            let path = PathBuf::from(cwd);
            if let Ok(resolved) = path.canonicalize()
                && resolved.is_dir()
            {
                if !self.path_in_context_roots(&resolved) {
                    self.context_roots.push(resolved.clone());
                }
                self.working_directory = resolved;
            }
        }
        self.context_roots.sort();
        self.invalidate_workspace_cache();
        self.shell_inspections.clear();
        Ok(())
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

    pub fn set_hard_access_root(&mut self, root: Option<PathBuf>) -> Result<()> {
        self.hard_access_root = root
            .map(|path| {
                path.canonicalize()
                    .map_err(anyhow::Error::from)
                    .map_err(|error| error.context("resolve hard access root"))
            })
            .transpose()?;
        Ok(())
    }

    pub(super) fn path_outside_hard_access_root(&self, path: &str) -> Result<bool> {
        let Some(root) = self.hard_access_root.as_ref() else {
            return Ok(false);
        };
        let resolved = self.resolve_session_path(path)?;
        Ok(!(resolved == *root || resolved.starts_with(root)))
    }
    pub fn set_protected_write_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.protected_write_paths = paths.into_iter().collect();
    }
}
