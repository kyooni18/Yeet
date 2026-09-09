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
        },
    })
}

#[test]
fn a_new_turn_does_not_inherit_old_progress_or_operations() {
    let mut app = App {
        conversation: vec![
            user(),
            tool(ToolCallStatus::Failed),
            tool(ToolCallStatus::Streaming),
            activity("failed"),
            user(),
        ],
        ..App::default()
    };
    assert_eq!(TaskStatus::for_app(&app), TaskStatus::Ready);
    assert_eq!(tool_step_counts(&app), (0, 0));
    assert!(live_operation(&app).is_none());
    app.conversation.push(tool(ToolCallStatus::Completed));
    assert_eq!(tool_step_counts(&app), (1, 0));
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
    assert!(!screen.contains("Working"));
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
            tool(ToolCallStatus::Streaming),
        ];
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
        assert!(footer.contains("Working"));
        assert!(footer.contains("Read file"));
        assert!(footer.contains("src/ui.rs"));
        assert!(footer.contains("BUFFERED DIRECTIVE"));
        assert!(!footer.contains("Enter send"));
        assert!(rows.join("\n").contains("Ctrl+End"));
        assert_eq!(rows.join("\n").contains("Current · Working"), width >= 110);
        assert_eq!(app.scroll_y, 0);
        if width == 100 {
            println!("{}", rows.join("\n"));
        }
    }
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
