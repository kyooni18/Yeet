//! Cross-process serialization of structured edit transactions.
//!
//! Sessions may share a workspace and run mutating tools concurrently. Only
//! structured edits wait for an in-flight edit's validation and commit; the
//! lock is released before the tool returns to its caller.

use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use fs2::FileExt;
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub(super) struct WorkspaceEditLock {
    file: File,
}

impl WorkspaceEditLock {
    pub(super) fn acquire(config_directory: &Path, workspace: &Path) -> Result<Self> {
        let directory = config_directory.join("transactions");
        fs::create_dir_all(&directory).with_context(|| {
            format!("create Yeet transaction directory {}", directory.display())
        })?;
        let path = lock_path(&directory, workspace);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open workspace edit lock {}", path.display()))?;
        file.lock_exclusive()
            .with_context(|| format!("lock workspace edit transaction {}", path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for WorkspaceEditLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn lock_path(directory: &Path, workspace: &Path) -> PathBuf {
    let canonical = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    // Keep the existing path so older Yeet processes use the same lock inode.
    directory.join(format!("workspace-{}.lock", &digest[..24]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn edit_transactions_wait_and_resume_instead_of_rejecting_other_sessions() {
        let config = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let first = WorkspaceEditLock::acquire(config.path(), workspace.path()).unwrap();
        let config_path = config.path().to_owned();
        let workspace_path = workspace.path().join(".");
        let (started_tx, started_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _second = WorkspaceEditLock::acquire(&config_path, &workspace_path).unwrap();
            acquired_tx.send(()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            acquired_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        drop(first);
        acquired_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        waiter.join().unwrap();
    }
}
