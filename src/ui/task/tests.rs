//! Regression coverage for turn boundaries, status precedence, and persistent UI.
use super::*;
use crate::model::{ConversationToolCall, ModelActivity, ShellPermission, ToolCallStatus};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

fn entry(kind: ConversationKind) -> ConversationEntry {
    ConversationEntry {
        id: uuid::Uuid::new_v4().to_string(),
        kind,
    }
}

fn user() -> ConversationEntry {
    entry(ConversationKind::User {
        content: "Improve task indicators".into(),
    })
}

fn activity(phase: &str) -> ConversationEntry {
    entry(ConversationKind::Activity {
        activity: ModelActivity {
            phase: json!(phase),
            title: phase.into(),
            detail: None,
            run_id: None,
        },
    })
}

fn tool(status: ToolCallStatus) -> ConversationEntry {
    entry(ConversationKind::ToolCall {
        tool_call: ConversationToolCall {
            id: "call".into(),
            call_id: None,
            index: None,
            name: "read_file".into(),
            arguments: json!({"path": "src/ui.rs"}).to_string(),
            status,
            label: None,
            detail: None,
            started_at: None,
            ended_at: None,
            duration_ms: None,
            attempt: None,
            parent_call_id: None,
            parallel_group_id: None,
            job_id: None,
            result: None,
            error: None,
        },
    })
}

#[test]
fn a_new_turn_does_not_inherit_old_progress_or_operations() {
    let mut app = App {
        conversation: vec![
            user(),
            tool(ToolCallStatus::Failed),
            tool(ToolCallStatus::Running),
            activity("failed"),
            user(),
        ],
        ..App::default()
    };
    assert_eq!(TaskStatus::for_app(&app), TaskStatus::Ready);
    assert_eq!(tool_step_counts(&app), (0, 0, 0));
    assert!(live_operation(&app).is_none());
    app.conversation.push(tool(ToolCallStatus::Completed));
    assert_eq!(tool_step_counts(&app), (1, 0, 0));
    app.conversation.push(tool(ToolCallStatus::Suppressed));
    assert_eq!(tool_step_counts(&app), (1, 0, 0));
}

#[test]
fn status_uses_actual_outcome_and_approval_overrides_streaming() {
    let mut app = App::default();
    for (phase, expected) in [
        ("done", TaskStatus::Complete),
        ("failed", TaskStatus::Failed),
        ("interrupted", TaskStatus::Interrupted),
    ] {
        app.conversation = vec![user(), activity(phase)];
        assert_eq!(TaskStatus::for_app(&app), expected);
    }
    app.state.is_streaming = true;
    assert_eq!(TaskStatus::for_app(&app), TaskStatus::Working);
    app.state.pending_shell_permission = Some(ShellPermission {
        id: "permit".into(),
        kind: "shell".into(),
        command: "cargo test".into(),
        operation: "test".into(),
        reason: "Verify changes".into(),
    });
    assert_eq!(TaskStatus::for_app(&app), TaskStatus::Approval);
    let mut terminal = Terminal::new(TestBackend::new(80, 2)).unwrap();
    terminal
        .draw(|frame| draw(frame, &app, frame.area()))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("Approval needed"));
    assert!(screen.contains("Enter allow"));
    assert!(screen.contains("Ctrl+C stop"));
    assert!(!screen.contains("Working"));

    let mut one_row = Terminal::new(TestBackend::new(80, 1)).unwrap();
    one_row
        .draw(|frame| draw(frame, &app, frame.area()))
        .unwrap();
    let one_row_screen: String = one_row
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(one_row_screen.contains("Approval needed"));
    assert!(one_row_screen.contains("Enter allow"));
    assert!(one_row_screen.contains("Ctrl+C stop"));
}

