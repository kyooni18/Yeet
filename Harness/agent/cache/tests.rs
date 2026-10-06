//! Tests for the parent module.

use super::*;
use crate::core::ToolDefinition;

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

fn tool(name: &str) -> ToolDefinition {
    ToolDefinition::new(
        name,
        format!("{name} description"),
        json!({"type":"object","properties":{}}),
    )
}

#[test]
fn cache_surface_stays_stable_as_only_the_post_breakpoint_trace_grows() {
    let tools = vec![
        tool("read_file"),
        tool("apply_file_edits"),
        tool("run_shell"),
    ];
    let first = request(tools.clone(), &["read source"]);
    let later = request(tools, &["read source", "edited", "verified"]);
    let first_diag = diagnostics(&first);
    let later_diag = diagnostics(&later);

    assert_eq!(
        first_diag["cacheSurfaceHash"],
        later_diag["cacheSurfaceHash"]
    );
    assert_ne!(first_diag["historyHash"], later_diag["historyHash"]);
}

#[test]
fn rolling_breakpoints_follow_recent_tool_results_without_mutating_old_trace() {
    let mut messages = vec![Message::system("stable"), Message::user("research it")];
    insert_turn_stable_overlays(
        &mut messages,
        "research it",
        &[Message::system("stable turn overlay").request_only()],
    );
    for index in 0..4 {
        messages.push(Message::assistant(format!("tool call {index}"), None));
        messages.push(Message::tool(
            format!("result {index}"),
            format!("call-{index}"),
            Some("web_read".into()),
        ));
    }

    advance_turn_cache_breakpoints(&mut messages, "research it");

    let breakpoints = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (message.cache_breakpoint == Some(true)).then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(breakpoints.len(), 5);
    assert_eq!(
        messages[breakpoints[0]].content.as_deref(),
        Some("research it")
    );
    assert_eq!(
        messages[breakpoints[1]].content.as_deref(),
        Some("stable turn overlay")
    );
    assert_eq!(
        messages[breakpoints[2]].content.as_deref(),
        Some("result 1")
    );
    assert_eq!(
        messages[breakpoints[3]].content.as_deref(),
        Some("result 2")
    );
    assert_eq!(
        messages[breakpoints[4]].content.as_deref(),
        Some("result 3")
    );
    assert_ne!(messages[4].cache_breakpoint, Some(true));
}

#[test]
fn stable_base_hash_survives_rolling_frontier_growth() {
    let tools = vec![tool("web_read")];
    let mut messages = vec![Message::system("stable"), Message::user("research it")];
    insert_turn_stable_overlays(
        &mut messages,
        "research it",
        &[Message::system("stable turn overlay").request_only()],
    );
    messages.push(Message::assistant("tool call 0", None));
    messages.push(Message::tool("result 0", "call-0", Some("web_read".into())));
    advance_turn_cache_breakpoints(&mut messages, "research it");

    let mut first = CallRequest::simple("openai/gpt-5.6-sol", messages.clone());
    first.context_key = Some("stable-session".into());
    first.prompt_cache = Some(true);
    first.tools = Some(tools.clone());
    let first_diag = diagnostics(&first);

    messages.push(Message::assistant("tool call 1", None));
    messages.push(Message::tool("result 1", "call-1", Some("web_read".into())));
    advance_turn_cache_breakpoints(&mut messages, "research it");
    let mut later = CallRequest::simple("openai/gpt-5.6-sol", messages);
    later.context_key = Some("stable-session".into());
    later.prompt_cache = Some(true);
    later.tools = Some(tools);
    let later_diag = diagnostics(&later);

    assert_eq!(first_diag["basePrefixHash"], later_diag["basePrefixHash"]);
    assert_eq!(
        first_diag["stablePrefixHash"],
        later_diag["stablePrefixHash"]
    );
    assert_eq!(
        first_diag["cacheSurfaceHash"],
        later_diag["cacheSurfaceHash"]
    );
    assert_ne!(
        first_diag["rollingFrontierHash"],
        later_diag["rollingFrontierHash"]
    );
    assert_ne!(
        first_diag["rollingCacheSurfaceHash"],
        later_diag["rollingCacheSurfaceHash"]
    );
}

