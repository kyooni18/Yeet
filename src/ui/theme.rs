//! Ratatui adapter for the shared Yeet theme engine.

use std::sync::{OnceLock, RwLock};

use ratatui::{
    Frame,
    layout::Rect,
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Padding},
};

pub(super) use crate::theme::Appearance;
use crate::theme::{Palette, Rgb};

static ACTIVE: OnceLock<RwLock<Palette>> = OnceLock::new();

fn active() -> Palette {
    let lock = ACTIVE.get_or_init(|| RwLock::new(load_palette()));
    *lock.read().expect("theme lock poisoned")
}

pub(super) fn initialize(appearance: Appearance, name_or_path: Option<&str>) {
    let palette = crate::theme::resolve_palette(appearance, name_or_path).palette;
    let lock = ACTIVE.get_or_init(|| RwLock::new(palette));
    *lock.write().expect("theme lock poisoned") = palette;
}

fn load_palette() -> Palette {
    let appearance = match std::env::var("YEET_THEME_MODE").ok().as_deref() {
        Some(value) if value.eq_ignore_ascii_case("light") => Appearance::Light,
        _ => Appearance::Dark,
    };
    let settings = crate::config::ConfigStore::default().theme_settings().ok();
    let name = match appearance {
        Appearance::Dark => settings
            .as_ref()
            .and_then(|settings| settings.dark.as_deref()),
        Appearance::Light => settings
            .as_ref()
            .and_then(|settings| settings.light.as_deref()),
    };
    crate::theme::resolve_palette(appearance, name).palette
}

fn color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.red, rgb.green, rgb.blue)
}

pub(super) fn background() -> Color {
    color(active().background)
}
pub(super) fn surface_color() -> Color {
    color(active().surface)
}
pub(super) fn surface_raised() -> Color {
    color(active().surface_raised)
}
pub(super) fn code_background() -> Color {
    color(active().code_background)
}
pub(super) fn selected_color() -> Color {
    let selected = color(active().selected);
    if selected == surface_raised() || selected == surface_color() {
        // Some palettes intentionally reuse their visual/raised surface for
        // selection. In a layered TUI that erases the selected row entirely,
        // so tint the raised surface with the accent for a visible highlight.
        color(active().surface_raised.mix(active().accent, 0.22))
    } else {
        selected
    }
}
pub(super) fn muted() -> Color {
    color(active().muted)
}
pub(super) fn border() -> Color {
    color(active().border)
}
pub(super) fn border_dim() -> Color {
    color(active().border_dim)
}
pub(super) fn accent() -> Color {
    color(active().accent)
}
pub(super) fn accent_hot() -> Color {
    color(active().accent_hot)
}
pub(super) fn accent_warm() -> Color {
    color(active().accent_warm)
}
pub(super) fn warning() -> Color {
    color(active().warning)
}
pub(super) fn success() -> Color {
    color(active().success)
}
pub(super) fn text() -> Color {
    color(active().text)
}
pub(super) fn text_dim() -> Color {
    color(active().text_dim)
}
pub(super) fn user() -> Color {
    color(active().user)
}
pub(super) fn user_surface() -> Color {
    color(active().user_surface)
}
pub(super) fn error() -> Color {
    color(active().error)
}

pub(super) fn base() -> Style {
    Style::default().fg(text()).bg(background())
}

pub(super) fn surface() -> Style {
    base().bg(surface_color())
}

pub(super) fn modal_surface() -> Style {
    base().bg(surface_raised())
}

pub(super) fn brand() -> Style {
    Style::default()
        .fg(accent_hot())
        .add_modifier(Modifier::BOLD)
}

pub(super) fn selected() -> Style {
    Style::default()
        .fg(accent_hot())
        .bg(selected_color())
        .add_modifier(Modifier::BOLD)
}

pub(super) fn pulse_color() -> Color {
    accent()
}

pub(super) fn modal_block(title: impl AsRef<str>) -> Block<'static> {
    modal_block_with_accent(title, accent())
}
pub(super) fn modal_block_with_accent(title: impl AsRef<str>, accent: Color) -> Block<'static> {
    let title = title.as_ref().trim();
    let (heading, hint) = title.split_once(" · ").unwrap_or((title, ""));
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(modal_surface())
        .border_style(Style::default().fg(accent))
        .padding(Padding::horizontal(1))
        .title(
            Line::from(format!(" {heading} "))
                .style(Style::default().fg(accent).add_modifier(Modifier::BOLD)),
        );
    if !hint.is_empty() {
        block =
            block.title_bottom(Line::from(format!(" {hint} ")).style(Style::default().fg(muted())));
    }
    block
}
pub(super) fn panel_block(title: impl AsRef<str>) -> Block<'static> {
    let title = title.as_ref().trim().to_owned();
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(surface())
        .border_style(Style::default().fg(border_dim()))
        .padding(Padding::horizontal(1))
        .title(
            Line::from(format!(" {title} "))
                .style(Style::default().fg(accent()).add_modifier(Modifier::BOLD)),
        )
}
pub(super) fn modal_backdrop(frame: &mut Frame<'_>, area: Rect) {
    let bounds = frame.area();
    frame
        .buffer_mut()
        .set_style(bounds, Style::default().fg(muted()).bg(background()));
    let shadow = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width,
        area.height,
    )
    .intersection(bounds);
    frame.render_widget(
        Block::default().style(Style::default().bg(background())),
        shadow,
    );
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(modal_surface()), area);
}

pub fn initialize_theme() {
    let settings = crate::config::ConfigStore::default()
        .theme_settings()
        .unwrap_or_default();
    let runtime = crate::model::RuntimeSettingsState {
        appearance: settings.appearance.unwrap_or_else(|| "auto".into()),
        theme_dark: settings.dark.unwrap_or_else(|| "kanagawa".into()),
        theme_light: settings.light.unwrap_or_else(|| "adwaita".into()),
        ..Default::default()
    };
    apply_runtime_theme(&runtime);
}

pub(crate) fn apply_runtime_theme(settings: &crate::model::RuntimeSettingsState) {
    let appearance = match std::env::var("YEET_THEME_MODE")
        .ok()
        .as_deref()
        .unwrap_or(settings.appearance.as_str())
    {
        value if value.eq_ignore_ascii_case("light") => Appearance::Light,
        _ => Appearance::Dark,
    };
    let theme_name = match appearance {
        Appearance::Dark => settings.theme_dark.as_str(),
        Appearance::Light => settings.theme_light.as_str(),
    };
    initialize(appearance, Some(theme_name));
}
