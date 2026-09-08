mod composer;
mod dialogs;
mod shell;
mod task;
mod theme;
use dialogs::{
    draw_agent_mode, draw_auth, draw_auth_key, draw_capabilities, draw_capability_detail,
    draw_help, draw_models, draw_permission, draw_provider_edit, draw_providers, draw_reasoning,
    draw_sandbox_policy, draw_sandbox_presets, draw_sessions, draw_settings, draw_settings_edit,
};

mod markdown;
use markdown::markdown_lines;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Position, Rect},
    prelude::{Color, Line, Modifier, Span, Style, Text},
    widgets::{
        Block, BorderType, Borders, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
};
use serde_json::Value;

use crate::{
    app::{App, Mode},
    model::{ConversationEntry, ConversationKind, ToolCallStatus},
};

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    frame.render_widget(Block::default().style(theme::base()), frame.area());
    let area = shell::draw_shell(frame, app);
    let suggestions = app.command_suggestions();
    let suggestion_height = if suggestions.is_empty() {
        0
    } else {
        (suggestions.len() as u16 + 2).min(8).min(area.height / 3)
    };
    let input_rows = composer::layout(&app.input, app.cursor, area.width.saturating_sub(4));
    let input_height = (input_rows.lines.len() as u16).clamp(3, 7) + 2;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(task::height(app)),
            Constraint::Length(suggestion_height),
            Constraint::Length(input_height),
            Constraint::Length(2),
        ])
        .split(area);

    draw_transcript(frame, app, chunks[0]);
    if suggestion_height > 0 {
        draw_suggestions(frame, app, chunks[2], &suggestions);
    }
    task::draw(frame, app, chunks[1]);
    draw_input(frame, app, chunks[3]);
    draw_status(frame, app, chunks[4]);

    match app.mode {
        Mode::Debate => dialogs::draw_debate(frame, app),
        Mode::Models => draw_models(frame, app),
        Mode::Reasoning => draw_reasoning(frame, app),
        Mode::AgentMode => draw_agent_mode(frame, app),
        Mode::Sessions => draw_sessions(frame, app),
        Mode::Capabilities => draw_capabilities(frame, app),
        Mode::CapabilityDetail => draw_capability_detail(frame, app),
        Mode::Auth => draw_auth(frame, app),
        Mode::AuthKey => draw_auth_key(frame, app),
        Mode::Providers => draw_providers(frame, app),
        Mode::ProviderEdit => draw_provider_edit(frame, app),
        Mode::Settings => draw_settings(frame, app),
        Mode::SandboxPresets => draw_sandbox_presets(frame, app),
        Mode::SandboxPolicy => draw_sandbox_policy(frame, app),
        Mode::SettingsEdit => {
            draw_sandbox_policy(frame, app);
            draw_settings_edit(frame, app);
        }
        Mode::Help => draw_help(frame),
        Mode::Chat => {}
    }

    if app.state.pending_shell_permission.is_some()
        || app.state.pending_native_app_permission.is_some()
    {
        draw_permission(frame, app);
    }
}

fn draw_transcript(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let area = area.inner(Margin {
        horizontal: 1,
        vertical: u16::from(area.height > 4),
    });
    if app.conversation.is_empty() {
        app.max_scroll = 0;
        app.scroll_y = 0;
        shell::draw_welcome(frame, area);
        return;
    }
    let text = transcript_text(app, area.width);
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    let line_count = paragraph.line_count(area.width.max(1)) as u16;
    app.max_scroll = line_count.saturating_sub(area.height);
    if app.follow_tail {
        app.scroll_y = app.max_scroll;
    } else {
        app.scroll_y = app.scroll_y.min(app.max_scroll);
    }
    frame.render_widget(paragraph.scroll((app.scroll_y, 0)), area);
    if app.max_scroll > 0 && area.width > 1 {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_style(Style::default().fg(theme::BORDER))
            .thumb_style(Style::default().fg(theme::ACCENT));
        let mut state = ScrollbarState::new(app.max_scroll as usize + 1)
            .position(app.scroll_y as usize)
            .viewport_content_length(area.height as usize);
        frame.render_stateful_widget(
            scrollbar,
            Rect::new(area.right(), area.y, 1, area.height),
            &mut state,
        );
    }
}

