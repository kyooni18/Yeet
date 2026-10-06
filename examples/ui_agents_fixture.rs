//! Browser mock boundary backed by the production UI reducer, with no agent I/O.
use serde::Deserialize;
use serde_json::json;
use std::io::{self, BufRead};
use yeet::{
    harness::HarnessState,
    shared_ui::{
        agents::{AgentAction, AgentState},
        application_session::ApplicationSession,
    },
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
        let input: Input = serde_json::from_str(&line).expect("decode fixture input");
        let mut application = ApplicationSession::new(&input.state);
        *application.compatibility_agent_state_mut() = input.ui_state;
        application.refresh(&input.state);
        let effect = input
            .action
            .map(|action| {
                let prepared = application.prepare_agent(action, &input.state);
                // This fixture's fake Harness accepts generated commands without I/O.
                application.commit_agent(prepared, &input.state).1
            })
            .unwrap_or_default();
        println!(
            "{}",
            json!({"ui_state": application.agent_state(),
            "view": application.agents_projection().view, "effect": effect})
        );
    }
}
