//! Shared terminal surfaces and modal chrome.
use ratatui::{
    Frame,
    layout::Rect,
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Padding},
};

pub(super) const BACKGROUND: Color = Color::Rgb(16, 19, 26);
pub(super) const SURFACE: Color = Color::Rgb(25, 30, 40);
pub(super) const SELECTED: Color = Color::Rgb(43, 62, 82);
pub(super) const MUTED: Color = Color::Rgb(145, 157, 176);
pub(super) const BORDER: Color = Color::Rgb(66, 80, 101);
pub(super) const ACCENT: Color = Color::Rgb(128, 205, 218);

pub(super) fn base() -> Style {
    Style::default()
        .fg(Color::Rgb(224, 230, 239))
        .bg(BACKGROUND)
}
pub(super) fn surface() -> Style {
    base().bg(SURFACE)
}

pub(super) fn modal_block(title: impl AsRef<str>) -> Block<'static> {
    let title = title.as_ref().trim();
    let (heading, hint) = title.split_once(" · ").unwrap_or((title, ""));
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(surface())
        .border_style(Style::default().fg(BORDER))
        .padding(Padding::horizontal(1))
        .title(Line::from(format!(" {heading} ")).style(Style::default().fg(ACCENT).bold()))
        .title_bottom(Line::from(format!(" {hint} ")).style(Style::default().fg(MUTED)))
}

pub(super) fn modal_backdrop(frame: &mut Frame<'_>, area: Rect) {
    // Dim the existing screen without changing its geometry or scroll state.
    let bounds = frame.area();
    frame.buffer_mut().set_style(
        bounds,
        Style::default().fg(Color::Rgb(76, 86, 103)).bg(BACKGROUND),
    );
    let shadow = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width,
        area.height,
    )
    .intersection(bounds);
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(7, 10, 15))),
        shadow,
    );
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(surface()), area);
}