#[test]
fn continuity_ignores_request_local_breakpoint_rotation() {
    let mut tracker = ContinuityTracker::default();
    let mut first = request(vec![tool("read_file")], &[]);
    first.metadata = Some(std::collections::HashMap::from([
        ("lane".into(), "agent".into()),
        ("contextWindowId".into(), "window-a".into()),
    ]));
    first.messages.push(Message::tool(
        "same evidence",
        "call-1",
        Some("read_file".into()),
    ));
    first.messages.last_mut().unwrap().cache_breakpoint = Some(true);
    let initial = tracker.diagnostics(&first);
    assert!(initial["historyPrefixContinues"].is_null());

    let mut second = first.clone();
    second.messages.last_mut().unwrap().cache_breakpoint = None;
    second.messages.push(Message::tool(
        "new evidence",
        "call-2",
        Some("read_file".into()),
    ));
    second.messages.last_mut().unwrap().cache_breakpoint = Some(true);
    let later = tracker.diagnostics(&second);
    assert_eq!(later["historyPrefixContinues"], json!(true));
    assert_eq!(later["historyPrefixRewriteDetected"], json!(false));
    assert_ne!(initial["breakpointPlanHash"], later["breakpointPlanHash"]);
    assert_eq!(initial["cacheEpoch"], 1);
    assert_eq!(later["cacheEpoch"], 1);
    assert_eq!(later["cacheEpochReason"], "stable");
}

#[test]
fn adding_a_tool_schema_changes_the_provider_cache_surface_but_not_affinity_family() {
    let first = request(vec![tool("read_file")], &[]);
    let promoted = request(vec![tool("read_file"), tool("apply_file_edits")], &[]);
    let first_diag = diagnostics(&first);
    let promoted_diag = diagnostics(&promoted);

    assert_ne!(
        first_diag["toolSchemaHash"],
        promoted_diag["toolSchemaHash"]
    );
    assert_ne!(
        first_diag["cacheSurfaceHash"],
        promoted_diag["cacheSurfaceHash"]
    );
    assert_eq!(
        first_diag["promptCacheKeyHash"],
        promoted_diag["promptCacheKeyHash"]
    );
}

#[test]
fn logical_tool_hashes_ignore_reordering_while_wire_envelope_detects_it() {
    let mut first = request(vec![tool("alpha"), tool("beta")], &[]);
    first.deferred_tools = Some(vec![tool("mcp_alpha"), tool("mcp_beta")]);
    let mut reordered = request(vec![tool("beta"), tool("alpha")], &[]);
    reordered.deferred_tools = Some(vec![tool("mcp_beta"), tool("mcp_alpha")]);

    let first_diag = diagnostics(&first);
    let reordered_diag = diagnostics(&reordered);

    assert_eq!(
        first_diag["toolSchemaHash"],
        reordered_diag["toolSchemaHash"]
    );
    assert_eq!(
        first_diag["deferredToolSchemaHash"],
        reordered_diag["deferredToolSchemaHash"]
    );
    assert_ne!(
        first_diag["wireAttachedToolSchemaHash"],
        reordered_diag["wireAttachedToolSchemaHash"]
    );
    assert_ne!(
        first_diag["wireDeferredToolSchemaHash"],
        reordered_diag["wireDeferredToolSchemaHash"]
    );
    assert_ne!(first_diag["toolOrderHash"], reordered_diag["toolOrderHash"]);
    assert_ne!(
        first_diag["deferredToolOrderHash"],
        reordered_diag["deferredToolOrderHash"]
    );
    assert_ne!(
        first_diag["toolEnvelopeHash"],
        reordered_diag["toolEnvelopeHash"]
    );
    assert_ne!(
        first_diag["cacheSurfaceHash"],
        reordered_diag["cacheSurfaceHash"]
    );
}

#[test]
fn deferred_tool_definitions_are_part_of_the_wire_tool_envelope() {
    let first = request(vec![tool("read_file")], &[]);
    let mut later = first.clone();
    later.deferred_tools = Some(vec![tool("mcp_krpc_observe")]);

    let first_diag = diagnostics(&first);
    let later_diag = diagnostics(&later);
    assert_ne!(
        first_diag["cacheSurfaceHash"],
        later_diag["cacheSurfaceHash"]
    );
    assert_ne!(
        first_diag["toolEnvelopeHash"],
        later_diag["toolEnvelopeHash"]
    );
    assert_ne!(
        first_diag["deferredToolSchemaHash"],
        later_diag["deferredToolSchemaHash"]
    );
    assert_eq!(
        first_diag["promptCacheKeyHash"],
        later_diag["promptCacheKeyHash"]
    );
    assert!(
        later_diag["wireToolSchemaChars"].as_u64().unwrap()
            > later_diag["toolSchemaChars"].as_u64().unwrap()
    );
}

