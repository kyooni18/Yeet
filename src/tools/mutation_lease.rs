//! Cross-process serialization for source mutations in one workspace.
//!
//! Agent tasks may inspect the same project concurrently, but once a task starts
//! mutating source it owns an advisory OS lock until the task finishes. This
//! prevents independent Yeet sessions from interleaving edits and then validating
//! a worktree that no longer corresponds to either task's evidence.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use fs2::{FileExt, lock_contended_error};
use serde_json::json;
use sha2::{Digest, Sha256};

fn lock_is_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error() == lock_contended_error().raw_os_error()
}

#[derive(Debug)]
pub(super) struct WorkspaceMutationLease {
    file: File,
}

impl WorkspaceMutationLease {
    pub(super) fn acquire(
        config_directory: &Path,
        workspace: &Path,
        session_id: Option<&str>,
        task_id: &str,
    ) -> Result<Self> {
        let directory = config_directory.join("transactions");
        fs::create_dir_all(&directory).with_context(|| {
            format!("create Yeet transaction directory {}", directory.display())
        })?;
        let path = lease_path(&directory, workspace);
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open workspace mutation lease {}", path.display()))?;

        match file.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if lock_is_contended(&error) => {
                let owner = read_owner(&path);
                let owner = owner.trim();
                let detail = if owner.is_empty() {
                    String::new()
                } else {
                    format!(
                        " Current owner: {}",
                        owner.chars().take(512).collect::<String>()
                    )
                };
                bail!(
                    "Workspace mutation lease is held by another active Yeet task for {}.{detail} Finish or cancel that task before editing this workspace from another session.",
                    workspace.display()
                );
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("lock workspace mutation lease {}", path.display()));
            }
        }

        let owner = json!({
            "pid": std::process::id(),
            "workspace": workspace.to_string_lossy(),
            "sessionId": session_id,
            "taskId": task_id,
        })
        .to_string();
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(owner.as_bytes())?;
        file.flush()?;
        Ok(Self { file })
    }
}

impl Drop for WorkspaceMutationLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn lease_path(directory: &Path, workspace: &Path) -> PathBuf {
    let canonical = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    directory.join(format!("workspace-{}.lock", &digest[..24]))
}

fn read_owner(path: &Path) -> String {
    let mut value = String::new();
    if let Ok(mut file) = File::open(path) {
        let _ = file.read_to_string(&mut value);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_blocks_a_second_writer_until_release() {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let first = WorkspaceMutationLease::acquire(
            config.path(),
            workspace.path(),
            Some("session-a"),
            "task-a",
        )
        .unwrap();

        let blocked = WorkspaceMutationLease::acquire(
            config.path(),
            workspace.path(),
            Some("session-b"),
            "task-b",
        )
        .unwrap_err();
        assert!(blocked.to_string().contains("another active Yeet task"));

        drop(first);
        WorkspaceMutationLease::acquire(
            config.path(),
            workspace.path(),
            Some("session-b"),
            "task-b",
        )
        .unwrap();
    }
}
