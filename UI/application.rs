//! Actual application view orchestration, independent of native rendering.
use super::{
    navigation::{NavigationState, Screen},
    shell::{WorkspaceView, workspace_children},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Content {
    Home,
    Session,
    Files,
    Diff,
    Agents,
    Auxiliary,
}

/// Primary content and ordered surfaces; platforms translate each semantic view
/// into their native presentation without selecting a different application tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationView {
    pub content: Content,
    pub surfaces: Vec<Screen>,
    pub overlays: Vec<Screen>,
    pub workspace_children: Vec<WorkspaceView>,
}
impl ApplicationView {
    pub fn from_navigation(navigation: &NavigationState) -> Self {
        let content = match navigation.primary_mode() {
            Screen::Chat if navigation.home_visible() => Content::Home,
            Screen::Chat => Content::Session,
            Screen::Files => Content::Files,
            Screen::Diff => Content::Diff,
            Screen::Agents => Content::Agents,
            _ => unreachable!("primary mode is a workbench view"),
        };
        let panel = if navigation.mode == Screen::Views {
            navigation.views_origin
        } else {
            navigation.mode
        };
        let mut overlays = match panel {
            Screen::Chat | Screen::Files | Screen::Diff | Screen::Agents => Vec::new(),
            Screen::SettingsEdit => vec![Screen::SandboxPolicy, Screen::SettingsEdit],
            _ => vec![panel],
        };
        if navigation.mode == Screen::Views {
            overlays.push(Screen::Views);
        }
        Self {
            content,
            surfaces: Vec::new(),
            overlays,
            workspace_children: workspace_children(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_ui::shell::{ShellState, ViewKind};

    #[test]
    fn application_projection_preserves_underlying_views_and_shared_workspace_order() {
        let mut navigation = NavigationState::default();
        assert_eq!(
            ApplicationView::from_navigation(&navigation).content,
            Content::Home
        );
        navigation.home_override = Some(false);
        let session = ApplicationView::from_navigation(&navigation);
        assert_eq!(session.content, Content::Session);
        let shell = ShellState::default().view();
        assert_eq!(
            session.workspace_children,
            shell
                .views
                .iter()
                .find(|view| view.kind == ViewKind::Workspace)
                .unwrap()
                .children
        );
        for (mode, content) in [
            (Screen::Files, Content::Files),
            (Screen::Diff, Content::Diff),
            (Screen::Agents, Content::Agents),
        ] {
            navigation.mode = mode;
            navigation.open_launcher();
            let view = ApplicationView::from_navigation(&navigation);
            assert_eq!(view.content, content);
            assert_eq!(view.overlays, vec![Screen::Views]);
        }
        navigation.set_mode(Screen::SettingsEdit);
        let view = ApplicationView::from_navigation(&navigation);
        assert_eq!(view.content, Content::Agents);
        assert_eq!(
            view.overlays,
            vec![Screen::SandboxPolicy, Screen::SettingsEdit]
        );
        assert!(view.surfaces.is_empty());
    }
}
