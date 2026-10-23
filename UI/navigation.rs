//! Shared navigation state and interaction policy. Native input, geometry and
//! resource loading are adapters; they report successful surface operations here.
use super::{
    surfaces::SurfaceId,
    workbench::{LAUNCHER, WorkbenchTab},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Screen {
    #[default]
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
    /// Primary workbench view beneath floating panels.
    #[serde(default)]
    workbench_mode: Screen,
    pub views_origin: Screen,
    pub views_index: usize,
}
impl Default for NavigationState {
    fn default() -> Self {
        Self {
            mode: Screen::Chat,
            home_override: None,
            workbench_mode: Screen::Chat,
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
    /// Open conversation instances. Empty means the single implicit session tab.
    pub sessions: Vec<SurfaceId>,
    pub active_session: Option<SurfaceId>,
    /// User-arranged order (drag/move). Unlisted tabs keep their natural order
    /// after the listed ones; Home always stays first.
    pub order: Vec<WorkbenchTab>,
    /// The Files browser tab was closed while file tabs stay open.
    pub files_browser_closed: bool,
    /// The host owns conversation tabs; no implicit session tab is listed.
    pub sessions_managed: bool,
}
impl WorkbenchResources {
    pub fn tabs(&self) -> Vec<WorkbenchTab> {
        let mut tabs = vec![WorkbenchTab::Home];
        if self.sessions.is_empty() {
            if !self.sessions_managed {
                tabs.push(WorkbenchTab::Session);
            }
        } else {
            tabs.extend(
                self.sessions
                    .iter()
                    .copied()
                    .map(WorkbenchTab::Conversation),
            );
        }
        if self.agents {
            tabs.push(WorkbenchTab::Agents);
        }
        if let Some(files) = &self.files {
            if !self.files_browser_closed {
                tabs.push(WorkbenchTab::Files);
            }
            tabs.extend(files.iter().copied().map(WorkbenchTab::File));
        }
        tabs.extend(self.diffs.iter().copied().map(WorkbenchTab::Diff));
        let rank = |tab: &WorkbenchTab| match tab {
            WorkbenchTab::Home => (0, 0),
            tab => self
                .order
                .iter()
                .position(|ordered| ordered == tab)
                .map_or((2, 0), |index| (1, index)),
        };
        // Stable sort keeps natural order among unranked tabs.
        tabs.sort_by_key(rank);
        tabs
    }
    /// Order after moving `tab` by `delta` positions; Home stays pinned.
    pub fn moved(&self, tab: WorkbenchTab, delta: isize) -> Vec<WorkbenchTab> {
        let mut tabs = self.tabs();
        if let Some(from) = tabs.iter().position(|t| *t == tab).filter(|i| *i > 0) {
            let to = from.saturating_add_signed(delta).clamp(1, tabs.len() - 1);
            let tab = tabs.remove(from);
            tabs.insert(to, tab);
        }
        tabs.retain(|tab| *tab != WorkbenchTab::Home);
        tabs
    }
    /// Order after dropping `tab` onto the position currently held by `target`.
    pub fn dropped(&self, tab: WorkbenchTab, target: WorkbenchTab) -> Vec<WorkbenchTab> {
        let tabs = self.tabs();
        match (
            tabs.iter().position(|t| *t == tab),
            tabs.iter().position(|t| *t == target),
        ) {
            (Some(from), Some(to)) => self.moved(tab, to as isize - from as isize),
            _ => self.moved(tab, 0),
        }
    }
}
impl NavigationState {
    fn is_workbench_mode(mode: Screen) -> bool {
        matches!(
            mode,
            Screen::Chat | Screen::Files | Screen::Diff | Screen::Agents
        )
    }
    /// Panels take input focus without navigating the workbench underneath them.
    pub fn set_mode(&mut self, mode: Screen) {
        self.workbench_mode = self.primary_mode();
        self.mode = mode;
        if Self::is_workbench_mode(mode) {
            self.workbench_mode = mode;
        }
    }
    pub fn primary_mode(&self) -> Screen {
        if Self::is_workbench_mode(self.mode) {
            self.mode
        } else {
            self.workbench_mode
        }
    }
    pub fn dismiss_panel(&mut self) {
        self.mode = self.primary_mode();
    }
    pub fn home_visible(&self) -> bool {
        self.home_override.unwrap_or(true)
    }
    pub fn active_tab(&self, resources: &WorkbenchResources) -> WorkbenchTab {
        match self.primary_mode() {
            Screen::Agents => WorkbenchTab::Agents,
            Screen::Diff => resources
                .active_diff
                .map_or(WorkbenchTab::Home, WorkbenchTab::Diff),
            Screen::Files => resources
                .active_file
                .map_or(WorkbenchTab::Files, WorkbenchTab::File),
            Screen::Chat if self.home_visible() => WorkbenchTab::Home,
            Screen::Chat => resources
                .active_session
                .map_or(WorkbenchTab::Session, WorkbenchTab::Conversation),
            _ => unreachable!("primary mode is a workbench view"),
        }
    }
    /// Apply only after the adapter has accepted any associated resource operation.
    pub fn activated(&mut self, tab: WorkbenchTab) {
        match tab {
            WorkbenchTab::Home
            | WorkbenchTab::Session
            | WorkbenchTab::Conversation(_)
            | WorkbenchTab::NewConversation => {
                self.home_override = Some(tab == WorkbenchTab::Home);
                self.set_mode(Screen::Chat);
            }
            WorkbenchTab::Files | WorkbenchTab::File(_) => self.set_mode(Screen::Files),
            WorkbenchTab::Diff(_) | WorkbenchTab::NewDiff => self.set_mode(Screen::Diff),
            WorkbenchTab::Agents => self.set_mode(Screen::Agents),
            WorkbenchTab::Launcher => self.open_launcher(),
            WorkbenchTab::CloseFile(_)
            | WorkbenchTab::CloseDiff(_)
            | WorkbenchTab::CloseConversation(_)
            | WorkbenchTab::CloseFiles
            | WorkbenchTab::CloseAgents => {}
        }
    }
    pub fn diff_closed(&mut self, remaining: usize) {
        if remaining == 0 && self.mode == Screen::Diff {
            self.activated(WorkbenchTab::Home);
        }
    }
    pub fn open_launcher(&mut self) {
        self.views_origin = self.mode;
        let item = match self.primary_mode() {
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
        self.set_mode(Screen::Views);
    }
    pub fn dismiss_launcher(&mut self) {
        self.set_mode(self.views_origin);
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
    #[test]
    fn floating_panels_preserve_primary_navigation_and_launcher_origin() {
        let id = Tabs::default().open("resource", ());
        let mut resources = WorkbenchResources {
            agents: true,
            files: Some(vec![id]),
            active_file: None,
            diffs: vec![id],
            active_diff: Some(id),
            ..Default::default()
        };
        for tab in [
            WorkbenchTab::Home,
            WorkbenchTab::Session,
            WorkbenchTab::Files,
            WorkbenchTab::File(id),
            WorkbenchTab::Diff(id),
            WorkbenchTab::Agents,
        ] {
            resources.active_file = matches!(tab, WorkbenchTab::File(_)).then_some(id);
            let mut state = NavigationState::default();
            state.activated(tab);
            for panel in [
                Screen::Settings,
                Screen::Models,
                Screen::Help,
                Screen::Sessions,
                Screen::Providers,
                Screen::Capabilities,
                Screen::Status,
            ] {
                state.set_mode(panel);
                assert_eq!(state.active_tab(&resources), tab);
                state.open_launcher();
                assert_eq!(state.active_tab(&resources), tab);
                state.dismiss_launcher();
                assert_eq!(state.mode, panel);
                state.dismiss_panel();
                assert_eq!(state.active_tab(&resources), tab);
                assert_eq!(state.mode, state.primary_mode());
            }
        }
    }
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
            ..Default::default()
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
