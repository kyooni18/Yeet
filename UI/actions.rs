//! Application actions independent of input devices and visual frameworks.

use crate::harness::HarnessCommand;
use serde::{Deserialize, Serialize};

/// Navigation intents retained on the existing wire contract.
///
/// File and diff positions are compatibility identities at the wire boundary.
/// Adapters must translate them into stable view identities before retaining
/// actions, so closing a view cannot retarget a queued interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "index", rename_all = "snake_case")]
pub enum NavigationAction {
    Home,
    Session,
    Files,
    File(usize),
    CloseFile(usize),
    Diff(usize),
    CloseDiff(usize),
    NewDiff,
    Launcher,
}

/// UI navigation stays local; only runtime commands cross into Harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "scope", content = "action", rename_all = "snake_case")]
pub enum Action {
    Navigate(NavigationAction),
    Command(HarnessCommand),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_actions_preserve_existing_wire_contract() {
        let fixtures = [
            (
                NavigationAction::Home,
                serde_json::json!({ "type": "home" }),
            ),
            (
                NavigationAction::Session,
                serde_json::json!({ "type": "session" }),
            ),
            (
                NavigationAction::Files,
                serde_json::json!({ "type": "files" }),
            ),
            (
                NavigationAction::File(2),
                serde_json::json!({ "type": "file", "index": 2 }),
            ),
            (
                NavigationAction::CloseFile(2),
                serde_json::json!({ "type": "close_file", "index": 2 }),
            ),
            (
                NavigationAction::Diff(2),
                serde_json::json!({ "type": "diff", "index": 2 }),
            ),
            (
                NavigationAction::CloseDiff(2),
                serde_json::json!({ "type": "close_diff", "index": 2 }),
            ),
            (
                NavigationAction::NewDiff,
                serde_json::json!({ "type": "new_diff" }),
            ),
            (
                NavigationAction::Launcher,
                serde_json::json!({ "type": "launcher" }),
            ),
        ];
        for (navigation, payload) in fixtures {
            let expected = serde_json::json!({ "scope": "navigate", "action": payload });
            assert_eq!(
                serde_json::to_value(Action::Navigate(navigation)).unwrap(),
                expected
            );
            let decoded: Action = serde_json::from_value(expected).unwrap();
            assert!(matches!(decoded, Action::Navigate(value) if value == navigation));
        }
        let expected = serde_json::json!({
            "scope": "command", "action": { "type": "submit", "text": "hello" }
        });
        let decoded: Action = serde_json::from_value(expected.clone()).unwrap();
        assert!(
            matches!(&decoded, Action::Command(HarnessCommand::Submit { text, .. }) if text == "hello")
        );
        assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
    }
}
