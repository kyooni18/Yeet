//! Focused tests for history compaction and retention behavior.

use super::*;
use crate::core::ToolCall;

#[test]
fn completed_task_history_discards_raw_tool_traces() {
    let mut history = vec![
            Message::system("system"),
            Message::user("inspect"),
            Message::assistant("", Some(vec![ToolCall {
                id: "read-1".into(),
                name: "read_file".into(),
                arguments: json!({"path":"src/lib.rs","startLine":1,"endLine":160,"refresh":true}),
            }])),
            Message::tool(
                json!({"path":"src/lib.rs","snapshot":"s1","startLine":1,"endLine":160,"totalLines":500,"lines":"x".repeat(20_000)}).to_string(),
                "read-1",
                Some("read_file".into()),
            ),
            Message::assistant("done", None),
        ];

    compact_completed_task_history(&mut history);

    let tool = history[3].content.as_deref().unwrap();
    assert!(!tool.contains("\"lines\""));
    assert!(tool.contains("contentOmitted"));
    assert_eq!(
        history[2].tool_calls.as_ref().unwrap()[0].arguments,
        json!({"path":"src/lib.rs","startLine":1,"endLine":160})
    );
    assert_eq!(history[4].content.as_deref(), Some("done"));
}

#[test]
fn current_turn_keeps_two_hot_tool_batches_and_compacts_older_evidence() {
    let mut history = vec![Message::system("system"), Message::user("inspect")];
    for index in 0..4 {
        let id = format!("read-{index}");
        history.push(Message::assistant(
                "",
                Some(vec![ToolCall {
                    id: id.clone(),
                    name: "read_file".into(),
                    arguments: json!({"path":format!("src/{index}.rs"),"startLine":1,"endLine":160,"refresh":true}),
                }]),
            ));
        history.push(Message::tool(
            json!({
                "path":format!("src/{index}.rs"),
                "snapshot":format!("s{index}"),
                "startLine":1,
                "endLine":160,
                "totalLines":500,
                "lines":"x".repeat(8_000)
            })
            .to_string(),
            id,
            Some("read_file".into()),
        ));
    }

    let mut compaction_end = None;
    assert!(compact_older_current_turn_tool_history(
        &mut history,
        1,
        16_000,
        8_000,
        &mut compaction_end,
    ));
    assert_eq!(compaction_end, Some(6));
    for tool_index in [3usize, 5] {
        let content = history[tool_index].content.as_deref().unwrap();
        assert!(content.contains("contentOmitted"));
        assert!(!content.contains("\"lines\""));
    }
    for tool_index in [7usize, 9] {
        let content = history[tool_index].content.as_deref().unwrap();
        assert!(content.contains("\"lines\""));
    }
}

#[test]
fn current_turn_compaction_waits_for_evidence_pressure() {
    let mut history = vec![Message::system("system"), Message::user("inspect")];
    for index in 0..3 {
        let id = format!("small-{index}");
        history.push(Message::assistant(
            "",
            Some(vec![ToolCall {
                id: id.clone(),
                name: "read_file".into(),
                arguments: json!({"path":format!("src/{index}.rs")}),
            }]),
        ));
        history.push(Message::tool(
            json!({"path":format!("src/{index}.rs"),"lines":"tiny"}).to_string(),
            id,
            Some("read_file".into()),
        ));
    }
    let original = history.clone();
    let mut compaction_end = None;
    assert!(!compact_older_current_turn_tool_history(
        &mut history,
        1,
        65_536,
        50_000,
        &mut compaction_end,
    ));
    assert_eq!(compaction_end, None);
    assert_eq!(history, original);
}

#[test]
fn current_turn_many_small_batches_do_not_force_cache_prefix_rewrite() {
    let mut history = vec![Message::system("system"), Message::user("inspect")];
    for index in 0..7 {
        let id = format!("small-{index}");
        history.push(Message::assistant(
            "",
            Some(vec![ToolCall {
                id: id.clone(),
                name: "read_file".into(),
                arguments: json!({"path":format!("src/{index}.rs")}),
            }]),
        ));
        history.push(Message::tool(
            json!({"path":format!("src/{index}.rs"),"lines":"x".repeat(4_000)}).to_string(),
            id,
            Some("read_file".into()),
        ));
    }

    // Seven batches exceeded the old count-only trigger, but ~9.4k model
    // tokens remain below 20% of a 65,536-token working budget (~13.1k).
    let original = history.clone();
    let mut compaction_end = None;
    assert!(!compact_older_current_turn_tool_history(
        &mut history,
        1,
        65_536,
        50_000,
        &mut compaction_end,
    ));
    assert_eq!(compaction_end, None);
    assert_eq!(history, original);
}

