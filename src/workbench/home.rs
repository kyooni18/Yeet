//! Compatibility controller: shared UI interaction plus Harness resource refresh.
use super::GitSnapshot;
use crate::{harness::resources::GitRefresh, shared_ui::home};
use std::{ops::{Deref, DerefMut}, path::Path};
#[derive(Debug, Default)]
pub struct HomeState {
    ui: home::HomeState,
    pub git: GitSnapshot,
    refresh: GitRefresh,
}
impl Deref for HomeState { type Target = home::HomeState; fn deref(&self) -> &Self::Target { &self.ui } }
impl DerefMut for HomeState { fn deref_mut(&mut self) -> &mut Self::Target { &mut self.ui } }
impl HomeState {
    pub fn replace_content(&mut self, content: home::WorkspaceContent) { self.ui.replace_content(content); }
    pub fn projection_state(&self) -> &home::HomeState { &self.ui }
    pub fn set_git_snapshot(&mut self, snapshot: GitSnapshot) {
        self.git = snapshot.clone();
        self.refresh.set_snapshot(snapshot);
    }
    pub fn refresh_git(&mut self, workspace: &Path, force: bool) {
        self.refresh.refresh_git(workspace, force);
        self.git = self.refresh.snapshot.clone();
    }
}
