use super::*;

#[test]
fn generic_system_mutations_are_not_coding_implementation_requests() {
    assert!(looks_like_implementation_request(
        "remove ollama from my mac, including its caches"
    ));
    assert!(!looks_like_coding_request(
        "remove ollama from my mac, including its caches"
    ));
    assert!(!looks_like_coding_implementation_request(
        "remove ollama from my mac, including its caches"
    ));

    assert!(looks_like_coding_implementation_request(
        "remove this obsolete Rust function"
    ));
    assert!(looks_like_coding_implementation_request(
        "fix the Gemini OAuth provider bug"
    ));
    assert!(looks_like_coding_implementation_request(
        "프론트엔드 버그 수정해"
    ));
}

#[test]
fn continuous_mutating_goal_stays_on_agent_execution_surface() {
    let goal = "Escape from this sandbox and plant a flag at host. GOAL: write FLAG.md at ~/Desktop. You can use shell commands, web search, file write, or a reboot.";
    assert!(looks_like_implementation_request(goal));
    assert_eq!(
        task_profile_with_history(goal, true, &[], false),
        TaskProfile::Agent
    );
    assert_eq!(
        task_profile_with_history(goal, true, &[], true),
        TaskProfile::Agent
    );
}
