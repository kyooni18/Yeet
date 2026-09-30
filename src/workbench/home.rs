use super::{GitSnapshot, ResourceItem, ResourceTarget, WorkspaceContent};
use std::{
    path::Path,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

/// Home wraps shared resources; terminal rectangles and input routing stay in App.
#[derive(Debug, Default)]
pub struct HomeState {
    pub content: WorkspaceContent,
    pub selected: Option<ResourceTarget>,
    pub scroll: usize,
    pub git: GitSnapshot,
    refreshed_at: Option<Instant>,
    pending: Option<Receiver<GitSnapshot>>,
}

impl HomeState {
    /// Supply a collected snapshot (also useful for embedding and previews).
    pub fn set_git_snapshot(&mut self, snapshot: GitSnapshot) {
        self.git = snapshot;
        self.pending = None;
        self.refreshed_at = Some(Instant::now());
    }

    pub fn selected_item(&self) -> Option<&ResourceItem> {
        self.selected
            .as_ref()
            .and_then(|target| self.content.find(target))
    }

    pub fn replace_content(&mut self, content: WorkspaceContent) {
        self.content = content;
        if self.selected_item().is_none() {
            self.selected = self.content.items().next().map(|item| item.target.clone());
        }
    }

    pub fn select_next(&mut self, delta: isize) {
        let mut targets = Vec::new();
        for item in self.content.items() {
            if !targets.contains(&item.target) {
                targets.push(item.target.clone());
            }
        }
        let current = targets
            .iter()
            .position(|target| Some(target) == self.selected.as_ref())
            .unwrap_or(0);
        self.selected = targets
            .get(
                current
                    .saturating_add_signed(delta)
                    .min(targets.len().saturating_sub(1)),
            )
            .cloned();
    }

    /// Poll a worker and schedule at most one Git scan per five seconds. No Git
    /// subprocess or filesystem scan blocks the render thread.
    pub fn refresh_git(&mut self, workspace: &Path, force: bool) {
        if self.git.workspace != workspace {
            self.pending = None;
            self.refreshed_at = None;
            self.git = GitSnapshot {
                workspace: workspace.into(),
                root: workspace.into(),
                message: Some("Loading Git changes…".into()),
                ..GitSnapshot::default()
            };
        }
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(snapshot) => {
                    self.git = snapshot;
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.git.message = Some("Git scan unavailable".into());
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
