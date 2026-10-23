use serde_json::json;

use super::TurnExecutionEvidence;
use crate::core::ToolCall;

fn shell_call(command: &str) -> ToolCall {
    ToolCall {
        id: command.to_owned(),
        name: "run_shell".into(),
        arguments: json!({"command": command}),
    }
}

#[test]
fn later_mutation_invalidates_previous_validation() {
    let mut evidence = TurnExecutionEvidence::default();
    evidence.observe_tool(
        &shell_call("cargo test"),
        r#"{"exitCode":0,"succeeded":true}"#,
        true,
        false,
        true,
        false,
    );
    assert!(evidence.verification_succeeded());

    let edit = ToolCall {
        id: "edit".into(),
        name: "apply_file_edits".into(),
        arguments: json!({"changes":[{"path":"src/lib.rs","edits":[]}]}),
    };
    evidence.observe_tool(&edit, r#"{"changed":true}"#, true, true, false, true);

    assert_eq!(evidence.successful_mutations(), 1);
    assert!(!evidence.verification_attempted());
    assert!(!evidence.verification_succeeded());
    assert!(evidence.last_validation_evidence().is_none());
}
