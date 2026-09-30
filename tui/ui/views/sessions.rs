//! Active session/conversation view. `Mode::Sessions` remains the session picker.
pub(in crate::tui::ui) mod tools;

use super::super::{
    shell,
    support::{
        markdown::markdown_lines,
        responsive,
        text::{format_elapsed, prefixed_wrapped_line, truncate_end, truncate_middle},
        theme,
    },
    task,
};
use crate::{
    app::App,
    model::{ConversationEntry, ConversationKind, ToolCallStatus},
};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    prelude::{Line, Modifier, Span, Style, Stylize, Text},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use tools::{WorkItem, work_group_lines, work_groups};

#[derive(Debug, PartialEq, Eq)]
struct TranscriptKey {
    revision: u64,
    session_id: Option<String>,
    conversation_ptr: usize,
    entries: usize,
    width: u16,
    expanded: bool,
    selected_work: Option<usize>,
    expanded_work: std::collections::BTreeSet<usize>,
    palette: crate::theme::Palette,
    streaming: bool,
    assistant_id: Option<String>,
    assistant_text: String,
    reasoning_id: Option<String>,
    reasoning_text: String,
    reasoning_summary: String,
    activity_id: Option<String>,
}

impl TranscriptKey {
    fn matches(&self, app: &App, width: u16) -> bool {
        self.revision == app.transcript_revision
            && self.session_id == app.state.current_session_id
            && self.conversation_ptr == app.conversation.as_ptr() as usize
            && self.entries == app.conversation.len()
            && self.width == width
            && self.expanded == app.tools_expanded
            && self.selected_work
                == app
                    .selected_work
                    .filter(|_| !app.input_focused && !app.sidebar_focus)
            && self.expanded_work == app.expanded_work
            && self.palette == theme::active_palette()
            && self.streaming == app.state.is_streaming
            && self.assistant_id == app.state.active_assistant_entry_id
            && self.assistant_text == app.state.active_assistant_text
            && self.reasoning_id == app.state.active_reasoning_entry_id
            && self.reasoning_text == app.state.active_reasoning_text
            && self.reasoning_summary == app.state.active_reasoning_summary
            && self.activity_id == app.state.active_activity_entry_id
    }

    fn for_app(app: &App, width: u16) -> Self {
        Self {
            revision: app.transcript_revision,
            session_id: app.state.current_session_id.clone(),
            conversation_ptr: app.conversation.as_ptr() as usize,
            entries: app.conversation.len(),
            width,
            expanded: app.tools_expanded,
            selected_work: app
                .selected_work
                .filter(|_| !app.input_focused && !app.sidebar_focus),
            expanded_work: app.expanded_work.clone(),
            palette: theme::active_palette(),
            streaming: app.state.is_streaming,
            assistant_id: app.state.active_assistant_entry_id.clone(),
            assistant_text: app.state.active_assistant_text.clone(),
            reasoning_id: app.state.active_reasoning_entry_id.clone(),
            reasoning_text: app.state.active_reasoning_text.clone(),
            reasoning_summary: app.state.active_reasoning_summary.clone(),
            activity_id: app.state.active_activity_entry_id.clone(),
        }
    }
}

/// Parsed/styled transcript plus wrapped-row offsets. Memory is proportional
/// to transcript text, not terminal width times the entire scrollback height.
#[derive(Debug)]
pub(crate) struct TranscriptCache {
    key: TranscriptKey,
    text: Text<'static>,
    starts: Vec<usize>,
    rows: usize,
    work_lines: Vec<usize>,
}

impl TranscriptCache {
    fn viewport(&self, scroll: u16, height: u16) -> (Text<'static>, u16) {
        let first = self
            .starts
            .partition_point(|row| *row <= scroll as usize)
            .saturating_sub(1);
        let end = self
            .starts
            .partition_point(|row| *row < scroll as usize + height as usize)
            .min(self.text.lines.len());
        let text = Text::from(self.text.lines[first..end].to_vec());
        (
            text,
            (scroll as usize).saturating_sub(self.starts[first]) as u16,
        )
    }

    fn new(key: TranscriptKey, mut text: Text<'static>) -> Self {
        if text.lines.is_empty() {
            text.lines.push(Line::default());
        }
        let mut rows = 0;
        let starts = text
            .lines
            .iter()
            .map(|line| {
                let start = rows;
                rows += Paragraph::new(line.clone())
                    .wrap(Wrap { trim: false })
                    .line_count(key.width.max(1))
                    .max(1);
                start
            })
            .collect();
        Self {
            key,
            text,
            starts,
            rows,
            work_lines: Vec::new(),
        }
    }
}

