mod chrome;
mod composer;
use composer::draw as draw_input;
mod dialogs;
#[cfg(test)]
mod render_tests;
mod responsive;
mod shell;
mod status;
mod task;
use status::draw as draw_status;
#[cfg(test)]
use status::{status_aux_line, status_line};
mod theme;
mod yeet_brand;
use dialogs::{
    draw_auth, draw_auth_key, draw_capabilities, draw_capability_detail, draw_goal, draw_help,
    draw_models, draw_permission, draw_provider_edit, draw_providers, draw_reasoning,
    draw_sandbox_policy, draw_sandbox_presets, draw_sessions, draw_settings, draw_settings_edit,
    draw_status_dialog,
};

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
                Style::default().fg(theme::accent_warm()),
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
                ToolCallStatus::Failed
                | ToolCallStatus::Cancelled
                | ToolCallStatus::Interrupted
                | ToolCallStatus::TimedOut => counts.1 += 1,
                ToolCallStatus::Preparing
                | ToolCallStatus::AwaitingPermission
                | ToolCallStatus::Running
                | ToolCallStatus::Suppressed => {}
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
            ConversationKind::Activity { activity } if !terminal_activity(activity)
        ) {
            index += 1;
            continue;
        }

        if matches!(&entry.kind, ConversationKind::ToolCall { .. }) {
            let start = index;
            while index < app.conversation.len()
                && matches!(
                    &app.conversation[index].kind,
                    ConversationKind::ToolCall { .. }
                )
            {
                index += 1;
            }
            let calls = app.conversation[start..index]
                .iter()
                .filter_map(|entry| match &entry.kind {
                    ConversationKind::ToolCall { tool_call } => Some(tool_call),
                    _ => None,
                })
                .collect::<Vec<_>>();
            lines.extend(tool_group_lines(
                app,
                &calls,
                width,
                app.state.is_streaming && start >= latest_turn_start,
            ));
            rendered_any = true;
            previous_compact = true;
            continue;
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
            ConversationKind::Assistant { tool_calls, .. } => {
                tool_calls.iter().rev().find(|call| {
                    matches!(
                        call.status,
                        ToolCallStatus::Preparing
                            | ToolCallStatus::AwaitingPermission
                            | ToolCallStatus::Running
                    )
                })
            }
            _ => None,
        })
    {
        return Some(LiveOperation {
            label: tool_activity_detail(call),
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

fn prefixed_wrapped_line(
    prefix: Span<'static>,
    line: Line<'static>,
    width: u16,
) -> Vec<Line<'static>> {
    let line_style = line.style;
    let prefix_width = prefix.width();
    let width = width as usize;
    if prefix_width >= width {
        let mut visible = String::new();
        let mut used = 0usize;
        for ch in prefix.content.chars() {
            let cells = char_cell_width(ch);
            if used + cells > width {
                break;
            }
            visible.push(ch);
            used += cells;
        }
        return vec![Line::from(Span::styled(visible, prefix.style))];
    }
    let content_width = width - prefix_width;
    let continuation_indent = transcript_continuation_indent(&line, content_width);
    let mut rows = vec![vec![prefix.clone()]];
    let mut used = 0usize;
    let mut has_content = false;

    let new_row = |rows: &mut Vec<Vec<Span<'static>>>| {
        let mut row = vec![prefix.clone()];
        if continuation_indent > 0 {
            row.push(Span::raw(" ".repeat(continuation_indent)));
        }
        rows.push(row);
    };

    for span in line.spans {
        let style = line_style.patch(span.style);
        let text = span.content.into_owned();
        for run in text_runs(&text) {
            let whitespace = run.chars().all(char::is_whitespace);
            let run_width = Span::raw(run).width();
            if whitespace {
                if !has_content && rows.len() > 1 {
                    continue;
                }
                if used + run_width <= content_width {
                    push_styled_text(rows.last_mut().unwrap(), run, style);
                    used += run_width;
                    has_content = true;
                } else {
                    new_row(&mut rows);
                    used = continuation_indent;
                    has_content = false;
                }
                continue;
            }

            if has_content && used + run_width > content_width {
                new_row(&mut rows);
                used = continuation_indent;
                has_content = false;
            }
            if used + run_width <= content_width {
                push_styled_text(rows.last_mut().unwrap(), run, style);
                used += run_width;
                has_content = true;
                continue;
            }

            for ch in run.chars() {
                let value = ch.to_string();
                let cells = Span::raw(&value).width();
                if used + cells > content_width {
                    if has_content {
                        new_row(&mut rows);
                        used = continuation_indent;
                        has_content = false;
                    }
                    if used + cells > content_width {
                        push_styled_text(rows.last_mut().unwrap(), "…", style);
                        break;
                    }
                }
                push_styled_text(rows.last_mut().unwrap(), &value, style);
                used += cells;
                has_content = true;
            }
        }
    }

    rows.into_iter().map(Line::from).collect()
}

fn entry_lines(app: &App, entry: &ConversationEntry, width: u16) -> Vec<Line<'static>> {
    match &entry.kind {
        ConversationKind::User { content } => {
            let mut lines = vec![
                Line::from(Span::styled(
                    "▌ You",
                    Style::default()
                        .fg(theme::user())
                        .add_modifier(Modifier::BOLD),
                ))
                .style(Style::default().bg(theme::user_surface())),
            ];
            for line in content.lines() {
                lines.extend(prefixed_wrapped_line(
                    Span::styled("▌ ", Style::default().fg(theme::user())),
                    Line::from(Span::styled(
                        line.to_owned(),
                        Style::default().fg(theme::text()),
                    )),
                    width,
                ));
            }
            for line in &mut lines {
                line.style = line.style.bg(theme::user_surface());
                let padding = usize::from(width).saturating_sub(line.width());
                line.spans.push(Span::raw(" ".repeat(padding)));
            }
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
            let mut lines = if content.trim().is_empty() {
                Vec::new()
            } else {
                vec![Line::styled("● Yeet", theme::brand())]
            };
            for line in markdown_lines(content) {
                lines.extend(prefixed_wrapped_line(
                    Span::styled("  ", Style::default().fg(theme::border())),
                    line,
                    width,
                ));
            }
            lines.extend(tool_group_lines(
                app,
                &tool_calls.iter().collect::<Vec<_>>(),
                width,
                app.state.is_streaming,
            ));
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
                Span::styled("Thinking", Style::default().fg(theme::accent_warm())),
                Span::styled(
                    summary
                        .map(|value| format!(" — {value}"))
                        .unwrap_or_default(),
                    Style::default().fg(theme::muted()),
                ),
            ])];
            for line in markdown_lines(content) {
                lines.extend(prefixed_wrapped_line(
                    Span::styled("┊ ", Style::default().fg(theme::border())),
                    line.style(Style::default().fg(theme::muted())),
                    width,
                ));
            }
            lines
        }
        ConversationKind::Activity { activity } => {
            let latest = app.state.active_activity_entry_id.as_deref() == Some(entry.id.as_str());
            vec![activity_line(app, activity, false, latest, width)]
        }
        ConversationKind::ToolCall { tool_call } => {
            tool_group_lines(app, &[tool_call], width, app.state.is_streaming)
        }
        ConversationKind::Skill {
            name,
            content,
            status,
        } => {
            let mut lines = vec![Line::from(vec![
                Span::styled("skill ", Style::default().fg(theme::accent())),
                Span::styled(name.clone(), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(
                    status
                        .as_deref()
                        .map(|value| format!(" · {value}"))
                        .unwrap_or_default(),
                    Style::default().fg(theme::muted()),
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
                theme::error()
            } else {
                theme::accent()
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
                    Style::default().fg(theme::text_dim()),
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
        (
            "·",
            Style::default()
                .fg(theme::accent_hot())
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(theme::accent_hot())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        match phase {
            "done" => (
                "·",
                Style::default().fg(theme::muted()),
                Style::default().fg(theme::text_dim()),
            ),
            "failed" => (
                "!",
                Style::default()
                    .fg(theme::error())
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(theme::error()),
            ),
            "interrupted" => (
                "—",
                Style::default().fg(theme::accent()),
                Style::default().fg(theme::text_dim()),
            ),
            _ => (
                "·",
                Style::default().fg(theme::muted()),
                Style::default().fg(theme::muted()),
            ),
        }
    };

    let mut spans = vec![
        Span::styled(format!("  {marker} "), marker_style),
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
            Style::default().fg(theme::muted()),
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
                Style::default().fg(theme::muted()),
            ));
        }
    }

    Line::from(spans)
}

#[cfg(test)]
fn tool_activity_line(call: &crate::model::ConversationToolCall, width: u16) -> Line<'static> {
    tool_activity_line_count(call, width, 1)
}

fn tool_activity_line_count(
    call: &crate::model::ConversationToolCall,
    width: u16,
    count: usize,
) -> Line<'static> {
    let active = matches!(
        call.status,
        ToolCallStatus::Preparing | ToolCallStatus::AwaitingPermission | ToolCallStatus::Running
    );
    let failed = matches!(
        call.status,
        ToolCallStatus::Failed
            | ToolCallStatus::Cancelled
            | ToolCallStatus::Interrupted
            | ToolCallStatus::TimedOut
    );
    let icon = tool_icon(&call.name);
    let icon_style = if failed {
        Style::default().fg(theme::error())
    } else if active {
        Style::default().fg(theme::accent())
    } else {
        Style::default().fg(theme::muted())
    };
    let text_style = if failed {
        Style::default().fg(theme::error())
    } else if active {
        Style::default().fg(theme::text())
    } else {
        Style::default().fg(theme::text_dim())
    };
    let prefix = format!("    {icon} ");
    let width = usize::from(width);
    let prefix = truncate_end(&prefix, width);
    let remaining = width.saturating_sub(Span::raw(&prefix).width());
    let detail = if count > 1 {
        compact_tool_pattern_detail(call)
    } else {
        tool_activity_detail(call)
    };
    let detail = if count > 1 {
        format!("{detail} ×{count}")
    } else {
        detail
    };
    let text = task::fit(&detail, remaining);
    Line::from(vec![
        Span::styled(prefix, icon_style),
        Span::styled(text, text_style),
    ])
}

