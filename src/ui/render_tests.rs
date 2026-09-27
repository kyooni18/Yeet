//! Cross-surface regressions for the terminal redesign.
use super::*;
use crate::model::{ConversationToolCall, ModelActivity, SessionSummary, ShellPermission};
use ratatui::{Terminal, backend::TestBackend};

fn render(app: &mut App, width: u16, height: u16) -> (String, ratatui::buffer::Buffer) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let text = buffer
        .content
        .chunks(width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    (text, buffer)
}

#[test]
fn welcome_preserves_composer_and_adapts_shortcut_cards() {
    for (width, height) in [(32, 8), (60, 14), (80, 24), (100, 36), (140, 38), (200, 50)] {
        let mut app = App::default();
        let (text, _) = render(&mut app, width, height);
        assert!(text.contains("Message"), "composer at {width}x{height}");
        assert!(text.contains("Enter send"), "send hint at {width}x{height}");
        if height >= 24 {
            assert!(text.contains("What are we building?"));
        }
        if height >= 36 {
            assert!(text.contains("Explore actions"));
            assert!(text.contains("Choose your model"));
        }
    }
}

#[test]
fn unfocused_composer_uses_quieter_surface_than_focused_input() {
    let mut app = App::default();
    let bounds = ratatui::layout::Rect::new(0, 0, 60, 5);

    let mut focused = Terminal::new(TestBackend::new(60, 5)).unwrap();
    focused
        .draw(|frame| composer::draw(frame, &mut app, bounds))
        .unwrap();
    let (x, y, _, _) = app.composer_area;
    assert_eq!(
        focused.backend().buffer()[(x, y)].bg,
        theme::surface_raised()
    );

    app.mode = Mode::Settings;
    let mut unfocused = Terminal::new(TestBackend::new(60, 5)).unwrap();
    unfocused
        .draw(|frame| composer::draw(frame, &mut app, bounds))
        .unwrap();
    let (x, y, _, _) = app.composer_area;
    assert_eq!(
        unfocused.backend().buffer()[(x, y)].bg,
        theme::surface_color()
    );
}

#[test]
fn welcome_shortcut_cards_use_raised_surface() {
    let mut app = App::default();
    let (text, buffer) = render(&mut app, 140, 38);
    let card_y = text
        .lines()
        .position(|line| line.contains("Choose your model"))
        .expect("welcome shortcut card is visible");
    let card_row = &buffer.content[card_y * 140..(card_y + 1) * 140];
    assert!(
        card_row
            .iter()
            .any(|cell| cell.bg == theme::surface_raised())
    );
}

#[test]
fn narrow_composer_placeholder_ends_cleanly() {
    let mut app = App::default();
    let (text, _) = render(&mut app, 36, 12);
    assert!(
        text.lines()
            .any(|line| line.contains("Write a message") && line.contains('…'))
    );
}

#[test]
fn generic_legacy_done_activity_is_hidden_but_informative_completion_survives() {
    let mut app = App {
        conversation: vec![
            ConversationEntry {
                id: "user".into(),
                kind: ConversationKind::User {
                    content: "Polish the TUI".into(),
                },
            },
            ConversationEntry {
                id: "legacy-done".into(),
                kind: ConversationKind::Activity {
                    activity: ModelActivity {
                        phase: serde_json::json!("done"),
                        title: "Done".into(),
                        detail: None,
                        run_id: None,
                    },
                },
            },
            ConversationEntry {
                id: "useful-done".into(),
                kind: ConversationKind::Activity {
                    activity: ModelActivity {
                        phase: serde_json::json!("done"),
                        title: "Validation complete".into(),
                        detail: Some("122 tests".into()),
                        run_id: None,
                    },
                },
            },
        ],
        ..App::default()
    };

    let (text, _) = render(&mut app, 100, 24);
    assert!(!text.contains("· Done"), "{text}");
    assert!(text.contains("Validation complete"), "{text}");
    assert!(text.contains("122 tests"), "{text}");
}

#[test]
fn user_and_assistant_have_distinct_readable_surfaces() {
    let mut app = App {
        conversation: vec![
            ConversationEntry {
                id: "user".into(),
                kind: ConversationKind::User {
                    content: "Review the layout".into(),
                },
            },
            ConversationEntry {
                id: "answer".into(),
                kind: ConversationKind::Assistant {
                    content: "Here is the review.".into(),
                    tool_calls: vec![],
                },
            },
        ],
        ..App::default()
    };
    let (text, buffer) = render(&mut app, 80, 24);
    assert!(text.contains("▌ You"));
    assert!(text.contains("◆ Yeet"));
    let rail = buffer
        .content
        .iter()
        .find(|cell| cell.symbol() == "▌")
        .unwrap();
    assert_eq!(rail.fg, theme::user());
    assert_eq!(rail.bg, theme::user_surface());
    assert!(text.contains("Here is the review."));
    assert!(text.contains("│ Here is the review."));
    let assistant_y = text
        .lines()
        .position(|line| line.contains("Here is the review."))
        .unwrap();
    let assistant_row = &buffer.content[assistant_y * 80..(assistant_y + 1) * 80];
    let assistant_rail = assistant_row
        .iter()
        .find(|cell| cell.symbol() == "│")
        .unwrap();
    assert_eq!(assistant_rail.fg, theme::border_dim());
    assert_eq!(
        buffer[(app.transcript_area.0, 0)].bg,
        theme::surface_color()
    );
    let (composer_x, composer_y, _, _) = app.composer_area;
    assert_eq!(buffer[(composer_x, composer_y)].bg, theme::surface_raised());
}

