use super::*;
use crate::harness::HarnessState;
use crate::model::{ConversationEntry, ConversationKind, ConversationToolCall};
fn tool(status: &str) -> ConversationToolCall {
    serde_json::from_value(
        serde_json::json!({"id":"call-1","name":"terminal","arguments":"{}","status":status}),
    )
    .unwrap()
}
fn entry(id: &str, kind: ConversationKind) -> ConversationEntry {
    ConversationEntry {
        id: id.into(),
        kind,
    }
}
#[test]
#[allow(clippy::field_reassign_with_default)] // ConversationState has private fields
fn failed_tools_start_collapsed_and_can_be_opened_manually() {
    let harness = HarnessState::default();
    let entries = vec![entry(
        "failed-tool",
        ConversationKind::ToolCall {
            tool_call: tool("failed"),
        },
    )];
    for individual_work_rows in [false, true] {
        let mut state = ConversationState::default();
        state.individual_work_rows = individual_work_rows;
        let view = state.project(&entries, &harness);
        assert!(matches!(&view.items[0], DisplayItem::Activity { group, .. }
            if group.failed && !group.expanded && group.status_label.is_some()));
        state.apply(
            ConversationAction::Toggle(view.items[0].id().to_owned()),
            &view,
            &harness,
        );
        let view = state.project(&entries, &harness);
        assert!(matches!(&view.items[0], DisplayItem::Activity { group, .. } if group.expanded));
    }
}
#[test]
fn grouped_identity_and_prior_collapse_survive_failure() {
    let mut harness = HarnessState::default();
    let entries = vec![entry(
        "tool-entry",
        ConversationKind::ToolCall {
            tool_call: tool("running"),
        },
    )];
    let mut state = ConversationState::inspect_work();
    let view = state.project(&entries, &harness);
    let id = view.items[0].id().to_owned();
    assert!(matches!(&view.items[0], DisplayItem::Activity{group,..} if group.expanded));
    state.apply(ConversationAction::Toggle(id.clone()), &view, &harness);
    let completed = vec![entry(
        "tool-entry",
        ConversationKind::ToolCall {
            tool_call: tool("completed"),
        },
    )];
    let view = state.project(&completed, &harness);
    assert_eq!(view.items[0].id(), id);
    assert!(matches!(&view.items[0], DisplayItem::Activity{group,..} if !group.expanded));
    let failed = vec![entry(
        "tool-entry",
        ConversationKind::ToolCall {
            tool_call: tool("failed"),
        },
    )];
    let view = state.project(&failed, &harness);
    assert!(
        matches!(&view.items[0], DisplayItem::Activity{group,..} if !group.expanded && group.failed)
    );
    harness.is_streaming = true;
    let suppressed = vec![entry(
        "tool-entry",
        ConversationKind::ToolCall {
            tool_call: tool("suppressed"),
        },
    )];
    assert!(state.project(&suppressed, &harness).items.is_empty());
}
#[test]
fn embedded_tools_deduplicate_and_only_latest_messages_offer_mutation() {
    let harness = HarnessState::default();
    let entries = vec![
        entry(
            "u1",
            ConversationKind::User {
                content: "first".into(),
            },
        ),
        entry(
            "a1",
            ConversationKind::Assistant {
                content: "answer".into(),
                tool_calls: vec![tool("completed")],
            },
        ),
        entry(
            "tc",
            ConversationKind::ToolCall {
                tool_call: tool("completed"),
            },
        ),
        entry(
            "u2",
            ConversationKind::User {
                content: "latest".into(),
            },
        ),
    ];
    let mut state = ConversationState::default();
    let view = state.project(&entries, &harness);
    let count: usize = view
        .items
        .iter()
        .filter_map(|item| match item {
            DisplayItem::Activity { group, .. } => Some(group.events.len()),
            _ => None,
        })
        .sum();
    assert_eq!(count, 1);
    assert!(
        state
            .apply(ConversationAction::Edit("u1".into()), &view, &harness)
            .edit_draft
            .is_none()
    );
    assert_eq!(
        state
            .apply(ConversationAction::Edit("u2".into()), &view, &harness)
            .edit_draft
            .as_deref(),
        Some("latest")
    );
    assert!(
        state
            .apply(ConversationAction::Regenerate("a1".into()), &view, &harness)
            .command
            .is_some()
    );
}
#[test]
fn failed_delivery_preserves_controller_state_and_runtime_refresh_keeps_disclosure() {
    use crate::shared_ui::application_session::ApplicationSession;
    let mut harness = HarnessState {
        conversation: Some(vec![entry(
            "a1",
            ConversationKind::Assistant {
                content: "answer".into(),
                tool_calls: vec![],
            },
        )]),
        ..Default::default()
    };
    let mut session = ApplicationSession::new(&harness);
    let before = session.projection().ui_revision;
    let prepared =
        session.prepare_conversation(ConversationAction::Regenerate("a1".into()), &harness);
    assert!(prepared.effect.command.is_some());
    drop(prepared);
    assert_eq!(session.projection().ui_revision, before);
    let prepared = session.prepare_conversation(ConversationAction::SetInspectWork(true), &harness);
    session.commit_conversation(prepared, &harness);
    assert!(session.conversation_state().inspect_work);
    harness.conversation.as_mut().unwrap().push(entry(
        "tc",
        ConversationKind::ToolCall {
            tool_call: tool("running"),
        },
    ));
    assert!(session.refresh(&harness).is_some());
    assert!(session.conversation_state().inspect_work);
    assert!(session.refresh(&harness).is_none());
    let sparse = HarnessState::default();
    assert!(session.refresh(&sparse).is_none());
    assert!(!session.conversation_projection().view.items.is_empty());
}

