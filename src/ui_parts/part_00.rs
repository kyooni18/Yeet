#[path = "../ui/chrome.rs"]
mod chrome;
#[path = "../ui/composer.rs"]
mod composer;
use composer::draw as draw_input;
#[path = "../ui/dialogs.rs"]
mod dialogs;
#[cfg(test)]
#[path = "../ui/render_tests.rs"]
mod render_tests;
#[path = "../ui/responsive.rs"]
mod responsive;
#[path = "../ui/shell.rs"]
mod shell;
#[path = "../ui/status.rs"]
mod status;
#[path = "../ui/task.rs"]
mod task;
use status::draw as draw_status;
#[cfg(test)]
use status::{status_aux_line, status_line};
#[path = "../ui/theme.rs"]
mod theme;
#[path = "../ui/yeet_brand.rs"]
mod yeet_brand;
use dialogs::{
    draw_auth, draw_auth_key, draw_capabilities, draw_capability_detail, draw_goal, draw_help,
    draw_models, draw_permission, draw_provider_edit, draw_providers, draw_reasoning,
    draw_sandbox_policy, draw_sandbox_presets, draw_sessions, draw_settings, draw_settings_edit,
    draw_status_dialog,
};

#[path = "../ui/markdown.rs"]
mod markdown;
use markdown::markdown_lines;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    prelude::{Color, Line, Modifier, Span, Style, Text},
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

pub fn initialize_theme() {
    let settings = crate::config::ConfigStore::default()
        .theme_settings()
        .unwrap_or_default();
    let runtime = crate::model::RuntimeSettingsState {
        appearance: settings.appearance.unwrap_or_else(|| "auto".into()),
        theme_dark: settings.dark.unwrap_or_else(|| "kanagawa".into()),
        theme_light: settings.light.unwrap_or_else(|| "adwaita".into()),
        ..Default::default()
    };
    apply_runtime_theme(&runtime);
}

pub(crate) fn apply_runtime_theme(settings: &crate::model::RuntimeSettingsState) {
    let appearance = match std::env::var("YEET_THEME_MODE")
        .ok()
        .as_deref()
        .unwrap_or(settings.appearance.as_str())
    {
        value if value.eq_ignore_ascii_case("light") => theme::Appearance::Light,
        _ => theme::Appearance::Dark,
    };
    let theme_name = match appearance {
        theme::Appearance::Dark => settings.theme_dark.as_str(),
        theme::Appearance::Light => settings.theme_light.as_str(),
    };
    theme::initialize(appearance, Some(theme_name));
}

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    if app.state.is_streaming {
        let activity_label = live_operation(app)
            .map(|operation| operation.label)
            .unwrap_or_else(|| live_activity(app).0);
        app.sync_activity_label(&activity_label);
    }
    frame.render_widget(Block::default().style(theme::base()), frame.area());
    let adaptive = responsive::metrics(frame.area());
    let (sidebar_area, sidebar_targets) = shell::sidebar_session_targets(app, frame.area());
    app.set_sidebar_session_targets(sidebar_area, sidebar_targets);
    let area = shell::draw_shell(frame, app);
    let suggestions = app.command_suggestions();
    let requested_suggestion_height = if suggestions.is_empty() {
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
    let suggestion_budget = area.height.saturating_sub(
        task_height
            .saturating_add(input_height)
            .saturating_add(adaptive.status_height)
            .saturating_add(1),
    );
    let suggestion_height = if requested_suggestion_height >= 3 && suggestion_budget >= 3 {
        requested_suggestion_height.min(suggestion_budget)
    } else {
        0
    };
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
        Mode::Goal => draw_goal(frame, app),
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
    let transcript_area = (area.x, area.y, area.width, area.height);
    let previous_area = app.transcript_area;
    if previous_area.2 > 0 && previous_area.3 > 0 && previous_area != transcript_area {
        app.clear_transcript_selection();
    }
    app.transcript_area = transcript_area;
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
            .track_symbol(Some("│"))
            .thumb_symbol("┃")
            .track_style(Style::default().fg(theme::border_dim()))
            .thumb_style(Style::default().fg(theme::accent()));
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
            cell.set_style(Style::default().bg(theme::accent()).fg(theme::background()));
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
        app.transcript_context_menu_area = (0, 0, 0, 0);
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
        theme::surface().fg(theme::muted())
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
                .border_style(Style::default().fg(theme::border()))
                .style(theme::surface()),
        ),
        area,
    );
}

fn draw_suggestions(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    suggestions: &[(String, String)],
) {
    let list = suggestion_list(suggestions);
    let mut state = ratatui::widgets::ListState::default().with_selected(Some(app.command_index));
    frame.render_stateful_widget(list, area, &mut state);
}

