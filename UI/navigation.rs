//! Shared navigation state and interaction policy. Native input, geometry and
//! resource loading are adapters; they report successful surface operations here.
use super::{
    surfaces::SurfaceId,
    workbench::{LAUNCHER, WorkbenchTab},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Screen {
    Chat,
    Debate,
    Models,
    Reasoning,
    Goal,
    Sessions,
    Capabilities,
    CapabilityDetail,
    Auth,
    AuthKey,
    Providers,
    ProviderEdit,
    Settings,
    SandboxPresets,
    SandboxPolicy,
    SettingsEdit,
    Status,
    Help,
    Files,
    Diff,
    Agents,
    AgentGroup,
    Views,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigationState {
    pub mode: Screen,
    pub home_override: Option<bool>,
    pub views_origin: Screen,
    pub views_index: usize,
}
impl Default for NavigationState {
    fn default() -> Self {
        Self {
            mode: Screen::Chat,
            home_override: None,
            views_origin: Screen::Chat,
            views_index: 0,
        }
    }
}
/// Current open resources; terminal/native widget state never enters this model.
#[derive(Debug, Default)]
pub struct WorkbenchResources {
    pub agents: bool,
    pub files: Option<Vec<SurfaceId>>,
    pub active_file: Option<SurfaceId>,
    pub diffs: Vec<SurfaceId>,
    pub active_diff: Option<SurfaceId>,
}
impl WorkbenchResources {
    pub fn tabs(&self) -> Vec<WorkbenchTab> {
        let mut tabs = vec![WorkbenchTab::Home, WorkbenchTab::Session];
        if self.agents {
            tabs.push(WorkbenchTab::Agents);
        }
        if let Some(files) = &self.files {
            tabs.push(WorkbenchTab::Files);
            tabs.extend(files.iter().copied().map(WorkbenchTab::File));
        }
        tabs.extend(self.diffs.iter().copied().map(WorkbenchTab::Diff));
        tabs
    }
}
impl NavigationState {
    pub fn home_visible(&self) -> bool {
        self.home_override.unwrap_or(true)
    }
    pub fn active_tab(&self, resources: &WorkbenchResources) -> WorkbenchTab {
        let mode = if self.mode == Screen::Views {
            self.views_origin
        } else {
            self.mode
        };
        match mode {
            Screen::Agents => WorkbenchTab::Agents,
            Screen::Diff => resources
                .active_diff
                .map_or(WorkbenchTab::Home, WorkbenchTab::Diff),
            Screen::Files => resources
                .active_file
                .map_or(WorkbenchTab::Files, WorkbenchTab::File),
            _ if self.home_visible() => WorkbenchTab::Home,
            _ => WorkbenchTab::Session,
        }
    }
    /// Apply only after the adapter has accepted any associated resource operation.
    pub fn activated(&mut self, tab: WorkbenchTab) {
        match tab {
            WorkbenchTab::Home | WorkbenchTab::Session => {
                self.home_override = Some(tab == WorkbenchTab::Home);
                self.mode = Screen::Chat;
            }
            WorkbenchTab::Files | WorkbenchTab::File(_) => self.mode = Screen::Files,
            WorkbenchTab::Diff(_) | WorkbenchTab::NewDiff => self.mode = Screen::Diff,
            WorkbenchTab::Agents => self.mode = Screen::Agents,
            WorkbenchTab::Launcher => self.open_launcher(),
            WorkbenchTab::CloseFile(_) | WorkbenchTab::CloseDiff(_) => {}
        }
    }
    pub fn diff_closed(&mut self, remaining: usize) {
        if remaining == 0 && self.mode == Screen::Diff {
            self.activated(WorkbenchTab::Home);
        }
    }
    pub fn open_launcher(&mut self) {
        self.views_origin = match self.mode {
            Screen::Files | Screen::Diff | Screen::Agents => self.mode,
            _ => Screen::Chat,
        };
        let item = match self.views_origin {
            Screen::Agents => WorkbenchTab::Agents,
            Screen::Diff => WorkbenchTab::NewDiff,
            Screen::Files => WorkbenchTab::Files,
            _ if self.home_visible() => WorkbenchTab::Home,
            _ => WorkbenchTab::Session,
        };
        self.views_index = LAUNCHER
            .iter()
            .position(|entry| entry.action == item)
            .unwrap_or(0);
        self.mode = Screen::Views;
    }
    pub fn dismiss_launcher(&mut self) {
        self.mode = self.views_origin;
    }
    pub fn move_launcher(&mut self, delta: isize) {
        self.views_index = self
            .views_index
            .saturating_add_signed(delta)
            .min(LAUNCHER.len() - 1);
    }
    pub fn launcher_selection(&self) -> WorkbenchTab {
        LAUNCHER[self.views_index.min(LAUNCHER.len() - 1)].action
    }
    pub fn cycle_tab(&self, resources: &WorkbenchResources, backwards: bool) -> WorkbenchTab {
        let tabs = resources.tabs();
        let current = tabs
            .iter()
            .position(|tab| *tab == self.active_tab(resources))
            .unwrap_or(0);
        let next = if backwards {
            (current + tabs.len() - 1) % tabs.len()
        } else {
            (current + 1) % tabs.len()
        };
        tabs[next]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_ui::surfaces::Tabs;
    #[test]
    fn launcher_and_resource_closure_preserve_shared_navigation() {
        let mut tabs = Tabs::default();
        let first = tabs.open("same", ());
        let second = tabs.open("same", ());
        let mut resources = WorkbenchResources {
            files: Some(vec![first]),
            active_file: Some(first),
            diffs: vec![first, second],
            active_diff: Some(second),
            agents: true,
        };
        let mut state = NavigationState::default();
        state.activated(WorkbenchTab::Diff(second));
        state.open_launcher();
        assert_eq!(state.active_tab(&resources), WorkbenchTab::Diff(second));
        assert_eq!(LAUNCHER[state.views_index].action, WorkbenchTab::NewDiff);
        state.dismiss_launcher();
        assert_eq!(state.mode, Screen::Diff);
        assert_eq!(state.cycle_tab(&resources, false), WorkbenchTab::Home);
        assert_eq!(state.cycle_tab(&resources, true), WorkbenchTab::Diff(first));
        tabs.close(first);
        resources.diffs = vec![second];
        state.diff_closed(resources.diffs.len());
        assert_eq!(state.active_tab(&resources), WorkbenchTab::Diff(second));
        tabs.close(second);
        resources.diffs.clear();
        resources.active_diff = None;
        state.diff_closed(0);
        assert_eq!(state.active_tab(&resources), WorkbenchTab::Home);
        state.activated(WorkbenchTab::Session);
        state.open_launcher();
        state.dismiss_launcher();
        assert_eq!(state.active_tab(&resources), WorkbenchTab::Session);
    }
}
