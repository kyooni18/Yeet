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
        let mode = if navigation.mode == Screen::Views {
            navigation.views_origin
        } else {
            navigation.mode
        };
        let content = match mode {
            Screen::Chat if navigation.home_visible() => Content::Home,
            Screen::Chat => Content::Session,
            Screen::Files => Content::Files,
            Screen::Diff => Content::Diff,
            Screen::Agents => Content::Agents,
            _ => Content::Auxiliary,
        };
        let surfaces = if mode == Screen::SettingsEdit {
            vec![Screen::SandboxPolicy, Screen::SettingsEdit]
        } else if content == Content::Auxiliary {
            vec![mode]
        } else {
            Vec::new()
        };
        Self {
            content,
            surfaces,
            overlays: if navigation.mode == Screen::Views {
                vec![Screen::Views]
            } else {
                Vec::new()
            },
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
        navigation.mode = Screen::SettingsEdit;
        let view = ApplicationView::from_navigation(&navigation);
        assert_eq!(view.content, Content::Auxiliary);
        assert_eq!(
            view.surfaces,
            vec![Screen::SandboxPolicy, Screen::SettingsEdit]
        );
        assert!(view.overlays.is_empty());
    }
}
