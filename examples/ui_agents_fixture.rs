//! Browser mock boundary backed by the production UI reducer, with no agent I/O.
use serde::Deserialize;
use serde_json::json;
use std::io::{self, BufRead};
use yeet::{
    harness::HarnessState,
    shared_ui::agents::{AgentAction, AgentState},
};

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    state: HarnessState,
    #[serde(default)]
    ui_state: AgentState,
    #[serde(default)]
    action: Option<AgentAction>,
}
fn main() {
    for line in io::stdin().lock().lines() {
        let line = line.expect("read fixture input");
        let mut input: Input = serde_json::from_str(&line).expect("decode fixture input");
        let effect = input
            .action
            .map(|action| input.ui_state.apply(action, &input.state))
            .unwrap_or_default();
        input.ui_state.reconcile(&input.state.agent_group);
        println!(
            "{}",
            json!({"ui_state":input.ui_state,"view":input.ui_state.view(&input.state),"effect":effect})
        );
    }
}