#[test]
#[allow(clippy::field_reassign_with_default)] // ConversationState has private fields
fn native_tool_rows_stay_separate_and_toggle_independently() {
    let harness = HarnessState::default();
    let make = |id: &str| {
        let mut call = tool("completed");
        call.id = id.to_owned();
        call.name = "run_shell".into();
        call.arguments = serde_json::json!({ "command": format!("check-{id}") }).to_string();
        call.result = Some(format!("result-{id}"));
        entry(id, ConversationKind::ToolCall { tool_call: call })
    };
    let entries = vec![make("one"), make("two"), make("three")];
    let mut native = ConversationState::default();
    native.individual_work_rows = true;
    let view = native.project(&entries, &harness);
    assert_eq!(view.items.len(), 3);
    assert_eq!(view.work_ids(), ["tool:one", "tool:two", "tool:three"]);
    assert!(view.items.iter().all(|item| {
        matches!(item, DisplayItem::Activity { group, .. } if group.events.len() == 1 && !group.expanded)
    }));
    native.apply(
        ConversationAction::Select(Some("tool:two".into())),
        &view,
        &harness,
    );
    native.apply(
        ConversationAction::Toggle("tool:two".into()),
        &view,
        &harness,
    );
    let view = native.project(&entries, &harness);
    assert_eq!(view.selected.as_deref(), Some("tool:two"));
    assert!(matches!(&view.items[1], DisplayItem::Activity { group, .. } if group.expanded));
    assert!(matches!(&view.items[0], DisplayItem::Activity { group, .. } if !group.expanded));
    assert!(matches!(&view.items[2], DisplayItem::Activity { group, .. } if !group.expanded));
    native.apply(ConversationAction::Reset, &view, &harness);
    assert!(
        native.individual_work_rows,
        "reset preserves native presentation"
    );
    assert_eq!(native.project(&entries, &harness).items.len(), 3);

    // Web/Remote still groups consecutive calls into one expandable disclosure.
    let mut remote = ConversationState::default();
    let remote_view = remote.project(&entries, &harness);
    assert_eq!(remote_view.items.len(), 1);
    assert!(
        matches!(&remote_view.items[0], DisplayItem::Activity { group, .. } if group.events.len() == 3)
    );
}

