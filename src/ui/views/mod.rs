//! View-specific renderers and the top-level view switcher.

pub(super) mod files;
pub(super) mod home;
pub(super) mod session_picker;
pub(super) mod sessions;
use super::support::theme;
use crate::app::App;
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Modifier, Span, Style},
    widgets::Paragraph,
};

pub(super) fn draw(frame: &mut Frame<'_>, app: &App) {
    let bounds = frame.area();
    if bounds.width < 16 || bounds.height < 5 {
        return;
    }
    let width = bounds.width.min(42);
    let height = bounds.height.min(6);
    let area = Rect::new(
        bounds.x + (bounds.width - width) / 2,
        bounds.y + (bounds.height - height) / 2,
        width,
        height,
    );
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block("Views");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    for (index, name) in ["Sessions", "Files"].into_iter().enumerate() {
        if index as u16 >= inner.height {
            break;
        }
        let selected = app.views_index == index;
        let style = if selected {
            Style::default()
                .fg(theme::text())
                .bg(theme::selected_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::text_dim())
        };
        let line = Line::from(vec![
            Span::styled(if selected { " › " } else { "   " }, style),
            Span::styled(name, style),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(inner.x, inner.y + index as u16, inner.width, 1),
        );
    }
    if inner.height >= 3 {
        frame.render_widget(
            Paragraph::new(" ↑↓/jk select  Enter/Space open  Esc")
                .style(Style::default().fg(theme::muted())),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}
