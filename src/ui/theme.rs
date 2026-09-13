//! Shared terminal surfaces and modal chrome.
use std::{sync::OnceLock, time::Instant};

use ratatui::{
    Frame,
    layout::Rect,
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Padding},
};

pub(super) const BACKGROUND: Color = Color::Rgb(7, 5, 6);
pub(super) const SURFACE: Color = Color::Rgb(15, 10, 12);
pub(super) const SURFACE_RAISED: Color = Color::Rgb(25, 15, 16);
pub(super) const CODE_BACKGROUND: Color = Color::Rgb(12, 8, 9);
pub(super) const SELECTED: Color = Color::Rgb(56, 19, 17);
pub(super) const MUTED: Color = Color::Rgb(158, 130, 128);
pub(super) const BORDER: Color = Color::Rgb(101, 42, 32);
pub(super) const BORDER_DIM: Color = Color::Rgb(50, 23, 20);
pub(super) const ACCENT: Color = Color::Rgb(242, 60, 43);
pub(super) const ACCENT_HOT: Color = Color::Rgb(255, 111, 72);
pub(super) const ACCENT_WARM: Color = Color::Rgb(255, 169, 96);
pub(super) const SUCCESS: Color = Color::Rgb(121, 214, 153);
pub(super) const TEXT: Color = Color::Rgb(246, 239, 233);
pub(super) const TEXT_DIM: Color = Color::Rgb(198, 177, 171);
pub(super) const ERROR: Color = Color::Rgb(255, 94, 87);

static EPOCH: OnceLock<Instant> = OnceLock::new();

pub(super) fn base() -> Style {
    Style::default().fg(TEXT).bg(BACKGROUND)
}
pub(super) fn surface() -> Style {
    base().bg(SURFACE)
}

pub(super) fn modal_surface() -> Style {
    base().bg(SURFACE_RAISED)
}

pub(super) fn brand() -> Style {
    Style::default().fg(ACCENT_HOT).add_modifier(Modifier::BOLD)
}

pub(super) fn selected() -> Style {
    Style::default()
        .fg(TEXT)
        .bg(SELECTED)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn animation_tick() -> usize {
    (EPOCH.get_or_init(Instant::now).elapsed().as_millis() / 80) as usize
}

pub(super) fn pulse_color() -> Color {
    match animation_tick() % 20 {
        0..=1 => ACCENT_WARM,
        2..=5 => ACCENT_HOT,
        6..=12 => ACCENT,
        _ => BORDER,
    }
}

pub(super) fn modal_block(title: impl AsRef<str>) -> Block<'static> {
    let title = title.as_ref().trim();
    let (heading, hint) = title.split_once(" · ").unwrap_or((title, ""));
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .style(modal_surface())
        .border_style(Style::default().fg(ACCENT))
        .padding(Padding::horizontal(1))
        .title(
            Line::from(format!(" ◢ {} // ", heading.to_ascii_uppercase()))
                .style(Style::default().fg(ACCENT_HOT).add_modifier(Modifier::BOLD)),
        );
    if !hint.is_empty() {
        block = block.title_bottom(Line::from(vec![
            Span::styled(
                " // ",
                Style::default()
                    .fg(ACCENT_WARM)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{hint} "), Style::default().fg(MUTED)),
        ]));
    }
    block
}

pub(super) fn panel_block(title: impl AsRef<str>) -> Block<'static> {
    let title = title.as_ref().trim().to_owned();
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .style(surface())
        .border_style(Style::default().fg(BORDER_DIM))
        .padding(Padding::horizontal(1))
        .title(
            Line::from(format!("┤ {title} ├")).style(
                Style::default()
                    .fg(ACCENT_WARM)
                    .add_modifier(Modifier::BOLD),
            ),
        )
}

pub(super) fn modal_backdrop(frame: &mut Frame<'_>, area: Rect) {
    // Dim the existing screen without changing its geometry or scroll state.
    let bounds = frame.area();
    frame.buffer_mut().set_style(
        bounds,
        Style::default().fg(Color::Rgb(90, 58, 54)).bg(BACKGROUND),
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
    frame.render_widget(Block::default().style(modal_surface()), area);
}
