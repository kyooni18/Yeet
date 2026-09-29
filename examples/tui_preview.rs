//! Render real TUI cells without a backend connection or credentials.
//! cargo run --example tui_preview -- welcome 140 38 > /tmp/yeet-welcome.json
//! cargo run --example tui_preview -- conversation 100 32 --ansi
use ratatui::{
    Terminal,
    backend::TestBackend,
    style::{Color, Modifier},
};
use serde_json::json;
use yeet::{
    app::{App, Mode},
    model::{
        ConversationEntry, ConversationKind, ConversationToolCall, ModelActivity, SessionSummary,
        ShellPermission, ToolCallStatus, WorkspaceSummary,
    },
};

fn tool_event(
    id: &str,
    name: &str,
    label: &str,
    detail: &str,
    status: ToolCallStatus,
    result: Option<&str>,
    error: Option<&str>,
) -> ConversationEntry {
    ConversationEntry {
        id: id.to_owned(),
        kind: ConversationKind::ToolCall {
            tool_call: ConversationToolCall {
                id: id.to_owned(),
                index: None,
                call_id: None,
                name: name.to_owned(),
                arguments: "{}".into(),
                status,
                label: Some(label.to_owned()),
                detail: Some(detail.to_owned()),
                started_at: None,
                ended_at: None,
                duration_ms: None,
                attempt: None,
                parent_call_id: None,
                parallel_group_id: None,
                job_id: None,
                result: result.map(str::to_owned),
                error: error.map(str::to_owned),
            },
        },
    }
}

fn color(color: Color, fallback: &str) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => fallback.to_owned(),
    }
}