#[test]
fn continuity_tracker_accepts_append_only_history_growth() {
    let tools = vec![tool("read_file")];
    let first = request(tools.clone(), &["read source"]);
    let later = request(tools, &["read source", "edited", "verified"]);
    let mut tracker = ContinuityTracker::default();

    let first_diag = tracker.diagnostics(&first);
    let later_diag = tracker.diagnostics(&later);

    assert_eq!(first_diag["historyPrefixContinues"], Value::Null);
    assert_eq!(later_diag["historyPrefixContinues"], true);
    assert_eq!(later_diag["historyPrefixRewriteDetected"], false);
    assert_eq!(later_diag["firstDivergentHistoryMessageIndex"], Value::Null);
    assert_eq!(first_diag["cacheEpoch"], 1);
    assert_eq!(later_diag["cacheEpoch"], 1);
    assert_eq!(later_diag["cacheEpochReason"], "stable");
}

#[test]
fn current_turn_request_only_checkpoints_remain_wire_append_only() {
    let tools = vec![tool("read_file")];
    let mut first = request(tools.clone(), &[]);
    first.metadata = Some(std::collections::HashMap::from([
        ("lane".into(), "agent".into()),
        ("contextWindowId".into(), "window-a".into()),
    ]));
    first.messages.push(Message::assistant("tool call 0", None));
    first.messages.push(Message::tool(
        "result 0",
        "call-0",
        Some("read_file".into()),
    ));
    first
        .messages
        .push(Message::system("retry checkpoint 1").request_only());

    let mut later = first.clone();
    later.messages.push(Message::assistant("tool call 1", None));
    later.messages.push(Message::tool(
        "result 1",
        "call-1",
        Some("read_file".into()),
    ));
    later
        .messages
        .push(Message::system("provenance checkpoint 2").request_only());

    let mut tracker = ContinuityTracker::default();
    let first_diag = tracker.diagnostics(&first);
    let later_diag = tracker.diagnostics(&later);
    assert_eq!(first_diag["cacheEpochReason"], "window-start");
    assert_eq!(later_diag["historyPrefixContinues"], true);
    assert_eq!(later_diag["wireHistoryPrefixContinues"], true);
    assert_eq!(later_diag["wireHistoryPrefixRewriteDetected"], false);
    assert_eq!(later_diag["cacheRelevantHistoryRewriteDetected"], false);
    assert_eq!(later_diag["cacheEpochReason"], "stable");
    assert_eq!(later_diag["cacheEpoch"], 1);
}

#[test]
fn same_session_runtime_rebind_preserves_cache_continuity() {
    let request = request(vec![tool("read_file")], &["read source"]);
    let mut tracker = ContinuityTracker::default();
    let first = tracker.diagnostics(&request);

    tracker.rebind_session(Some("session-a"), Some("session-a"));
    let same_session = tracker.diagnostics(&request);
    assert_eq!(same_session["cacheEpoch"], 1);
    assert_eq!(same_session["cacheEpochReason"], "stable");

    tracker.rebind_session(Some("session-a"), Some("session-b"));
    let changed_session = tracker.diagnostics(&request);
    assert_eq!(changed_session["cacheEpoch"], 1);
    assert_eq!(changed_session["cacheEpochReason"], "window-start");
    assert_eq!(first["cacheEpochReason"], "window-start");
}

