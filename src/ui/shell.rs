//! Responsive application shell around the conversation.
use super::{
    task::{self, TaskStatus},
    theme, truncate_end, truncate_middle,
};
use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Padding, Paragraph},
};

pub(super) fn draw_shell(frame: &mut Frame<'_>, app: &App) -> Rect {
    let bounds = frame.area();
    let body = if bounds.width >= 110 && bounds.height >= 20 {
        let columns =
            Layout::horizontal([Constraint::Length(30), Constraint::Min(1)]).split(bounds);
        sidebar(frame, app, columns[0]);
        columns[1]
    } else {
        bounds
    };
    let rows = Layout::vertical([
        Constraint::Length(if bounds.height >= 16 { 3 } else { 1 }),
        Constraint::Min(1),
    ])
    .split(body);
    let title = conversation_title(app);
    let header = if rows[0].height > 1 {
        Block::default().borders(Borders::BOTTOM)
    } else {
        Block::default()
    }
    .border_style(Style::default().fg(theme::BORDER))
    .padding(Padding::horizontal(2));
    let inner = header.inner(rows[0]);
    frame.render_widget(header, rows[0]);
    let badge = TaskStatus::for_app(app).badge(app);
    let badge_width = badge.width() as u16;
    let show_badge = inner.width >= badge_width + 16;
    let title_width = inner
        .width
        .saturating_sub(if show_badge { badge_width + 2 } else { 0 });
    frame.render_widget(
        Paragraph::new(task::fit(title, title_width as usize)).style(Style::default().bold()),
        Rect::new(inner.x, inner.y, title_width, inner.height.min(1)),
    );
    if show_badge {
        frame.render_widget(
            Paragraph::new(Line::from(badge)),
            Rect::new(inner.right() - badge_width, inner.y, badge_width, 1),
        );
    }
    if inner.height > 1 {
        let turns = app
            .conversation
            .iter()
            .filter(|entry| matches!(entry.kind, crate::model::ConversationKind::User { .. }))
            .count();
        let location = if app.follow_tail {
            "Latest messages"
        } else {
            "History · Ctrl+End for latest"
        };
        let context = format!(
            "{turns} {} · {location}",
            if turns == 1 { "turn" } else { "turns" }
        );
        frame.render_widget(
            Paragraph::new(task::fit(&context, inner.width as usize))
                .style(Style::default().fg(theme::MUTED)),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
    }
    let available = rows[1].inner(Margin {
        horizontal: u16::from(body.width >= 50) * 2,
        vertical: 0,
    });
    let width = available.width.min(108);
    Rect::new(
        available.x + (available.width - width) / 2,
        available.y,
        width,
        available.height,
    )
}

fn sidebar(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let block = Block::default()
        .style(theme::surface())
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(theme::BORDER))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = Layout::vertical([
        Constraint::Length(7),
        Constraint::Min(1),
        Constraint::Length(5),
    ])
    .split(inner);
    let workspace = std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "Workspace".into());
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                truncate_middle(&workspace, inner.width as usize),
                Style::default().bold(),
            ),
            Line::styled("Workspace", Style::default().fg(theme::MUTED)),
            Line::raw(""),
            shortcut("+ New session", "Ctrl+N", inner.width),
            shortcut("All sessions", "Alt+S", inner.width),
            Line::raw(""),
            shortcut("RECENT", &app.sessions().len().to_string(), inner.width)
                .style(Style::default().fg(theme::MUTED)),
        ]),
        rows[0],
    );
    let selected = app
        .sessions()
        .iter()
        .position(|session| Some(session.id.as_str()) == app.state.current_session_id.as_deref());
    let mut items = Vec::new();
    // Unsaved conversations still need a visible current-session indicator.
    if selected.is_none() {
        items.push(session_item(
            app,
            conversation_title(app),
            true,
            "",
            inner.width,
        ));
    }
    for session in app.sessions() {
        let current = Some(session.id.as_str()) == app.state.current_session_id.as_deref();
        items.push(session_item(
            app,
            &session.title,
            current,
            &format!("{} messages", session.message_count),
            inner.width,
        ));
    }
    let mut state = ListState::default().with_selected(Some(selected.unwrap_or(0)));
    frame.render_stateful_widget(
        List::new(items)
            .highlight_symbol("▎")
            .highlight_style(Style::default().bg(theme::SELECTED)),
        rows[1],
        &mut state,
    );
    let footer = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme::BORDER));
    frame.render_widget(
        Paragraph::new(vec![
            shortcut("Models", "Alt+M", inner.width),
            shortcut("Agent mode", "Alt+A", inner.width),
            shortcut("Settings", "/settings", inner.width),
            shortcut("Help", "?", inner.width),
        ])
        .block(footer),
        rows[2],
    );
}

fn conversation_title(app: &App) -> &str {
    app.sessions()
        .iter()
        .find(|session| Some(session.id.as_str()) == app.state.current_session_id.as_deref())
        .map(|session| session.title.as_str())
        .or_else(|| {
            app.conversation.iter().find_map(|entry| match &entry.kind {
                crate::model::ConversationKind::User { content } => Some(content.as_str()),
                _ => None,
            })
        })
        .unwrap_or("New conversation")
}

fn session_item(
    app: &App,
    title: &str,
    current: bool,
    detail: &str,
    width: u16,
) -> ListItem<'static> {
    let status = TaskStatus::for_app(app);
    let marker = if current { status.marker(app) } else { "·" };
    let color = if current {
        status.color()
    } else {
        theme::MUTED
    };
    ListItem::new(vec![
        Line::from(vec![
            Span::styled(format!("{marker} "), Style::default().fg(color)),
            Span::styled(
                task::fit(title, width.saturating_sub(3) as usize),
                if current {
                    Style::default().bold()
                } else {
                    Style::default()
                },
            ),
        ]),
        Line::styled(
            format!(
                "  {}",
                task::fit(
                    &if current {
                        format!("Current · {}", status.label())
                    } else {
                        detail.to_owned()
                    },
                    width.saturating_sub(3) as usize
                )
            ),
            Style::default().fg(if current { color } else { theme::MUTED }),
        ),
    ])
}

fn shortcut(label: &str, key: &str, width: u16) -> Line<'static> {
    let key_width = Span::raw(key).width();
    let label = truncate_end(label, (width as usize).saturating_sub(key_width + 1));
    let gap = (width as usize).saturating_sub(Span::raw(&label).width() + key_width);
    Line::from(vec![
        Span::raw(label),
        Span::raw(" ".repeat(gap)),
        Span::styled(key.to_owned(), Style::default().fg(theme::MUTED)),
    ])
}

pub(super) fn draw_welcome(frame: &mut Frame<'_>, area: Rect) {
    if area.height < 6 {
        return;
    }
    let height = 7.min(area.height);
    let center = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(height) / 2,
        area.width,
        height,
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled("◆", Style::default().fg(theme::ACCENT).bold()),
            Line::raw(""),
            Line::styled("What would you like to build?", Style::default().bold()),
            Line::styled(
                "Ask a question or describe a change.",
                Style::default().fg(theme::MUTED),
            ),
            Line::raw(""),
            Line::styled(
                "Alt+M Models   ·   Alt+S Sessions   ·   / Commands",
                Style::default().fg(theme::MUTED),
            ),
        ])
        .alignment(Alignment::Center),
        center,
    );
}