fn draw_suggestions(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    suggestions: &[(&'static str, &'static str)],
) {
    let items = suggestions
        .iter()
        .enumerate()
        .map(|(index, (name, description))| {
            let style = if index == app.command_index {
                Style::default()
                    .bg(theme::SELECTED)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {name:<14}"), style),
                Span::styled(*description, style.fg(Color::Gray)),
            ]))
        });
    let list = List::new(items).block(theme::modal_block("Commands · ↑/↓ choose · Tab complete"));
    let mut state = ratatui::widgets::ListState::default().with_selected(Some(app.command_index));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_input(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let border_style = if app.state.is_streaming {
        Style::default().fg(theme::ACCENT)
    } else {
        Style::default().fg(theme::BORDER)
    };
    let title = if app.state.is_streaming {
        " ❯  Draft next message ".to_owned()
    } else {
        " ❯  Message ".to_owned()
    };
    let hint = if app.state.is_streaming {
        " Esc interrupts · Send when task finishes "
    } else {
        " Enter send · / commands "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .style(theme::surface())
        .padding(ratatui::widgets::Padding::horizontal(1))
        .title(title)
        .title_bottom(Line::from(hint).style(Style::default().fg(theme::MUTED)))
        .title_style(
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let layout = composer::layout(&app.input, app.cursor, inner.width);
    let scroll = layout.row.saturating_sub(inner.height as usize - 1);
    let lines = if app.input.is_empty() {
        vec![Line::styled(
            "Ask anything…",
            Style::default().fg(theme::MUTED),
        )]
    } else {
        layout
            .lines
            .iter()
            .skip(scroll)
            .take(inner.height as usize)
            .map(|line| Line::raw(line.clone()))
            .collect()
    };
    frame.render_widget(Paragraph::new(lines), inner);
    if app.mode == Mode::Chat
        && app.state.pending_shell_permission.is_none()
        && app.state.pending_native_app_permission.is_none()
    {
        frame.set_cursor_position(Position::new(
            inner.x + (layout.column as u16).min(inner.width - 1),
            inner.y + (layout.row - scroll) as u16,
        ));
    }
}

fn draw_status(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }

    let secondary = secondary_status_line(app, width);

    if let Some(message) = app
        .state
        .error_message
        .as_deref()
        .or(app.backend_message.as_deref())
    {
        let text = truncate_end(message, width.saturating_sub(3));
        frame.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from(vec![
                    Span::styled(
                        " × ",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(text, Style::default().fg(Color::Red)),
                ]),
                secondary,
            ])),
            area,
        );
        return;
    }

    frame.render_widget(
        Paragraph::new(Text::from(vec![primary_status_line(app, width), secondary])),
        area,
    );
}

fn primary_status_line(app: &App, width: usize) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(model_spans(&app.state.active_model, width));

    if width >= 42 {
        let mode = if app.state.active_agent_mode.is_empty() {
            "auto"
        } else {
            &app.state.active_agent_mode
        };
        spans.push(separator());
        spans.push(Span::styled("⌘ mode ", Style::default().fg(theme::MUTED)));
        spans.push(Span::styled(
            mode.to_owned(),
            Style::default().fg(Color::Gray),
        ));
    }

    if width >= 56 {
        let reasoning = if app.state.active_reasoning_level.is_empty() {
            "auto"
        } else {
            &app.state.active_reasoning_level
        };
        spans.push(separator());
        spans.push(Span::styled("✦ think ", Style::default().fg(theme::MUTED)));
        spans.push(Span::styled(
            reasoning.to_owned(),
            Style::default().fg(Color::Gray),
        ));
    }

    Line::from(spans)
}

