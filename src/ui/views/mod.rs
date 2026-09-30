//! View-specific renderers and the top-level view switcher.

pub(super) mod files;
pub(super) mod home;
pub(super) mod session_picker;
pub(super) mod sessions;
use super::support::theme;
use crate::app::{App, WorkbenchTab};
use ratatui::{
    Frame,
    layout::Rect,
    prelude::{Line, Modifier, Span, Style},
    widgets::Paragraph,
};

pub(super) fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let bounds = frame.area();
    if bounds.width < 16 || bounds.height < 5 {
        return;
    }
    let width = bounds.width.min(58);
    let height = bounds.height.min(9);
    let area = Rect::new(
        bounds.x + (bounds.width - width) / 2,
        bounds.y + (bounds.height - height) / 2,
        width,
        height,
    );
    theme::modal_backdrop(frame, area);
    let block = theme::modal_block("Open a view");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    for (index, (target, name, description)) in [
        (WorkbenchTab::Home, "Home", "Workspace overview"),
        (WorkbenchTab::Session, "Session", "Current conversation"),
        (WorkbenchTab::Files, "Files", "Browse workspace files"),
    ]
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
            Span::styled(format!("{name:<10}"), style),
            Span::styled(description, style.fg(theme::muted())),
        ]);
        let row = Rect::new(inner.x, inner.y + index as u16, inner.width, 1);
        app.view_targets.push((row, target));
        frame.render_widget(Paragraph::new(line).style(style), row);
    }
    if inner.height >= 5 {
        frame.render_widget(
            Paragraph::new(" Ctrl+O views · Ctrl+Tab switch · × close")
                .style(Style::default().fg(theme::muted())),
            Rect::new(inner.x, inner.bottom() - 2, inner.width, 1),
        );
    }
    if inner.height >= 4 {
        frame.render_widget(
            Paragraph::new(" ↑↓/jk select  Enter open  Esc back")
                .style(Style::default().fg(theme::muted())),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}