#[test]
fn failed_turn_keeps_the_failure_reason_visible() {
    let mut app = App {
        conversation: vec![user(), tool(ToolCallStatus::Failed), activity("failed")],
        ..App::default()
    };
    assert_eq!(height(&app), 2);

    let mut terminal = Terminal::new(TestBackend::new(80, 2)).unwrap();
    terminal
        .draw(|frame| draw(frame, &app, frame.area()))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("Failed"));
    assert!(screen.contains("1 tool step failed"));

    app.state.error_message = Some("Provider connection failed".into());
    terminal
        .draw(|frame| draw(frame, &app, frame.area()))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("Provider connection failed"));

    let mut short_wide = Terminal::new(TestBackend::new(70, 12)).unwrap();
    short_wide
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let short_wide_screen: String = short_wide
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(short_wide_screen.contains("Failed"));
    assert!(short_wide_screen.contains("Provider connection failed"));
}

#[test]
fn live_progress_remains_visible_while_reading_history() {
    for (width, height) in [(60, 20), (100, 30), (140, 32)] {
        let mut app = App {
            follow_tail: false,
            ..App::default()
        };
        app.state.is_streaming = true;
        app.state.active_model = "openai/test-model".into();
        app.conversation = vec![
            user(),
            entry(ConversationKind::Assistant {
                content: "Earlier response\n".repeat(60),
                tool_calls: vec![],
            }),
            tool(ToolCallStatus::Running),
        ];
        assert_eq!(super::height(&app), 1);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let rows: Vec<String> = buffer
            .content
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        let footer = rows[height as usize - 9..].join("\n");
        let progress_row = rows
            .iter()
            .find(|row| row.contains("Read file"))
            .expect("live operation should stay visible in the task rail");
        assert!(progress_row.contains("Working"));
        assert!(progress_row.contains("1 RUN"));
        assert!(progress_row.contains("src/ui.rs"));
        assert!(footer.contains("Next message"));
        assert!(footer.contains("Send after reply"));
        assert!(!footer.contains("Enter queue"));
        assert!(!footer.contains("Enter send"));
        assert_eq!(rows.join("\n").matches("Ctrl+End").count(), 1);
        assert_eq!(rows.join("\n").contains("Current · Working"), width >= 110);
        assert_eq!(app.scroll_y, 0);
        if width == 100 {
            println!("{}", rows.join("\n"));
        }
    }
}

#[test]
fn short_wide_history_keeps_return_to_latest_visible_during_streaming() {
    let mut app = App {
        follow_tail: false,
        ..App::default()
    };
    app.state.is_streaming = true;
    app.state.active_model = "openai/test-model".into();
    app.conversation = vec![user(), tool(ToolCallStatus::Running)];

    let mut terminal = Terminal::new(TestBackend::new(70, 12)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let rows: Vec<String> = terminal
        .backend()
        .buffer()
        .content
        .chunks(70)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect();
    let screen = rows.join("\n");
    assert!(screen.contains("Ctrl+End"));
    assert!(screen.contains("Read file"));
    assert!(screen.contains("Working"));
}

#[test]
fn compact_task_rail_keeps_history_hint_and_avoids_working_echo() {
    let mut app = App {
        follow_tail: false,
        conversation: vec![user()],
        ..App::default()
    };
    assert_eq!(height(&app), 1);

    let mut terminal = Terminal::new(TestBackend::new(80, 1)).unwrap();
    terminal
        .draw(|frame| draw(frame, &app, frame.area()))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("Ready"));
    assert!(screen.contains("History"));
    assert!(screen.contains("Ctrl+End"));

    app.follow_tail = true;
    app.conversation.clear();
    app.state.is_streaming = true;
    terminal
        .draw(|frame| draw(frame, &app, frame.area()))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert_eq!(screen.matches("Working").count(), 1);
}

#[test]
fn labels_fit_terminal_cells_and_do_not_inject_new_lines() {
    for width in 0..32 {
        let fitted = fit("긴 대화 제목\nwith a long follow-up", width);
        assert!(Span::raw(&fitted).width() <= width);
        assert!(!fitted.contains('\n'));
    }
    assert_eq!(fit("한글", 3), "한…");
}

#[test]
fn tiny_screens_and_long_titles_render_without_panicking() {
    for (width, height) in [(1, 1), (20, 8), (80, 12), (140, 32)] {
        let mut app = App::default();
        app.state.is_streaming = true;
        app.conversation = vec![entry(ConversationKind::User {
            content: "긴 대화 제목 ".repeat(100),
        })];
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
    }
}