pub(in crate::tui::ui) fn draw(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    viewport_shape: responsive::Shape,
) {
    draw_desktop(frame, app, area, viewport_shape)
}

fn draw_desktop(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    viewport_shape: responsive::Shape,
) {
    let area = if viewport_shape == responsive::Shape::Portrait && area.height > 7 {
        Rect::new(
            area.x + 1,
            area.y + 4,
            area.width.saturating_sub(2),
            area.height - 5,
        )
    } else {
        area.inner(Margin {
            horizontal: 1,
            vertical: u16::from(area.height > 4),
        })
    };
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::code_background())),
        area,
    );
    let transcript_area = (area.x, area.y, area.width, area.height);
    let previous_area = app.transcript_area;
    if previous_area.2 > 0 && previous_area.3 > 0 && previous_area != transcript_area {
        app.clear_transcript_selection();
    }
    app.transcript_area = transcript_area;
    if app.conversation.is_empty() {
        app.max_scroll = 0;
        app.scroll_y = 0;
        app.transcript_cells.clear();
        app.work_rows.clear();
        if app.state.current_session_id.is_some() {
            let (title, detail) = if app.state.is_streaming {
                (
                    "Waiting for a response",
                    "The conversation will appear here as it arrives.",
                )
            } else if app.sessions().iter().any(|session| {
                Some(session.id.as_str()) == app.state.current_session_id.as_deref()
                    && session.message_count > 0
            }) {
                (
                    "Conversation unavailable",
                    "Choose the session again with Alt+S to reload it.",
                )
            } else {
                (
                    "New session",
                    "Write a message below to begin this conversation.",
                )
            };
            draw_empty_session(frame, area, title, detail);
        } else {
            shell::draw_welcome(frame, area, viewport_shape);
        }
        return;
    }
    if app
        .transcript_cache
        .as_ref()
        .is_none_or(|cache| !cache.key.matches(app, area.width))
    {
        let (text, work_lines) = transcript_content(app, area.width);
        let mut cache = TranscriptCache::new(TranscriptKey::for_app(app, area.width), text);
        cache.work_lines = work_lines;
        app.transcript_cache = Some(cache);
    }
    let cache = app
        .transcript_cache
        .as_ref()
        .expect("transcript layout cached");
    app.work_rows = cache
        .work_lines
        .iter()
        .map(|line| cache.starts[*line].min(u16::MAX as usize) as u16)
        .collect();
    if app
        .selected_work
        .is_some_and(|index| index >= app.work_rows.len())
    {
        app.selected_work = None;
    }
    app.max_scroll = cache.rows.min(u16::MAX as usize) as u16;
    app.max_scroll = app.max_scroll.saturating_sub(area.height);
    if app.follow_tail {
        app.scroll_y = app.max_scroll;
    } else {
        app.scroll_y = app.scroll_y.min(app.max_scroll);
    }
    // Start at the logical line containing the viewport, not at the beginning
    // of the session. Paragraph only reflows the few visible logical lines.
    let (text, offset) = cache.viewport(app.scroll_y, area.height);
    frame.render_widget(
        Paragraph::new(text.clone())
            .wrap(Wrap { trim: false })
            .scroll((offset, 0)),
        area,
    );
    capture_transcript_cells(app, area, &text, offset);
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

fn draw_empty_session(frame: &mut Frame<'_>, area: Rect, title: &str, detail: &str) {
    let width = area.width.saturating_sub(4).min(72);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height / 3;
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(title, Style::default().fg(theme::text()).bold()),
            Line::default(),
            Line::styled(detail, Style::default().fg(theme::muted())),
        ]),
        Rect::new(x, y, width, area.height.saturating_sub(y - area.y).min(3)),
    );
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

