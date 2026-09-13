mod chrome;
mod composer;
mod dialogs;
mod responsive;
mod shell;
mod task;
mod theme;
mod yeet_brand;
use dialogs::{
    draw_auth, draw_auth_key, draw_capabilities, draw_capability_detail, draw_help, draw_infinity,
    draw_models, draw_permission, draw_provider_edit, draw_providers, draw_reasoning,
    draw_sandbox_policy, draw_sandbox_presets, draw_sessions, draw_settings, draw_settings_edit,
    draw_status_dialog,
};

mod markdown;
use markdown::markdown_lines;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Position, Rect},
    prelude::{Line, Modifier, Span, Style, Text},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use serde_json::Value;

use crate::{
    app::{App, Mode},
    model::{ConversationEntry, ConversationKind, ToolCallStatus},
};

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    frame.render_widget(Block::default().style(theme::base()), frame.area());
    let adaptive = responsive::metrics(frame.area());
    let area = shell::draw_shell(frame, app);
    let suggestions = app.command_suggestions();
    let suggestion_height = if suggestions.is_empty() {
        0
    } else {
        (suggestions.len() as u16 + 2)
            .min(adaptive.suggestion_height)
            .min(area.height / 3)
    };
    let input_rows = composer::layout(&app.input, app.cursor, area.width.saturating_sub(4));
    let input_height = (input_rows.lines.len() as u16)
        .clamp(adaptive.input_min_lines, adaptive.input_max_lines)
        + 2;
    let task_height = task::height(app).min(adaptive.task_height);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(task_height),
            Constraint::Length(suggestion_height),
            Constraint::Length(input_height),
            Constraint::Length(adaptive.status_height),
        ])
        .split(area);

    draw_transcript(frame, app, chunks[0], adaptive.shape);
    if suggestion_height > 0 {
        draw_suggestions(frame, app, chunks[2], &suggestions);
    }
    task::draw(frame, app, chunks[1]);
    draw_input(frame, app, chunks[3]);
    draw_status(frame, app, chunks[4]);
    draw_transcript_context_menu(frame, app);

    match app.mode {
        Mode::Debate => dialogs::draw_debate(frame, app),
        Mode::Models => draw_models(frame, app),
        Mode::Reasoning => draw_reasoning(frame, app),
        Mode::Infinity => draw_infinity(frame, app),
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
        Mode::Status => draw_status_dialog(frame, app),
        Mode::Help => draw_help(frame),
        Mode::Chat => {}
    }

    if app.state.pending_shell_permission.is_some()
        || app.state.pending_native_app_permission.is_some()
    {
        draw_permission(frame, app);
    }

    chrome::draw(frame, app);
}

