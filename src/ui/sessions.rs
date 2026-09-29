//! Active session/conversation view. `Mode::Sessions` remains the session picker.
mod legacy;
pub(super) mod tools;
use super::text::{format_elapsed, prefixed_wrapped_line, truncate_end, truncate_middle};
use super::{markdown::markdown_lines, responsive, shell, task, theme};
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
use tools::{WorkEvent, reasoning_summary_render_lines, tool_group_lines};

pub(super) fn draw(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    viewport_shape: responsive::Shape,
) {
    match viewport_shape {
        responsive::Shape::Tiny
        | responsive::Shape::ShortWide
        | responsive::Shape::Portrait
        | responsive::Shape::Compact => legacy::draw(frame, app, area, viewport_shape),
        responsive::Shape::Standard | responsive::Shape::Wide | responsive::Shape::UltraWide => {
            draw_desktop(frame, app, area, viewport_shape)
        }
    }
}

fn draw_desktop(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    viewport_shape: responsive::Shape,
) {
    let area = area.inner(Margin {
        horizontal: 1,
        vertical: u16::from(area.height > 4),
    });
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

pub(super) fn draw_context_menu(frame: &mut Frame<'_>, app: &mut App) {
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

fn transcript_text(app: &App, width: u16) -> Text<'static> {
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

        let starts_tool_work = matches!(&entry.kind, ConversationKind::ToolCall { .. })
            || summary_only_reasoning(app, entry).is_some();
        if starts_tool_work {
            let mut end = index;
            let mut events = Vec::new();
            while end < app.conversation.len() {
                let current = &app.conversation[end];
                match &current.kind {
                    ConversationKind::ToolCall { tool_call } => {
                        events.push(WorkEvent::Tool(tool_call));
                    }
                    ConversationKind::Reasoning { .. } => {
                        match summary_only_reasoning(app, current) {
                            Some(summary) => events.push(WorkEvent::Reasoning(summary)),
                            None => break,
                        }
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
                let work = tool_group_lines(&events, width);
                if !work.is_empty() {
                    if rendered_any && !previous_compact {
                        lines.push(Line::default());
                    }
                    lines.extend(work);
                    rendered_any = true;
                    previous_compact = true;
                }
                continue;
            }
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
                    "─".repeat(width.saturating_sub(2) as usize),
                    Style::default().fg(theme::border_dim()),
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

    Text::from(lines)
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

fn summary_only_reasoning<'a>(app: &'a App, entry: &'a ConversationEntry) -> Option<&'a str> {
    let (content, summary) = reasoning_parts(app, entry)?;
    content
        .trim()
        .is_empty()
        .then_some(summary?.trim())
        .filter(|summary| !summary.is_empty())
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
                lines.extend(prefixed_wrapped_line(Span::raw("  "), line, width));
            }
            let tool_events = tool_calls.iter().map(WorkEvent::Tool).collect::<Vec<_>>();
            lines.extend(tool_group_lines(&tool_events, width));
            lines
        }
        ConversationKind::Reasoning { content, summary } => {
            let (content, summary) =
                reasoning_parts(app, entry).unwrap_or((content.as_str(), summary.as_deref()));
            let mut lines = vec![Line::from(vec![
                Span::styled("  \u{f02d} ", Style::default().fg(theme::accent_warm())),
                Span::styled(
                    "Thinking",
                    Style::default()
                        .fg(theme::text_dim())
                        .add_modifier(Modifier::BOLD),
                ),
            ])];
            if let Some(summary) = summary {
                lines.extend(reasoning_summary_render_lines(
                    summary,
                    width,
                    "  │ • ",
                    "  │   ",
                ));
            }
            for line in session_markdown_lines(content) {
                lines.extend(prefixed_wrapped_line(
                    Span::styled("  │ ", Style::default().fg(theme::surface_color())),
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
            tool_group_lines(&[WorkEvent::Tool(tool_call)], width)
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
    let bubble_width = if width >= 90 {
        ((u32::from(width) * 58 / 100) as u16).min(72)
    } else {
        width.saturating_sub(2)
    };
    let indent = usize::from(width - bubble_width);
    let surface = theme::surface();
    let blank = || {
        Line::from(vec![
            Span::raw(" ".repeat(indent)),
            Span::styled(" ".repeat(bubble_width as usize), surface),
        ])
    };
    let mut lines = Vec::new();
    if width >= 90 {
        lines.push(blank());
    }
    for source in content.split('\n') {
        for row in prefixed_wrapped_line(
            Span::styled(" ", surface),
            Line::styled(source.to_owned(), surface),
            bubble_width.saturating_sub(1),
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