// Render a separate copy layer with decoration spans marked, retaining exact
// terminal-cell coordinates (including wide Unicode graphemes).
fn capture_transcript_cells(app: &mut App, area: Rect, text: &Text<'static>, offset: u16) {
    use ratatui::{buffer::Buffer, widgets::Widget};
    let ignored = ratatui::style::Color::Rgb(1, 2, 3);
    let mut copy = text.clone();
    for line in &mut copy.lines {
        let bubble = line.spans.first().is_some_and(|span| {
            span.style.bg.is_none() && span.content.chars().all(|ch| ch == ' ')
        }) && line
            .spans
            .iter()
            .skip(1)
            .all(|span| span.style.bg == Some(theme::surface_color()));
        let divider = line
            .spans
            .iter()
            .all(|span| span.style.fg == Some(theme::hairline()));
        let work_header = line.spans.len() == 5
            && line.spans[2].content == " "
            && line.spans[4].style.fg == Some(theme::muted());
        let count = line.spans.len();
        for (index, span) in line.spans.iter_mut().enumerate() {
            let padding = bubble && (index == 0 || index == 1 || index + 1 == count);
            let decoration = span.content.contains('\u{a0}')
                || (span.content.contains('│')
                    && span.content.chars().all(|ch| ch == '│' || ch == ' '));
            if padding || divider || decoration || (work_header && (index < 3 || index == 4)) {
                span.style = span.style.fg(ignored);
            }
        }
    }
    let mut buffer = Buffer::empty(area);
    Paragraph::new(copy)
        .wrap(Wrap { trim: false })
        .scroll((offset, 0))
        .render(area, &mut buffer);
    app.transcript_cells = (area.y..area.bottom())
        .map(|row| {
            (area.x..area.right())
                .map(|column| {
                    let cell = &buffer[(column, row)];
                    if cell.fg == ignored {
                        String::new()
                    } else {
                        cell.symbol().to_owned()
                    }
                })
                .collect()
        })
        .collect();
}

pub(in crate::tui::ui) fn draw_context_menu(frame: &mut Frame<'_>, app: &mut App) {
    let Some(menu) = app.transcript_context_menu else {
        app.transcript_context_menu_area = (0, 0, 0, 0);
        return;
    };
    let screen = frame.area();
    if screen.width < 18 || screen.height < 5 {
        app.transcript_context_menu_area = (0, 0, 0, 0);
        return;
    }
    let width = 22u16.min(screen.width);
    let height = 5u16.min(screen.height);
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
        ListItem::new(Line::styled(" Copy      ⌘C", copy_style)),
        ListItem::new(Line::styled(" Paste     ⌘V", theme::surface())),
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

#[cfg(test)]
fn transcript_text(app: &App, width: u16) -> Text<'static> {
    transcript_content(app, width).0
}

fn transcript_content(app: &App, width: u16) -> (Text<'static>, Vec<usize>) {
    let mut work_lines = Vec::new();
    let mut lines = Vec::new();
    let mut rendered_any = false;
    let mut previous_compact = false;
    let mut index = 0;
    while index < app.conversation.len() {
        let entry = &app.conversation[index];
        if matches!(
            &entry.kind,
            ConversationKind::Activity { activity } if !visible_terminal_activity(activity)
        ) {
            index += 1;
            continue;
        }

        if matches!(
            &entry.kind,
            ConversationKind::ToolCall { .. } | ConversationKind::Reasoning { .. }
        ) {
            let mut end = index;
            let mut items = Vec::new();
            while end < app.conversation.len() {
                let current = &app.conversation[end];
                match &current.kind {
                    ConversationKind::ToolCall { tool_call } => {
                        items.push(WorkItem::Tool(tool_call))
                    }
                    ConversationKind::Reasoning { .. } => {
                        if let Some(summary) = summary_reasoning(app, current) {
                            let live = app.state.is_streaming
                                && app.state.active_reasoning_entry_id.as_deref()
                                    == Some(current.id.as_str());
                            items.push(WorkItem::Summary {
                                text: summary,
                                live,
                            });
                        } else if let Some((text, _)) = reasoning_parts(app, current)
                            && !text.trim().is_empty()
                        {
                            let live = app.state.is_streaming
                                && app.state.active_reasoning_entry_id.as_deref()
                                    == Some(current.id.as_str());
                            items.push(WorkItem::Reasoning { text, live });
                        }
                    }
                    ConversationKind::Activity { activity } if !terminal_activity(activity) => {}
                    _ => break,
                }
                end += 1;
            }
            index = end;
            let (work, headers) = tools::work_group_lines_selected(
                &work_groups(&items),
                width,
                app.tools_expanded,
                app.selected_work
                    .filter(|_| !app.input_focused && !app.sidebar_focus),
                &app.expanded_work,
                work_lines.len(),
            );
            if !work.is_empty() {
                if rendered_any && !previous_compact {
                    lines.push(Line::default());
                }
                work_lines.extend(headers.into_iter().map(|line| lines.len() + line));
                lines.extend(work);
                rendered_any = true;
                previous_compact = true;
            }
            continue;
        }

        let compact = compact_transcript_entry(&entry.kind);
        let entry_content = entry_lines(app, entry, width);
        if entry_content.is_empty() {
            index += 1;
            continue;
        }
        if rendered_any && (!compact || !previous_compact) {
            if previous_compact && matches!(entry.kind, ConversationKind::Assistant { .. }) {
                lines.push(Line::styled(
                    "─".repeat(width as usize),
                    Style::default().fg(theme::hairline()),
                ));
            } else {
                lines.push(Line::default());
            }
        }
        lines.extend(entry_content);
        rendered_any = true;
        previous_compact = compact
            || matches!(&entry.kind, ConversationKind::Assistant { tool_calls, .. } if !tool_calls.is_empty());
        index += 1;
    }

    (Text::from(lines), work_lines)
}