fn tool_call_status_bucket(call: &crate::model::ConversationToolCall) -> u8 {
    match call.status {
        ToolCallStatus::Preparing
        | ToolCallStatus::AwaitingPermission
        | ToolCallStatus::Running => 1,
        ToolCallStatus::Failed
        | ToolCallStatus::Cancelled
        | ToolCallStatus::Interrupted
        | ToolCallStatus::TimedOut => 2,
        ToolCallStatus::Completed => 0,
        ToolCallStatus::Suppressed => 3,
    }
}

const COLLAPSED_TOOL_GROUP_LIMIT: usize = 4;

fn grouped_tool_calls<'a>(
    calls: &[&'a crate::model::ConversationToolCall],
    merge_non_adjacent: bool,
) -> Vec<(&'a crate::model::ConversationToolCall, usize)> {
    let mut groups: Vec<(&'a crate::model::ConversationToolCall, usize)> = Vec::new();
    for call in calls {
        let status_bucket = tool_call_status_bucket(call);
        if merge_non_adjacent
            && let Some(index) = groups.iter().position(|(previous, _)| {
                previous.name == call.name && tool_call_status_bucket(previous) == status_bucket
            })
        {
            groups[index].1 += 1;
            continue;
        }
        if let Some((previous, count)) = groups.last_mut()
            && previous.name == call.name
            && tool_call_status_bucket(previous) == status_bucket
        {
            *count += 1;
        } else {
            groups.push((*call, 1));
        }
    }
    groups
}

fn tool_pattern_label(name: &str) -> String {
    match name {
        "apply_file_edits" => "Edit files".into(),
        "search_workspace" => "Search files".into(),
        "search_artifact" => "Search output".into(),
        "web_search" => "Search web".into(),
        "read_file" | "read_files" => "Read files".into(),
        "read_artifact" => "Read output".into(),
        "read_document" => "Read document".into(),
        "web_read" => "Read web".into(),
        "run_shell" => "Shell".into(),
        "list_files" => "List files".into(),
        "analyze_data" => "Analyze data".into(),
        "activate_capability" => "Activate capability".into(),
        _ => humanize_tool_name(name),
    }
}

fn humanize_tool_name(name: &str) -> String {
    let human = name.replace('_', " ").replace('-', " ");
    let mut chars = human.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => "Tool".into(),
    }
}