#[test]
fn current_turn_tool_payload_alone_does_not_force_cache_prefix_rewrite() {
    let mut history = vec![Message::system("system"), Message::user("inspect")];
    for index in 0..7 {
        let id = format!("large-{index}");
        history.push(Message::assistant(
            "",
            Some(vec![ToolCall {
                id: id.clone(),
                name: "read_file".into(),
                arguments: json!({"path":format!("src/{index}.rs")}),
            }]),
        ));
        history.push(Message::tool(
            json!({"path":format!("src/{index}.rs"),"lines":"x".repeat(6_000)}).to_string(),
            id,
            Some("read_file".into()),
        ));
    }

    // This exceeds the old 20%-of-budget tool-payload trigger (~13.1k
    // estimated tool tokens) but the whole request is still far below 60% of
    // the 65,536-token working budget, so rewriting the cache prefix is wasteful.
    let original = history.clone();
    let mut compaction_end = None;
    assert!(!compact_older_current_turn_tool_history(
        &mut history,
        1,
        65_536,
        50_000,
        &mut compaction_end,
    ));
    assert_eq!(compaction_end, None);
    assert_eq!(history, original);
}

#[test]
fn current_turn_compaction_frontier_stays_fixed_as_new_batches_arrive() {
    let mut canonical = vec![Message::system("system"), Message::user("inspect")];
    for index in 0..4 {
        let id = format!("read-{index}");
        canonical.push(Message::assistant(
            "",
            Some(vec![ToolCall {
                id: id.clone(),
                name: "read_file".into(),
                arguments: json!({"path":format!("src/{index}.rs")}),
            }]),
        ));
        canonical.push(Message::tool(
            json!({"path":format!("src/{index}.rs"),"lines":"x".repeat(8_000)}).to_string(),
            id,
            Some("read_file".into()),
        ));
    }

    let mut compaction_end = None;
    let mut first_request = canonical.clone();
    assert!(compact_older_current_turn_tool_history(
        &mut first_request,
        1,
        16_000,
        8_000,
        &mut compaction_end,
    ));
    let fixed_frontier = compaction_end;
    assert_eq!(fixed_frontier, Some(6));

    let id = "read-4".to_owned();
    canonical.push(Message::assistant(
        "",
        Some(vec![ToolCall {
            id: id.clone(),
            name: "read_file".into(),
            arguments: json!({"path":"src/4.rs"}),
        }]),
    ));
    canonical.push(Message::tool(
        json!({"path":"src/4.rs","lines":"x".repeat(8_000)}).to_string(),
        id,
        Some("read_file".into()),
    ));

    let mut later_request = canonical;
    assert!(compact_older_current_turn_tool_history(
        &mut later_request,
        1,
        16_000,
        8_000,
        &mut compaction_end,
    ));
    assert_eq!(compaction_end, fixed_frontier);
    assert!(
        later_request[3]
            .content
            .as_deref()
            .unwrap()
            .contains("contentOmitted")
    );
    assert!(
        later_request[5]
            .content
            .as_deref()
            .unwrap()
            .contains("contentOmitted")
    );
    assert!(
        later_request[7]
            .content
            .as_deref()
            .unwrap()
            .contains("\"lines\"")
    );
    assert!(
        later_request[9]
            .content
            .as_deref()
            .unwrap()
            .contains("\"lines\"")
    );
    assert!(
        later_request[11]
            .content
            .as_deref()
            .unwrap()
            .contains("\"lines\"")
    );
}

