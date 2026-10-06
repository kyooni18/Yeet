//! Shared application shell interaction and layout semantics.
//!
//! This preserves the existing Remote shell. Native adapters report their
//! available layout and translate surface placement without choosing which
//! application panels are visible. Focus handles and pixel/cell geometry remain
//! in those adapters.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    #[default]
    Compact,
    Expanded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Navigation,
    Inspector,
    Models,
    Settings,
    Agents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShellAction {
    SetLayout { layout: Layout },
    OpenNavigation,
    CloseNavigation,
    ToggleInspector,
    CloseInspector,
    OpenModels,
    CloseModels,
    OpenSettings,
    CloseSettings,
    OpenAgents,
    CloseAgents,
    Dismiss,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellState {
    pub layout: Layout,
    pub navigation: bool,
    pub inspector: bool,
    pub models: bool,
    pub settings: bool,
    pub agents: bool,
    #[serde(skip)]
    layout_initialized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    Hidden,
    Content,
    Docked,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    Navigation,
    Workspace,
    Inspector,
    Models,
    Settings,
    Agents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceView {
    SessionHeader,
    Conversation,
    Composer,
}

/// Shared workspace composition used by graphical and terminal adapters.
pub fn workspace_children() -> Vec<WorkspaceView> {
    vec![
        WorkspaceView::SessionHeader,
        WorkspaceView::Conversation,
        WorkspaceView::Composer,
    ]
}

/// An actual application view, rather than a native widget primitive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub kind: ViewKind,
    pub placement: Placement,
    pub children: Vec<WorkspaceView>,
}

/// Ordered application composition; adapters supply native components and sizing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellView {
    pub layout: Layout,
    pub views: Vec<View>,
    /// Whether the existing skip-to-conversation control is available.
    pub skip_conversation: bool,
    pub dismiss: Option<Surface>,
}

impl ShellView {
    pub fn placement(&self, kind: ViewKind) -> Placement {
        self.views
            .iter()
            .find(|view| view.kind == kind)
            .map_or(Placement::Hidden, |view| view.placement)
    }
}

impl ShellState {
    /// Returns the surface closed by this action, when one was closed.
    /// Adapters can use that semantic identity to restore native focus.
    pub fn apply(&mut self, action: ShellAction) -> Option<Surface> {
        use ShellAction::*;
        let closed = match action {
            SetLayout { layout } => {
                if !self.layout_initialized && layout == Layout::Expanded {
                    self.navigation = true;
                } else if self.layout_initialized
                    && self.layout != layout
                    && layout == Layout::Compact
                {
                    self.navigation = false;
                    self.inspector = false;
                }
                self.layout = layout;
                self.layout_initialized = true;
                None
            }
            OpenNavigation => {
                if self.layout == Layout::Compact {
                    self.inspector = false;
                }
                self.navigation = true;
                None
            }
            CloseNavigation => self.close(Surface::Navigation),
            ToggleInspector => {
                if self.layout == Layout::Compact {
                    self.navigation = false;
                }
                if self.inspector {
                    self.close(Surface::Inspector)
                } else {
                    self.inspector = true;
                    None
                }
            }
            CloseInspector => self.close(Surface::Inspector),
            OpenModels => {
                self.models = true;
                None
            }
            CloseModels => self.close(Surface::Models),
            OpenSettings => {
                self.settings = true;
                None
            }
            CloseSettings => self.close(Surface::Settings),
            OpenAgents => {
                self.inspector = false;
                self.agents = true;
                None
            }
            CloseAgents => self.close(Surface::Agents),
            Dismiss => self
                .dismiss_target()
                .and_then(|surface| self.close(surface)),
        };
        closed
    }

    fn close(&mut self, surface: Surface) -> Option<Surface> {
        let open = match surface {
            Surface::Navigation => &mut self.navigation,
            Surface::Inspector => &mut self.inspector,
            Surface::Models => &mut self.models,
            Surface::Settings => &mut self.settings,
            Surface::Agents => &mut self.agents,
        };
        let was_open = *open;
        *open = false;
        was_open.then_some(surface)
    }

    fn dismiss_target(&self) -> Option<Surface> {
        [
            (self.models, Surface::Models),
            (self.settings, Surface::Settings),
            (self.agents, Surface::Agents),
            (self.inspector, Surface::Inspector),
            (self.navigation, Surface::Navigation),
        ]
        .into_iter()
        .find_map(|(open, surface)| open.then_some(surface))
    }