fn secondary_status_line(app: &App, width: usize) -> Line<'static> {
    let usage = &app.state.token_usage;
    let mut spans = vec![Span::raw(" ")];

    if let Some(context) = app.state.active_model_context_length {
        push_status_metric(&mut spans, "◫ ctx", compact_number(context));
    }

    if width >= 34 {
        let input = usage.input_tokens.unwrap_or(0);
        let output = usage.output_tokens.unwrap_or(0);
        if input > 0 || output > 0 {
            push_status_metric(
                &mut spans,
                "⇅ tok",
                format!(
                    "in {} out {}",
                    compact_number(input),
                    compact_number(output)
                ),
            );
        }
    }

    if width >= 58
        && let Some(cached) = usage.cached_input_tokens.filter(|value| *value > 0)
    {
        push_status_metric(&mut spans, "↻ cache", compact_number(cached));
    }

    if width >= 74
        && let Some(reasoning) = usage.reasoning_tokens.filter(|value| *value > 0)
    {
        push_status_metric(&mut spans, "◌ reason", compact_number(reasoning));
    }

    if width >= 74
        && let Some(cost) = usage.estimated_cost_usd.filter(|value| *value > 0.0)
    {
        let value = if cost < 0.01 {
            format!("${cost:.4}")
        } else {
            format!("${cost:.2}")
        };
        push_status_metric(&mut spans, "$ cost", value);
    }

    if width >= 88 && app.state.credit_usage > 0 {
        push_status_metric(&mut spans, "◆ credits", app.state.credit_usage.to_string());
    }

    if app.state.is_streaming
        && width >= 48
        && let Some(elapsed) = app.stream_elapsed()
    {
        push_status_metric(&mut spans, "◷ time", format_elapsed(elapsed.as_millis()));
    }

    Line::from(spans)
}

fn push_status_metric(spans: &mut Vec<Span<'static>>, label: &str, value: String) {
    if spans.len() > 1 {
        spans.push(separator());
    }
    spans.push(Span::styled(
        format!("{label} "),
        Style::default().fg(theme::MUTED),
    ));
    spans.push(Span::styled(value, Style::default().fg(Color::Gray)));
}

fn separator() -> Span<'static> {
    Span::styled(" · ", Style::default().fg(theme::MUTED))
}

fn model_spans(model: &str, width: usize) -> Vec<Span<'static>> {
    if model.is_empty() {
        return vec![Span::styled(
            "◈ model  Alt+M select".to_owned(),
            Style::default().fg(Color::Yellow),
        )];
    }

    let (provider, name) = model.split_once('/').unwrap_or(("", model));
    if width < 46 || provider.is_empty() {
        return vec![
            Span::styled("◈ model ", Style::default().fg(theme::MUTED)),
            Span::styled(
                truncate_middle(
                    name,
                    if width < 34 {
                        width.saturating_sub(6).max(8)
                    } else {
                        30
                    },
                ),
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
    }

    vec![
        Span::styled("◈ model ", Style::default().fg(theme::MUTED)),
        Span::styled(provider.to_owned(), Style::default().fg(theme::MUTED)),
        Span::styled("/", Style::default().fg(theme::MUTED)),
        Span::styled(
            truncate_middle(name, if width < 76 { 24 } else { 32 }),
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ),
    ]
}

fn live_activity(app: &App) -> (String, Option<&str>) {
    if app.state.active_reasoning_entry_id.is_some() {
        return ("Reasoning".into(), None);
    }
    let Some(id) = app.state.active_activity_entry_id.as_deref() else {
        return ("Working".into(), None);
    };
    app.conversation
        .iter()
        .rev()
        .find(|entry| entry.id == id)
        .and_then(|entry| match &entry.kind {
            ConversationKind::Activity { activity } => {
                Some((activity.title.clone(), activity.detail.as_deref()))
            }
            _ => None,
        })
        .unwrap_or_else(|| ("Working".into(), None))
}

fn tool_step_counts(app: &App) -> (usize, usize) {
    let mut counts = (0usize, 0usize);
    for entry in task::latest_turn(app) {
        let calls: &[crate::model::ConversationToolCall] = match &entry.kind {
            ConversationKind::ToolCall { tool_call } => std::slice::from_ref(tool_call),
            ConversationKind::Assistant { tool_calls, .. } => tool_calls,
            _ => continue,
        };
        for call in calls {
            match call.status {
                ToolCallStatus::Completed => counts.0 += 1,
                ToolCallStatus::Failed => counts.1 += 1,
                ToolCallStatus::Streaming => {}
            }
        }
    }
    counts
}

fn spinner_frame(elapsed_ms: u128) -> &'static str {
    const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    FRAMES[((elapsed_ms / 80) as usize) % FRAMES.len()]
}

