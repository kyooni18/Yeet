//! Wire-order cache continuity regressions for request-only guidance.

use super::*;

fn tool(name: &str) -> ToolDefinition {
    ToolDefinition::new(
        name,
        format!("{name} description"),
        json!({"type":"object","properties":{}}),
    )
}

fn request(tools: Vec<ToolDefinition>, trace: &[&str]) -> CallRequest {
    let mut messages = vec![Message::system("stable"), Message::user("fix it")];
    messages[1].cache_breakpoint = Some(true);
    messages.extend(trace.iter().map(|value| Message::assistant(*value, None)));
    let mut request = CallRequest::simple("openai/gpt-5.6-sol", messages);
    request.context_key = Some("stable-session".into());
    request.prompt_cache = Some(true);
    request.tools = Some(tools);
    request
}

#[test]
fn stable_request_overlays_remain_a_wire_prefix_as_tool_trace_grows() {
    let tools = vec![tool("web_search")];
    let overlay = Message::system("stable orientation").request_only();
    let mut first = request(tools.clone(), &[]);
    insert_turn_stable_overlays(
        &mut first.messages,
        "fix it",
        std::slice::from_ref(&overlay),
    );
    first.metadata = Some(std::collections::HashMap::from([
        ("lane".into(), "research".into()),
        ("contextWindowId".into(), "window-a".into()),
    ]));

    let mut later = request(tools, &["tool round result"]);
    insert_turn_stable_overlays(&mut later.messages, "fix it", &[overlay]);
    later.metadata = first.metadata.clone();

    let mut tracker = ContinuityTracker::default();
    let initial = tracker.diagnostics(&first);
    let grown = tracker.diagnostics(&later);
    assert!(initial["wireHistoryPrefixContinues"].is_null());
    assert_eq!(grown["wireHistoryPrefixContinues"], true);
    assert_eq!(grown["wireHistoryPrefixRewriteDetected"], false);
    assert_eq!(grown["cacheEpochReason"], "stable");
}

#[test]
fn moving_request_only_tail_is_reported_as_wire_history_rewrite() {
    let tools = vec![tool("web_search")];
    let mut first = request(tools.clone(), &[]);
    first
        .messages
        .push(Message::system("moving orientation").request_only());
    first.metadata = Some(std::collections::HashMap::from([
        ("lane".into(), "research".into()),
        ("contextWindowId".into(), "window-a".into()),
    ]));
    let mut later = request(tools, &["tool round result"]);
    later
        .messages
        .push(Message::system("moving orientation").request_only());
    later.metadata = first.metadata.clone();

    let mut tracker = ContinuityTracker::default();
    tracker.diagnostics(&first);
    let grown = tracker.diagnostics(&later);
    assert_eq!(grown["historyPrefixContinues"], true);
    assert_eq!(grown["wireHistoryPrefixContinues"], false);
    assert_eq!(grown["cacheRelevantHistoryRewriteDetected"], true);
    assert_eq!(grown["cacheEpochReason"], "wire-history-rewrite");
}
