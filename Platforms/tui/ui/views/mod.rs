//! View-specific renderers and the top-level view switcher.

pub(super) mod agents;
pub(super) mod diff;
pub(super) mod files;
pub(super) mod home;
pub(super) mod session_picker;
pub(super) mod sessions;
use super::support::theme;
use crate::shared_ui::workbench::LAUNCHER;
use crate::tui::app::App;
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
    let floating = crate::tui::kit::FloatingView {
        width: 58,
        height: 10,
        modal: true,
    };
    let area = floating.area(bounds);
    theme::modal_backdrop(frame, area);
    let launcher = crate::shared_ui::workbench::Tab::new(
        crate::shared_ui::workbench::WorkbenchTab::Launcher, String::new(),
    );
    let block = theme::modal_block(&launcher.label);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    for (index, item) in LAUNCHER.iter().enumerate() {
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
            Span::styled(format!("{:<10}", item.label), style),
            Span::styled(item.description, style.fg(theme::muted())),
        ]);
        let row = Rect::new(inner.x, inner.y + index as u16, inner.width, 1);
        app.view_targets.push((row, item.action));
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
