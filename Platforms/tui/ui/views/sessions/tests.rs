use super::*;
use ratatui::{Terminal, backend::TestBackend};

#[test]
fn cached_viewport_matches_full_transcript_and_invalidates_on_updates() {
    let mut app = App::default();
    app.conversation = (0..120).map(|index| ConversationEntry {
        id: index.to_string(),
        kind: ConversationKind::Assistant {
            content: format!("## Turn {index}\n\nWrapped **styled** words and Unicode 日本語 🦀.\n\n```rust\nlet value = {index};\n```"),
            tool_calls: Vec::new(),
        },
    }).collect();
    for width in [24, 80] {
        let text = transcript_text(&app, width);
        let rows = Paragraph::new(text.clone())
            .wrap(Wrap { trim: false })
            .line_count(width);
        let mut expected = Terminal::new(TestBackend::new(width, 12)).unwrap();
        let mut actual = Terminal::new(TestBackend::new(width, 12)).unwrap();
        app.follow_tail = false;
        for scroll in [
            0,
            1,
            7,
            rows.saturating_sub(12) / 2,
            rows.saturating_sub(12),
        ] {
            app.scroll_y = scroll as u16;
            expected
                .draw(|frame| {
                    frame.render_widget(
                        Paragraph::new(text.clone())
                            .wrap(Wrap { trim: false })
                            .scroll((scroll as u16, 0)),
                        Rect::new(0, 0, width, 12),
                    )
                })
                .unwrap();
            if app
                .transcript_cache
                .as_ref()
                .is_none_or(|cache| cache.key.width != width)
            {
                app.transcript_cache = Some(TranscriptCache::new(
                    TranscriptKey::for_app(&app, width),
                    text.clone(),
                ));
            }
            // Compare directly with the cache's viewport rather than margins.
            let cache = app.transcript_cache.as_ref().unwrap();
            let (visible, offset) = cache.viewport(scroll as u16, 12);
            actual
                .draw(|frame| {
                    frame.render_widget(
                        Paragraph::new(visible)
                            .wrap(Wrap { trim: false })
                            .scroll((offset, 0)),
                        Rect::new(0, 0, width, 12),
                    )
                })
                .unwrap();
            assert_eq!(actual.backend().buffer(), expected.backend().buffer());
        }
    }
    let old = TranscriptKey::for_app(&app, 80);
    app.state.active_assistant_entry_id = Some("119".into());
    app.state.active_assistant_text = "New streamed content".into();
    assert_ne!(old, TranscriptKey::for_app(&app, 80));
    let old = TranscriptKey::for_app(&app, 80);
    app.merge_state(crate::model::BridgeState {
        conversation: Some(app.conversation.clone()),
        ..Default::default()
    });
    assert_ne!(old, TranscriptKey::for_app(&app, 80));
}

#[test]
fn reasoning_details_are_visible_outside_tool_groups_and_toggle_independently() {
    let mut app = App::default();
    app.input_focused = false;
    app.conversation = (0..2)
        .map(|index| ConversationEntry {
            id: index.to_string(),
            kind: ConversationKind::Reasoning {
                content: format!("Step {index}\n\nDetail {index}"),
                summary: None,
            },
        })
        .collect();
    let (visible, headers) = transcript_content(&app, 60);
    assert_eq!(headers.len(), 2);
    let visible_text = visible.to_string();
    assert!(visible_text.contains("Detail 0"));
    assert!(visible_text.contains("Detail 1"));

    app.selected_work = Some(1);
    app.apply_conversation_action(crate::shared_ui::conversation::ConversationAction::Select(Some("reasoning:1".into())));
    app.apply_conversation_action(crate::shared_ui::conversation::ConversationAction::Toggle("reasoning:1".into()));
    let (partially_collapsed, headers) = transcript_content(&app, 60);
    assert!(partially_collapsed.lines.len() < visible.lines.len());
    assert!(partially_collapsed.to_string().contains("Detail 0"));
    assert!(!partially_collapsed.to_string().contains("Detail 1"));
    assert!(
        partially_collapsed.lines[headers[1]]
            .style
            .add_modifier
            .contains(Modifier::REVERSED)
    );

    app.input_focused = true;
    let (focused, headers) = transcript_content(&app, 60);
    assert!(
        !focused.lines[headers[1]]
            .style
            .add_modifier
            .contains(Modifier::REVERSED)
    );
}

#[test]
fn copying_user_bubbles_excludes_padding_and_keeps_content_indentation() {
    let mut app = App::default();
    let area = Rect::new(2, 3, 100, 8);
    app.transcript_area = (area.x, area.y, area.width, area.height);
    let text = Text::from(super::user_message_lines("hello\n  indented", area.width));
    super::capture_transcript_cells(&mut app, area, &text, 0);
    app.selection_start = Some((area.x, area.y));
    app.selection_end = Some((area.right() - 1, area.bottom() - 1));
    assert_eq!(
        app.selected_transcript_text().as_deref(),
        Some("hello\n  indented")
    );
}

#[test]
fn reasoning_summary_stays_visible_separate_from_its_tool_dropdown() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = App::default();
    app.activate_workbench_tab(crate::app::WorkbenchTab::Session);
    app.input_focused = false;
    app.input = "keep draft".into();
    app.conversation = vec![
        ConversationEntry {
            id: "summary".into(),
            kind: ConversationKind::Reasoning {
                content: "private reasoning".into(),
                summary: Some("Inspect the source\n\nCheck the implementation".into()),
            },
        },
        ConversationEntry {
            id: "tool".into(),
            kind: ConversationKind::ToolCall {
                tool_call: serde_json::from_value(serde_json::json!({
                    "id":"tool", "name":"read_file",
                    "arguments":"{\"path\":\"src/example.rs\"}", "status":"completed"
                }))
                .unwrap(),
            },
        },
    ];
    let (separate, headers) = transcript_content(&app, 90);
    assert_eq!(
        headers.len(),
        2,
        "reasoning and tool work have separate rows"
    );
    let text = separate.to_string();
    assert!(text.contains("Inspect the source"));
    assert!(text.contains("Check the implementation"));
    assert!(!text.contains("src/example.rs"));

    app.work_rows = headers.iter().map(|row| *row as u16).collect();
    let view = app.application.conversation_state().view(&app.conversation, &app.state);
    app.work_ids = view.items.iter().filter(|item| !matches!(item, crate::shared_ui::conversation::DisplayItem::Entry { .. })).map(|item| item.id().to_owned()).collect();
    app.selected_work = Some(1); // Toggle the tool row, not the reasoning block.
    for key in [KeyCode::Char(' '), KeyCode::Enter] {
        assert!(app.handle_work_selection_key(&KeyEvent::new(key, KeyModifiers::NONE)));
        let toggled = transcript_content(&app, 90).0.to_string();
        assert!(toggled.contains("src/example.rs"));
        assert!(toggled.contains("Check the implementation"));
        assert!(!app.input_focused);
        assert_eq!(app.input, "keep draft");
        assert!(app.handle_work_selection_key(&KeyEvent::new(key, KeyModifiers::NONE)));
        let collapsed = transcript_content(&app, 90).0.to_string();
        assert!(!collapsed.contains("src/example.rs"));
        assert!(collapsed.contains("Check the implementation"));
    }
}