fn compact_tool_pattern_detail(call: &crate::model::ConversationToolCall) -> String {
    tool_pattern_label(&call.name)
}

fn tool_icon(name: &str) -> &'static str {
    match name {
        "apply_file_edits" => "✎",
        "search_workspace" | "search_artifact" | "web_search" => "⌕",
        "read_file" | "read_files" | "read_artifact" | "read_document" | "web_read" => "▤",
        "run_shell" => "⌘",
        "list_files" => "≡",
        "analyze_data" => "▦",
        "activate_capability" => "◇",
        _ => "·",
    }
}

fn tool_activity_detail(call: &crate::model::ConversationToolCall) -> String {
    let active = matches!(
        call.status,
        ToolCallStatus::Preparing | ToolCallStatus::AwaitingPermission | ToolCallStatus::Running
    );
    let failed = matches!(
        call.status,
        ToolCallStatus::Failed
            | ToolCallStatus::Cancelled
            | ToolCallStatus::Interrupted
            | ToolCallStatus::TimedOut
    );
    let verb = match call.name.as_str() {
        "apply_file_edits" => {
            if active {
                "Editing"
            } else {
                "Edited"
            }
        }
        "search_workspace" | "search_artifact" | "web_search" => {
            if active {
                "Searching"
            } else {
                "Searched"
            }
        }
        "read_file" | "read_files" => {
            if active {
                "Reading"
            } else {
                "Read"
            }
        }
        "read_artifact" => {
            if active {
                "Reading output"
            } else {
                "Read output"
            }
        }
        "read_document" => {
            if active {
                "Reading document"
            } else {
                "Read document"
            }
        }
        "web_read" => {
            if active {
                "Reading web source"
            } else {
                "Read web source"
            }
        }
        "run_shell" => {
            if active {
                "Running command"
            } else {
                "Ran command"
            }
        }
        "list_files" => {
            if active {
                "Listing files"
            } else {
                "Listed files"
            }
        }
        "analyze_data" => {
            if active {
                "Analyzing data"
            } else {
                "Analyzed data"
            }
        }
        _ => {
            if active {
                "Working"
            } else {
                "Completed"
            }
        }
    };
    let detail = match call.name.as_str() {
        "apply_file_edits" => edit_activity_detail(call),
        "search_workspace" => search_activity_detail(call),
        "read_file" | "read_files" => read_activity_detail(call),
        "read_artifact" | "search_artifact" => String::new(),
        _ => String::new(),
    };
    if failed {
        if detail.is_empty() {
            format!("Failed · {verb}")
        } else {
            format!("Failed · {detail}")
        }
    } else if detail.is_empty() {
        verb.to_owned()
    } else {
        format!("{verb} {detail}")
    }
}

