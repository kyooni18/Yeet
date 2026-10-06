//! Nonblocking resource refresh. No application selection or presentation policy.
use super::GitSnapshot;
use std::{path::Path, sync::mpsc::{self, Receiver}, time::{Duration, Instant}};
#[derive(Debug, Default)]
pub struct GitRefresh {
    pub snapshot: GitSnapshot,
    refreshed_at: Option<Instant>,
    pending: Option<Receiver<GitSnapshot>>,
}
impl GitRefresh {
    pub fn set_snapshot(&mut self, snapshot: GitSnapshot) {
        self.snapshot = snapshot;
        self.pending = None;
        self.refreshed_at = Some(Instant::now());
    }
    /// Poll a worker and schedule at most one Git scan per five seconds. No Git
    /// subprocess or filesystem scan blocks the render thread.
    pub fn refresh_git(&mut self, workspace: &Path, force: bool) {
        if self.snapshot.workspace != workspace {
            self.pending = None;
            self.refreshed_at = None;
            self.snapshot = GitSnapshot {
                workspace: workspace.into(),
                root: workspace.into(),
                message: Some("Loading Git changes…".into()),
                ..GitSnapshot::default()
            };
        }
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(snapshot) => {
                    self.snapshot = snapshot;
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.snapshot.message = Some("Git scan unavailable".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.pending.is_some()
            || (!force
                && self
                    .refreshed_at
                    .is_some_and(|time| time.elapsed() < Duration::from_secs(5)))
        {
            return;
        }
        let workspace = workspace.to_path_buf();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.refreshed_at = Some(Instant::now());
        let _ = std::thread::Builder::new()
            .name("workbench-git".into())
            .spawn(move || {
                let _ = tx.send(GitSnapshot::load(&workspace));
            });
    }
}