#[test]
fn native_application_session_is_configured_to_show_individual_calls() {
    use crate::shared_ui::application_session::ApplicationSession;
    let mut harness = HarnessState::default();
    let mut second = tool("completed");
    second.id = "call-2".into();
    harness.conversation = Some(vec![
        entry(
            "first",
            ConversationKind::ToolCall {
                tool_call: tool("completed"),
            },
        ),
        entry("second", ConversationKind::ToolCall { tool_call: second }),
    ]);
    let native = ApplicationSession::new_tui(&harness);
    assert!(native.conversation_state().individual_work_rows);
    assert_eq!(
        native.conversation_projection().view.work_ids(),
        ["tool:call-1", "tool:call-2"]
    );
    let remote = ApplicationSession::new(&harness);
    assert!(!remote.conversation_state().individual_work_rows);
    assert_eq!(remote.conversation_projection().view.items.len(), 1);
}

#[test]
fn compact_groups_keep_meaningful_copy_and_expose_child_disclosures() {
    let harness = HarnessState::default();
    let mut state = ConversationState::default();
    let make = |id: &str, status: &str| {
        let mut call = tool(status);
        call.id = id.into();
        call.name = "run_shell".into();
        call.arguments = serde_json::json!({"command": format!("check-{id}")}).to_string();
        call.result = Some("literal **output**".into());
        entry(id, ConversationKind::ToolCall { tool_call: call })
    };
    let entries = vec![make("first", "completed"), make("last", "completed")];
    let view = state.project(&entries, &harness);
    let DisplayItem::Activity { id, group } = &view.items[0] else {
        panic!("tool group")
    };
    assert!(group.summary.contains("check-last"));
    assert!(!group.expanded);
    assert!(
        group
            .events
            .iter()
            .all(|event| event.trace().status_label.as_deref() == Some("Completed"))
    );
    state.apply(ConversationAction::Toggle(id.clone()), &view, &harness);
    let view = state.project(&entries, &harness);
    assert_eq!(
        view.work_ids(),
        ["activity-group:tool:first", "tool:first", "tool:last"]
    );
    state.apply(
        ConversationAction::Select(Some("tool:first".into())),
        &view,
        &harness,
    );
    state.apply(
        ConversationAction::Toggle("tool:first".into()),
        &view,
        &harness,
    );
    let view = state.project(&entries, &harness);
    assert_eq!(view.selected.as_deref(), Some("tool:first"));
    let DisplayItem::Activity { group, .. } = &view.items[0] else {
        panic!("tool group")
    };
    assert!(group.events[0].trace().expanded);
    assert!(
        group.events[0]
            .trace()
            .details
            .iter()
            .any(|detail| detail.content == "literal **output**")
    );

    let waiting = vec![
        make("first", "awaiting_permission"),
        make("last", "running"),
    ];
    let view = state.project(&waiting, &harness);
    let DisplayItem::Activity { group, .. } = &view.items[0] else {
        panic!("tool group")
    };
    assert!(
        group.summary.contains("check-first"),
        "approval takes precedence over running work"
    );
    assert!(group.awaits_permission && group.active);
    assert!(!group.events[0].trace().active, "waiting must not spin");
}

