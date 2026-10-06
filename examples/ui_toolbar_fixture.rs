//! Browser fake Harness executes production toolbar projection and controller.
use serde::Deserialize;
use serde_json::json;
use std::io::{self, BufRead};
use yeet::{
    harness::HarnessState,
    shared_ui::{
        application_session::ApplicationSession,
        shell::{ShellAction, ShellState},
        toolbar::ToolbarAction,
    },
};
#[derive(Deserialize)]
struct Input {
    state: HarnessState,
    shell: ShellState,
    action: Option<ToolbarAction>,
}
fn main() {
    for line in io::stdin().lock().lines() {
        let input: Input = serde_json::from_str(&line.unwrap()).unwrap();
        let mut ui = ApplicationSession::new(&input.state);
        ui.apply_shell(
            ShellAction::SetLayout {
                layout: input.shell.layout,
            },
            &input.state,
        );
        for (open, action) in [
            (input.shell.navigation, ShellAction::OpenNavigation),
            (input.shell.inspector, ShellAction::ToggleInspector),
            (input.shell.models, ShellAction::OpenModels),
            (input.shell.settings, ShellAction::OpenSettings),
            (input.shell.agents, ShellAction::OpenAgents),
        ] {
            if open {
                ui.apply_shell(action, &input.state);
            }
        }
        if !input.shell.navigation {
            ui.apply_shell(ShellAction::CloseNavigation, &input.state);
        }
        let mut command = None;
        if let Some(action) = input.action {
            let prepared = ui.prepare_toolbar(action, &input.state);
            command = prepared.effect.command.clone();
            ui.commit_toolbar(prepared, &input.state);
        }
        println!(
            "{}",
            json!({"projection":ui.projection(),"command":command})
        );
    }
}