fn format_elapsed(elapsed_ms: u128) -> String {
    if elapsed_ms < 60_000 {
        format!("{:.1}s", elapsed_ms as f64 / 1_000.0)
    } else {
        let seconds = elapsed_ms / 1_000;
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    }
}

fn truncate_end(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_owned();
    }
    if max_chars == 1 {
        return "…".into();
    }
    format!("{}…", value.chars().take(max_chars - 1).collect::<String>())
}

fn truncate_middle(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_chars {
        return value.to_owned();
    }
    if max_chars <= 2 {
        return "…".into();
    }
    let left = (max_chars - 1) / 2;
    let right = max_chars - 1 - left;
    format!(
        "{}…{}",
        chars[..left].iter().collect::<String>(),
        chars[chars.len() - right..].iter().collect::<String>()
    )
}

fn transcript_text(app: &App, width: u16) -> Text<'static> {
    let mut lines = Vec::new();
    let mut rendered_any = false;
    let mut previous_compact = false;

    for entry in &app.conversation {
        if matches!(
            &entry.kind,
            ConversationKind::Activity { activity } if !terminal_activity(activity)
        ) {
            continue;
        }
        let compact = compact_transcript_entry(&entry.kind);
        if rendered_any && (!compact || !previous_compact) {
            lines.push(Line::from(""));
        }
        lines.extend(entry_lines(app, entry, width));
        rendered_any = true;
        previous_compact = compact;
    }

    Text::from(lines)
}

struct LiveOperation {
    label: String,
    detail: Option<String>,
}

fn live_operation(app: &App) -> Option<LiveOperation> {
    if app.state.active_reasoning_entry_id.is_some() {
        return Some(LiveOperation {
            label: "Reasoning".into(),
            detail: live_reasoning_detail(app),
        });
    }

    if let Some(call) = task::latest_turn(app)
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            ConversationKind::ToolCall { tool_call }
                if matches!(tool_call.status, ToolCallStatus::Streaming) =>
            {
                Some(tool_call)
            }
            ConversationKind::Assistant { tool_calls, .. } => tool_calls
                .iter()
                .rev()
                .find(|call| matches!(call.status, ToolCallStatus::Streaming)),
            _ => None,
        })
    {
        return Some(LiveOperation {
            label: format!("{} {}", tool_glyph(&call.name), tool_title(&call.name)),
            detail: tool_argument_summary(&call.arguments, 72),
        });
    }

    if app.state.active_assistant_entry_id.is_some() {
        return Some(LiveOperation {
            label: "Responding".into(),
            detail: None,
        });
    }

    None
}

fn live_reasoning_detail(app: &App) -> Option<String> {
    let id = app.state.active_reasoning_entry_id.as_deref()?;
    if !app.state.active_reasoning_summary.trim().is_empty() {
        return Some(app.state.active_reasoning_summary.trim().to_owned());
    }
    if let Some(line) = app
        .state
        .active_reasoning_text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
    {
        return Some(line.trim().to_owned());
    }
    app.conversation.iter().rev().find_map(|entry| {
        if entry.id != id {
            return None;
        }
        match &entry.kind {
            ConversationKind::Reasoning { content, summary } => summary
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| content.lines().rev().find(|line| !line.trim().is_empty()))
                .map(str::trim)
                .map(str::to_owned),
            _ => None,
        }
    })
}

