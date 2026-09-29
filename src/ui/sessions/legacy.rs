//! The pre-workbench transcript presentation retained for narrow terminals.
use super::super::markdown::markdown_lines;
use super::super::text::prefixed_wrapped_line;
use super::{
    App, capture_transcript_cells, draw_selection, responsive, shell, theme,
    tools::{self, WorkEvent, reasoning_summary_render_lines},
};
use crate::model::{ConversationEntry, ConversationKind};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    prelude::{Line, Modifier, Span, Style, Text},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};

pub(super) fn draw(
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
        app.transcript_cells.clear();
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

fn transcript_text(app: &App, width: u16) -> Text<'static> {
    let mut lines = Vec::new();
    let mut rendered_any = false;
    let mut previous_compact = false;
    let latest_turn_start = app
        .conversation
        .iter()
        .rposition(|entry| matches!(entry.kind, ConversationKind::User { .. }))
        .unwrap_or(0);
    let mut index = 0;

    while index < app.conversation.len() {
        let entry = &app.conversation[index];
        if matches!(
            &entry.kind,
            ConversationKind::Activity { activity } if !super::visible_terminal_activity(activity)
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
                    ConversationKind::Activity { activity }
                        if !super::terminal_activity(activity) => {}
                    _ => break,
                }
                end += 1;
            }

            if events
                .iter()
                .any(|event| matches!(event, WorkEvent::Tool(_)))
            {
                index = end;
                lines.extend(tools::tool_group_lines_legacy(
                    &events,
                    width,
                    app.state.is_streaming && start >= latest_turn_start,
                ));
                rendered_any = true;
                previous_compact = true;
                continue;
            }
        }

        let compact = super::compact_transcript_entry(&entry.kind);
        if rendered_any && (!compact || !previous_compact) {
            lines.push(Line::default());
        }
        lines.extend(entry_lines(app, entry, width));
        rendered_any = true;
        previous_compact = compact;
        index += 1;
    }

    Text::from(lines)
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
                vec![Line::styled("◆ Yeet", theme::brand())]
            };
            for line in markdown_lines(content) {
                lines.extend(prefixed_wrapped_line(
                    Span::styled("  │ ", Style::default().fg(theme::border_dim())),
                    line,
                    width,
                ));
            }
            let events = tool_calls.iter().map(WorkEvent::Tool).collect::<Vec<_>>();
            lines.extend(tools::tool_group_lines_legacy(
                &events,
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
                Span::styled("  ◇ ", Style::default().fg(theme::accent_warm())),
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
            for line in markdown_lines(content) {
                lines.extend(prefixed_wrapped_line(
                    Span::styled("  │ ", Style::default().fg(theme::border_dim())),
                    line.style(Style::default().fg(theme::muted())),
                    width,
                ));
            }
            lines
        }
        _ => super::entry_lines(app, entry, width),
    }
}
