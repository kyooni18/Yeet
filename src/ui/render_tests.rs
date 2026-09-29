//! Cross-surface regressions for the terminal redesign.
use super::sessions::tools::{WorkEvent, tool_group_lines};
use super::status::status_line;
use super::{composer, draw, theme};
use crate::model::{ConversationToolCall, ModelActivity, SessionSummary, ShellPermission};
use crate::{
    app::{App, Mode},
    model::{ConversationEntry, ConversationKind, ToolCallStatus},
};
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
        assert!(text.contains('›'), "composer at {width}x{height}");
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
fn session_user_bubble_is_right_aligned_and_assistant_prose_is_unframed() {
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
    let (text, buffer) = render(&mut app, 140, 38);
    assert!(text.contains("Review the layout"));
    assert!(text.contains("Here is the review."));
    assert!(!text.contains("▌ You"));
    assert!(!text.contains("◆ Yeet"));
    assert!(!text.contains("│ Here is the review."));
    let user_y = text
        .lines()
        .enumerate()
        .skip(usize::from(app.transcript_area.1))
        .take(usize::from(app.transcript_area.3))
        .find_map(|(y, line)| line.contains("Review the layout").then_some(y))
        .expect("user message row inside transcript");
    let user_row = &buffer.content[user_y * 140..(user_y + 1) * 140];
    let user_x = (usize::from(app.transcript_area.0)..user_row.len())
        .find(|&x| user_row[x].symbol() == "R")
        .expect("user message text");
    assert!(
        user_x > usize::from(app.transcript_area.0 + app.transcript_area.2 / 3),
        "user bubble should sit on the right: {user_x}, {:?}",
        app.transcript_area
    );
    assert_eq!(user_row[user_x].bg, theme::surface_color());
    assert_eq!(
        buffer[(app.transcript_area.0, 0)].bg,
        theme::surface_color()
    );
    let (composer_x, composer_y, _, _) = app.composer_area;
    assert_eq!(buffer[(composer_x, composer_y)].bg, theme::surface_raised());
}

#[test]
fn empty_session_and_picker_remain_readable_at_responsive_widths() {
    for (width, height) in [(80, 24), (140, 38)] {
        let mut app = App::default();
        app.state.current_session_id = Some("fresh-session".into());
        let (text, _) = render(&mut app, width, height);
        assert!(text.contains("New session"), "{width}x{height}: {text}");
        assert!(text.contains("Write a message below"), "{width}x{height}");

        app.state.saved_sessions.push(SessionSummary {
            id: "fresh-session".into(),
            title: "Restored session".into(),
            updated_at: "preview".into(),
            model: "test-model".into(),
            message_count: 3,
        });
        let (text, _) = render(&mut app, width, height);
        assert!(
            text.contains("Conversation unavailable"),
            "{width}x{height}: {text}"
        );

        app.mode = Mode::Sessions;
        let (text, _) = render(&mut app, width, height);
        assert!(text.contains("Sessions"), "{width}x{height}: {text}");
        assert!(
            text.contains("Restored session"),
            "{width}x{height}: {text}"
        );
    }
}

#[test]
fn stopped_tool_groups_keep_errors_visible_and_hide_suppressed_calls() {
    let mut call: ConversationToolCall = serde_json::from_value(serde_json::json!({
        "id": "call", "name": "run_shell", "arguments": "{\"command\":\"cargo test\"}", "status": "failed"
    })).unwrap();
    let failed = tool_group_lines(&[WorkEvent::Tool(&call)], 80);
    assert!(
        failed
            .iter()
            .any(|line| line.to_string().contains("Failed"))
    );
    call.status = ToolCallStatus::Completed;
    let completed = tool_group_lines(&[WorkEvent::Tool(&call)], 80);
    assert_eq!(completed.len(), 1);
    assert!(!completed[0].to_string().contains("Activity"));
    assert!(completed[0].to_string().starts_with("  │ "));
    assert_eq!(completed[0].spans[0].style.fg, Some(theme::surface_color()));
    assert_eq!(completed[0].spans[2].style.fg, Some(theme::text_dim()));
    call.status = ToolCallStatus::Suppressed;
    assert!(tool_group_lines(&[WorkEvent::Tool(&call)], 80).is_empty());
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

#[cfg(test)]
mod overall_layout_tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn history_status_explains_return_shortcut_at_narrow_widths() {
        let app = App {
            follow_tail: false,
            max_scroll: 200,
            scroll_y: 40,
            ..App::default()
        };
        for width in [20, 40, 80] {
            let line = status_line(&app, width);
            assert!(line.to_string().contains("Ctrl+End"));
            assert!(line.width() <= width);
        }
    }

    #[test]
    fn missing_usage_is_not_shown_as_zero_percent() {
        let mut app = App::default();
        app.state.active_model_context_length = Some(100_000);
        app.state.current_context_tokens = None;
        assert!(!status_line(&app, 120).to_string().contains("0%"));
    }

    #[test]
    fn wide_status_shows_model_and_context() {
        let mut app = App::default();
        app.state.active_model = "openai/gpt-5.6-sol".into();
        app.state.active_model_context_length = Some(262_144);
        app.state.current_context_tokens = Some(58_300);
        app.state.active_reasoning_level = "high".into();

        let line = status_line(&app, 120);
        let text = line.to_string();
        assert!(text.contains("gpt-5.6-sol"));
        assert!(text.contains("58.3k/262"));
        assert!(line.width() <= 120);
    }

    #[test]
    fn shell_renders_from_small_terminal_to_ultrawide() {
        for (width, height) in [(32, 8), (60, 14), (80, 24), (120, 40), (180, 40)] {
            let mut app = App::default();
            app.conversation.push(ConversationEntry {
                id: "message".into(),
                kind: ConversationKind::User {
                    content: "Review these changes".into(),
                },
            });
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            assert!(app.transcript_area.3 > 0);
        }
    }
}

#[test]
fn portrait_uses_borderless_single_row_composer_and_one_row_header() {
    let mut app = App::default();
    app.input = "hello".into();
    app.cursor = 5;
    let (text, _) = render(&mut app, 40, 60);
    let rows: Vec<&str> = text.lines().collect();
    assert!(
        rows.iter()
            .any(|row| row.trim_start().starts_with("› hello"))
    );
    assert!(!text.contains("Message"));
    assert!(!text.contains("LATEST"));
    assert_eq!(app.composer_area.3, 1);
}

#[test]
fn files_view_shows_hint_cue_and_toggled_hints_panel() {
    let mut app = App::default();
    app.open_files();
    let (text, _) = render(&mut app, 40, 30);
    assert!(text.contains("? hints"));
    assert!(!text.contains("changed only"));
    app.files.as_mut().unwrap().hints = true;
    let (text, _) = render(&mut app, 40, 30);
    assert!(text.contains("changed only"));
    assert!(text.contains("parent folder"));
}
