//! Composer surface, character wrapping, and cursor placement.
use super::{task, theme};
use crate::app::{App, Mode};
pub(super) use crate::text_layout::layout;
use ratatui::{
    Frame,
    layout::{Position, Rect},
    prelude::{Line, Modifier, Span, Style},
    widgets::{Block, BorderType, Borders, Paragraph},
};

pub(super) fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let focused = app.mode == Mode::Chat
        && app.state.pending_shell_permission.is_none()
        && app.state.pending_native_app_permission.is_none();
    let accent = if app.state.is_streaming {
        theme::accent_warm()
    } else {
        theme::accent()
    };
    let border_color = if !focused {
        theme::border_dim()
    } else if app.state.is_streaming {
        theme::accent_warm()
    } else {
        theme::border()
    };
    let border_style = Style::default().fg(border_color);
    let title = if app.state.is_streaming {
        " ◌ Draft while running "
    } else {
        " › Message "
    };
    let hint = composer_hint(app.state.is_streaming, area.width);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .style(Style::default().fg(theme::text()).bg(if focused {
            theme::surface_raised()
        } else {
            theme::surface_color()
        }))
        .padding(ratatui::widgets::Padding::horizontal(1))
        .title(Line::styled(
            title,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(hint);
    let inner = block.inner(area);
    app.composer_area = (inner.x, inner.y, inner.width, inner.height);
    app.composer_width = inner.width;
    app.composer_scroll = 0;
    if inner.width == 0 || inner.height == 0 {
        frame.render_widget(block, area);
        return;
    }
    let layout = layout(&app.input, app.cursor, inner.width);
    let scroll = layout.row.saturating_sub(inner.height as usize - 1);
    app.composer_scroll = scroll;
    let draft_meta = if layout.lines.len() > inner.height as usize && area.width >= 34 {
        Some(format!(
            " Lines {}–{} of {} ",
            scroll + 1,
            (scroll + inner.height as usize).min(layout.lines.len()),
            layout.lines.len()
        ))
    } else if area.width >= 44 && layout.lines.len() > 1 {
        Some(format!(
            " {} lines · {} chars ",
            layout.lines.len(),
            app.input.chars().count()
        ))
    } else if area.width >= 44 && app.input.chars().count() >= 80 {
        Some(format!(" {} chars ", app.input.chars().count()))
    } else {
        None
    };
    if let Some(meta) = draft_meta {
        block = block.title(
            Line::from(meta)
                .right_aligned()
                .style(Style::default().fg(theme::muted())),
        );
    }
    frame.render_widget(block, area);
    let lines = if app.input.is_empty() {
        let placeholder = if app.state.is_streaming {
            "Draft your next message…"
        } else {
            "Write a message, or / for commands…"
        };
        vec![Line::styled(
            task::fit(placeholder, inner.width as usize),
            Style::default()
                .fg(theme::muted())
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
    if focused {
        frame.set_cursor_position(Position::new(
            inner.x + (layout.column as u16).min(inner.width - 1),
            inner.y + (layout.row - scroll) as u16,
        ));
    }
}

fn composer_hint(streaming: bool, width: u16) -> Line<'static> {
    let items: &[(&str, &str)] = if streaming {
        if width < 34 {
            &[("Esc", "stop")]
        } else if width < 60 {
            &[("Esc", "stop"), ("Enter", "queue")]
        } else if width < 104 {
            &[
                ("Esc", "stop"),
                ("Enter", "queue"),
                ("Shift+Enter", "newline"),
            ]
        } else {
            &[
                ("Esc", "stop"),
                ("Enter", "queue"),
                ("Shift+Enter", "newline"),
                ("Alt+↑/↓", "history"),
            ]
        }
    } else if width < 34 {
        &[("Enter", "send")]
    } else if width < 60 {
        &[("Enter", "send"), ("/", "commands")]
    } else if width < 104 {
        &[
            ("Enter", "send"),
            ("Shift+Enter", "newline"),
            ("/", "commands"),
        ]
    } else {
        &[
            ("Enter", "send"),
            ("Shift+Enter", "newline"),
            ("Alt+↑/↓", "history"),
            ("/", "commands"),
        ]
    };
    let key_color = if streaming {
        theme::accent_warm()
    } else {
        theme::accent()
    };
    let mut spans = vec![Span::raw(" ")];
    for (index, (key, action)) in items.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ·  ", Style::default().fg(theme::border())));
        }
        spans.push(Span::styled(
            (*key).to_owned(),
            Style::default().fg(key_color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {action}"),
            Style::default().fg(theme::muted()),
        ));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
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
            .draw(|frame| draw(frame, &mut app, frame.area()))
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
            .draw(|frame| draw(frame, &mut app, frame.area()))
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
            .draw(|frame| draw(frame, &mut app, frame.area()))
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