#[test]
fn continuity_tracks_append_only_new_turn_and_resets_on_new_window() {
    let mut tracker = ContinuityTracker::default();
    let mut first = request(vec![tool("read_file")], &[]);
    first.metadata = Some(std::collections::HashMap::from([
        ("lane".into(), "agent".into()),
        ("contextWindowId".into(), "window-a".into()),
    ]));
    first
        .messages
        .insert(2, Message::system("turn-only overlay").request_only());
    let first_diag = tracker.diagnostics(&first);

    let mut next_turn = first.clone();
    next_turn
        .messages
        .retain(|message| message.request_only != Some(true));
    next_turn.messages[1].cache_breakpoint = None;
    next_turn.messages.push(Message::assistant("done", None));
    next_turn
        .messages
        .push(Message::user("next task").cache_breakpoint());
    let next_diag = tracker.diagnostics(&next_turn);

    assert_eq!(first_diag["cacheEpoch"], 1);
    assert_eq!(first_diag["cacheEpochReason"], "window-start");
    assert_eq!(next_diag["historyPrefixContinues"], true);
    assert_eq!(next_diag["historyPrefixRewriteDetected"], false);
    assert_eq!(next_diag["basePrefixContinues"], true);
    assert_eq!(next_diag["cacheRelevantHistoryRewriteDetected"], false);
    assert_ne!(first_diag["basePrefixHash"], next_diag["basePrefixHash"]);
    assert_eq!(
        first_diag["promptCacheKeyHash"],
        next_diag["promptCacheKeyHash"]
    );
    assert_eq!(next_diag["cacheEpoch"], 1);
    assert_eq!(next_diag["cacheEpochReason"], "stable");

    next_turn
        .metadata
        .as_mut()
        .unwrap()
        .insert("contextWindowId".into(), "window-b".into());
    let new_window_diag = tracker.diagnostics(&next_turn);
    assert_eq!(new_window_diag["historyPrefixContinues"], Value::Null);
    assert_eq!(new_window_diag["historyPrefixRewriteDetected"], false);
    assert_eq!(new_window_diag["cacheEpoch"], 1);
    assert_eq!(new_window_diag["cacheEpochReason"], "window-start");
}

#[test]
fn skill_attachment_state_changes_append_without_rewriting_cached_history() {
    let mut tracker = ContinuityTracker::default();
    let mut request = request(vec![tool("read_file")], &[]);
    request.metadata = Some(std::collections::HashMap::from([
        ("lane".into(), "agent".into()),
        ("contextWindowId".into(), "skill-cache-window".into()),
    ]));
    let original_history = request.messages.clone();
    let first = tracker.diagnostics(&request);

    request.messages.push(Message::assistant("done", None));
    request.messages.push(Message::system(
        "Attached Skill: skyline\nPersistent instructions",
    ));
    request.messages.push(Message::user("continue"));
    let attached = tracker.diagnostics(&request);
    assert_eq!(request.messages[..original_history.len()], original_history);
    assert_eq!(attached["historyPrefixContinues"], true);
    assert_eq!(attached["historyPrefixRewriteDetected"], false);
    assert_eq!(attached["cacheEpoch"], first["cacheEpoch"]);
    assert_eq!(attached["cacheEpochReason"], "stable");

    let before_detach = request.messages.clone();
    request
        .messages
        .push(Message::assistant("done again", None));
    request.messages.push(Message::system(
        "Skill attachment state: skyline detached. Earlier history remains unchanged.",
    ));
    request.messages.push(Message::user("continue without it"));
    let detached = tracker.diagnostics(&request);
    assert_eq!(request.messages[..before_detach.len()], before_detach);
    assert_eq!(detached["historyPrefixContinues"], true);
    assert_eq!(detached["historyPrefixRewriteDetected"], false);
    assert_eq!(detached["cacheEpoch"], first["cacheEpoch"]);
    assert_eq!(detached["cacheEpochReason"], "stable");
}

#[test]
fn continuity_tracker_detects_rewriting_older_tool_history() {
    let tools = vec![tool("read_file")];
    let first = request(
        tools.clone(),
        &["large original tool result", "later result"],
    );
    let mut rewritten = request(
        tools,
        &["compacted old result", "later result", "new result"],
    );
    let mut tracker = ContinuityTracker::default();

    let initial = tracker.diagnostics(&first);
    let result = tracker.diagnostics(&rewritten);

    assert_eq!(initial["cacheEpoch"], 1);
    assert_eq!(initial["cacheEpochReason"], "window-start");
    assert_eq!(result["historyPrefixContinues"], false);
    assert_eq!(result["historyPrefixRewriteDetected"], true);
    assert_eq!(result["cacheRelevantHistoryRewriteDetected"], true);
    assert_eq!(result["firstDivergentHistoryMessageIndex"], 2);
    assert_eq!(result["cacheEpoch"], 2);
    assert_eq!(result["cacheEpochReason"], "history-rewrite");

    rewritten
        .messages
        .push(Message::assistant("post-rewrite append", None));
    let recovered = tracker.diagnostics(&rewritten);
    assert_eq!(recovered["historyPrefixContinues"], true);
    assert_eq!(recovered["historyPrefixRewriteDetected"], false);
    assert_eq!(recovered["cacheEpoch"], 2);
    assert_eq!(recovered["cacheEpochReason"], "stable");
}

