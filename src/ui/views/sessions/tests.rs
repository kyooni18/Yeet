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
fn reasoning_work_headers_select_and_expand_independently() {
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
    let (collapsed, headers) = transcript_content(&app, 60);
    assert_eq!(headers.len(), 2);
    app.selected_work = Some(1);
    app.expanded_work.insert(1);
    let (expanded, headers) = transcript_content(&app, 60);
    assert!(expanded.lines.len() > collapsed.lines.len());
    assert!(
        expanded.lines[headers[1]]
            .style
            .add_modifier
            .contains(Modifier::REVERSED)
    );
    let text = expanded.to_string();
    assert!(text.contains("Detail 1"));
    assert!(!text.contains("Detail 0"));
    app.input_focused = true;
    let (focused, headers) = transcript_content(&app, 60);
    assert!(
        !focused.lines[headers[1]]
            .style
            .add_modifier
            .contains(Modifier::REVERSED)
    );
}