fn compact_transcript_entry(kind: &ConversationKind) -> bool {
    matches!(
        kind,
        ConversationKind::Activity { .. }
            | ConversationKind::Reasoning { .. }
            | ConversationKind::ToolCall { .. }
    )
}

fn terminal_activity(activity: &crate::model::ModelActivity) -> bool {
    matches!(
        activity.phase.as_str(),
        Some("done" | "failed" | "interrupted")
    )
}

fn entry_lines(app: &App, entry: &ConversationEntry, width: u16) -> Vec<Line<'static>> {
    match &entry.kind {
        ConversationKind::User { content } => content
            .lines()
            .map(|line| Line::styled(format!(" {line} "), theme::surface()))
            .collect(),
        ConversationKind::Assistant {
            content,
            tool_calls,
        } => {
            let content =
                if app.state.active_assistant_entry_id.as_deref() == Some(entry.id.as_str()) {
                    &app.state.active_assistant_text
                } else {
                    content
                };
            let elapsed_ms = app.stream_elapsed().unwrap_or_default().as_millis();
            let mut lines = markdown_lines(content);
            for call in tool_calls {
                lines.extend(tool_lines(elapsed_ms, call, width));
            }
            lines
        }
        ConversationKind::Reasoning { content, summary } => {
            let (content, summary) =
                if app.state.active_reasoning_entry_id.as_deref() == Some(entry.id.as_str()) {
                    (
                        app.state.active_reasoning_text.as_str(),
                        (!app.state.active_reasoning_summary.is_empty())
                            .then_some(app.state.active_reasoning_summary.as_str()),
                    )
                } else {
                    (content.as_str(), summary.as_deref())
                };
            let mut lines = vec![Line::from(vec![
                Span::styled("◌ reasoning", Style::default().fg(theme::MUTED)),
                Span::styled(
                    summary
                        .map(|value| format!(" · {value}"))
                        .unwrap_or_default(),
                    Style::default().fg(theme::MUTED),
                ),
            ])];
            for line in content.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default().fg(theme::MUTED),
                )));
            }
            lines
        }
        ConversationKind::Activity { activity } => {
            let latest = app.state.active_activity_entry_id.as_deref() == Some(entry.id.as_str());
            vec![activity_line(app, activity, false, latest, width)]
        }
        ConversationKind::ToolCall { tool_call } => tool_lines(
            app.stream_elapsed().unwrap_or_default().as_millis(),
            tool_call,
            width,
        ),
        ConversationKind::Skill {
            name,
            content,
            status,
        } => {
            let mut lines = vec![Line::from(vec![
                Span::styled("skill ", Style::default().fg(Color::Magenta)),
                Span::styled(name.clone(), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(
                    status
                        .as_deref()
                        .map(|value| format!(" · {value}"))
                        .unwrap_or_default(),
                    Style::default().fg(theme::MUTED),
                ),
            ])];
            lines.extend(markdown_lines(content));
            lines
        }
        ConversationKind::Mcp {
            server,
            name,
            content,
            is_error,
        } => {
            let color = if *is_error { Color::Red } else { Color::Blue };
            let mut lines = vec![Line::from(Span::styled(
                format!("MCP {server}/{name}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ))];
            lines.extend(content.lines().map(|line| Line::from(line.to_owned())));
            lines
        }
        ConversationKind::System { content } => content
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    line.to_owned(),
                    Style::default().fg(Color::Gray),
                ))
            })
            .collect(),
    }
}