#[test]
fn cache_usage_diagnostics_separate_hits_writes_and_ordinary_input() {
    let request = request(vec![tool("read_file")], &[]);
    let diagnostics = diagnostics(&request);
    let usage = Usage {
        input_tokens: Some(10_000),
        cached_input_tokens: Some(6_000),
        cache_write_input_tokens: Some(1_500),
        cost_equivalent_input_tokens: Some(4_975),
        transport_attempts: Some(2),
        ..Default::default()
    };
    let result = usage_diagnostics(&diagnostics, Some(&usage));

    assert_eq!(result["cacheStatus"], "hit");
    assert_eq!(result["inputTokens"], 10_000);
    assert_eq!(result["cachedInputTokens"], 6_000);
    assert_eq!(result["cacheWriteInputTokens"], 1_500);
    assert_eq!(result["ordinaryInputTokens"], 2_500);
    assert_eq!(result["costEquivalentInputTokens"], 4_975);
    assert_eq!(result["transportAttempts"], 2);
    assert_eq!(result["cacheHitRate"], 0.6);
    assert_eq!(result["cacheWriteRate"], 0.15);
}

#[test]
fn enabled_cache_with_measured_input_and_no_hit_is_reported_as_a_miss() {
    let request = request(vec![tool("read_file")], &[]);
    let diagnostics = diagnostics(&request);
    let usage = Usage {
        input_tokens: Some(2_048),
        cached_input_tokens: Some(0),
        ..Default::default()
    };
    let result = usage_diagnostics(&diagnostics, Some(&usage));

    assert_eq!(result["cacheStatus"], "miss");
    assert_eq!(result["ordinaryInputTokens"], 2_048);
    assert_eq!(result["cacheHitRate"], 0.0);
}

#[test]
fn missing_provider_cache_details_are_unreported_not_a_false_miss() {
    let request = request(vec![tool("read_file")], &[]);
    let diagnostics = diagnostics(&request);
    let usage = Usage {
        input_tokens: Some(2_048),
        ..Default::default()
    };
    let result = usage_diagnostics(&diagnostics, Some(&usage));

    assert_eq!(result["cacheStatus"], "unreported");
    assert_eq!(result["cachedInputTokens"], Value::Null);
    assert_eq!(result["ordinaryInputTokens"], Value::Null);
    assert_eq!(result["cacheHitRate"], Value::Null);
}

#[test]
fn provider_cache_miss_diagnostics_remain_separate_from_client_epoch_reason() {
    let request = request(vec![tool("read_file")], &[]);
    let mut diagnostics = diagnostics(&request);
    diagnostics["cacheEpochReason"] = json!("stable");
    let usage = Usage {
        input_tokens: Some(4_096),
        cached_input_tokens: Some(0),
        provider_cache_diagnostic_type: Some("cache_miss".into()),
        provider_cache_miss_reason: Some("input_changed".into()),
        provider_cache_missed_tokens: Some(1_024),
        provider_comparison_reusable_tokens: Some(3_072),
        ..Default::default()
    };
    let result = usage_diagnostics(&diagnostics, Some(&usage));

    assert_eq!(result["cacheStatus"], "miss");
    assert_eq!(result["cacheEpochReason"], "stable");
    assert_eq!(result["providerCacheDiagnosticType"], "cache_miss");
    assert_eq!(result["providerCacheMissReason"], "input_changed");
    assert_eq!(result["providerCacheMissedTokens"], 1_024);
    assert_eq!(result["providerComparisonReusableTokens"], 3_072);
    assert_eq!(result["cacheMissAttribution"], "provider-reported");
}
