//! Generate browser test data from the same reducer used by the Remote host.
//! Run `cargo run --example ui_shell_fixture > web/tests/fixtures/ui-shell.json`.
use serde_json::json;
use yeet::shared_ui::shell::{Layout, ShellAction, ShellState};

fn main() {
    let actions = [
        ("set_layout:compact", ShellAction::SetLayout { layout: Layout::Compact }),
        ("set_layout:expanded", ShellAction::SetLayout { layout: Layout::Expanded }),
        ("open_navigation", ShellAction::OpenNavigation),
        ("close_navigation", ShellAction::CloseNavigation),
        ("toggle_inspector", ShellAction::ToggleInspector),
        ("close_inspector", ShellAction::CloseInspector),
        ("open_models", ShellAction::OpenModels),
        ("close_models", ShellAction::CloseModels),
        ("open_settings", ShellAction::OpenSettings),
        ("close_settings", ShellAction::CloseSettings),
        ("open_agents", ShellAction::OpenAgents),
        ("close_agents", ShellAction::CloseAgents),
        ("dismiss", ShellAction::Dismiss),
    ];
    let mut states = vec![ShellState::default()];
    let mut nodes = Vec::new();
    let mut index = 0;
    while index < states.len() {
        let current = states[index].clone();
        let mut transitions = serde_json::Map::new();
        for (key, action) in actions {
            let mut next = current.clone();
            next.apply(action);
            let target = match states.iter().position(|candidate| *candidate == next) {
                Some(target) => target,
                None => { states.push(next); states.len() - 1 }
            };
            transitions.insert(key.into(), json!(target));
        }
        nodes.push(json!({ "state": current, "view": current.view(), "transitions": transitions }));
        index += 1;
    }
    println!("{}", serde_json::to_string(&nodes).unwrap());
}
