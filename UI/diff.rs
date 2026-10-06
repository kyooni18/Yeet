//! Semantic actions for the shared Diff resource view.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stable Diff intents. Platforms attach these actions to their own hit targets
/// and translate keyboard gestures without passing renderer-local row indexes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum DiffAction {
    SelectFile(PathBuf),
    SetFull(bool),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_actions_roundtrip_stable_paths_and_display_mode() {
        let actions = [
            DiffAction::SelectFile(PathBuf::from("workspace/src/main.rs")),
            DiffAction::SetFull(true),
        ];
        for action in actions {
            let encoded = serde_json::to_string(&action).unwrap();
            let decoded: DiffAction = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, action);
        }
    }
}
