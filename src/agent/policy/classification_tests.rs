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
