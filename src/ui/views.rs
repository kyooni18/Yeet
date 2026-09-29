//! Small switcher for the two top-level TUI views.
use super::theme;
use crate::app::App;
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Modifier, Span, Style},
    widgets::Paragraph,
};

pub(super) fn draw(frame: &mut Frame<'_>, app: &App) {
    let bounds = frame.area();
    if bounds.width < 12 || bounds.height < 6 {
        return;
    }
    let width = bounds.width.min(52);
    let height = bounds.height.min(8);
    let area = Rect::new(
        bounds.x + (bounds.width - width) / 2,
        bounds.y + (bounds.height - height) / 2,
        width,
        height,
    );
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block(" Views · Enter new tab · Space current tab · Esc close ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    for (index, (name, description)) in
        [("Sessions", "Conversation"), ("Files", "Workspace browser")]
            .into_iter()
            .enumerate()
    {
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
            Span::styled(format!("{name:<12}"), style),
            Span::styled(description, style),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(inner.x, inner.y + index as u16, inner.width, 1),
        );
    }
    if inner.height > 3 {
        frame.render_widget(
            Paragraph::new("  ↑/k  ↓/j  ←/h back  →/l open")
                .style(Style::default().fg(theme::muted())),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}
