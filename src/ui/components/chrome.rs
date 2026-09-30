//! Minimal terminal-native framing layered around the conversation surface.
//!
//! The shell already carries navigation and task state, so chrome stays static
//! and intentionally quiet.  This avoids repaint-only animation competing with
//! transcript readability or input responsiveness.

use super::super::{responsive, theme};
use crate::app::{App, Mode};
use ratatui::{Frame, prelude::Color};

const MIN_GUIDE_GUTTER: u16 = 2;

pub(crate) fn draw(frame: &mut Frame<'_>, app: &App) {
    if app.mode != Mode::Chat
        || app.state.pending_shell_permission.is_some()
        || app.state.pending_native_app_permission.is_some()
    {
        return;
    }
    if !app.conversation.is_empty() {
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

    draw_content_guides(frame.buffer_mut(), area, adaptive, app.transcript_area);
}

fn draw_content_guides(
    buffer: &mut ratatui::buffer::Buffer,
    area: ratatui::layout::Rect,
    adaptive: responsive::Metrics,
    transcript_area: (u16, u16, u16, u16),
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

    let (_, transcript_y, _, transcript_height) = transcript_area;
    if transcript_height == 0 {
        return;
    }
    let top = transcript_y;
    let bottom = transcript_y
        .saturating_add(transcript_height)
        .saturating_sub(1)
        .min(area.bottom().saturating_sub(1));
    if bottom <= top {
        return;
    }
    // Treat the empty conversation as a gallery stage rather than a box.
    // Warm leading marks and cool trailing marks give the frame a direction,
    // while sparse registration ticks establish depth without enclosing content.
    paint(buffer, left, top, "╭", theme::accent());
    paint(
        buffer,
        left.saturating_add(1),
        top,
        "─",
        theme::border_dim(),
    );
    paint(
        buffer,
        right.saturating_sub(1),
        top,
        "─",
        theme::border_dim(),
    );
    paint(buffer, right, top, "╮", theme::accent_hot());

    let center = content_x.saturating_add(content_width / 2);
    if center > left.saturating_add(2) && center < right.saturating_sub(2) {
        paint(buffer, center, top, "·", theme::muted());
    }

    let span = bottom.saturating_sub(top);
    if span >= 10 {
        let middle = top.saturating_add(span / 2);
        paint(buffer, left, middle, "╴", theme::border_dim());
        paint(buffer, right, middle, "╶", theme::border_dim());

        let upper = top.saturating_add(span / 3);
        let lower = top.saturating_add(span.saturating_mul(2) / 3);
        paint(buffer, left, upper, "·", theme::accent());
        paint(buffer, right, lower, "·", theme::accent_hot());
    }

    paint(buffer, left, bottom, "╰", theme::accent());
    paint(
        buffer,
        left.saturating_add(1),
        bottom,
        "─",
        theme::border_dim(),
    );
    paint(
        buffer,
        right.saturating_sub(1),
        bottom,
        "─",
        theme::border_dim(),
    );
    paint(buffer, right, bottom, "╯", theme::accent());
    if center > left.saturating_add(2) && center < right.saturating_sub(2) {
        paint(buffer, center, bottom, "◇", theme::border());
    }
}

fn paint(buffer: &mut ratatui::buffer::Buffer, x: u16, y: u16, symbol: &str, color: Color) {
    buffer[(x, y)].set_symbol(symbol).set_fg(color);
}
