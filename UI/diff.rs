//! Semantic actions for the shared Diff resource view.
use super::surfaces::SurfaceId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Host-supplied resource inventory. Git and filesystem collection stay in
/// Harness; this DTO gives ApplicationSession the current valid targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffContent {
    pub id: SurfaceId,
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub selected: Option<PathBuf>,
}

/// Shared application state for one open Diff surface. Patch rendering and
/// scroll position remain platform-local.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffView {
    pub id: SurfaceId,
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub selected: Option<PathBuf>,
    pub full: bool,
}

/// Stable Diff intents. Platforms attach these actions to their own hit targets
/// and translate keyboard gestures without passing renderer-local row indexes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum DiffAction {
    SelectFile { view: SurfaceId, target: PathBuf },
    SetFull { view: SurfaceId, full: bool },
}
impl DiffAction {
    pub fn view(&self) -> SurfaceId {
        match self {
            Self::SelectFile { view, .. } | Self::SetFull { view, .. } => *view,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_actions_roundtrip_stable_paths_and_display_mode() {
        let mut tabs = crate::shared_ui::surfaces::Tabs::default();
        let view = tabs.open("Diff", ());
        let actions = [
            DiffAction::SelectFile {
                view,
                target: PathBuf::from("workspace/src/main.rs"),
            },
            DiffAction::SetFull { view, full: true },
        ];
        for action in actions {
            let encoded = serde_json::to_string(&action).unwrap();
            let decoded: DiffAction = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, action);
        }
    }
}
