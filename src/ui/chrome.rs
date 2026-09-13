//! Minimal terminal-native framing layered around the conversation surface.
//!
//! The shell already carries navigation and task state, so chrome stays static
//! and intentionally quiet.  This avoids repaint-only animation competing with
//! transcript readability or input responsiveness.

use super::{responsive, theme};
use crate::app::{App, Mode};
use ratatui::{Frame, prelude::Color};

pub(super) fn draw(frame: &mut Frame<'_>, app: &App) {
    if app.mode != Mode::Chat
        || app.state.pending_shell_permission.is_some()
        || app.state.pending_native_app_permission.is_some()
    {
        return;
    }

    let area = frame.area();
    if area.width < 8 || area.height < 8 {
        return;
    }
    let adaptive = responsive::metrics(area);
    if !matches!(
        adaptive.shape,
        responsive::Shape::Wide | responsive::Shape::UltraWide
    ) {
        return;
    }

    draw_content_guides(frame.buffer_mut(), area, adaptive);
}

fn draw_content_guides(
    buffer: &mut ratatui::buffer::Buffer,
    area: ratatui::layout::Rect,
    adaptive: responsive::Metrics,
) {
    let sidebar = adaptive.sidebar_width.unwrap_or(0);
    let body_x = area.x.saturating_add(sidebar);
    let body_width = area.width.saturating_sub(sidebar);
    let margin = adaptive.horizontal_margin.min(body_width / 2);
    let available_x = body_x.saturating_add(margin);
    let available_width = body_width.saturating_sub(margin.saturating_mul(2));
    let content_width = available_width.min(adaptive.content_max_width);
    let content_x = available_x.saturating_add(available_width.saturating_sub(content_width) / 2);

    let Some(left) = content_x.checked_sub(2) else {
        return;
    };
    let right = content_x.saturating_add(content_width).saturating_add(1);
    if left <= area.x || right >= area.right() {
        return;
    }

    let top = area
        .y
        .saturating_add(adaptive.header_height)
        .saturating_add(1);
    let bottom = area.bottom().saturating_sub(adaptive.status_height + 1);
    if bottom <= top {
        return;
    }
    for y in top..bottom {
        paint(buffer, left, y, "│", theme::BORDER_DIM);
        paint(buffer, right, y, "│", theme::BORDER_DIM);
    }
}

fn paint(buffer: &mut ratatui::buffer::Buffer, x: u16, y: u16, symbol: &str, color: Color) {
    buffer[(x, y)].set_symbol(symbol).set_fg(color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{buffer::Buffer, layout::Rect};

    #[test]
    fn content_guides_are_static_and_stay_outside_content() {
        let area = Rect::new(0, 0, 160, 30);
        let metrics = responsive::metrics(area);
        let mut buffer = Buffer::empty(area);

        draw_content_guides(&mut buffer, area, metrics);

        let sidebar = metrics.sidebar_width.unwrap_or(0);
        let body_width = area.width - sidebar;
        let available_width = body_width - metrics.horizontal_margin * 2;
        let content_width = available_width.min(metrics.content_max_width);
        let content_x = sidebar + metrics.horizontal_margin + (available_width - content_width) / 2;
        let left = content_x - 2;
        let right = content_x + content_width + 1;
        let y = metrics.header_height + 1;

        assert_eq!(buffer[(left, y)].symbol(), "│");
        assert_eq!(buffer[(right, y)].symbol(), "│");
        assert_eq!(buffer[(content_x, y)].symbol(), " ");
    }
}
