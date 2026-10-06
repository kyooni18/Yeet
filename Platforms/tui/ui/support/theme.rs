//! Ratatui adapter for the shared Yeet theme engine.

use std::sync::{Mutex, OnceLock, RwLock};

use ratatui::{
    Frame,
    layout::Rect,
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Padding},
};

pub(in crate::platforms::tui::ui) use crate::shared_ui::theme::Appearance;
use crate::shared_ui::theme::{Palette, Rgb};

static RESOURCES: OnceLock<Mutex<crate::theme::ThemeProjectionCache>> = OnceLock::new();

static ACTIVE: OnceLock<RwLock<Palette>> = OnceLock::new();

pub(in crate::platforms::tui::ui) fn active_palette() -> Palette {
    active()
}

fn active() -> Palette {
    let lock = ACTIVE.get_or_init(|| RwLock::new(Palette::kanagawa()));
    *lock.read().expect("theme lock poisoned")
}

fn color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.red, rgb.green, rgb.blue)
}

pub(in crate::platforms::tui::ui) fn background() -> Color {
    color(active().background)
}
pub(in crate::platforms::tui::ui) fn surface_color() -> Color {
    color(active().surface)
}
pub(in crate::platforms::tui::ui) fn surface_raised() -> Color {
    color(active().surface_raised)
}
pub(in crate::platforms::tui::ui) fn composer_info_surface() -> Color {
    color(active().surface.mix(active().surface_raised, 0.25))
}
pub(in crate::platforms::tui::ui) fn composer_info_border() -> Color {
    color(active().surface_raised.mix(active().border, 0.15))
}
pub(in crate::platforms::tui::ui) fn code_background() -> Color {
    color(active().code_background)
}
pub(in crate::platforms::tui::ui) fn selected_color() -> Color {
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
pub(in crate::platforms::tui::ui) fn muted() -> Color {
    color(active().muted)
}
pub(in crate::platforms::tui::ui) fn border() -> Color {
    color(active().border)
}
pub(in crate::platforms::tui::ui) fn border_dim() -> Color {
    color(active().border_dim)
}
pub(in crate::platforms::tui::ui) fn accent() -> Color {
    color(active().accent)
}
pub(in crate::platforms::tui::ui) fn accent_hot() -> Color {
    color(active().accent_hot)
}
pub(in crate::platforms::tui::ui) fn accent_warm() -> Color {
    color(active().accent_warm)
}
pub(in crate::platforms::tui::ui) fn warning() -> Color {
    color(active().warning)
}
pub(in crate::platforms::tui::ui) fn success() -> Color {
    color(active().success)
}
pub(in crate::platforms::tui::ui) fn text() -> Color {
    color(active().text)
}
pub(in crate::platforms::tui::ui) fn text_dim() -> Color {
    color(active().text_dim)
}
/// Neutral grey between body text and muted, used for secondary labels.
pub(in crate::platforms::tui::ui) fn secondary() -> Color {
    color(active().text.mix(active().muted, 0.5))
}
/// Barely-visible rule on the conversation canvas (tree rails, dividers).
pub(in crate::platforms::tui::ui) fn hairline() -> Color {
    color(active().code_background.mix(active().surface, 0.8))
}
/// Current-row highlight inside the session rail.
pub(in crate::platforms::tui::ui) fn rail_selected() -> Color {
    color(active().background.mix(active().surface, 0.5))
}
pub(in crate::platforms::tui::ui) fn status_background() -> Color {
    color(active().code_background.mix(Rgb::new(0, 0, 0), 0.1))
}
pub(in crate::platforms::tui::ui) fn meter_track() -> Color {
    color(active().surface_raised.mix(active().border, 0.5))
}
pub(in crate::platforms::tui::ui) fn user() -> Color {
    color(active().user)
}
pub(in crate::platforms::tui::ui) fn error() -> Color {
    color(active().error)
}
pub(in crate::platforms::tui::ui) fn error_subtle() -> Color {
    color(active().error.mix(active().background, 0.45))
}

pub(in crate::platforms::tui::ui) fn base() -> Style {
    Style::default().fg(text()).bg(background())
}

pub(in crate::platforms::tui::ui) fn surface() -> Style {
    base().bg(surface_color())
}

pub(in crate::platforms::tui::ui) fn modal_surface() -> Style {
    base().bg(surface_raised())
}

pub(in crate::platforms::tui::ui) fn brand() -> Style {
    Style::default()
        .fg(accent_hot())
        .add_modifier(Modifier::BOLD)
}

pub(in crate::platforms::tui::ui) fn selected() -> Style {
    Style::default()
        .fg(accent_hot())
        .bg(selected_color())
        .add_modifier(Modifier::BOLD)
}

pub(in crate::platforms::tui::ui) fn modal_block(title: impl AsRef<str>) -> Block<'static> {
    modal_block_with_accent(title, accent())
}
pub(in crate::platforms::tui::ui) fn modal_block_with_accent(
    title: impl AsRef<str>,
    accent: Color,
) -> Block<'static> {
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
pub(in crate::platforms::tui::ui) fn panel_block(title: impl AsRef<str>) -> Block<'static> {
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
pub(in crate::platforms::tui::ui) fn modal_backdrop(frame: &mut Frame<'_>, area: Rect) {
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
    let mut projected = settings.clone();
    RESOURCES
        .get_or_init(Default::default)
        .lock()
        .expect("theme resource lock poisoned")
        .hydrate(&mut projected);
    // The terminal override remains a host observation; UI chooses the palette.
    if let Ok(appearance) = std::env::var("YEET_THEME_MODE") {
        projected.appearance = appearance;
    }
    let palette = crate::shared_ui::theme::palette_for_settings(&projected, Appearance::Dark);
    let lock = ACTIVE.get_or_init(|| RwLock::new(palette));
    *lock.write().expect("theme lock poisoned") = palette;
}

pub(crate) fn refresh_runtime_theme(settings: &crate::model::RuntimeSettingsState) {
    RESOURCES
        .get_or_init(Default::default)
        .lock()
        .expect("theme resource lock poisoned")
        .invalidate();
    apply_runtime_theme(settings);
}