fn activity_line(
    app: &App,
    activity: &crate::model::ModelActivity,
    active: bool,
    show_elapsed: bool,
    width: u16,
) -> Line<'static> {
    let phase = activity.phase.as_str().unwrap_or_default();
    let (marker, marker_style, title_style) = if active {
        let elapsed = app.stream_elapsed().unwrap_or_default();
        (
            spinner_frame(elapsed.as_millis()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        match phase {
            "done" => (
                "✓",
                Style::default().fg(Color::Green),
                Style::default().fg(Color::Gray),
            ),
            "failed" => (
                "×",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                Style::default().fg(Color::Red),
            ),
            "interrupted" => (
                "■",
                Style::default().fg(Color::Yellow),
                Style::default().fg(Color::Gray),
            ),
            _ => (
                "·",
                Style::default().fg(theme::MUTED),
                Style::default().fg(theme::MUTED),
            ),
        }
    };

    let mut spans = vec![
        Span::styled(format!("{marker} "), marker_style),
        Span::styled(activity.title.clone(), title_style),
    ];

    let width = width as usize;
    if width >= 42
        && let Some(detail) = activity.detail.as_deref().filter(|value| !value.is_empty())
    {
        let budget = if width >= 100 {
            52
        } else if width >= 72 {
            34
        } else {
            22
        };
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled(
            truncate_middle(detail, budget),
            Style::default().fg(theme::MUTED),
        ));
    }

    if width >= 64 && show_elapsed {
        let elapsed = if active {
            app.stream_elapsed()
        } else {
            app.latest_turn_duration()
        };
        if let Some(elapsed) = elapsed {
            spans.push(Span::styled(
                format!("  {}", format_elapsed(elapsed.as_millis())),
                Style::default().fg(theme::MUTED),
            ));
        }
    }

    Line::from(spans)
}

fn tool_lines(
    elapsed_ms: u128,
    call: &crate::model::ConversationToolCall,
    width: u16,
) -> Vec<Line<'static>> {
    let (marker, marker_style, title_style) = match call.status {
        ToolCallStatus::Streaming => (
            spinner_frame(elapsed_ms),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        ToolCallStatus::Completed => (
            "✓",
            Style::default().fg(Color::Green),
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ),
        ToolCallStatus::Failed => (
            "×",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    };

    let mut primary = vec![
        Span::styled(format!("{marker} "), marker_style),
        Span::styled(
            format!("{} ", tool_glyph(&call.name)),
            Style::default().fg(theme::MUTED),
        ),
        Span::styled(tool_title(&call.name), title_style),
    ];
    if width >= 48 {
        primary.push(Span::styled("  ·  ", Style::default().fg(theme::MUTED)));
        primary.push(Span::styled(
            truncate_middle(&call.name, (width as usize).saturating_sub(28).min(34)),
            Style::default().fg(theme::MUTED),
        ));
    }

    let mut lines = vec![Line::from(primary)];
    if width >= 32
        && let Some(summary) =
            tool_argument_summary(&call.arguments, (width as usize).saturating_sub(5))
    {
        lines.push(Line::from(vec![
            Span::styled("  └ ", Style::default().fg(theme::MUTED)),
            Span::styled(summary, Style::default().fg(theme::MUTED)),
        ]));
    }
    lines
}

fn tool_glyph(name: &str) -> &'static str {
    match name {
        "read_file" | "read_files" | "read_document" | "read_artifact" => "▣",
        "list_files" => "≡",
        "search_workspace" | "search_artifact" | "find_capabilities" => "⌕",
        "apply_file_edits" => "✎",
        "run_shell" => "⌘",
        "web_search" => "◎",
        "web_read" => "◉",
        "analyze_data" => "▦",
        "artifact_info" => "◫",
        "activate_capability" => "◇",
        _ => "⚙",
    }
}

fn tool_title(name: &str) -> String {
    match name {
        "read_file" => "Read file".into(),
        "read_files" => "Read files".into(),
        "list_files" => "List files".into(),
        "search_workspace" => "Search workspace".into(),
        "read_document" => "Read document".into(),
        "analyze_data" => "Analyze data".into(),
        "artifact_info" => "Inspect artifact".into(),
        "read_artifact" => "Read artifact".into(),
        "search_artifact" => "Search artifact".into(),
        "run_shell" => "Run shell".into(),
        "apply_file_edits" => "Edit files".into(),
        "find_capabilities" => "Find capabilities".into(),
        "activate_capability" => "Activate capability".into(),
        "web_search" => "Search web".into(),
        "web_read" => "Read web source".into(),
        _ => humanize_tool_name(name),
    }
}

