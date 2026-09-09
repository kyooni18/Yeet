//! Shared terminal surfaces and modal chrome.
use std::{sync::OnceLock, time::Instant};

use ratatui::{
    Frame,
    layout::Rect,
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Padding},
};

pub(super) const BACKGROUND: Color = Color::Rgb(8, 3, 3);
pub(super) const SURFACE: Color = Color::Rgb(18, 7, 8);
pub(super) const CODE_BACKGROUND: Color = Color::Rgb(27, 8, 10);
pub(super) const SELECTED: Color = Color::Rgb(72, 11, 15);
pub(super) const MUTED: Color = Color::Rgb(156, 126, 130);
pub(super) const BORDER: Color = Color::Rgb(104, 15, 20);
pub(super) const BORDER_DIM: Color = Color::Rgb(58, 8, 11);
pub(super) const ACCENT: Color = Color::Rgb(218, 22, 27);
pub(super) const ACCENT_HOT: Color = Color::Rgb(255, 74, 65);
pub(super) const TEXT: Color = Color::Rgb(241, 236, 231);
pub(super) const TEXT_DIM: Color = Color::Rgb(188, 171, 171);
pub(super) const ERROR: Color = Color::Rgb(255, 74, 65);

static EPOCH: OnceLock<Instant> = OnceLock::new();

pub(super) fn base() -> Style {
    Style::default().fg(TEXT).bg(BACKGROUND)
}
pub(super) fn surface() -> Style {
    base().bg(SURFACE)
}

pub(super) fn brand() -> Style {
    Style::default()
        .fg(ACCENT_HOT)
        .bg(BACKGROUND)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn animation_tick() -> usize {
    (EPOCH.get_or_init(Instant::now).elapsed().as_millis() / 80) as usize
}

pub(super) fn pulse_color() -> Color {
    match animation_tick() % 18 {
        0..=2 => ACCENT_HOT,
        3..=8 => ACCENT,
        _ => BORDER,
    }
}

pub(super) fn modal_block(title: impl AsRef<str>) -> Block<'static> {
    let title = title.as_ref().trim();
    let (heading, hint) = title.split_once(" · ").unwrap_or((title, ""));
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .style(surface())
        .border_style(Style::default().fg(pulse_color()))
        .padding(Padding::horizontal(1))
        .title(Line::from(format!(" {heading} ")).style(brand()))
        .title_bottom(Line::from(format!(" {hint} ")).style(Style::default().fg(MUTED)))
}

pub(super) fn modal_backdrop(frame: &mut Frame<'_>, area: Rect) {
    // Dim the existing screen without changing its geometry or scroll state.
    let bounds = frame.area();
    frame.buffer_mut().set_style(
        bounds,
        Style::default().fg(Color::Rgb(85, 49, 52)).bg(BACKGROUND),
    );
    let shadow = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width,
        area.height,
    )
    .intersection(bounds);
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(3, 1, 1))),
        shadow,
    );
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(surface()), area);
}
