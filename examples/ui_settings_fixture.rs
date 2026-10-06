//! Browser fake Harness using the production shared settings reducer.
use serde::Deserialize;
use serde_json::json;
use std::io::{self, BufRead};
use yeet::{harness::HarnessState, shared_ui::settings::*};
#[derive(Deserialize)]
struct Input {
    state: HarnessState,
    environment: SettingsEnvironment,
    #[serde(default)]
    ui_state: SettingsState,
    #[serde(default)]
    action: Option<SettingsAction>,
}
fn main() {
    for line in io::stdin().lock().lines() {
        let input: Input =
            serde_json::from_str(&line.expect("read fixture")).expect("decode fixture");
        let mut ui = input.ui_state;
        ui.reconcile(&input.environment, &input.state);
        let effect = input
            .action
            .map(|action| ui.apply(action, &input.environment, &input.state))
            .unwrap_or_default();
        println!(
            "{}",
            json!({"ui_state":ui,"view":ui.view(&input.environment,&input.state),"effect":effect.ui,"command":effect.command})
        );
    }
}