fn short_tool_path(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_owned()
}

fn edit_activity_detail(call: &crate::model::ConversationToolCall) -> String {
    let value = serde_json::from_str::<Value>(&call.arguments).ok();
    let path = value
        .as_ref()
        .and_then(|value| value.get("changes"))
        .and_then(Value::as_array)
        .and_then(|changes| changes.first())
        .and_then(|change| change.get("path"))
        .and_then(Value::as_str)
        .map(short_tool_path)
        .unwrap_or_else(|| "file".into());
    let (added, removed) = edit_line_delta(call, value.as_ref());
    match (added, removed) {
        (0, 0) => path,
        (added, 0) => format!("{path} +{added}"),
        (0, removed) => format!("{path} −{removed}"),
        (added, removed) => format!("{path} +{added} −{removed}"),
    }
}

fn edit_line_delta(
    call: &crate::model::ConversationToolCall,
    arguments: Option<&Value>,
) -> (usize, usize) {
    if let Ok(result) = serde_json::from_str::<Value>(call.result.as_deref().unwrap_or("")) {
        let mut added = 0;
        let mut removed = 0;
        if let Some(files) = result.get("files").and_then(Value::as_array) {
            for file in files {
                if let Some(hunks) = file
                    .get("diff")
                    .and_then(|diff| diff.get("hunks"))
                    .and_then(Value::as_array)
                {
                    for hunk in hunks {
                        if let Some(lines) = hunk.get("lines").and_then(Value::as_array) {
                            for line in lines.iter().filter_map(Value::as_str) {
                                if line.starts_with('+') {
                                    added += 1;
                                }
                                if line.starts_with('-') {
                                    removed += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        if added > 0 || removed > 0 {
            return (added, removed);
        }
    }
    let mut added = 0;
    let mut removed = 0;
    let Some(changes) = arguments
        .and_then(|value| value.get("changes"))
        .and_then(Value::as_array)
    else {
        return (0, 0);
    };
    for change in changes {
        if let Some(file_op) = change.get("fileOp").and_then(Value::as_object) {
            match file_op.get("kind").and_then(Value::as_str) {
                Some("create") => added += line_count(file_op.get("text")),
                Some("delete") => removed += 1,
                _ => {}
            }
        }
        for edit in change
            .get("edits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match edit.get("kind").and_then(Value::as_str) {
                Some("insert") | Some("insertAfterBlock") => added += line_count(edit.get("text")),
                Some("delete") | Some("deleteBlock") => {
                    removed += range_line_count(edit.get("range"))
                }
                Some("replace") | Some("replaceBlock") => {
                    added += line_count(edit.get("text"));
                    removed += range_line_count(edit.get("range"));
                }
                _ => {}
            }
        }
    }
    (added, removed)
}

fn line_count(value: Option<&Value>) -> usize {
    value
        .and_then(Value::as_str)
        .map(|text| text.lines().count().max(1))
        .unwrap_or(0)
}

fn range_line_count(value: Option<&Value>) -> usize {
    let Some(range) = value.and_then(Value::as_object) else {
        return 0;
    };
    let start = range.get("start").and_then(Value::as_u64).unwrap_or(0);
    let end = range.get("end").and_then(Value::as_u64).unwrap_or(start);
    end.saturating_sub(start).saturating_add(1) as usize
}

fn search_activity_detail(call: &crate::model::ConversationToolCall) -> String {
    let Ok(value) = serde_json::from_str::<Value>(&call.arguments) else {
        return "workspace".into();
    };
    let query = value.get("query").and_then(Value::as_str).unwrap_or("");
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .map(short_tool_path);
    match (path, query.is_empty()) {
        (Some(path), false) => format!("{path} for {query}"),
        (Some(path), true) => path,
        (None, false) => format!("for {query}"),
        (None, true) => "workspace".into(),
    }
}

fn read_activity_detail(call: &crate::model::ConversationToolCall) -> String {
    let Ok(value) = serde_json::from_str::<Value>(&call.arguments) else {
        return "file".into();
    };
    if let Some(path) = value.get("path").and_then(Value::as_str) {
        return short_tool_path(path);
    }
    value
        .get("requests")
        .and_then(Value::as_array)
        .and_then(|requests| requests.first())
        .and_then(|request| request.get("path"))
        .and_then(Value::as_str)
        .map(short_tool_path)
        .unwrap_or_else(|| "file".into())
}

fn tool_argument_summary(arguments: &str, max_chars: usize) -> Option<String> {
    if arguments.trim().is_empty() || max_chars == 0 {
        return None;
    }
    let value: Value = serde_json::from_str(arguments).ok()?;
    let object = value.as_object()?;

    if let Some(purpose) = object.get("purpose").and_then(Value::as_str) {
        let clean: String = purpose
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect();
        let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
        if !clean.is_empty() {
            return Some(truncate_middle(&clean, max_chars));
        }
    }

    for key in ["path", "query", "capability", "url"] {
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

    None
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
mod suggestion_tests {
    use super::*;
    use ratatui::{
        buffer::Buffer,
        widgets::{ListState, StatefulWidget},
    };

    #[test]
    fn selection_highlights_the_whole_row_and_keeps_a_visible_marker() {
        let suggestions = vec![
            ("/help".into(), "Show help".into()),
            ("/models".into(), "Choose model".into()),
        ];
        let area = Rect::new(0, 0, 64, 4);
        let mut buffer = Buffer::empty(area);
        let mut state = ListState::default().with_selected(Some(1));
        StatefulWidget::render(suggestion_list(&suggestions), area, &mut buffer, &mut state);
        assert_eq!(buffer[(2, 2)].symbol(), "›");
        assert_eq!(buffer[(2, 1)].symbol(), " ");
        for x in 2..62 {
            assert_eq!(buffer[(x, 2)].bg, theme::selected_color(), "column {x}");
            assert_ne!(buffer[(x, 1)].bg, theme::selected_color());
        }
    }

    #[test]
    fn long_commands_have_a_gap_before_the_description_and_selection_scrolls() {
        let suggestions = vec![
            ("/help".into(), "Help".into()),
            ("/long-command-name".into(), "Description".into()),
        ];
        let area = Rect::new(0, 0, 64, 3);
        let mut buffer = Buffer::empty(area);
        let mut state = ListState::default().with_selected(Some(1));
        StatefulWidget::render(suggestion_list(&suggestions), area, &mut buffer, &mut state);
        let row: String = (0..area.width).map(|x| buffer[(x, 1)].symbol()).collect();
        assert!(row.contains("› /long-command-name  Description"), "{row}");
        assert_eq!(state.offset(), 1);
    }
}

#[cfg(test)]
mod tool_summary_layout_tests {
    use super::*;

    fn call(status: ToolCallStatus) -> crate::model::ConversationToolCall {
        crate::model::ConversationToolCall {
            id: "summary-test".into(),
            index: None,
            call_id: None,
            name: "run_shell".into(),
            arguments: r#"{"purpose":"Verify Rust changes","command":"cargo check"}"#.into(),
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
        }
    }

    #[test]
    fn status_rows_stay_human_readable() {
        for status in [
            ToolCallStatus::Running,
            ToolCallStatus::Completed,
            ToolCallStatus::Failed,
            ToolCallStatus::Suppressed,
        ] {
            let line = tool_activity_line(&call(status), 100);
            let text = line.to_string();
            assert!(text.contains("command"), "{text}");
            assert!(!text.contains("run_shell"));
            assert!(!text.contains("cargo check"));
        }
    }

    #[test]
    fn narrow_unicode_summaries_fit_terminal_cells() {
        let mut call = call(ToolCallStatus::Completed);
        call.arguments = serde_json::json!({"purpose": "检查界面 🦀 ".repeat(50)}).to_string();
        for width in [0, 1, 4, 12, 24, 40, 80] {
            let line = tool_activity_line(&call, width);
            assert!(line.width() <= usize::from(width));
        }
    }

    #[test]
    fn activity_rows_use_compact_file_language() {
        let mut edit = call(ToolCallStatus::Completed);
        edit.name = "apply_file_edits".into();
        edit.arguments = serde_json::json!({
            "changes": [{
                "path": "src/ui.rs",
                "edits": [{
                    "kind": "replace",
                    "range": {"start": 1, "end": 48},
                    "text": (0..46).map(|_| "line").collect::<Vec<_>>().join("\n")
                }]
            }]
        })
        .to_string();
        assert_eq!(
            tool_activity_line(&edit, 100).to_string(),
            "    ✎ Edited ui.rs +46 −48"
        );

        let mut search = call(ToolCallStatus::Completed);
        search.name = "search_workspace".into();
        search.arguments = serde_json::json!({
            "query": "elapsed_ms",
            "path": "src/ui.rs"
        })
        .to_string();
        assert_eq!(
            tool_activity_line(&search, 100).to_string(),
            "    ⌕ Searched ui.rs for elapsed_ms"
        );
    }
}

fn tool_group_lines(
    app: &App,
    calls: &[&crate::model::ConversationToolCall],
    width: u16,
    expanded: bool,
) -> Vec<Line<'static>> {
    let calls = calls
        .iter()
        .copied()
        .filter(|call| !matches!(call.status, ToolCallStatus::Suppressed))
        .collect::<Vec<_>>();
    if calls.is_empty() {
        return Vec::new();
    }

    let needs_attention = |call: &crate::model::ConversationToolCall| {
        !matches!(call.status, ToolCallStatus::Completed)
    };
    let attention = calls.iter().filter(|call| needs_attention(call)).count();
    let groups = grouped_tool_calls(&calls, !expanded);
    let elapsed = tool_group_elapsed(app, &calls);

    let (marker, header_color) = if attention == 0 {
        ("✓", theme::muted())
    } else if app.state.is_streaming {
        (activity_marker(app), activity_marker_color(app))
    } else {
        ("!", theme::error())
    };

    let mut header = format!(
        "  {marker} Tools · {} call{}",
        calls.len(),
        if calls.len() == 1 { "" } else { "s" }
    );
    if elapsed > 0 {
        header.push_str(&format!(" · {}", format_work_elapsed(elapsed)));
    }
    let header = truncate_end(&header, usize::from(width));
    let mut lines = vec![Line::styled(header, Style::default().fg(header_color))];

    let visible_groups = if expanded || attention > 0 {
        groups.len()
    } else {
        groups.len().min(COLLAPSED_TOOL_GROUP_LIMIT)
    };
    lines.extend(
        groups
            .iter()
            .take(visible_groups)
            .map(|(call, count)| tool_activity_line_count(call, width, *count)),
    );

    let hidden_groups = groups.len().saturating_sub(visible_groups);
    if hidden_groups > 0 {
        let overflow = format!(
            "    … +{hidden_groups} more action{}",
            if hidden_groups == 1 { "" } else { "s" }
        );
        lines.push(Line::styled(
            truncate_end(&overflow, usize::from(width)),
            Style::default().fg(theme::muted()),
        ));
    }

    lines
}

fn tool_group_elapsed(app: &App, calls: &[&crate::model::ConversationToolCall]) -> u128 {
    if app.state.is_streaming {
        if let Some(elapsed) = app.stream_elapsed() {
            return elapsed.as_millis();
        }
    }
    let recorded = calls
        .iter()
        .filter_map(|call| call.duration_ms)
        .map(u128::from)
        .sum::<u128>();
    if recorded > 0 {
        recorded
    } else {
        app.latest_turn_duration()
            .map(|duration| duration.as_millis())
            .unwrap_or_default()
    }
}

fn format_work_elapsed(elapsed_ms: u128) -> String {
    let seconds = elapsed_ms / 1_000;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod compact_tool_group_tests {
    use super::*;

    #[test]
    fn activity_text_morphs_between_live_phases() {
        assert_eq!(
            morph_activity_text("Thinking", "Searching", 0.0),
            "Thinking"
        );
        assert_eq!(
            morph_activity_text("Thinking", "Searching", 1.0),
            "Searching"
        );
        let middle = morph_activity_text("Thinking", "Searching", 0.5);
        assert_ne!(middle, "Thinking");
        assert_ne!(middle, "Planning");
    }

    #[test]
    fn renders_readable_tool_summary_and_fits_narrow_screens() {
        let mut calls = (0..8)
            .map(|index| crate::model::ConversationToolCall {
                id: index.to_string(),
                index: None,
                call_id: None,
                name: "run_shell".into(),
                arguments: serde_json::json!({"purpose": format!("Check {index}")}).to_string(),
                status: ToolCallStatus::Completed,
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
            })
            .collect::<Vec<_>>();
        calls[0].status = ToolCallStatus::Failed;
        calls[1].status = ToolCallStatus::Running;
        let refs = calls.iter().collect::<Vec<_>>();
        let app = App::default();
        let lines = tool_group_lines(&app, &refs, 100, true);
        assert_eq!(lines.len(), 4); // header plus one row per status/action group
        let text = lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Tools · 8 calls"));
        assert!(!text.contains("pattern"));
        assert!(text.contains("Failed · Ran command"));
        assert!(text.contains("Running command"));
        assert!(text.contains("Shell ×6"));
        // Completed work remains legible after streaming instead of collapsing to counts only.
        assert_eq!(tool_group_lines(&app, &refs, 100, false).len(), 4);
        for width in [0, 1, 2, 12, 24, 80] {
            assert!(
                tool_group_lines(&app, &refs, width, true)
                    .iter()
                    .all(|line| line.width() <= usize::from(width))
            );
        }
    }

    #[test]
    fn completed_tool_summary_merges_repeated_actions_across_the_turn() {
        let call = |id: &str, name: &str| crate::model::ConversationToolCall {
            id: id.into(),
            index: None,
            call_id: None,
            name: name.into(),
            arguments: "{}".into(),
            status: ToolCallStatus::Completed,
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
        };
        let calls = [
            call("1", "web_search"),
            call("2", "web_read"),
            call("3", "web_search"),
            call("4", "web_read"),
        ];
        let refs = calls.iter().collect::<Vec<_>>();
        let app = App::default();
        let text = tool_group_lines(&app, &refs, 100, false)
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Tools · 4 calls"));
        assert!(text.contains("Search web ×2"));
        assert!(text.contains("Read web ×2"));
    }
}

#[cfg(test)]
mod composer_viewport_tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn long_draft_shows_visible_range_and_follows_cursor() {
        let mut app = App::default();
        app.input = (1..=12)
            .map(|n| format!("draft {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.cursor = app.input.chars().count();
        let mut terminal = Terminal::new(TestBackend::new(60, 5)).unwrap();
        terminal
            .draw(|frame| draw_input(frame, &mut app, frame.area()))
            .unwrap();
        let contents = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(contents.contains("Lines 10–12 of 12"));
        assert!(contents.contains("draft 12"));
        app.cursor = 0;
        terminal
            .draw(|frame| draw_input(frame, &mut app, frame.area()))
            .unwrap();
        let contents = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(contents.contains("Lines 1–3 of 12"));
        assert!(contents.contains("draft 1"));
    }

    #[test]
    fn multiline_draft_shows_compact_metadata_when_not_scrolled() {
        let mut app = App::default();
        app.input = "alpha\nbeta\ngamma".into();
        app.cursor = app.input.chars().count();
        let mut terminal = Terminal::new(TestBackend::new(60, 6)).unwrap();
        terminal
            .draw(|frame| draw_input(frame, &mut app, frame.area()))
            .unwrap();
        let contents = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(contents.contains("3 lines"));
        assert!(contents.contains("16 chars"));
    }
}

#[cfg(test)]
mod overall_layout_tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn history_status_explains_return_shortcut_at_narrow_widths() {
        let mut app = App::default();
        app.follow_tail = false;
        app.max_scroll = 200;
        app.scroll_y = 40;
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
        assert!(status_line(&app, 120).to_string().contains("unavailable"));
    }

    #[test]
    fn auxiliary_status_reports_usage_and_selection_shortcuts() {
        let mut app = App::default();
        app.state.token_usage.input_tokens = Some(12_500);
        app.state.token_usage.output_tokens = Some(2_300);
        app.state.token_usage.cached_input_tokens = Some(10_000);
        app.state.token_usage.cache_measured_input_tokens = Some(12_500);
        app.state.token_usage.model_calls = Some(4);

        let rich = status_aux_line(&app, 120).to_string();
        assert!(rich.contains("in 12.5k"));
        assert!(rich.contains("out 2.3k"));
        assert!(rich.contains("cache 80%"));
        assert!(rich.contains("calls 4"));
        for width in [12, 20, 40, 80] {
            assert!(status_aux_line(&app, width).width() <= width);
        }

        app.selection_start = Some((1, 1));
        app.selection_end = Some((2, 1));
        let selected = status_aux_line(&app, 80).to_string();
        assert!(selected.contains("Ctrl+C"));
        assert!(selected.contains("Esc"));
    }

    #[test]
    fn responsive_status_uses_second_row_only_when_space_allows() {
        assert_eq!(
            responsive::metrics(Rect::new(0, 0, 80, 24)).status_height,
            2
        );
        assert_eq!(
            responsive::metrics(Rect::new(0, 0, 60, 14)).status_height,
            1
        );
        assert_eq!(
            responsive::metrics(Rect::new(0, 0, 180, 40)).status_height,
            2
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
