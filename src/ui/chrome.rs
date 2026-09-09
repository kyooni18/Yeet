//! Ambient YEET frame rails layered over the main chat surface.
//!
//! These are intentionally terminal-native rather than image effects.  The
//! moving nodes make the entire app feel electrically alive while leaving the
//! transcript and dialogs readable.

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
    if matches!(
        adaptive.shape,
        responsive::Shape::Portrait | responsive::Shape::Tiny
    ) {
        return;
    }

    let tick = theme::animation_tick();
    let left = area.x;
    let right = area.right().saturating_sub(1);
    let top = area.y + 1;
    let bottom = area.bottom().saturating_sub(2);
    let travel = bottom.saturating_sub(top).max(1);
    let hot_left = top + ((tick as u16 / 2) % travel);
    let hot_right = bottom.saturating_sub((tick as u16 / 3) % travel);
    let buffer = frame.buffer_mut();

    for y in top..=bottom {
        let phase = ((y - top) as usize + tick / 7) % 4;
        let rail = match phase {
            0 => "╎",
            1 => "┊",
            2 => "╏",
            _ => "┊",
        };
        paint(buffer, left, y, rail, theme::BORDER);
        paint(buffer, right, y, rail, theme::BORDER);
    }

    paint(buffer, left, hot_left, "◆", theme::ACCENT_HOT);
    paint(buffer, right, hot_right, "◆", theme::ACCENT_HOT);

    let upper = if tick % 12 < 6 { "╥" } else { "╫" };
    let lower = if tick % 12 < 6 { "╨" } else { "╫" };
    paint(buffer, left, area.y, upper, theme::ACCENT);
    paint(buffer, right, area.y, upper, theme::ACCENT);
    paint(
        buffer,
        left,
        area.bottom().saturating_sub(1),
        lower,
        theme::ACCENT,
    );
    paint(
        buffer,
        right,
        area.bottom().saturating_sub(1),
        lower,
        theme::ACCENT,
    );

    if matches!(
        adaptive.shape,
        responsive::Shape::Wide | responsive::Shape::UltraWide
    ) {
        draw_inner_bus(buffer, area, adaptive, tick);
    }
}

fn draw_inner_bus(
    buffer: &mut ratatui::buffer::Buffer,
    area: ratatui::layout::Rect,
    adaptive: responsive::Metrics,
    tick: usize,
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
    let travel = bottom.saturating_sub(top).max(1);
    let hot = top + ((tick as u16 / 4) % travel);
    for y in top..bottom {
        let symbol = if (usize::from(y - top) + tick / 8) % 5 == 0 {
            "┇"
        } else {
            "╎"
        };
        paint(buffer, left, y, symbol, theme::BORDER_DIM);
        paint(buffer, right, y, symbol, theme::BORDER_DIM);
    }
    paint(buffer, left, hot, "◈", theme::ACCENT);
    paint(buffer, right, hot, "◈", theme::ACCENT);
}

fn paint(buffer: &mut ratatui::buffer::Buffer, x: u16, y: u16, symbol: &str, color: Color) {
    buffer[(x, y)].set_symbol(symbol).set_fg(color);
}