fn humanize_tool_name(name: &str) -> String {
    let mut words = name
        .split('_')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    if words.is_empty() {
        return "Tool".into();
    }

    let first = words.remove(0);
    let mut title = first.to_owned();
    if let Some(initial) = title.get_mut(0..1) {
        initial.make_ascii_uppercase();
    }
    if !words.is_empty() {
        title.push(' ');
        title.push_str(&words.join(" "));
    }
    title
}

fn tool_argument_summary(arguments: &str, max_chars: usize) -> Option<String> {
    if arguments.trim().is_empty() || max_chars == 0 {
        return None;
    }
    let value: Value = serde_json::from_str(arguments).ok()?;
    let object = value.as_object()?;

    for key in ["path", "query", "capability", "id", "url"] {
        if let Some(text) = object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let prefix = if key == "path" {
                String::new()
            } else {
                format!("{key} ")
            };
            return Some(truncate_middle(&format!("{prefix}{text}"), max_chars));
        }
    }

    for collection in ["requests", "changes"] {
        if let Some(values) = object.get(collection).and_then(Value::as_array) {
            let paths = values
                .iter()
                .filter_map(|value| value.get("path").and_then(Value::as_str))
                .collect::<Vec<_>>();
            if !paths.is_empty() {
                return Some(truncate_middle(&summarize_values(&paths), max_chars));
            }
        }
    }

    let compact = value.to_string();
    (compact != "{}").then(|| truncate_middle(&compact, max_chars))
}

fn summarize_values(values: &[&str]) -> String {
    let shown = values
        .iter()
        .take(2)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if values.len() > 2 {
        format!("{shown}  +{}", values.len() - 2)
    } else {
        shown
    }
}

fn compact_number(value: u64) -> String {
    if value >= 1_000_000 {
        compact_scaled(value, 1_000_000, "M")
    } else if value >= 1_000 {
        compact_scaled(value, 1_000, "k")
    } else {
        value.to_string()
    }
}