#[test]
fn stopped_tool_groups_keep_errors_visible_and_hide_suppressed_calls() {
    let mut call: ConversationToolCall = serde_json::from_value(serde_json::json!({
        "id": "call", "name": "run_shell", "arguments": "{\"command\":\"cargo test\"}", "status": "failed"
    })).unwrap();
    let app = App::default();
    let failed = tool_group_lines(&app, &[WorkEvent::Tool(&call)], 80, false);
    assert!(
        failed
            .iter()
            .any(|line| line.to_string().contains("Failed"))
    );
    call.status = ToolCallStatus::Completed;
    let completed = tool_group_lines(&app, &[WorkEvent::Tool(&call)], 80, false);
    assert_eq!(completed.len(), 1);
    assert!(!completed[0].to_string().contains("Activity"));
    assert!(completed[0].to_string().starts_with("  │ "));
    assert_eq!(completed[0].spans[0].style.fg, Some(theme::muted()));
    assert_eq!(completed[0].spans[2].style.fg, Some(theme::text_dim()));
    call.status = ToolCallStatus::Suppressed;
    assert!(tool_group_lines(&app, &[WorkEvent::Tool(&call)], 80, false).is_empty());
}

#[test]
fn shell_location_badge_uses_selected_palette_surface() {
    let mut app = App::default();
    let (_, buffer) = render(&mut app, 100, 24);
    let location = buffer
        .content
        .iter()
        .find(|cell| cell.symbol() == "L")
        .expect("latest location badge should be visible");
    assert_eq!(location.fg, theme::accent_hot());
    assert_eq!(location.bg, theme::selected_color());
}

#[test]
fn sidebar_hit_targets_match_rendered_session_rows() {
    let mut app = App::default();
    app.state.saved_sessions = (0..3)
        .map(|i| SessionSummary {
            id: format!("session-{i}"),
            title: format!("Session number {i}"),
            updated_at: "preview".into(),
            model: "test".into(),
            message_count: 2,
        })
        .collect();
    app.state.current_session_id = Some("session-0".into());
    let (text, _) = render(&mut app, 140, 38);
    let rows = text.lines().collect::<Vec<_>>();
    assert!(!app.sidebar_session_targets.is_empty());
    for (y, id) in &app.sidebar_session_targets {
        let session = app
            .state
            .saved_sessions
            .iter()
            .find(|session| &session.id == id)
            .unwrap();
        assert!(rows[*y as usize].contains(&session.title));
    }
    render(&mut app, 80, 24);
    assert!(app.sidebar_session_targets.is_empty());
}

#[test]
fn unicode_draft_cursor_stays_inside_composer_after_resize() {
    let mut app = App::default();
    app.input = "한글 테스트 🦀\n".repeat(10);
    app.cursor = app.input.chars().count();
    for (width, height) in [(20, 8), (60, 14), (80, 24), (140, 38)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let position = terminal.get_cursor_position().unwrap();
        let (x, y) = (position.x, position.y);
        assert!(x < width && y < height);
        assert!(y > app.transcript_area.1 + app.transcript_area.3);
        assert!(y < height - 1);
    }
}

#[test]
fn approval_and_failure_remain_visible_above_modal_backdrop() {
    let mut app = App::default();
    app.state.pending_shell_permission = Some(ShellPermission {
        id: "request".into(),
        kind: "shell".into(),
        command: "cargo test".into(),
        operation: "test".into(),
        reason: "Check layout".into(),
    });
    for (width, height) in [(60, 20), (100, 32), (160, 40)] {
        let (text, _) = render(&mut app, width, height);
        assert!(text.contains("cargo test"));
        assert!(text.contains("allow"));
    }
    app.state.pending_shell_permission = None;
    app.state.error_message = Some("Provider connection closed".into());
    for (width, height) in [(60, 14), (100, 32)] {
        let (text, _) = render(&mut app, width, height);
        assert!(text.contains("Failed"));
        assert!(text.contains("Provider connection closed"));
    }
}
