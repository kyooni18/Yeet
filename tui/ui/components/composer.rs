//! Composer surface: a borderless prompt row (`›` marker, text, caret).
use super::super::support::theme;
pub(crate) use crate::text_layout::layout;
use crate::tui::app::{App, Mode};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    prelude::{Line, Style},
    widgets::{Block, Paragraph},
};

/// `text_x` is the column the draft starts at, so it can line up with the transcript.
pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, text_x: u16) {
    let focused = app.input_focused
        && !app.sidebar_focus
        && app.mode == Mode::Chat
        && app.state.pending_shell_permission.is_none()
        && app.state.pending_native_app_permission.is_none();
    frame.render_widget(
        Block::default().style(Style::default().fg(theme::text()).bg(if focused {
            theme::surface_color()
        } else {
            theme::background()
        })),
        area,
    );
    let portrait = super::super::support::responsive::shape(frame.area())
        == super::super::support::responsive::Shape::Portrait;
    let top = u16::from(area.height >= 2);
    let bottom = u16::from(portrait && area.height >= 3);
    let text_x = text_x
        .min(area.x + area.width / 3)
        .max(area.x.saturating_add(5));
    let inner = Rect::new(
        text_x,
        area.y.saturating_add(top),
        area.right().saturating_sub(text_x + 7),
        area.height.saturating_sub(top + bottom),
    );
    app.composer_area = (inner.x, inner.y, inner.width, inner.height);
    app.composer_width = inner.width;
    app.composer_scroll = 0;
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::styled("+", Style::default().fg(theme::secondary()))),
        Rect::new(text_x - 3, inner.y, 1, 1),
    );
    frame.render_widget(
        Paragraph::new(Line::styled("→", Style::default().fg(theme::secondary()))),
        Rect::new(area.right().saturating_sub(5), inner.y, 1, 1),
    );
    let layout = layout(&app.input, app.cursor, inner.width);
    let scroll = layout.row.saturating_sub(inner.height as usize - 1);
    app.composer_scroll = scroll;
    let lines: Vec<Line> = layout
        .lines
        .iter()
        .skip(scroll)
        .take(inner.height as usize)
        .map(|line| Line::raw(line.clone()))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
    if focused {
        frame.set_cursor_position(Position::new(
            inner.x + (layout.column as u16).min(inner.width - 1),
            inner.y + (layout.row - scroll) as u16,
        ));
    }
}

#[cfg(test)]
mod composer_viewport_tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn contents(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn long_draft_scrolls_to_follow_the_cursor() {
        let mut app = App::default();
        app.input = (1..=12)
            .map(|n| format!("draft {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.cursor = app.input.chars().count();
        let mut terminal = Terminal::new(TestBackend::new(60, 3)).unwrap();
        terminal
            .draw(|frame| draw(frame, &mut app, frame.area(), 0))
            .unwrap();
        assert!(contents(&terminal).contains("draft 12"));
        app.cursor = 0;
        terminal
            .draw(|frame| draw(frame, &mut app, frame.area(), 0))
            .unwrap();
        assert!(contents(&terminal).contains("draft 1"));
        assert!(!contents(&terminal).contains("draft 12"));
    }
}