fn suggestion_list(suggestions: &[(String, String)]) -> List<'_> {
    let items = suggestions.iter().map(|(name, description)| {
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("{name:<14}  "),
                Style::default().fg(theme::accent()),
            ),
            Span::styled(description.as_str(), Style::default().fg(theme::text_dim())),
        ]))
    });
    List::new(items)
        .block(theme::modal_block("Commands · ↑/↓ choose · Tab complete"))
        .highlight_style(theme::selected())
        .highlight_symbol("› ")
}

fn live_activity(app: &App) -> (String, Option<&str>) {
    if app.state.active_reasoning_entry_id.is_some() {
        let title = app
            .state
            .active_activity_entry_id
            .as_deref()
            .and_then(|id| {
                app.conversation.iter().rev().find_map(|entry| {
                    (entry.id == id).then(|| match &entry.kind {
                        ConversationKind::Activity { activity } => activity.title.clone(),
                        _ => String::new(),
                    })
                })
            })
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "Reasoning".into());
        return (title, None);
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

pub(super) fn animated_activity_label(app: &App, label: &str) -> String {
    let Some(started_at) = app.activity_label_started_at else {
        return label.to_owned();
    };
    let progress = (started_at.elapsed().as_millis() as f32
        / f32::from(app.activity_label_transition_ms))
    .clamp(0.0, 1.0);
    if progress >= 1.0 || app.activity_label_to != label {
        return label.to_owned();
    }
    morph_activity_text(&app.activity_label_from, label, progress)
}

fn morph_activity_text(from: &str, to: &str, progress: f32) -> String {
    let from = from.chars().collect::<Vec<_>>();
    let to = to.chars().collect::<Vec<_>>();
    let length = from.len().max(to.len());
    let mut result = String::new();
    for index in 0..length {
        let stagger = index as f32 / (length.max(1) as f32) * 0.35;
        let local = ((progress - stagger) / 0.65).clamp(0.0, 1.0);
        let character = if local < 0.5 {
            from.get(index).copied().unwrap_or(' ')
        } else {
            to.get(index).copied().unwrap_or(' ')
        };
        result.push(character);
    }
    result.trim_end().to_owned()
}

fn tool_step_counts(app: &App) -> (usize, usize, usize) {
    let mut counts = (0usize, 0usize, 0usize);
    for entry in task::latest_turn(app) {
        let calls: &[crate::model::ConversationToolCall] = match &entry.kind {
            ConversationKind::ToolCall { tool_call } => std::slice::from_ref(tool_call),
            ConversationKind::Assistant { tool_calls, .. } => tool_calls.as_slice(),
            _ => continue,
        };
        for call in calls {
            match call.status {
                ToolCallStatus::Completed => counts.0 += 1,
                ToolCallStatus::Failed
                | ToolCallStatus::Cancelled
                | ToolCallStatus::Interrupted
                | ToolCallStatus::TimedOut => counts.1 += 1,
                ToolCallStatus::Preparing
                | ToolCallStatus::AwaitingPermission
                | ToolCallStatus::Running => counts.2 += 1,
                ToolCallStatus::Suppressed => {}
            }
        }
    }
    counts
}

fn format_elapsed(elapsed_ms: u128) -> String {
    if elapsed_ms < 60_000 {
        format!("{:.1}s", elapsed_ms as f64 / 1_000.0)
    } else {
        let seconds = elapsed_ms / 1_000;
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    }
}

/// A deliberately small motion cue for the one piece of UI that is still live.
/// The glyph family changes with the actual live phase, while the cadence stays stable.
pub(super) fn activity_marker(app: &App) -> &'static str {
    let frame = activity_frame(app);
    if app.state.active_reasoning_entry_id.is_some() {
        ["—", "–", "-", "·", "∙", "○", "◌", "◉", "◌", "○", "∙", "·"][frame]
    } else if app.state.active_assistant_entry_id.is_some() {
        ["-", "–", "—", "·", "•", "●", "◉", "●", "•", "·", "–", "-"][frame]
    } else {
        ["-", "–", "—", "◌", "◍", "◉", "◍", "◌", "—", "–", "-", "·"][frame]
    }
}

