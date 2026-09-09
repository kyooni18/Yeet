//! Responsive application shell around the conversation.
use super::{
    responsive,
    task::{self, TaskStatus},
    theme, truncate_end, truncate_middle, yeet_brand,
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
    let adaptive = responsive::metrics(bounds);
    let body = if let Some(sidebar_width) = adaptive.sidebar_width {
        let columns = Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(1)])
            .split(bounds);
        sidebar(frame, app, columns[0]);
        columns[1]
    } else {
        bounds
    };
    let rows = Layout::vertical([
        Constraint::Length(adaptive.header_height),
        Constraint::Min(1),
    ])
    .split(body);
    let title = conversation_title(app);
    let header = if rows[0].height > 1 {
        Block::default().borders(Borders::BOTTOM)
    } else {
        Block::default()
    }
    .border_style(Style::default().fg(theme::pulse_color()))
    .padding(Padding::horizontal(2));
    let inner = header.inner(rows[0]);
    frame.render_widget(header, rows[0]);
    let badge = TaskStatus::for_app(app).badge(app);
    let badge_width = badge.width() as u16;
    let show_badge = inner.width >= badge_width + 16;
    let title_width = inner
        .width
        .saturating_sub(if show_badge { badge_width + 2 } else { 0 });
    let branded_title = if title == "New conversation" {
        "YEET // NEW DIRECTIVE".to_owned()
    } else {
        format!("YEET // {title}")
    };
    frame.render_widget(
        Paragraph::new(task::fit(&branded_title, title_width as usize)).style(theme::brand()),
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
        let location = if app.follow_tail || app.conversation.is_empty() {
            "Latest messages"
        } else {
            "History · Ctrl+End for latest"
        };
        let context = format!(
            "{turns} {} · {location}",
            if turns == 1 { "turn" } else { "turns" }
        );
        let core = if app.state.is_streaming {
            "◆ CORE//EXECUTING"
        } else {
            "◇ CORE//STANDBY"
        };
        let core_width = Span::raw(core).width() as u16;
        let show_core = inner.width >= core_width + 28;
        let context_width = inner
            .width
            .saturating_sub(if show_core { core_width + 2 } else { 0 });
        frame.render_widget(
            Paragraph::new(task::fit(&context, context_width as usize))
                .style(Style::default().fg(theme::MUTED)),
            Rect::new(inner.x, inner.y + 1, context_width, 1),
        );
        if show_core {
            frame.render_widget(
                Paragraph::new(core).style(Style::default().fg(theme::pulse_color()).bold()),
                Rect::new(inner.right() - core_width, inner.y + 1, core_width, 1),
            );
        }
    }
    let available = rows[1].inner(Margin {
        horizontal: adaptive.horizontal_margin.min(rows[1].width / 2),
        vertical: 0,
    });
    let width = available.width.min(adaptive.content_max_width);
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
        Constraint::Length(9),
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
            Line::from(vec![
                Span::styled("◆ ", Style::default().fg(theme::pulse_color())),
                Span::styled("YEET", theme::brand()),
                Span::styled(" // NODE", Style::default().fg(theme::MUTED)),
            ]),
            Line::styled(
                "AGENTIC EXECUTION TERMINAL",
                Style::default().fg(theme::MUTED),
            ),
            signal_line(inner.width),
            Line::styled(
                truncate_middle(&workspace, inner.width as usize),
                Style::default().fg(theme::TEXT).bold(),
            ),
            Line::raw(""),
            shortcut("+ New session", "Ctrl+N", inner.width),
            shortcut("All sessions", "Alt+S", inner.width),
            Line::styled("CORE LINK // ONLINE", Style::default().fg(theme::ACCENT)),
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

fn signal_line(width: u16) -> Line<'static> {
    let usable = width.saturating_sub(2).clamp(6, 20) as usize;
    let tick = theme::animation_tick();
    let hot = (tick / 2) % usable;
    let mut spans = Vec::with_capacity(usable + 2);
    spans.push(Span::styled("╟", Style::default().fg(theme::BORDER)));
    for index in 0..usable {
        let (symbol, color) = if index == hot {
            ("◆", theme::ACCENT_HOT)
        } else if index.abs_diff(hot) == 1 {
            ("━", theme::ACCENT)
        } else {
            ("─", theme::BORDER)
        };
        spans.push(Span::styled(symbol, Style::default().fg(color)));
    }
    spans.push(Span::styled("╢", Style::default().fg(theme::BORDER)));
    Line::from(spans)
}

pub(super) fn draw_welcome(frame: &mut Frame<'_>, area: Rect, viewport_shape: responsive::Shape) {
    yeet_brand::draw(frame, area, viewport_shape);
}
