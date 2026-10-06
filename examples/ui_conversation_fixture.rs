//! Browser fake Harness backed by the production shared transcript reducer.
use serde::Deserialize;
use serde_json::json;
use std::io::{self, BufRead};
use yeet::{
    harness::HarnessState,
    shared_ui::conversation::{ConversationAction, ConversationState},
};
#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    state: serde_json::Value,
    #[serde(default)]
    ui_state: ConversationState,
    #[serde(default)]
    action: Option<ConversationAction>,
}
fn main() {
    for line in io::stdin().lock().lines() {
        let mut input: Input =
            serde_json::from_str(&line.expect("read fixture")).expect("decode fixture");
        // The fake browser Harness can supply an unfinished tool before its
        // name arrives. Runtime's typed representation uses an empty name.
        normalize_unfinished_tools(&mut input.state);
        let harness: HarnessState =
            serde_json::from_value(input.state).expect("decode fake Harness state");
        let mut state = input.ui_state;
        let entries = harness.conversation.as_deref().unwrap_or_default();
        let view = state.project(entries, &harness);
        let effect = input
            .action
            .map(|action| state.apply(action, &view, &harness))
            .unwrap_or_default();
        println!(
            "{}",
            json!({"ui_state":state,"view":state.project(entries,&harness),"effect":effect})
        );
    }
}

fn normalize_unfinished_tools(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(items) => items.iter_mut().for_each(normalize_unfinished_tools),
        serde_json::Value::Object(object) => {
            if object.contains_key("arguments")
                && object.contains_key("status")
                && object.contains_key("id")
            {
                object
                    .entry("name")
                    .or_insert_with(|| serde_json::Value::String(String::new()));
            }
            object.values_mut().for_each(normalize_unfinished_tools);
        }
        _ => {}
    }
}