pub(super) fn activity_marker_color(app: &App) -> Color {
    let frame = activity_frame(app);
    if app.state.active_reasoning_entry_id.is_some() {
        [
            theme::muted(),
            theme::muted(),
            theme::muted(),
            theme::accent_warm(),
            theme::accent_warm(),
            theme::accent_hot(),
            theme::accent_hot(),
            theme::accent_warm(),
            theme::accent_warm(),
            theme::muted(),
            theme::muted(),
            theme::muted(),
        ][frame]
    } else if app.state.active_assistant_entry_id.is_some() {
        [
            theme::user(),
            theme::text_dim(),
            theme::text(),
            theme::text_dim(),
            theme::text(),
            theme::text_dim(),
            theme::user(),
            theme::user(),
            theme::text_dim(),
            theme::user(),
            theme::text_dim(),
            theme::user(),
        ][frame]
    } else {
        [
            theme::accent(),
            theme::accent(),
            theme::accent_hot(),
            theme::accent_hot(),
            theme::accent_hot(),
            theme::accent(),
            theme::accent(),
            theme::accent_hot(),
            theme::accent_hot(),
            theme::accent(),
            theme::accent(),
            theme::accent(),
        ][frame]
    }
}

fn activity_frame(app: &App) -> usize {
    let elapsed_ms = app
        .stream_elapsed()
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    ((elapsed_ms / 110) as usize) % 12
}

fn cell_width(value: &str) -> usize {
    Span::raw(value).width()
}

fn char_cell_width(value: char) -> usize {
    cell_width(&value.to_string())
}

fn truncate_end(value: &str, max_cells: usize) -> String {
    if max_cells == 0 {
        return String::new();
    }
    if cell_width(value) <= max_cells {
        return value.to_owned();
    }
    if max_cells == 1 {
        return "…".into();
    }

    let budget = max_cells - 1;
    let mut used = 0usize;
    let mut output = String::new();
    for ch in value.chars() {
        let width = char_cell_width(ch);
        if used + width > budget {
            break;
        }
        output.push(ch);
        used += width;
    }
    output.push('…');
    output
}

