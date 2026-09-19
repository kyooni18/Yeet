//! Composer surface, character wrapping, and cursor placement.
use super::theme;
use crate::app::{App, Mode};
pub(super) use crate::text_layout::layout;
use ratatui::{
    Frame,
    layout::{Position, Rect},
    prelude::{Line, Modifier, Style},
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
    let border_style = Style::default().fg(if focused { accent } else { theme::border() });
    let title = if app.state.is_streaming {
        " Draft while running "
    } else {
        " Message "
    };
    let hint = if app.state.is_streaming {
        if area.width < 34 {
            " Esc stop "
        } else if area.width < 60 {
            " Esc stop · Keep drafting "
        } else if area.width < 104 {
            " Esc stop · Enter after reply · Shift+Enter newline "
        } else {
            " Esc stop  ·  Keep drafting  ·  Enter after reply  ·  Shift+Enter newline  ·  Alt+↑/↓ history "
        }
    } else if area.width < 34 {
        " Enter send "
    } else if area.width < 60 {
        " Enter send · / commands "
    } else if area.width < 104 {
        " Enter send · Shift+Enter newline · / commands "
    } else {
        " Enter send  ·  Shift+Enter newline  ·  Alt+↑/↓ history  ·  / commands "
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .style(theme::surface())
        .padding(ratatui::widgets::Padding::horizontal(1))
        .title(Line::styled(
            title,
            Style::default()
                .fg(theme::background())
                .bg(accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(hint).style(
                Style::default()
                    .fg(theme::muted())
                    .bg(theme::surface_color()),
            ),
        );
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
        vec![Line::styled(
            if app.state.is_streaming {
                "Draft your next message…"
            } else {
                "Ask anything, or / for commands…"
            },
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
