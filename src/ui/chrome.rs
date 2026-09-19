//! Minimal terminal-native framing layered around the conversation surface.
//!
//! The shell already carries navigation and task state, so chrome stays static
//! and intentionally quiet.  This avoids repaint-only animation competing with
//! transcript readability or input responsiveness.

use super::{responsive, theme};
use crate::app::{App, Mode};
use ratatui::{Frame, prelude::Color};

const MIN_GUIDE_GUTTER: u16 = 2;

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
    let left_gutter = left.saturating_sub(body_x);
    let right_gutter = area.right().saturating_sub(right.saturating_add(1));
    if left <= area.x
        || right >= area.right()
        || left_gutter < MIN_GUIDE_GUTTER
        || right_gutter < MIN_GUIDE_GUTTER
    {
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
        paint(buffer, left, y, "│", theme::border_dim());
        paint(buffer, right, y, "│", theme::border_dim());
    }
}

fn paint(buffer: &mut ratatui::buffer::Buffer, x: u16, y: u16, symbol: &str, color: Color) {
    buffer[(x, y)].set_symbol(symbol).set_fg(color);
}