fn truncate_middle(value: &str, max_cells: usize) -> String {
    if max_cells == 0 {
        return String::new();
    }
    if cell_width(value) <= max_cells {
        return value.to_owned();
    }
    if max_cells <= 2 {
        return "…".into();
    }

    let chars = value.chars().collect::<Vec<_>>();
    let widths = chars
        .iter()
        .map(|ch| char_cell_width(*ch))
        .collect::<Vec<_>>();
    let payload = max_cells - 1;
    let left_target = payload / 2;
    let right_target = payload - left_target;

    let mut left_count = 0usize;
    let mut left_width = 0usize;
    while left_count < chars.len() {
        let width = widths[left_count];
        if left_width + width > left_target {
            break;
        }
        left_width += width;
        left_count += 1;
    }

    let mut right_start = chars.len();
    let mut right_width = 0usize;
    while right_start > left_count {
        let width = widths[right_start - 1];
        if right_width + width > right_target {
            break;
        }
        right_width += width;
        right_start -= 1;
    }

    let mut remaining = payload.saturating_sub(left_width + right_width);
    loop {
        let mut progressed = false;
        if left_count < right_start {
            let width = widths[left_count];
            if width <= remaining {
                left_count += 1;
                remaining -= width;
                progressed = true;
            }
        }
        if left_count < right_start {
            let width = widths[right_start - 1];
            if width <= remaining {
                right_start -= 1;
                remaining -= width;
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    format!(
        "{}…{}",
        chars[..left_count].iter().collect::<String>(),
        chars[right_start..].iter().collect::<String>()
    )
}

fn transcript_text(app: &App, width: u16) -> Text<'static> {
    let mut lines = Vec::new();
    let mut rendered_any = false;
    let mut previous_compact = false;
    let mut index = 0;
    let latest_turn_start = app
        .conversation
        .iter()
        .rposition(|entry| matches!(entry.kind, ConversationKind::User { .. }))
        .unwrap_or(0);

    while index < app.conversation.len() {
        let entry = &app.conversation[index];
        if matches!(
            &entry.kind,
            ConversationKind::Activity { activity } if !visible_terminal_activity(activity)
        ) {
            index += 1;
            continue;
        }

        let starts_tool_work = matches!(&entry.kind, ConversationKind::ToolCall { .. })
            || matches!(
                &entry.kind,
                ConversationKind::Reasoning {
                    content,
                    summary: Some(summary),
                } if content.trim().is_empty() && !summary.trim().is_empty()
            );
        if starts_tool_work {
            let start = index;
            let mut end = index;
            let mut events = Vec::new();
            while end < app.conversation.len() {
                match &app.conversation[end].kind {
                    ConversationKind::ToolCall { tool_call } => {
                        events.push(WorkEvent::Tool(tool_call));
                    }
                    ConversationKind::Reasoning {
                        content,
                        summary: Some(summary),
                    } if content.trim().is_empty() && !summary.trim().is_empty() => {
                        events.push(WorkEvent::Reasoning(summary.as_str()));
                    }
                    ConversationKind::Activity { activity } if !terminal_activity(activity) => {}
                    _ => break,
                }
                end += 1;
            }

            if events
                .iter()
                .any(|event| matches!(event, WorkEvent::Tool(_)))
            {
                index = end;
                lines.extend(tool_group_lines(
                    app,
                    &events,
                    width,
                    app.state.is_streaming && start >= latest_turn_start,
                ));
                rendered_any = true;
                previous_compact = true;
                continue;
            }
        }

        let compact = compact_transcript_entry(&entry.kind);
        if rendered_any && (!compact || !previous_compact) {
            lines.push(Line::from(""));
        }
        lines.extend(entry_lines(app, entry, width));
        rendered_any = true;
        previous_compact = compact;
        index += 1;
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
            label: live_activity(app).0,
            detail: live_reasoning_detail(app),
        });
    }

    if let Some(call) = task::latest_turn(app)
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            ConversationKind::ToolCall { tool_call }
                if matches!(
                    tool_call.status,
                    ToolCallStatus::Preparing
                        | ToolCallStatus::AwaitingPermission
                        | ToolCallStatus::Running
                ) =>
            {
                Some(tool_call)
            }
            ConversationKind::Assistant { tool_calls, .. } => tool_calls.iter().rev().find(|call| {
                matches!(
                    call.status,
                    ToolCallStatus::Preparing
                        | ToolCallStatus::AwaitingPermission
                        | ToolCallStatus::Running
                )
            }),
            _ => None,
        })
    {
        return Some(LiveOperation {
            label: tool_activity_title(call),
            detail: tool_activity_summary(call),
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
    if !app.state.active_reasoning_summary.trim().is_empty()
        && let Some(item) = reasoning_summary_items(&app.state.active_reasoning_summary).last()
    {
        let plain = markdown_plain_text(item);
        if !plain.is_empty() {
            return Some(plain);
        }
    }
    if let Some(line) = app
        .state
        .active_reasoning_text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
    {
        let plain = markdown_plain_text(line.trim());
        if !plain.is_empty() {
            return Some(plain);
        }
    }
    app.conversation.iter().rev().find_map(|entry| {
        if entry.id != id {
            return None;
        }
        match &entry.kind {
            ConversationKind::Reasoning { content, summary } => summary
                .as_deref()
                .and_then(|value| reasoning_summary_items(value).last().cloned())
                .map(|value| markdown_plain_text(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    content
                        .lines()
                        .rev()
                        .find(|line| !line.trim().is_empty())
                        .map(|line| markdown_plain_text(line.trim()))
                        .filter(|value| !value.is_empty())
                }),
            _ => None,
        }
    })
}

fn markdown_plain_text(content: &str) -> String {
    markdown_lines(content)
        .into_iter()
        .map(|line| line.to_string())
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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

fn visible_terminal_activity(activity: &crate::model::ModelActivity) -> bool {
    if !terminal_activity(activity) {
        return false;
    }
    if activity.phase.as_str() != Some("done") {
        return true;
    }

    let title = activity.title.trim();
    let generic_title = title.eq_ignore_ascii_case("done")
        || title.eq_ignore_ascii_case("completed")
        || title.eq_ignore_ascii_case("complete");
    let detail_empty = activity
        .detail
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty();
    !(generic_title && detail_empty)
}

fn transcript_continuation_indent(line: &Line<'static>, content_width: usize) -> usize {
    let Some(first) = line.spans.first() else {
        return 0;
    };
    if first.style.bg == Some(theme::code_background()) {
        return 2.min(content_width.saturating_sub(1));
    }
    let marker = first.content.trim();
    let ordered = marker
        .strip_suffix('.')
        .or_else(|| marker.strip_suffix(')'))
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()));
    let structural = marker.starts_with('│')
        || marker.starts_with('•')
        || marker.starts_with('☐')
        || marker.starts_with('☑')
        || ordered;
    if structural {
        first.width().min(content_width.saturating_sub(1))
    } else {
        0
    }
}

fn text_runs(value: &str) -> Vec<&str> {
    if value.is_empty() {
        return Vec::new();
    }
    let mut runs = Vec::new();
    let mut start = 0;
    let mut whitespace = None;
    for (index, ch) in value.char_indices() {
        let current = ch.is_whitespace();
        if whitespace.is_some_and(|previous| previous != current) {
            runs.push(&value[start..index]);
            start = index;
        }
        whitespace = Some(current);
    }
    runs.push(&value[start..]);
    runs
}

fn push_styled_text(row: &mut Vec<Span<'static>>, value: &str, style: Style) {
    if value.is_empty() {
        return;
    }
    if let Some(last) = row.last_mut().filter(|last| last.style == style) {
        last.content.to_mut().push_str(value);
    } else {
        row.push(Span::styled(value.to_owned(), style));
    }
}