fn compact_scaled(value: u64, divisor: u64, suffix: &str) -> String {
    let scaled = value as f64 / divisor as f64;
    if scaled >= 100.0 || (scaled.fract() * 10.0).round() == 0.0 {
        format!("{}{suffix}", scaled.round() as u64)
    } else {
        format!("{scaled:.1}{suffix}")
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let width = (area.width.saturating_mul(percent_x) / 100)
        .max(area.width.min(64))
        .min(112)
        .min(area.width);
    let height = (area.height.saturating_mul(percent_y) / 100)
        .max(area.height.min(12))
        .min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialogs_render_within_small_and_wide_terminals() {
        use ratatui::{Terminal, backend::TestBackend};
        for (width, height) in [(20, 8), (80, 24), (160, 48)] {
            for mode in [
                Mode::Chat,
                Mode::Models,
                Mode::Sessions,
                Mode::Reasoning,
                Mode::AgentMode,
                Mode::Settings,
                Mode::Help,
            ] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                let mut app = App {
                    mode,
                    ..App::default()
                };
                terminal.draw(|frame| draw(frame, &mut app)).unwrap();
                let popup = centered_rect(72, 72, Rect::new(0, 0, width, height));
                assert!(popup.right() <= width && popup.bottom() <= height);
                if mode == Mode::Models {
                    let buffer = terminal.backend().buffer();
                    assert_eq!(buffer[(popup.x, popup.y)].bg, theme::SURFACE);
                    assert_eq!(buffer[(popup.x, popup.y)].symbol(), "╭");
                }
            }
        }
    }

    #[test]
    fn shell_shows_sidebar_only_when_conversation_has_room() {
        use ratatui::{Terminal, backend::TestBackend};
        for width in [80, 140] {
            let mut terminal = Terminal::new(TestBackend::new(width, 32)).unwrap();
            let mut app = App::default();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let buffer = terminal.backend().buffer();
            let screen: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert_eq!(screen.contains("RECENT"), width >= 110);
            assert!(screen.contains("New conversation"));
            assert!(screen.contains("What would you like to build?"));
            assert!(screen.contains("Enter send"));
        }
    }

    #[test]
    fn composer_keeps_end_of_long_draft_visible() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut app = App::default();
        app.input = format!("{}END", "한글 입력 확인 ".repeat(100));
        app.cursor = app.input.chars().count();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("END"));
        app.cursor = 0;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!screen.contains("END"));
        assert!(screen.contains("한"));
        assert!(screen.contains("글"));
    }

    #[test]
    fn status_numbers_keep_useful_precision() {
        assert_eq!(compact_number(999), "999");
        assert_eq!(compact_number(1_500), "1.5k");
        assert_eq!(compact_number(12_000), "12k");
        assert_eq!(compact_number(1_250_000), "1.2M");
    }

    #[test]
    fn status_elision_preserves_both_ends() {
        assert_eq!(
            truncate_middle("Qwen3.8-27B-UD-IQ4_XS.gguf", 15),
            "Qwen3.8…XS.gguf"
        );
        assert_eq!(truncate_end("provider unavailable", 10), "provider …");
    }

    #[test]
    fn spinner_advances_on_eighty_millisecond_ticks() {
        assert_ne!(spinner_frame(0), spinner_frame(80));
        assert_eq!(spinner_frame(0), spinner_frame(800));
    }

    #[test]
    fn tool_step_counts_track_completed_and_failed_calls() {
        let call = |status| crate::model::ConversationToolCall {
            id: "x".into(),
            index: Some(0),
            call_id: None,
            name: "run_shell".into(),
            arguments: "{}".into(),
            status,
        };
        let mut app = App::default();
        app.conversation.push(crate::model::ConversationEntry {
            id: "a".into(),
            kind: ConversationKind::ToolCall {
                tool_call: call(ToolCallStatus::Completed),
            },
        });
        app.conversation.push(crate::model::ConversationEntry {
            id: "b".into(),
            kind: ConversationKind::Assistant {
                content: String::new(),
                tool_calls: vec![
                    call(ToolCallStatus::Failed),
                    call(ToolCallStatus::Streaming),
                ],
            },
        });
        assert_eq!(tool_step_counts(&app), (1, 1));
    }

    #[test]
    fn tool_calls_render_status_title_name_and_argument_detail() {
        let call = crate::model::ConversationToolCall {
            id: "ui-call".into(),
            index: Some(0),
            call_id: Some("call-1".into()),
            name: "read_file".into(),
            arguments: serde_json::json!({
                "path": "src/ui.rs",
                "startLine": 100,
                "endLine": 140
            })
            .to_string(),
            status: ToolCallStatus::Completed,
        };

        let lines = tool_lines(0, &call, 80);
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert_eq!(rendered.len(), 2);
        assert!(rendered[0].contains("✓"));
        assert!(rendered[0].contains("Read file"));
        assert!(rendered[0].contains("read_file"));
        assert!(rendered[1].contains("src/ui.rs"));
    }

    #[test]
    fn tool_argument_summary_compacts_multi_file_calls() {
        let arguments = serde_json::json!({
            "changes": [
                {"path": "src/ui.rs"},
                {"path": "src/backend.rs"},
                {"path": "src/app.rs"}
            ]
        })
        .to_string();

        let summary = tool_argument_summary(&arguments, 80).unwrap();
        assert!(summary.contains("src/ui.rs"));
        assert!(summary.contains("src/backend.rs"));
        assert!(summary.contains("+1"));
    }
}
