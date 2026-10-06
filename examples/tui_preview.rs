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
    app::{App, Mode, files::FilesState, keymap::Action},
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
            // Mirrors the Desktop / Session mockup frame.
            let now = chrono::Utc::now();
            for (session, (title, age)) in app.state.saved_sessions.iter_mut().zip([
                ("MM305 crosswind tuning", 18),
                ("TUI shell direction", 180),
                ("Runtime MCP attach", 540),
                ("Provider cleanup", 1260),
                ("Foundation memory", 2580),
            ]) {
                session.title = title.into();
                session.updated_at = (now - chrono::Duration::seconds(age)).to_rfc3339();
            }
            app.state.active_model = "anthropic/opus-5.5".into();
            app.state.active_model_context_length = Some(200_000);
            app.state.current_context_tokens = Some(58_000);
            app.state.token_usage.input_tokens = Some(1_200_000);
            app.state.token_usage.output_tokens = Some(48_000);
            app.state.token_usage.cached_input_tokens = Some(1_044_000);
            app.state.token_usage.cache_measured_input_tokens = Some(1_200_000);
            app.state.is_streaming = true;
            app.state.active_run_id = Some("run-preview".into());
            app.input =
                "Compare roll-rate peaks with MM304; flag touchdown dispersion above 150 m.".into();
            app.cursor = app.input.chars().count();
            let summary = |id: &str, text: &str| ConversationEntry {
                id: id.into(),
                kind: ConversationKind::Reasoning {
                    content: String::new(),
                    summary: Some(text.into()),
                },
            };
            let done = ToolCallStatus::Completed;
            app.conversation = vec![
                ConversationEntry {
                    id: "session-user".into(),
                    kind: ConversationKind::User {
                        content: "The handoff still prefers the nominal 12 km HAC whenever it qualifies.\nMake the radius genuinely state-dependent, then verify Final delivery.".into(),
                    },
                },
                ConversationEntry {
                    id: "intro".into(),
                    kind: ConversationKind::Assistant {
                        content: "The early exit is the problem. A qualifying nominal candidate prevents tighter flyable radii from competing.\nI’m tracing the qualification gate against the MM304 handoff energy before changing the ranking.".into(),
                        tool_calls: vec![],
                    },
                },
                summary("s1", "**Analyzed recent MM304 logs**"),
                tool_event("r1", "web_read", "Read", "ntrs.nasa.gov · TAEM energy management", done, Some("ok"), None),
                tool_event("q1", "search_workspace", "Search", "\"MM304\"", done, Some("ok"), None),
                tool_event("r2", "web_read", "Read", "nasa.gov/archive/entry-guidance", ToolCallStatus::Failed, None, Some("404")),
                summary("s2", "**Editing MM304 guidance logic to reflect proper decision**"),
                tool_event("e1", "apply_file_edits", "Edit", "guidance_taem.c", done, Some("ok"), None),
                tool_event("q2", "search_workspace", "Search", "\"MM304 gate\"", done, Some("ok"), None),
                ConversationEntry {
                    id: "thinking".into(),
                    kind: ConversationKind::Reasoning {
                        content: "NASA document indicates the radius should absorb excess energy before the HAC turn.".into(),
                        summary: None,
                    },
                },
                summary("s3", "**Clarifying MM304 behavior**"),
                tool_event("r3", "read_file", "Read", "guidance_taem.c", done, Some("ok"), None),
                tool_event("q3", "search_workspace", "Search", "\"handoff energy\"", done, Some("ok"), None),
                tool_event("e2", "apply_file_edits", "Edit", "guidance_taem.c", done, Some("ok"), None),
                summary("s4", "**Editing MM304 guidance logic to reflect proper decision**"),
                tool_event("e3", "apply_file_edits", "Edit", "guidance_taem.c", done, Some("ok"), None),
                tool_event("q4", "search_workspace", "Search", "\"MM305 qualification gate\"", ToolCallStatus::Running, None, None),
                ConversationEntry {
                    id: "live-answer".into(),
                    kind: ConversationKind::Assistant {
                        content: "The ranking now lets every flyable radius compete while Final-speed shortfall remains dominant.\nNext I’m checking whether the tighter candidate remains inside the native authority gate.".into(),
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
        "diff" => {
            let root = std::env::current_dir()?;
            let mut state = yeet::app::diff::DiffState::default();
            state.root = root;
            state.paths = [
                "guidance/guidance_taem.c",
                "guidance/guidance_taem.h",
                "team/candidate_search.c",
                "team/candidate_search.h",
                "tests/team_candidates.c",
                "tests/mm305-sweep.txt",
            ]
            .into_iter()
            .map(Into::into)
            .collect();
            state.statuses = vec![" M".into(); 6];
            state.original_paths = vec![None; 6];
            state.totals = yeet::workbench::ChangeStats {
                added: 73,
                removed: 29,
            };
            state.base = "8f3a2c1".into();
            state.branch = "candidate search".into();
            state.full = true;
            state.lines = [
                "diff --git a/guidance/guidance_taem.c b/guidance/guidance_taem.c",
                "--- a/guidance/guidance_taem.c",
                "+++ b/guidance/guidance_taem.c",
                "@@ -418,6 +418,8 @@ select_team_mode · energy branch",
                "     const double e_nominal = hac_energy_nominal(state);",
                "-    if (energy > e_s_turn) {",
                "-        return TEAM_S_TURN;",
                "-    }",
                "+    if (energy >= e_s_turn) return TEAM_S_TURN;",
                "+    if (energy < e_low) {",
                "+        const double requested = radius_for_energy(energy);",
                "+        hac_radius = clamp(requested, HAC_MIN_RADIUS, nominal_radius);",
                "+    }",
                "     candidate.hac_radius_m = hac_radius;",
                "     return TEAM_HAC;",
                "@@ -451,3 +453,4 @@ rank_candidate · Final delivery",
                "     double score = final_speed_shortfall * SHORTFALL_COST;",
                "-    score += radius_delta * RADIUS_BIAS;",
                "+    if (final_speed_shortfall <= PREDICTION_NOISE_MPS)",
                "+        score += geometry_cost + lead_arc_penalty;",
                "     candidate.score = score;",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect();
            app.diff_tabs.open("Diff", state);
            app.mode = Mode::Diff;
            app.input =
                "Compare roll-rate peaks with MM304; flag touchdown dispersion above 150 m.".into();
            app.cursor = app.input.chars().count();
        }
        "files" => {
            let dir = std::env::current_dir()?.join("src/ui");
            let mut files = FilesState::open(dir);
            let selected = files
                .visible()
                .iter()
                .position(|entry| entry.name == "theme.rs")
                .unwrap_or(0);
            for _ in 0..selected {
                files.apply(Action::MoveDown);
            }
            files.apply(Action::OpenEntry);
            app.files = Some(files);
            app.mode = Mode::Files;
        }
        "home" | "home-launcher" => {
            app.mode = Mode::Chat;
            app.home_override = Some(true);
            app.conversation.clear();
            app.input = "Review the Home resource model and navigation changes.".into();
            app.cursor = app.input.chars().count();
            let root = std::env::current_dir()?;
            app.state.known_workspaces[0].path = root.display().to_string();
            for (index, session) in app.state.saved_sessions.iter_mut().enumerate() {
                session.title = [
                    "Home resource model",
                    "Session navigation",
                    "Git review",
                    "Provider usage",
                    "Workspace memory",
                ][index]
                    .into();
                session.updated_at =
                    (chrono::Utc::now() - chrono::Duration::minutes(index as i64 * 8)).to_rfc3339();
            }
            for path in ["src/app/home.rs", "src/workbench/content.rs"] {
                app.recent_views
                    .visit(yeet::workbench::ResourceTarget::File(root.join(path)), path);
            }
            app.home
                .set_git_snapshot(yeet::workbench::GitSnapshot::load(&root));
            let mut files = FilesState::open(root.join("src"));
            files.open_path(root.join("src/theme.rs"), false);
            files.open_path(root.join("src/ui/views/home.rs"), false);
            app.files = Some(files);
            if scene == "home-launcher" {
                app.mode = Mode::Views;
                app.views_origin = Mode::Chat;
                app.views_index = 0;
            }
        }
        "agents" | "agent" => {
            use yeet::model::{
                AgentActivityItem, AgentActivityKind as Kind, AgentGroupItem, AgentMemberItem,
            };
            let at = |seconds: i64| {
                (chrono::Utc::now() - chrono::Duration::seconds(seconds)).to_rfc3339()
            };
            let member =
                |name: &str, status: &str, task: &str, started: i64, tokens: (u64, u64)| {
                    AgentMemberItem {
                        id: name.into(),
                        description: name.into(),
                        role: "researcher".into(),
                        model: "opus-5.5 (high)".into(),
                        status: status.into(),
                        task_status: task.into(),
                        summary: None,
                        started_at: at(started),
                        input_tokens: tokens.0,
                        output_tokens: tokens.1,
                        ..Default::default()
                    }
                };
            let entry = |seconds,
                         from: Option<&str>,
                         to: Option<&str>,
                         kind,
                         tool: Option<&str>,
                         text: &str| AgentActivityItem {
                at: at(seconds),
                from: from.map(Into::into),
                to: to.map(Into::into),
                kind,
                tool: tool.map(Into::into),
                text: text.into(),
            };
            app.state.agent_group = AgentGroupItem {
                members: vec![
                    member("Planner", "running", "running", 848, (312_000, 41_000)),
                    member("Runtime", "running", "running", 700, (402_000, 52_000)),
                    member(
                        "Verification",
                        "idle",
                        "needs_verification",
                        600,
                        (250_000, 31_000),
                    ),
                    member("Telemetry", "idle", "reported", 900, (120_000, 12_000)),
                    member("Docs", "idle", "reported", 1_200, (116_000, 12_000)),
                ],
                activity: vec![
                    entry(
                        848,
                        None,
                        Some("Planner"),
                        Kind::Message,
                        None,
                        "Find the smallest radius that still delivers Final speed.",
                    ),
                    entry(
                        540,
                        Some("Runtime"),
                        Some("Planner"),
                        Kind::Message,
                        None,
                        "Guidance code still prefers the nominal radius.",
                    ),
                    entry(
                        360,
                        Some("Docs"),
                        None,
                        Kind::Tool,
                        Some("web_read"),
                        "NASA shuttle TAEM geometry reference",
                    ),
                    entry(
                        300,
                        Some("Planner"),
                        Some("Verification"),
                        Kind::Message,
                        None,
                        "Re-run the gate once the radius change lands.",
                    ),
                    entry(
                        300,
                        Some("Planner"),
                        None,
                        Kind::Finished,
                        None,
                        "Final-speed shortfall stays dominant inside the prediction-noise band",
                    ),
                    entry(
                        240,
                        Some("Verification"),
                        None,
                        Kind::Finished,
                        None,
                        "Offline heading sweep accepted",
                    ),
                    entry(
                        180,
                        Some("Verification"),
                        Some("Planner"),
                        Kind::Message,
                        None,
                        "Final speed misses at 3 km for headings above 270°.",
                    ),
                    entry(
                        120,
                        Some("Planner"),
                        Some("Runtime"),
                        Kind::Message,
                        None,
                        "Use 3 km as the floor and sweep upward from there.",
                    ),
                    entry(
                        60,
                        Some("Verification"),
                        None,
                        Kind::Tool,
                        Some("run_shell"),
                        "MM305 qualification gate",
                    ),
                    entry(
                        60,
                        Some("Runtime"),
                        None,
                        Kind::Tool,
                        Some("apply_file_edits"),
                        "guidance_taem.c +18 −6",
                    ),
                    entry(
                        41,
                        Some("Planner"),
                        None,
                        Kind::Tool,
                        Some("read_file"),
                        "NTRS TAEM energy notes §4.3",
                    ),
                    entry(
                        2,
                        Some("Planner"),
                        None,
                        Kind::Tool,
                        Some("run_shell"),
                        "Sweeping feasible radii against the Final-speed gate",
                    ),
                ],
                started_at: Some(at(1_240)),
                input_tokens: 1_200_000,
                output_tokens: 148_000,
                ..Default::default()
            };
            app.application.compatibility_agent_state_mut().open = true;
            app.mode = Mode::Agents;
            app.input_focused = false;
            if scene == "agent" {
                app.application.compatibility_agent_state_mut().selected = Some("Planner".into());
            }
        }
        "views" => {
            app.files = Some(FilesState::open(std::env::current_dir()?));
            app.mode = Mode::Views;
            app.views_origin = Mode::Files;
            app.views_index = 1;
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