#[test]
fn large_context_keeps_cache_prefix_until_reserve_pressure() {
    let mut history = vec![Message::system("system"), Message::user("inspect")];
    for index in 0..8 {
        let id = format!("large-{index}");
        history.push(Message::assistant(
            "",
            Some(vec![ToolCall {
                id: id.clone(),
                name: "read_file".into(),
                arguments: json!({"path":format!("src/{index}.rs")}),
            }]),
        ));
        history.push(Message::tool(
            json!({"path":format!("src/{index}.rs"),"lines":"x".repeat(70_000)}).to_string(),
            id,
            Some("read_file".into()),
        ));
    }

    let original = history.clone();
    let mut compaction_end = None;
    assert!(!compact_older_current_turn_tool_history(
        &mut history,
        1,
        272_000,
        228_416,
        &mut compaction_end,
    ));
    assert_eq!(compaction_end, None);
    assert_eq!(history, original);

    for index in 8..12 {
        let id = format!("large-{index}");
        history.push(Message::assistant(
            "",
            Some(vec![ToolCall {
                id: id.clone(),
                name: "read_file".into(),
                arguments: json!({"path":format!("src/{index}.rs")}),
            }]),
        ));
        history.push(Message::tool(
            json!({"path":format!("src/{index}.rs"),"lines":"x".repeat(70_000)}).to_string(),
            id,
            Some("read_file".into()),
        ));
    }
    assert!(compact_older_current_turn_tool_history(
        &mut history,
        1,
        272_000,
        228_416,
        &mut compaction_end,
    ));
    let fixed_frontier = compaction_end;
    assert!(fixed_frontier.is_some());

    let id = "large-12".to_owned();
    history.push(Message::assistant(
        "",
        Some(vec![ToolCall {
            id: id.clone(),
            name: "read_file".into(),
            arguments: json!({"path":"src/12.rs"}),
        }]),
    ));
    history.push(Message::tool(
        json!({"path":"src/12.rs","lines":"x".repeat(70_000)}).to_string(),
        id,
        Some("read_file".into()),
    ));
    assert!(compact_older_current_turn_tool_history(
        &mut history,
        1,
        272_000,
        228_416,
        &mut compaction_end,
    ));
    assert_eq!(compaction_end, fixed_frontier);
}

#[test]
fn completed_conversation_aging_waits_for_token_pressure() {
    let mut history = vec![Message::system(SYSTEM_INSTRUCTION)];
    for index in 0..13 {
        history.push(Message::user(format!("short request {index}")));
        history.push(Message::assistant(format!("short answer {index}"), None));
    }

    assert!(!trim_completed_conversation_history_for_budget(
        &mut history,
        100_000,
    ));
    assert_eq!(
        history
            .iter()
            .filter(|message| message.role == MessageRole::User)
            .count(),
        13
    );
    assert!(history.iter().all(|message| {
        message
            .content
            .as_deref()
            .is_none_or(|content| !content.starts_with(CHECKPOINT_PREFIX))
    }));

    assert!(trim_completed_conversation_history_for_budget(
        &mut history,
        1,
    ));
    assert_eq!(
        history
            .iter()
            .filter(|message| message.role == MessageRole::User)
            .count(),
        RETAINED_COMPLETED_USER_TURNS
    );
    assert!(history.iter().any(|message| {
        message
            .content
            .as_deref()
            .is_some_and(|content| content.starts_with(CHECKPOINT_PREFIX))
    }));
}

#[test]
fn completed_tool_trace_stays_verbatim_until_context_pressure() {
    let raw_tool = json!({
        "path":"src/lib.rs",
        "snapshot":"stable-snapshot",
        "lines":"x".repeat(20_000)
    })
    .to_string();
    let mut history = vec![
        Message::system(SYSTEM_INSTRUCTION),
        Message::user("inspect"),
        Message::assistant(
            "",
            Some(vec![ToolCall {
                id: "read-1".into(),
                name: "read_file".into(),
                arguments: json!({"path":"src/lib.rs"}),
            }]),
        ),
        Message::tool(raw_tool.clone(), "read-1", Some("read_file".into())),
        Message::assistant("done", None),
    ];
    let original = history.clone();

    assert!(!trim_completed_conversation_history_for_budget(
        &mut history,
        100_000,
    ));
    assert_eq!(history, original);
    assert_eq!(history[3].content.as_deref(), Some(raw_tool.as_str()));

    assert!(trim_completed_conversation_history_for_budget(
        &mut history,
        1_000,
    ));
    let compact = history[3].content.as_deref().unwrap();
    assert!(compact.contains("contentOmitted"));
    assert!(!compact.contains("\"lines\""));
}

#[test]
fn completed_conversation_history_is_bounded_by_user_turns() {
    let mut history = vec![Message::system(SYSTEM_INSTRUCTION)];
    for index in 0..12 {
        history.push(Message::user(format!("request {index}")));
        history.push(Message::assistant(format!("answer {index}"), None));
    }

    trim_completed_conversation_history(&mut history);

    assert_eq!(
        history
            .iter()
            .filter(|message| message.role == MessageRole::User)
            .count(),
        RETAINED_COMPLETED_USER_TURNS
    );
    assert_eq!(history[0].role, MessageRole::System);
    let checkpoint = history[1].content.as_deref().unwrap();
    assert!(checkpoint.starts_with(CHECKPOINT_PREFIX));
    assert!(checkpoint.contains("User: request 0"));
    assert!(checkpoint.contains("Assistant: answer 5"));
    assert_eq!(history[2].content.as_deref(), Some("request 6"));
    assert_eq!(
        history
            .last()
            .and_then(|message| message.content.as_deref()),
        Some("answer 11")
    );
}

