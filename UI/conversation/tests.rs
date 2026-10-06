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
fn grouped_identity_survives_completion_and_attention_overrides_prior_collapse() {
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
        matches!(&view.items[0], DisplayItem::Activity{group,..} if group.expanded && group.failed)
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
    let mut harness = HarnessState::default();
    harness.conversation = Some(vec![entry(
        "a1",
        ConversationKind::Assistant {
            content: "answer".into(),
            tool_calls: vec![],
        },
    )]);
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