fn reasoning_parts<'a>(
    app: &'a App,
    entry: &'a ConversationEntry,
) -> Option<(&'a str, Option<&'a str>)> {
    let ConversationKind::Reasoning { content, summary } = &entry.kind else {
        return None;
    };
    if app.state.active_reasoning_entry_id.as_deref() != Some(entry.id.as_str()) {
        return Some((content, summary.as_deref()));
    }
    Some((
        if app.state.active_reasoning_text.is_empty() {
            content
        } else {
            &app.state.active_reasoning_text
        },
        if app.state.active_reasoning_summary.is_empty() {
            summary.as_deref()
        } else {
            Some(&app.state.active_reasoning_summary)
        },
    ))
}

fn summary_reasoning<'a>(app: &'a App, entry: &'a ConversationEntry) -> Option<&'a str> {
    let (_, summary) = reasoning_parts(app, entry)?;
    summary.map(str::trim).filter(|summary| !summary.is_empty())
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

fn entry_lines(app: &App, entry: &ConversationEntry, width: u16) -> Vec<Line<'static>> {
    match &entry.kind {
        ConversationKind::User { content } => user_message_lines(content, width),
        ConversationKind::Assistant {
            content,
            tool_calls,
        } => {
            let content = if app.state.active_assistant_entry_id.as_deref()
                == Some(entry.id.as_str())
                && !app.state.active_assistant_text.is_empty()
            {
                &app.state.active_assistant_text
            } else {
                content
            };
            let mut lines = Vec::new();
            for line in session_markdown_lines(content) {
                lines.extend(prefixed_wrapped_line(Span::raw(""), line, width));
            }
            let items = tool_calls.iter().map(WorkItem::Tool).collect::<Vec<_>>();
            lines.extend(work_group_lines(
                &work_groups(&items),
                width,
                app.tools_expanded,
            ));
            lines
        }
        ConversationKind::Reasoning { .. } | ConversationKind::ToolCall { .. } => Vec::new(),
        ConversationKind::Activity { activity } => {
            let latest = app.state.active_activity_entry_id.as_deref() == Some(entry.id.as_str());
            vec![activity_line(app, activity, false, latest, width)]
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

fn session_markdown_lines(content: &str) -> Vec<Line<'static>> {
    let mut lines = markdown_lines(content);
    for line in &mut lines {
        for span in &mut line.spans {
            if span.style.bg == Some(theme::code_background()) {
                span.style = span.style.bg(theme::surface_color());
            }
        }
    }
    lines
}

fn user_message_lines(content: &str, width: u16) -> Vec<Line<'static>> {
    if width < 8 {
        return vec![Line::styled(
            truncate_end(content, width as usize),
            theme::surface(),
        )];
    }
    let cap = if width >= 90 {
        (u32::from(width) * 55 / 100) as u16
    } else {
        ((u32::from(width) * 80 / 100) as u16).max(6)
    };
    // Shrink-wrap short messages; long ones wrap inside the cap.
    let longest = content
        .split('\n')
        .map(|line| Span::raw(line).width())
        .max()
        .unwrap_or(0);
    let bubble_width = (longest as u16).saturating_add(4).clamp(6, cap.max(6));
    let indent = usize::from(width - bubble_width);
    let surface = theme::surface();
    let blank = || {
        Line::from(vec![
            Span::raw(" ".repeat(indent)),
            // NBSP: ratatui wraps a full-width all-space line into two rows.
            Span::styled("\u{a0}".repeat(bubble_width as usize), surface),
        ])
    };
    let mut lines = Vec::new();
    if width >= 90 {
        lines.push(blank());
    }
    for source in content.split('\n') {
        for row in prefixed_wrapped_line(
            Span::raw("  "),
            Line::styled(source.to_owned(), surface),
            bubble_width.saturating_sub(2),
        ) {
            let right_padding = (bubble_width as usize).saturating_sub(row.width());
            let mut spans = vec![Span::raw(" ".repeat(indent))];
            spans.extend(
                row.spans
                    .into_iter()
                    .map(|span| Span::styled(span.content.into_owned(), surface.patch(span.style))),
            );
            spans.push(Span::styled(" ".repeat(right_padding), surface));
            lines.push(Line::from(spans));
        }
    }
    if width >= 90 {
        lines.push(blank());
    }
    lines
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
mod tests;