fn draw_transcript(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    viewport_shape: responsive::Shape,
) {
    let area = area.inner(Margin {
        horizontal: 1,
        vertical: u16::from(area.height > 4),
    });
    app.transcript_area = (area.x, area.y, area.width, area.height);
    if app.conversation.is_empty() {
        app.max_scroll = 0;
        app.scroll_y = 0;
        shell::draw_welcome(frame, area, viewport_shape);
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
    capture_transcript_cells(frame, app, area);
    draw_selection(frame, app, area);
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

fn draw_selection(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let (Some(start), Some(end)) = (app.selection_start, app.selection_end) else {
        return;
    };
    let start_point = (start.1, start.0);
    let end_point = (end.1, end.0);
    let start = start_point.min(end_point);
    let end = start_point.max(end_point);
    let buffer = frame.buffer_mut();
    for row in start.0..=end.0 {
        if row < area.y || row >= area.bottom() {
            continue;
        }
        let from = if row == start.0 { start.1 } else { area.x };
        let to = if row == end.0 {
            end.1
        } else {
            area.right().saturating_sub(1)
        };
        for column in from.max(area.x)..=to.min(area.right().saturating_sub(1)) {
            let cell = &mut buffer[(column, row)];
            cell.set_style(Style::default().bg(theme::ACCENT).fg(theme::BACKGROUND));
        }
    }
}

fn capture_transcript_cells(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let buffer = frame.buffer_mut();
    app.transcript_cells = (area.y..area.bottom())
        .map(|row| {
            (area.x..area.right())
                .map(|column| buffer[(column, row)].symbol().to_owned())
                .collect()
        })
        .collect();
}

fn draw_transcript_context_menu(frame: &mut Frame<'_>, app: &mut App) {
    let Some(menu) = app.transcript_context_menu else {
        app.transcript_context_menu_area = (0, 0, 0, 0);
        return;
    };
    let screen = frame.area();
    if screen.width < 18 || screen.height < 4 {
        return;
    }
    let width = 22u16.min(screen.width);
    let height = 4u16.min(screen.height);
    let x = menu
        .x
        .min(screen.right().saturating_sub(width))
        .max(screen.x);
    let y = menu
        .y
        .min(screen.bottom().saturating_sub(height))
        .max(screen.y);
    let area = Rect::new(x, y, width, height);
    app.transcript_context_menu_area = (area.x, area.y, area.width, area.height);

    let copy_style = if app.selected_transcript_text().is_some() {
        theme::surface()
    } else {
        theme::surface().fg(theme::MUTED)
    };
    let items = vec![
        ListItem::new(Line::styled(" Copy", copy_style)),
        ListItem::new(Line::styled(" Clear selection", theme::surface())),
    ];
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(theme::surface()), area);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::BORDER))
                .style(theme::surface()),
        ),
        area,
    );
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
                Span::styled(*description, style.fg(theme::TEXT_DIM)),
            ]))
        });
    let list = List::new(items).block(theme::modal_block("Commands · ↑/↓ choose · Tab complete"));
    let mut state = ratatui::widgets::ListState::default().with_selected(Some(app.command_index));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_input(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let border_style = if app.state.is_streaming {
        Style::default().fg(theme::pulse_color())
    } else {
        Style::default().fg(theme::BORDER)
    };
    let roomy = area.width >= 54;
    let title = if app.state.is_streaming && roomy {
        " ◈  BUFFERED DIRECTIVE // UPLINK  ".to_owned()
    } else if app.state.is_streaming {
        " ◈  UPLINK // BUF  ".to_owned()
    } else if roomy {
        " ◈  DIRECTIVE // UPLINK  ·  READY  ".to_owned()
    } else {
        " ◈  UPLINK // READY  ".to_owned()
    };
    let hint = if area.width < 34 {
        " ENTER//TX "
    } else if app.state.is_streaming && area.width < 60 {
        " ESC//ABORT · ENTER//QUEUE "
    } else if !app.state.is_streaming && area.width < 60 {
        " ENTER//TX · / //CMDS "
    } else if app.state.is_streaming {
        " ESC//ABORT  ·  ENTER//QUEUE  ·  ALT+↑/↓//HISTORY  ·  / //COMMANDS "
    } else {
        " ENTER//TRANSMIT  ·  ALT+↑/↓//HISTORY  ·  / //COMMANDS "
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
                .fg(if app.state.is_streaming {
                    theme::ACCENT_HOT
                } else {
                    theme::ACCENT_WARM
                })
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
            "› awaiting directive_",
            Style::default()
                .fg(theme::MUTED)
                .add_modifier(Modifier::ITALIC),
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
    frame.render_widget(Block::default().style(theme::surface()), area);
    frame.render_widget(Paragraph::new(status_line(app, width)), area);
}

fn status_line(app: &App, width: usize) -> Line<'static> {
    let current_tokens = app.state.current_context_tokens.unwrap_or(0);
    let total_tokens = app.state.active_model_context_length;
    let (total, percent) = match total_tokens {
        Some(total) if total > 0 => (
            compact_number(total),
            format!("{:.0}%", current_tokens as f64 * 100.0 / total as f64),
        ),
        Some(total) => (compact_number(total), "?%".to_owned()),
        None => ("?".to_owned(), "?%".to_owned()),
    };

    let reasoning = if app.state.active_reasoning_level.is_empty() {
        "auto"
    } else {
        app.state.active_reasoning_level.as_str()
    };
    let permission = app
        .state
        .sandbox_settings
        .as_ref()
        .map(|settings| settings.permission_mode())
        .unwrap_or("ask");
    let current = compact_number(current_tokens);
    let prefix = if width >= 3 { " ◈ " } else { "" };
    let available = width.saturating_sub(prefix.chars().count());
    let model_full = if app.state.active_model.is_empty() {
        "no model".to_owned()
    } else {
        truncate_middle(&app.state.active_model, 30)
    };
    let model_short = if app.state.active_model.is_empty() {
        "no model".to_owned()
    } else {
        let short = app
            .state
            .active_model
            .rsplit_once('/')
            .map(|(_, model)| model)
            .unwrap_or(app.state.active_model.as_str());
        truncate_middle(short, 18)
    };
    let model_tiny = truncate_middle(&model_short, 11);
    let reasoning_short = match reasoning {
        "medium" => "med",
        "auto" => "auto",
        "low" => "low",
        "high" => "high",
        other => other,
    };
    let permission_short = match permission {
        "unlimited" => "unlim",
        other => other,
    };
    let reasoning_micro = match reasoning_short {
        "high" => "hi",
        "med" => "me",
        "low" => "lo",
        "auto" => "au",
        other => other,
    };
    let permission_micro = match permission_short {
        "unlim" => "un",
        "auto" => "au",
        "ask" => "as",
        other => other,
    };
    let model_micro = truncate_middle(&model_short, 7);
    let meter = context_meter(current_tokens, total_tokens, 7);
    let candidates = [
        format!(
            "model {model_full} │ ctx {current}/{total} {percent} {meter} │ reason {reasoning} │ perm {permission}"
        ),
        format!("{model_short} │ ctx {percent} {meter} │ {reasoning} │ {permission}"),
        format!("{model_short} · {percent} · {reasoning_short}/{permission_short}"),
        format!("{model_tiny} {percent} {reasoning_short}/{permission_short}"),
        format!("{model_micro} {percent} {reasoning_micro}/{permission_micro}"),
    ];
    let text = candidates
        .into_iter()
        .find(|candidate| Span::raw(candidate).width() <= available)
        .unwrap_or_else(|| {
            task::fit(
                &format!("{model_micro} {percent} {reasoning_micro}/{permission_micro}"),
                available,
            )
        });
    Line::from(vec![
        Span::styled(
            prefix,
            Style::default()
                .fg(theme::ACCENT_WARM)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(text, Style::default().fg(theme::TEXT_DIM)),
    ])
}

fn context_meter(current: u64, total: Option<u64>, cells: usize) -> String {
    let Some(total) = total.filter(|total| *total > 0) else {
        return "·".repeat(cells);
    };
    let filled =
        ((current.saturating_mul(cells as u64) + total / 2) / total).min(cells as u64) as usize;
    format!(
        "{}{}",
        "▰".repeat(filled),
        "▱".repeat(cells.saturating_sub(filled))
    )
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
                ToolCallStatus::Streaming | ToolCallStatus::Suppressed => {}
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
        ConversationKind::User { content } => {
            let mut lines = vec![Line::from(vec![
                Span::styled("╾ ", Style::default().fg(theme::BORDER)),
                Span::styled(
                    "USER // DIRECTIVE",
                    Style::default()
                        .fg(theme::ACCENT_HOT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ╼", Style::default().fg(theme::BORDER)),
            ])];
            lines.extend(content.lines().map(|line| {
                Line::from(vec![
                    Span::styled("┃ ", Style::default().fg(theme::ACCENT)),
                    Span::styled(format!("{line} "), theme::surface()),
                ])
            }));
            lines
        }
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
            let mut lines = vec![Line::from(vec![
                Span::styled("╾ ", Style::default().fg(theme::BORDER)),
                Span::styled("YEET // RESPONSE", theme::brand()),
                Span::styled(" ╼", Style::default().fg(theme::BORDER)),
            ])];
            for line in markdown_lines(content) {
                let mut spans = Vec::with_capacity(line.spans.len() + 1);
                spans.push(Span::styled("┃ ", Style::default().fg(theme::BORDER)));
                spans.extend(line.spans);
                lines.push(Line::from(spans));
            }
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
                Span::styled("◇ COGNITION //", Style::default().fg(theme::ACCENT)),
                Span::styled(
                    summary
                        .map(|value| format!(" · {value}"))
                        .unwrap_or_default(),
                    Style::default().fg(theme::MUTED),
                ),
            ])];
            for line in content.lines() {
                lines.push(Line::from(vec![
                    Span::styled("┊ ", Style::default().fg(theme::BORDER_DIM)),
                    Span::styled(line.to_owned(), Style::default().fg(theme::MUTED)),
                ]));
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
                Span::styled("skill ", Style::default().fg(theme::ACCENT)),
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
            let color = if *is_error {
                theme::ERROR
            } else {
                theme::ACCENT
            };
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
                    Style::default().fg(theme::TEXT_DIM),
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
                .fg(theme::ACCENT_HOT)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(theme::ACCENT_HOT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        match phase {
            "done" => (
                "✓",
                Style::default().fg(theme::TEXT),
                Style::default().fg(theme::TEXT_DIM),
            ),
            "failed" => (
                "×",
                Style::default()
                    .fg(theme::ERROR)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(theme::ERROR),
            ),
            "interrupted" => (
                "■",
                Style::default().fg(theme::ACCENT),
                Style::default().fg(theme::TEXT_DIM),
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
                .fg(theme::ACCENT_HOT)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(theme::ACCENT_HOT)
                .add_modifier(Modifier::BOLD),
        ),
        ToolCallStatus::Completed => (
            "✓",
            Style::default()
                .fg(theme::SUCCESS)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(theme::TEXT_DIM)
                .add_modifier(Modifier::BOLD),
        ),
        ToolCallStatus::Failed => (
            "×",
            Style::default()
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
        ),
        ToolCallStatus::Suppressed => (
            "⊘",
            Style::default().fg(theme::MUTED),
            Style::default().fg(theme::TEXT_DIM),
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
    if width >= 42 {
        primary.push(Span::styled("  [", Style::default().fg(theme::BORDER)));
        primary.push(Span::styled(
            tool_class(&call.name),
            Style::default()
                .fg(theme::ACCENT_WARM)
                .add_modifier(Modifier::BOLD),
        ));
        primary.push(Span::styled("]", Style::default().fg(theme::BORDER)));
    }
    if width >= 64 {
        let used = Line::from(primary.clone()).width();
        let budget = (width as usize).saturating_sub(used + 5).min(34);
        if budget >= 4 {
            primary.push(Span::styled(
                "  ·  ",
                Style::default().fg(theme::BORDER_DIM),
            ));
            primary.push(Span::styled(
                truncate_middle(&call.name, budget),
                Style::default().fg(theme::MUTED),
            ));
        }
    }

    let mut lines = vec![Line::from(primary)];
    if width >= 32
        && let Some(summary) =
            tool_argument_summary(&call.arguments, (width as usize).saturating_sub(7))
    {
        lines.push(Line::from(vec![
            Span::styled("  ╰─ ", Style::default().fg(theme::BORDER_DIM)),
            Span::styled(summary, Style::default().fg(theme::MUTED)),
        ]));
    }
    lines
}

fn tool_class(name: &str) -> &'static str {
    match name {
        "read_file" | "read_files" | "read_document" | "read_artifact" | "list_files" => "IO",
        "search_workspace" | "search_artifact" | "find_capabilities" => "FIND",
        "apply_file_edits" => "EDIT",
        "run_shell" | "shell_job" => "EXEC",
        "web_search" | "web_read" => "NET",
        "analyze_data" => "DATA",
        "computer_use" | "desktop_control" => "DESK",
        "artifact_info" => "META",
        "activate_capability" => "CAP",
        _ => "TOOL",
    }
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
    responsive::modal_rect(area, percent_x, percent_y)
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
                Mode::Settings,
                Mode::Status,
                Mode::Help,
            ] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                let mut app = App {
                    mode,
                    ..App::default()
                };
                terminal.draw(|frame| draw(frame, &mut app)).unwrap();
                let popup = centered_rect(76, 74, Rect::new(0, 0, width, height));
                assert!(popup.right() <= width && popup.bottom() <= height);
                if mode == Mode::Models {
                    let buffer = terminal.backend().buffer();
                    assert_eq!(buffer[(popup.x, popup.y)].bg, theme::SURFACE_RAISED);
                    assert_eq!(buffer[(popup.x, popup.y)].symbol(), "╔");
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
            assert!(screen.contains("NEW DIRECTIVE"));
            assert!(screen.contains("YEET"));
            assert!(screen.contains("AWAITING DIRECTIVE"));
            assert!(screen.contains("ENTER//TRANSMIT"));
        }
    }

    #[test]
    fn visual_hierarchy_uses_surface_layers_and_command_modals() {
        use ratatui::{Terminal, backend::TestBackend};

        let (width, height) = (140, 32);
        let bounds = Rect::new(0, 0, width, height);
        let metrics = responsive::metrics(bounds);
        let content_x = metrics
            .sidebar_width
            .expect("wide layout should expose sidebar");
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut app = App::default();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(content_x, 0)].bg, theme::SURFACE);
        assert_eq!(
            buffer[(content_x, metrics.header_height.saturating_sub(1))].fg,
            theme::BORDER_DIM
        );

        app.mode = Mode::Models;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let popup = centered_rect(76, 74, bounds);
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(popup.x, popup.y)].symbol(), "╔");
        assert_eq!(buffer[(popup.x, popup.y)].bg, theme::SURFACE_RAISED);
        let title_x = (popup.x..popup.right())
            .find(|&x| buffer[(x, popup.y)].symbol() == "M")
            .expect("model modal should render a title");
        assert_eq!(buffer[(title_x, popup.y)].bg, theme::SURFACE_RAISED);
    }

    #[test]
    fn adaptive_window_ratios_render_without_starving_core_surfaces() {
        use ratatui::{Terminal, backend::TestBackend};

        for (width, height) in [(32, 8), (48, 42), (80, 24), (120, 12), (160, 30), (220, 32)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut app = App::default();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let screen: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(screen.contains("YEET"), "missing brand at {width}x{height}");
            assert!(
                app.transcript_area.2 > 0 && app.transcript_area.3 > 0,
                "transcript collapsed at {width}x{height}"
            );

            app.mode = Mode::Models;
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let popup = centered_rect(76, 74, Rect::new(0, 0, width, height));
            assert!(popup.right() <= width && popup.bottom() <= height);
        }
    }

    #[test]
    fn portrait_welcome_uses_mobile_density() {
        use ratatui::{Terminal, backend::TestBackend};

        let (width, height) = (88, 52);
        assert_eq!(
            responsive::shape(Rect::new(0, 0, width, height)),
            responsive::Shape::Portrait
        );

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut app = App {
            follow_tail: false,
            ..App::default()
        };
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(
            !screen.contains("██"),
            "portrait welcome should avoid the desktop title"
        );
        assert!(!screen.contains("AGENTIC SYSTEMS CORE"));
        assert!(
            !screen.contains("History"),
            "empty mobile sessions should stay at latest"
        );
        assert!(screen.contains("AWAITING DIRECTIVE"));
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
    fn bottom_status_keeps_runtime_controls_and_detailed_usage_out() {
        let mut app = App::default();
        app.state.active_model = "openai/test".into();
        app.state.active_model_context_length = Some(128_000);
        app.state.current_context_tokens = Some(64_000);
        app.state.token_usage.input_tokens = Some(12_000);
        app.state.token_usage.output_tokens = Some(3_000);
        app.state.token_usage.cached_input_tokens = Some(8_000);
        app.state.token_usage.estimated_cost_usd = Some(1.25);
        app.state.active_reasoning_level = "high".into();

        let text = status_line(&app, 120)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(text.contains("model openai/test"));
        assert!(text.contains("ctx 64k/128k 50%"));
        assert!(text.contains("reason high"));
        assert!(text.contains("perm ask"));
        assert!(!text.contains("12k"));
        assert!(!text.contains("3k"));
        assert!(!text.contains("cache"));
        assert!(!text.contains("cost"));
    }

    #[test]
    fn bottom_status_preserves_runtime_controls_when_narrow() {
        let mut app = App::default();
        app.state.active_model = "openai/gpt-5.6-codex".into();
        app.state.active_model_context_length = Some(128_000);
        app.state.current_context_tokens = Some(64_000);
        app.state.token_usage.input_tokens = Some(12_000);
        app.state.token_usage.output_tokens = Some(3_000);
        app.state.active_reasoning_level = "high".into();

        let text = status_line(&app, 40)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(text.contains("50%"));
        assert!(text.contains("high") || text.contains("hi"));
        assert!(text.contains("ask") || text.contains("as"));
        assert!(text.contains("gpt"));
        assert!(!text.contains("12k"));
    }

    #[test]
    fn bottom_status_never_exceeds_terminal_width() {
        let mut app = App::default();
        app.state.active_model = "provider/a-very-long-model-identifier".into();
        app.state.active_model_context_length = Some(200_000);
        app.state.current_context_tokens = Some(123_456);
        app.state.active_reasoning_level = "medium".into();

        for width in 1..=120 {
            assert!(status_line(&app, width).width() <= width);
        }
    }

    #[test]
    fn status_dialog_renders_api_usage_headroom() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
        let mut app = App {
            mode: Mode::Status,
            ..App::default()
        };
        app.state.active_model = "openai/gpt-5.6-codex".into();
        app.state.active_model_context_length = Some(128_000);
        app.state.current_context_tokens = Some(32_000);
        app.state
            .auth_providers
            .push(crate::model::AuthProviderItem {
                provider: "openai".into(),
                authenticated: true,
                method: "oauth".into(),
                expires_at: None,
                usage: Some(crate::model::ProviderUsageStatus {
                    provider: "openai".into(),
                    available: true,
                    source: "codex".into(),
                    fetched_at: "2026-09-09T20:00:00+09:00".into(),
                    plan: Some("pro".into()),
                    windows: vec![crate::model::ProviderUsageWindow {
                        id: "five-hour".into(),
                        label: "5h".into(),
                        used_percent: 25,
                        remaining_percent: 75,
                        resets_at: None,
                    }],
                    message: None,
                }),
                error: None,
            });

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("Usage & provider headroom"));
        assert!(screen.contains("openai"));
        assert!(screen.contains("5h 75%"));
        assert!(screen.contains("Session & permissions"));
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

    #[test]
    fn transcript_context_menu_renders_only_for_chat_selection() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut app = App::default();
        app.conversation.push(crate::model::ConversationEntry {
            id: "assistant".into(),
            kind: ConversationKind::Assistant {
                content: "selectable conversation text".into(),
                tool_calls: Vec::new(),
            },
        });
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let (x, y, _, _) = app.transcript_area;
        app.selection_start = Some((x, y));
        app.selection_end = Some((x.saturating_add(5), y));
        app.transcript_context_menu = Some(crate::app::TranscriptContextMenu {
            x: x.saturating_add(2),
            y,
        });

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("Copy"));
        assert!(screen.contains("Clear selection"));
        assert!(app.transcript_context_menu_area.2 > 0);
        let (menu_x, menu_y, _, _) = app.transcript_context_menu_area;
        assert_eq!(
            terminal.backend().buffer()[(menu_x.saturating_add(1), menu_y.saturating_add(1))].bg,
            theme::SURFACE
        );
    }
}
