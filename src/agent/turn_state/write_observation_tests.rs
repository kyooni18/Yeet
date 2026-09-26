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

#[test]
fn failed_write_observation_is_not_a_successful_mutation_but_remains_visible() {
    let mut evidence = TurnExecutionEvidence::default();
    let shell = shell_call("rm -rf target/tmp && false");
    evidence.observe_tool(
        &shell,
        r#"{"exitCode":1,"succeeded":false}"#,
        false,
        true,
        false,
        false,
    );

    assert_eq!(evidence.successful_mutations(), 0);
    assert!(evidence.unresolved_failed_mutation());
    let overlay = evidence.request_overlay().expect("failed write provenance");
    let text = overlay.content.as_deref().unwrap_or_default();
    assert!(text.contains("succeeded=false"));

    let successful_shell = shell_call("rm -rf target/tmp");
    evidence.observe_tool(
        &successful_shell,
        r#"{"exitCode":0,"succeeded":true}"#,
        true,
        true,
        false,
        false,
    );
    assert_eq!(evidence.successful_mutations(), 1);
    assert!(!evidence.unresolved_failed_mutation());
}
