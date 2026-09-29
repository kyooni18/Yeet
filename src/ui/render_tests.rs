//! Cross-surface regressions for the terminal redesign.
use super::status::{status_line, usage_line};
use super::{composer, draw, theme};
use crate::model::{ModelActivity, SessionSummary, ShellPermission};
use crate::{
    app::{App, Mode},
    model::{ConversationEntry, ConversationKind},
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
        assert!(text.contains('+'), "composer at {width}x{height}");
        if height >= 24 {
            assert!(text.contains("What are we building?"));
        }
    }
}

#[test]
fn views_switcher_renders_vertical_choices() {
    let mut app = App::default();
    app.mode = Mode::Views;
    let (text, _) = render(&mut app, 60, 16);
    let lines = text.lines().collect::<Vec<_>>();
    let sessions = lines.iter().position(|line| line.contains("Sessions"));
    let files = lines.iter().position(|line| line.contains("Files"));
    assert!(sessions.is_some_and(|row| files == Some(row + 1)), "{text}");
    assert!(!text.contains("Workspace browser"));
    assert!(!text.contains("new tab"));
}

#[test]
fn portrait_session_picker_is_a_grouped_full_screen_list() {
    let mut app = App::default();
    app.mode = Mode::Sessions;
    app.state.saved_sessions.push(SessionSummary {
        id: "selected".into(),
        title: "Selected conversation".into(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        model: "test".into(),
        message_count: 1,
    });
    app.state.current_session_id = Some("selected".into());
    let (text, _) = render(&mut app, 58, 48);
    assert!(text.contains("Selected conversation"), "{text}");
    assert!(text.contains("New session"), "{text}");
    assert!(text.contains("1 session"), "{text}");
    assert!(!text.contains("Enter open"), "{text}");
}

#[test]
fn portrait_files_shows_selected_file_details_without_a_tab_bar() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sample.rs"), "one\ntwo\n").unwrap();
    let mut app = App::default();
    app.files = Some(crate::app::files::FilesState::open(
        dir.path().to_path_buf(),
    ));
    app.mode = Mode::Files;
    let (text, _) = render(&mut app, 58, 48);
    assert!(text.contains("sample.rs"), "{text}");
    assert!(text.contains("2 lines"), "{text}");
    assert!(!text.contains("Home"), "{text}");
}

#[test]
fn desktop_files_keeps_inspector_breadcrumb_and_composer_visible() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sample.rs"), "one\ntwo\n").unwrap();
    let mut app = App::default();
    app.files = Some(crate::app::files::FilesState::open(
        dir.path().to_path_buf(),
    ));
    app.mode = Mode::Files;
    let (text, _) = render(&mut app, 144, 44);
    for expected in ["sample.rs", "INFORMATION", "GIT", "2 · UTF-8 · LF", "›"] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
}

#[test]
fn unfocused_composer_uses_quieter_surface_than_focused_input() {
    let mut app = App::default();
    let bounds = ratatui::layout::Rect::new(0, 0, 60, 5);

    let mut focused = Terminal::new(TestBackend::new(60, 5)).unwrap();
    focused
        .draw(|frame| composer::draw(frame, &mut app, bounds, 0))
        .unwrap();
    let (x, y, _, _) = app.composer_area;
    assert_eq!(
        focused.backend().buffer()[(x, y)].bg,
        theme::surface_color()
    );

    app.mode = Mode::Settings;
    let mut unfocused = Terminal::new(TestBackend::new(60, 5)).unwrap();
    unfocused
        .draw(|frame| composer::draw(frame, &mut app, bounds, 0))
        .unwrap();
    let (x, y, _, _) = app.composer_area;
    assert_eq!(unfocused.backend().buffer()[(x, y)].bg, theme::background());
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
        buffer[(app.transcript_area.0, app.transcript_area.1)].bg,
        theme::code_background()
    );
    let (composer_x, composer_y, _, _) = app.composer_area;
    assert_eq!(buffer[(composer_x, composer_y)].bg, theme::surface_color());
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

        let line = usage_line(&app, 118).unwrap();
        let text = line.to_string();
        assert!(text.contains("gpt-5.6-sol (high)"));
        assert!(text.contains("58.3k/262"));
        assert!(line.width() <= 118);
        assert_eq!(
            line.spans.first().map(|span| span.content.as_ref()),
            Some("│")
        );
        assert_eq!(
            line.spans.last().map(|span| span.content.as_ref()),
            Some("│")
        );
        assert!(
            status_line(&app, 60)
                .to_string()
                .contains("CTX 58.3k / 262")
        );
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
            .any(|row| row.trim_start().starts_with("+  hello"))
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

#[test]
fn user_bubble_padding_rows_are_adjacent_to_message() {
    let mut app = App {
        conversation: vec![ConversationEntry {
            id: "u".into(),
            kind: ConversationKind::User {
                content: "hi".into(),
            },
        }],
        ..App::default()
    };
    let (_, buf) = render(&mut app, 160, 20);
    let (x, y, w, _) = app.transcript_area;
    let column = x + w - 4;
    let lit: Vec<u16> = (y..y + 6)
        .filter(|&row| buf[(column, row)].bg == theme::surface_color())
        .collect();
    assert_eq!(lit.len(), 3, "{lit:?}");
    assert_eq!(
        lit[2] - lit[0],
        2,
        "bubble rows must be contiguous: {lit:?}"
    );
}