#[test]
fn conversation_checkpoint_rolls_forward_without_nesting() {
    let mut history = vec![Message::system(SYSTEM_INSTRUCTION)];
    for index in 0..8 {
        history.push(Message::user(format!("request {index}")));
        history.push(Message::assistant(format!("answer {index}"), None));
    }
    trim_completed_conversation_history(&mut history);

    for index in 8..12 {
        history.push(Message::user(format!("request {index}")));
        history.push(Message::assistant(format!("answer {index}"), None));
    }
    trim_completed_conversation_history(&mut history);

    let checkpoints = history
        .iter()
        .filter_map(|message| message.content.as_deref())
        .filter(|content| content.starts_with(CHECKPOINT_PREFIX))
        .collect::<Vec<_>>();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].matches(CHECKPOINT_PREFIX).count(), 1);
    assert_eq!(checkpoints[0].matches(CHECKPOINT_GUIDANCE).count(), 1);
    assert!(checkpoints[0].contains("User: request 0"));
    assert!(checkpoints[0].contains("User: request 5"));
    assert_eq!(
        history
            .iter()
            .filter(|message| message.role == MessageRole::User)
            .count(),
        RETAINED_COMPLETED_USER_TURNS
    );
}

#[test]
fn repeated_compaction_preserves_search_list_and_edit_counts() {
    for (name, input, key, expected) in [
        (
            "search_workspace",
            json!({"matches": [1, 2, 3]}),
            "matchCount",
            3,
        ),
        ("list_files", json!({"entries": [1, 2]}), "entryCount", 2),
    ] {
        let first = compact_completed_tool_result(Some(name), &input.to_string());
        let second = compact_completed_tool_result(Some(name), &first);
        let value: Value = serde_json::from_str(&second).unwrap();
        assert_eq!(value[key], json!(expected));
        assert_eq!(first, second);
    }
    let input = json!({"changes": [{"path": "a", "edits": [1, 2, 3]}]});
    let first = compact_completed_tool_arguments("apply_file_edits", &input);
    let second = compact_completed_tool_arguments("apply_file_edits", &first);
    assert_eq!(second["changes"][0]["editCount"], json!(3));
    assert_eq!(first, second);
}

#[test]
fn generic_large_tool_result_keeps_only_recovery_metadata() {
    let mut history = vec![Message::tool(
        json!({
            "status":"ok",
            "summary":"important result",
            "artifactId":"artifact-1",
            "payload":"x".repeat(4_000),
        })
        .to_string(),
        "generic-1",
        Some("custom_tool".into()),
    )];

    compact_completed_task_history(&mut history);

    let compact: Value = serde_json::from_str(history[0].content.as_deref().unwrap()).unwrap();
    assert_eq!(compact.get("status"), Some(&json!("ok")));
    assert_eq!(compact.get("summary"), Some(&json!("important result")));
    assert_eq!(compact.get("artifactId"), Some(&json!("artifact-1")));
    assert!(compact.get("payload").is_none());
    assert_eq!(compact.get("historical"), Some(&json!(true)));
}

#[test]
fn aged_turns_drop_image_bytes_and_bound_large_text() {
    let mut history = vec![Message::system(SYSTEM_INSTRUCTION)];
    let mut first = Message::user_with_images(
        format!("prefix {} suffix", "x".repeat(8_000)),
        vec![crate::core::ImageAttachment {
            media_type: "image/png".into(),
            data: "a".repeat(20_000),
            name: Some("screen.png".into()),
        }],
    );
    first.content.as_mut().unwrap().push_str(" durable-tail");
    history.push(first);
    history.push(Message::assistant("y".repeat(8_000), None));
    history.push(Message::user("middle"));
    history.push(Message::assistant("middle answer", None));
    history.push(Message::user("recent one"));
    history.push(Message::assistant("recent answer", None));
    history.push(Message::user("recent two"));
    history.push(Message::assistant("recent answer two", None));

    trim_completed_conversation_history(&mut history);

    assert!(history[1].images.is_none());
    let old_user = history[1].content.as_deref().unwrap();
    assert!(old_user.contains("historical image payload omitted: screen.png"));
    assert!(old_user.contains("durable-tail"));
    assert!(old_user.chars().count() < 2_500);
    assert!(history[2].content.as_deref().unwrap().chars().count() <= AGED_ASSISTANT_MESSAGE_CHARS);
    assert_eq!(history[5].content.as_deref(), Some("recent one"));
    assert_eq!(history[7].content.as_deref(), Some("recent two"));
}