#[test]
fn active_tool_progress_does_not_repeat_real_tool_calls() {
    let harness = HarnessState {
        is_streaming: true,
        active_activity_entry_id: Some("progress".into()),
        ..Default::default()
    };
    let progress = entry(
        "progress",
        ConversationKind::Activity {
            activity: crate::model::ModelActivity {
                phase: serde_json::json!("tool"),
                title: "Running command".into(),
                detail: Some("cargo check".into()),
                run_id: Some("run-1".into()),
            },
        },
    );
    let user = entry(
        "user",
        ConversationKind::User {
            content: "Check".into(),
        },
    );
    let mut command = tool("running");
    command.name = "run_shell".into();
    command.arguments = r#"{"command":"cargo check"}"#.into();
    let real_tool = entry("call", ConversationKind::ToolCall { tool_call: command });
    let state = ConversationState::default();
    let with_tool = state.view(
        &[user.clone(), progress.clone(), real_tool.clone()],
        &harness,
    );
    assert!(with_tool.items.iter().all(|item| {
        !matches!(item, DisplayItem::Activity { group, .. } if group.events.iter().any(|event| matches!(event, ActivityEvent::Activity { .. })))
    }));
    assert!(with_tool.items.iter().any(|item| {
        matches!(item, DisplayItem::Activity { group, .. } if group.summary.contains("cargo check") && group.active)
    }));

    // Before a real call arrives, progress must remain visible.
    let preparing = state.view(&[user.clone(), progress.clone()], &harness);
    assert!(preparing.items.iter().any(|item| {
        matches!(item, DisplayItem::Activity { group, .. } if group.summary.contains("Running command"))
    }));

    // A previous user's tool cannot suppress a later user's current progress.
    let next_turn = state.view(
        &[
            user,
            real_tool,
            entry(
                "next",
                ConversationKind::User {
                    content: "Again".into(),
                },
            ),
            progress.clone(),
        ],
        &harness,
    );
    assert!(next_turn.items.iter().any(|item| {
        matches!(item, DisplayItem::Activity { group, .. } if group.summary.contains("Running command"))
    }));

    // Provider-managed work has no equivalent tool trace and must remain visible.
    let provider = entry(
        "provider",
        ConversationKind::Activity {
            activity: crate::model::ModelActivity {
                phase: serde_json::json!("provider-tool"),
                title: "Searching web".into(),
                detail: None,
                run_id: Some("run-1".into()),
            },
        },
    );
    let provider_view = state.view(&[progress, provider], &harness);
    assert!(provider_view.items.iter().any(|item| {
        matches!(item, DisplayItem::Activity { group, .. } if group.events.iter().any(|event|
            matches!(event, ActivityEvent::Activity { trace, .. } if trace.title == "Searching web")
        ))
    }));
}

#[test]
fn long_prompts_default_to_preview_and_toggle_without_truncating_copy() {
    let harness = HarnessState::default();
    for content in [
        "🦀".repeat(450),
        "one\ntwo\nthree\nfour\nfive\nsix\nseven".into(),
    ] {
        let entries = vec![entry(
            "prompt",
            ConversationKind::User {
                content: content.clone(),
            },
        )];
        let mut state = ConversationState::default();
        let view = state.project(&entries, &harness);
        assert_eq!(view.work_ids(), vec!["entry:prompt"]);
        assert!(
            matches!(&view.items[0], DisplayItem::Entry { prompt: Some(p), .. } if !p.expanded && p.preview.ends_with('…') && p.preview.len() < content.len())
        );
        let effect = state.apply(ConversationAction::Copy("prompt".into()), &view, &harness);
        assert_eq!(effect.copy.unwrap().text, content);
        state.apply(
            ConversationAction::Toggle("entry:prompt".into()),
            &view,
            &harness,
        );
        let view = state.project(&entries, &harness);
        assert!(matches!(&view.items[0], DisplayItem::Entry { prompt: Some(p), .. } if p.expanded));
        state.apply(
            ConversationAction::Toggle("entry:prompt".into()),
            &view,
            &harness,
        );
        assert!(
            matches!(&state.project(&entries, &harness).items[0], DisplayItem::Entry { prompt: Some(p), .. } if !p.expanded)
        );
    }
    let view = ConversationState::default().view(
        &[entry(
            "short",
            ConversationKind::User {
                content: "hello".into(),
            },
        )],
        &harness,
    );
    assert!(matches!(
        &view.items[0],
        DisplayItem::Entry { prompt: None, .. }
    ));
}