    pub fn view(&self) -> ShellView {
        let expanded = self.layout == Layout::Expanded;
        let overlay = self.models || self.settings || self.agents;
        let panel = |open: bool| {
            if !open || (!expanded && overlay) {
                Placement::Hidden
            } else if expanded {
                Placement::Docked
            } else {
                Placement::Overlay
            }
        };
        let sheet = |open: bool| {
            if open {
                Placement::Overlay
            } else {
                Placement::Hidden
            }
        };
        let navigation = panel(self.navigation);
        let inspector = panel(self.inspector);
        let mut views = vec![
            View {
                kind: ViewKind::Navigation,
                placement: navigation,
                children: Vec::new(),
            },
            View {
                kind: ViewKind::Workspace,
                placement: Placement::Content,
                children: workspace_children(),
            },
        ];
        for (kind, placement) in [
            (ViewKind::Inspector, inspector),
            (ViewKind::Models, sheet(self.models)),
            (ViewKind::Settings, sheet(self.settings)),
            (ViewKind::Agents, sheet(self.agents)),
        ] {
            views.push(View {
                kind,
                placement,
                children: Vec::new(),
            });
        }
        ShellView {
            layout: self.layout,
            views,
            skip_conversation: navigation != Placement::Overlay
                && inspector != Placement::Overlay
                && !self.models
                && !self.settings,
            dismiss: self.dismiss_target(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_panels_restore_after_sheets_and_dismiss_in_existing_order() {
        let mut state = ShellState::default();
        state.apply(ShellAction::OpenNavigation);
        state.apply(ShellAction::OpenAgents);
        state.apply(ShellAction::OpenSettings);
        state.apply(ShellAction::OpenModels);
        assert_eq!(
            state.view().placement(ViewKind::Navigation),
            Placement::Hidden
        );
        for surface in [
            Surface::Models,
            Surface::Settings,
            Surface::Agents,
            Surface::Navigation,
        ] {
            assert_eq!(state.apply(ShellAction::Dismiss), Some(surface));
        }
        assert_eq!(state.apply(ShellAction::Dismiss), None);
        let view = state.view();
        assert_eq!(
            view.views.iter().map(|view| view.kind).collect::<Vec<_>>(),
            vec![
                ViewKind::Navigation,
                ViewKind::Workspace,
                ViewKind::Inspector,
                ViewKind::Models,
                ViewKind::Settings,
                ViewKind::Agents
            ]
        );
        assert_eq!(
            view.views[1].children,
            vec![
                WorkspaceView::SessionHeader,
                WorkspaceView::Conversation,
                WorkspaceView::Composer
            ]
        );
        state.apply(ShellAction::OpenNavigation);
        state.apply(ShellAction::OpenModels);
        state.apply(ShellAction::CloseModels);
        assert_eq!(
            state.view().placement(ViewKind::Navigation),
            Placement::Overlay
        );
        state.apply(ShellAction::ToggleInspector);
        assert_eq!(
            state.view().placement(ViewKind::Navigation),
            Placement::Hidden
        );
        assert_eq!(
            state.view().placement(ViewKind::Inspector),
            Placement::Overlay
        );
    }

    #[test]
    fn layout_initialization_and_transitions_preserve_panel_intent() {
        let mut state = ShellState::default();
        state.apply(ShellAction::SetLayout {
            layout: Layout::Expanded,
        });
        state.apply(ShellAction::ToggleInspector);
        state.apply(ShellAction::OpenModels);
        assert_eq!(
            state.view().placement(ViewKind::Navigation),
            Placement::Docked
        );
        assert_eq!(
            state.view().placement(ViewKind::Inspector),
            Placement::Docked
        );
        state.apply(ShellAction::SetLayout {
            layout: Layout::Compact,
        });
        assert!(!state.navigation && !state.inspector);
        assert!(state.models);
        state.apply(ShellAction::OpenNavigation);
        state.apply(ShellAction::SetLayout {
            layout: Layout::Compact,
        });
        assert!(
            state.navigation,
            "same layout reports preserve navigation on reconnect"
        );
        state.apply(ShellAction::CloseNavigation);
        state.apply(ShellAction::SetLayout {
            layout: Layout::Expanded,
        });
        assert!(!state.navigation);
    }
}