fn main() -> anyhow::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let scene = args.get(1).map(String::as_str).unwrap_or("conversation");
    let width = args
        .get(2)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(140)
        .clamp(1, 240);
    let height = args
        .get(3)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(38)
        .clamp(1, 100);
    let mut app = App::default();
    app.state.active_model = "openai/example-model".into();
    app.state.active_reasoning_level = "high".into();
    app.state.current_context_tokens = Some(24_800);
    app.state.active_model_context_length = Some(128_000);
    app.state.known_workspaces = vec![WorkspaceSummary {
        id: "yeet".into(),
        path: "/workspace/Yeet".into(),
        display_name: "Yeet".into(),
        updated_at: None,
        session_count: 5,
        is_current: true,
    }];
    app.state.saved_sessions = [
        "MM305 crosswind tuning",
        "Refine the terminal experience",
        "Runtime MCP attach",
        "Provider cleanup",
        "Foundation memory",
    ]
    .iter()
    .enumerate()
    .map(|(i, title)| SessionSummary {
        id: format!("session-{i}"),
        title: (*title).into(),
        updated_at: "preview".into(),
        model: app.state.active_model.clone(),
        message_count: 8,
    })
    .collect();
    if scene != "welcome" {
        app.state.current_session_id = Some("session-0".into());
        app.conversation = vec![
            ConversationEntry {id: "user".into(), kind: ConversationKind::User {content: "Give the terminal a stronger visual identity. Keep the conversation easy to read.".into()}},
            ConversationEntry {id: "thoughts".into(), kind: ConversationKind::Reasoning {content: "I’ll check the layout, message hierarchy, and keyboard navigation together.".into(), summary: None}},
            ConversationEntry {id: "reply".into(), kind: ConversationKind::Assistant {content: "## A clearer workspace\n\nThe conversation now has distinct message styles, a focused composer, and a persistent view of the current task.\n\n- **Coral** identifies Yeet and the active composer.\n- **Teal** separates your messages from the response.\n- **Violet** marks model choices and commands.\n\n```rust\nlet workspace = Workspace::open(path)?;\nworkspace.render(&mut terminal);\n```\n\nTry `/models` to choose a model, or **Alt+S** to switch sessions.".into(), tool_calls: vec![]}},
            ConversationEntry {id: "done".into(), kind: ConversationKind::Activity {activity: ModelActivity {phase: json!("done"), title: "Updated the terminal layout".into(), detail: None, run_id: None}}},
        ];
    }
    match scene {
        "session" => {
            app.state.current_session_id = Some("session-0".into());
            app.state.is_streaming = true;
            app.state.active_assistant_entry_id = Some("live-answer".into());
            app.state.active_assistant_text = "The session preview uses the real transcript renderer and streaming state. The tool steps below show how completed work, an error, and a live operation appear in sequence.".into();
            app.state.active_activity_entry_id = Some("live-activity".into());
            app.state.active_run_id = Some("run-preview".into());
            app.state.current_context_tokens = Some(8_420);
            app.conversation = vec![
                ConversationEntry {
                    id: "session-user".into(),
                    kind: ConversationKind::User {
                        content: "Inspect the current session layout and make the activity stream easier to scan.".into(),
                    },
                },
                ConversationEntry {
                    id: "session-reasoning".into(),
                    kind: ConversationKind::Reasoning {
                        content: "I’ll check the view structure, inspect the render path, and keep the active session state attached to the existing shell.".into(),
                        summary: Some("Reviewing the UI integration points".into()),
                    },
                },
                tool_event(
                    "read-complete",
                    "read_file",
                    "Read",
                    "src/ui/shell.rs",
                    ToolCallStatus::Completed,
                    Some("Inspected shell layout and session context."),
                    None,
                ),
                tool_event(
                    "read-failed",
                    "read_file",
                    "Read",
                    "missing reference file",
                    ToolCallStatus::Failed,
                    None,
                    Some("File does not exist"),
                ),
                tool_event(
                    "edit-running",
                    "apply_file_edits",
                    "Edit",
                    "src/ui/sessions.rs",
                    ToolCallStatus::Running,
                    None,
                    None,
                ),
                ConversationEntry {
                    id: "live-activity".into(),
                    kind: ConversationKind::Activity {
                        activity: ModelActivity {
                            phase: json!("working"),
                            title: "Rendering the active session".into(),
                            detail: Some("Checking conversation and tool progress".into()),
                            run_id: Some("run-preview".into()),
                        },
                    },
                },
                ConversationEntry {
                    id: "live-answer".into(),
                    kind: ConversationKind::Assistant {
                        content: String::new(),
                        tool_calls: vec![],
                    },
                },
            ];
        }
        "empty-session" => {
            app.state.current_session_id = Some("session-empty".into());
            app.state.saved_sessions.insert(
                0,
                SessionSummary {
                    id: "session-empty".into(),
                    title: "New session".into(),
                    updated_at: "preview".into(),
                    model: app.state.active_model.clone(),
                    message_count: 0,
                },
            );
            app.conversation.clear();
        }
        "session-picker" => {
            app.mode = Mode::Sessions;
        }
        "working" | "history" => {
            app.state.is_streaming = true;
            app.state.active_activity_entry_id = Some("live".into());
            app.conversation.push(ConversationEntry {
                id: "live".into(),
                kind: ConversationKind::Activity {
                    activity: ModelActivity {
                        phase: json!("working"),
                        title: "Checking terminal layouts".into(),
                        detail: Some("Verifying narrow widths and Unicode wrapping".into()),
                        run_id: None,
                    },
                },
            });
            if scene == "history" {
                app.follow_tail = false;
                app.conversation.insert(
                    1,
                    ConversationEntry {
                        id: "earlier".into(),
                        kind: ConversationKind::Assistant {
                            content: "Earlier conversation content.\n".repeat(70),
                            tool_calls: vec![],
                        },
                    },
                );
            }
        }
        "approval" => {
            app.state.pending_shell_permission = Some(ShellPermission {
                id: "preview".into(),
                kind: "shell".into(),
                command: "cargo test --lib ui::".into(),
                operation: "Run UI tests".into(),
                reason: "Verify terminal layouts before continuing.".into(),
            })
        }
        "failure" => {
            app.state.error_message =
                Some("The provider connection closed. Your draft is preserved.".into())
        }
        "commands" => {
            app.input = "/".into();
            app.cursor = 1;
        }
        "help" => app.mode = Mode::Help,
        "models" => {
            app.mode = Mode::Models;
            app.state.available_models = vec![
                "openai/example-model".into(),
                "openai/fast-model".into(),
                "anthropic/example-model".into(),
            ];
        }
        "welcome" | "conversation" => (),
        _ => anyhow::bail!("Unknown scene: {scene}"),
    }
    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
    terminal.draw(|frame| yeet::ui::draw(frame, &mut app))?;
    if args.iter().any(|arg| arg == "--ansi") {
        for row in terminal.backend().buffer().content.chunks(width as usize) {
            for cell in row {
                print!("\x1b[0m");
                if let Color::Rgb(r, g, b) = cell.fg {
                    print!("\x1b[38;2;{r};{g};{b}m");
                }
                if let Color::Rgb(r, g, b) = cell.bg {
                    print!("\x1b[48;2;{r};{g};{b}m");
                }
                if cell.modifier.contains(Modifier::BOLD) {
                    print!("\x1b[1m");
                }
                if cell.modifier.contains(Modifier::ITALIC) {
                    print!("\x1b[3m");
                }
                print!("{}", cell.symbol());
            }
            println!("\x1b[0m");
        }
        return Ok(());
    }
    let cells = terminal.backend().buffer().content.iter().map(|cell| json!({
        "text": cell.symbol(), "fg": color(cell.fg, "#e9eff7"), "bg": color(cell.bg, "#10141d"),
        "bold": cell.modifier.contains(Modifier::BOLD), "italic": cell.modifier.contains(Modifier::ITALIC),
    })).collect::<Vec<_>>();
    println!(
        "{}",
        json!({"scene":scene,"width":width,"height":height,"cells":cells})
    );
    Ok(())
}
