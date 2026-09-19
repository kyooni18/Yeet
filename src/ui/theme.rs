//! Runtime theme engine for the TUI.
//!
//! Yeet themes are deliberately expressed as semantic colors instead of widget
//! colors.  This keeps the renderer independent of the source format and lets
//! users bring a VS Code JSON theme or a Neovim palette/Lua colorscheme.

use std::{
    fs,
    path::Path,
    sync::{OnceLock, RwLock},
};

use ratatui::{
    Frame,
    layout::Rect,
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Padding},
};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Appearance {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy)]
struct Palette {
    background: Color,
    surface: Color,
    surface_raised: Color,
    code_background: Color,
    selected: Color,
    muted: Color,
    border: Color,
    border_dim: Color,
    accent: Color,
    accent_hot: Color,
    accent_warm: Color,
    warning: Color,
    success: Color,
    text: Color,
    text_dim: Color,
    user: Color,
    user_surface: Color,
    error: Color,
}

impl Palette {
    const fn kanagawa() -> Self {
        // Keep the default palette deliberately restrained: warm neutrals carry
        // the interface, while coral is reserved for focus and interaction.
        Self {
            background: rgb(20, 18, 18),
            surface: rgb(29, 25, 25),
            surface_raised: rgb(40, 33, 33),
            code_background: rgb(14, 12, 12),
            selected: rgb(75, 38, 35),
            muted: rgb(147, 129, 125),
            border: rgb(87, 61, 58),
            border_dim: rgb(57, 42, 40),
            accent: rgb(255, 109, 88),
            accent_hot: rgb(255, 139, 113),
            accent_warm: rgb(224, 168, 106),
            warning: rgb(231, 185, 92),
            success: rgb(145, 194, 145),
            text: rgb(244, 235, 230),
            text_dim: rgb(202, 180, 170),
            user: rgb(232, 168, 126),
            user_surface: rgb(54, 35, 31),
            error: rgb(255, 95, 90),
        }
    }

    const fn adwaita() -> Self {
        // Adwaita light roles. The palette is intentionally high contrast so
        // the terminal remains readable when a light terminal background is in use.
        Self {
            background: rgb(250, 250, 250),
            surface: rgb(255, 255, 255),
            surface_raised: rgb(246, 245, 244),
            code_background: rgb(242, 242, 242),
            selected: rgb(204, 232, 255),
            muted: rgb(94, 92, 100),
            border: rgb(192, 191, 188),
            border_dim: rgb(218, 216, 213),
            accent: rgb(53, 132, 228),
            accent_hot: rgb(28, 113, 216),
            accent_warm: rgb(145, 65, 172),
            warning: rgb(229, 165, 10),
            success: rgb(51, 209, 122),
            text: rgb(46, 52, 54),
            text_dim: rgb(94, 92, 100),
            user: rgb(38, 162, 105),
            user_surface: rgb(211, 244, 227),
            error: rgb(192, 28, 40),
        }
    }
}

const fn rgb(red: u8, green: u8, blue: u8) -> Color {
    Color::Rgb(red, green, blue)
}

static ACTIVE: OnceLock<RwLock<Palette>> = OnceLock::new();

fn active() -> Palette {
    let lock = ACTIVE.get_or_init(|| RwLock::new(load_palette()));
    *lock.read().expect("theme lock poisoned")
}

/// Select the built-in or imported palette for the current process.
///
/// This is public to the renderer, while the palette itself stays private so
/// call sites can only use semantic roles. Calling it more than once is safe.
pub(super) fn initialize(appearance: Appearance, name_or_path: Option<&str>) {
    let palette = load_palette_for(appearance, name_or_path);
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
        Appearance::Dark => settings.as_ref().and_then(|s| s.dark.as_deref()),
        Appearance::Light => settings.as_ref().and_then(|s| s.light.as_deref()),
    };
    load_palette_for(appearance, name)
}

fn load_palette_for(appearance: Appearance, name_or_path: Option<&str>) -> Palette {
    let default = match appearance {
        Appearance::Dark => Palette::kanagawa(),
        Appearance::Light => Palette::adwaita(),
    };
    let Some(name_or_path) = name_or_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return default;
    };
    if name_or_path.eq_ignore_ascii_case("kanagawa") {
        return Palette::kanagawa();
    }
    if name_or_path.eq_ignore_ascii_case("adwaita") {
        return Palette::adwaita();
    }
    ThemeFile::load(Path::new(name_or_path))
        .map(|file| file.apply(default))
        .unwrap_or(default)
}

#[derive(Debug, Default)]
struct ThemeFile {
    colors: Vec<(String, Color)>,
}

impl ThemeFile {
    fn load(path: &Path) -> Option<Self> {
        let source = fs::read_to_string(path).ok()?;
        Self::parse(&source)
    }

    fn parse(source: &str) -> Option<Self> {
        if let Ok(value) = serde_json::from_str::<Value>(source) {
            return Some(Self::from_json(&value));
        }
        Some(Self::from_lua(source))
    }

    fn from_json(value: &Value) -> Self {
        let mut colors = Vec::new();
        collect_json_colors(value, String::new(), &mut colors);
        // VS Code tokenColors are not always in `colors`; their foregrounds
        // still make useful semantic accents for markdown/code rendering.
        if let Some(tokens) = value.get("tokenColors").and_then(Value::as_array) {
            for token in tokens {
                let scope = token.get("scope").map(value_text).unwrap_or_default();
                if let Some(color) = token
                    .get("settings")
                    .and_then(|settings| settings.get("foreground"))
                    .and_then(Value::as_str)
                    .and_then(parse_color)
                {
                    colors.push((format!("token.{scope}"), color));
                }
            }
        }
        Self { colors }
    }

    fn from_lua(source: &str) -> Self {
        let mut colors = Vec::new();
        for line in source.lines() {
            let bytes = line.as_bytes();
            for index in 0..bytes.len().saturating_sub(6) {
                if bytes[index] != b'#' {
                    continue;
                }
                let end = index + 7;
                let Some(color) = (end <= bytes.len())
                    .then(|| parse_color(&line[index..end]))
                    .flatten()
                else {
                    continue;
                };
                let before = &line[..index];
                let key = before
                    .rsplit_once('=')
                    .map(|(prefix, _)| prefix)
                    .or_else(|| before.rsplit_once(':').map(|(prefix, _)| prefix))
                    .unwrap_or(before)
                    .trim()
                    .rsplit(|character: char| {
                        character.is_whitespace()
                            || matches!(character, '=' | ':' | '{' | '}' | ',')
                    })
                    .find(|part| !part.is_empty())
                    .unwrap_or_default()
                    .trim_matches(['"', '\'']);
                colors.push((key.to_owned(), color));
            }
        }
        Self { colors }
    }

    fn apply(&self, mut palette: Palette) -> Palette {
        for (key, color) in &self.colors {
            let key = key.to_ascii_lowercase();
            let role = role_for(&key);
            if let Some(role) = role {
                set_role(&mut palette, role, *color);
            }
        }
        palette
    }
}

#[derive(Clone, Copy)]
enum Role {
    Background,
    Surface,
    SurfaceRaised,
    CodeBackground,
    Selected,
    Muted,
    Border,
    BorderDim,
    Accent,
    AccentHot,
    AccentWarm,
    Warning,
    Success,
    Text,
    TextDim,
    User,
    UserSurface,
    Error,
}

fn role_for(key: &str) -> Option<Role> {
    let key = key.replace(['_', '-'], ".");
    if key.ends_with("editor.background") || key.ends_with("background") && key.contains("terminal")
    {
        Some(Role::Background)
    } else if key == "bg" || key.ends_with(".bg") {
        Some(Role::Background)
    } else if key.contains("sidebar.background") || key.ends_with("panel.background") {
        Some(Role::Surface)
    } else if key.contains("hover.background")
        || key.contains("suggest.background")
        || key == "bg_light"
    {
        Some(Role::SurfaceRaised)
    } else if key.contains("code") || key.contains("preformat") {
        Some(Role::CodeBackground)
    } else if key.contains("selection") || key.contains("selected") {
        Some(Role::Selected)
    } else if key.contains("border") || key.contains("outline") || key.contains("split") {
        Some(Role::Border)
    } else if key == "border.dim" || key.contains("inactiveborder") {
        Some(Role::BorderDim)
    } else if key.contains("warning") || key.contains("yellow") {
        Some(Role::Warning)
    } else if key.contains("error") || key.contains("red") {
        Some(Role::Error)
    } else if key.contains("success") || key.contains("passed") || key.contains("green") {
        Some(Role::Success)
    } else if key.contains("user.background")
        || key.contains("user.surface")
        || key.contains("user.bubble")
    {
        Some(Role::UserSurface)
    } else if key.contains("cyan") || key.contains("user") || key.contains("modifiedresource") {
        Some(Role::User)
    } else if key.contains("magenta") || key.contains("purple") || key.contains("violet") {
        Some(Role::AccentWarm)
    } else if key.contains("text.dim")
        || key.contains("foreground.dim")
        || key.contains("inactiveforeground")
    {
        Some(Role::TextDim)
    } else if key.contains("foreground")
        || key == "fg"
        || key.ends_with(".fg")
        || key.contains("text")
    {
        Some(Role::Text)
    } else if key.contains("muted") || key.contains("description") || key.contains("linenumber") {
        Some(Role::Muted)
    } else if key.contains("hot")
        || key.contains("bright")
        || key.contains("link")
        || key.contains("badge")
    {
        Some(Role::AccentHot)
    } else if key.contains("blue")
        || key.contains("accent")
        || key.contains("focus")
        || key.contains("button")
    {
        Some(Role::Accent)
    } else {
        None
    }
}

fn set_role(palette: &mut Palette, role: Role, color: Color) {
    match role {
        Role::Background => palette.background = color,
        Role::Surface => palette.surface = color,
        Role::SurfaceRaised => palette.surface_raised = color,
        Role::CodeBackground => palette.code_background = color,
        Role::Selected => palette.selected = color,
        Role::Muted => palette.muted = color,
        Role::Border => palette.border = color,
        Role::BorderDim => palette.border_dim = color,
        Role::Accent => palette.accent = color,
        Role::AccentHot => palette.accent_hot = color,
        Role::AccentWarm => palette.accent_warm = color,
        Role::Warning => palette.warning = color,
        Role::Success => palette.success = color,
        Role::Text => palette.text = color,
        Role::TextDim => palette.text_dim = color,
        Role::User => palette.user = color,
        Role::UserSurface => palette.user_surface = color,
        Role::Error => palette.error = color,
    }
}

fn collect_json_colors(value: &Value, prefix: String, colors: &mut Vec<(String, Color)>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                if let Some(color) = value.as_str().and_then(parse_color) {
                    colors.push((path, color));
                } else {
                    collect_json_colors(value, path, colors);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_json_colors(value, prefix.clone(), colors);
            }
        }
        _ => {}
    }
}

fn value_text(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_default()
}

fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim().trim_matches(['"', '\'']);
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(red, green, blue))
}

#[allow(dead_code)]
fn find_colors(line: &str) -> Vec<Color> {
    let bytes = line.as_bytes();
    let mut colors = Vec::new();
    for index in 0..bytes.len().saturating_sub(6) {
        if bytes[index] == b'#' {
            let end = index + 7;
            if end <= bytes.len() {
                if let Some(color) = parse_color(&line[index..end]) {
                    colors.push(color);
                }
            }
        }
    }
    colors
}

// Semantic accessors keep the existing renderer source readable while making
// every color resolve from the active theme at render time.
pub(super) fn background() -> Color {
    active().background
}
pub(super) fn surface_color() -> Color {
    active().surface
}
pub(super) fn surface_raised() -> Color {
    active().surface_raised
}
pub(super) fn code_background() -> Color {
    active().code_background
}
pub(super) fn selected_color() -> Color {
    active().selected
}
pub(super) fn muted() -> Color {
    active().muted
}
pub(super) fn border() -> Color {
    active().border
}
pub(super) fn border_dim() -> Color {
    active().border_dim
}
pub(super) fn accent() -> Color {
    active().accent
}
pub(super) fn accent_hot() -> Color {
    active().accent_hot
}
pub(super) fn accent_warm() -> Color {
    active().accent_warm
}
pub(super) fn warning() -> Color {
    active().warning
}
pub(super) fn success() -> Color {
    active().success
}
pub(super) fn text() -> Color {
    active().text
}
pub(super) fn text_dim() -> Color {
    active().text_dim
}
pub(super) fn user() -> Color {
    active().user
}
pub(super) fn user_surface() -> Color {
    active().user_surface
}
pub(super) fn error() -> Color {
    active().error
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
    modal_block_with_accent(title, accent_warm())
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
            Line::from(format!(" {heading} ")).style(
                Style::default()
                    .fg(background())
                    .bg(accent)
                    .add_modifier(Modifier::BOLD),
            ),
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
            Line::from(format!(" {title} ")).style(
                Style::default()
                    .fg(accent_warm())
                    .add_modifier(Modifier::BOLD),
            ),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_have_expected_defaults() {
        assert_eq!(Palette::kanagawa().background, Color::Rgb(20, 18, 18));
        assert_eq!(Palette::adwaita().background, Color::Rgb(250, 250, 250));
    }

    #[test]
    fn parses_vscode_colors() {
        let theme = ThemeFile::parse(
            r##"{"colors":{"editor.background":"#112233","editor.foreground":"#ddeeff"}}"##,
        )
        .unwrap();
        let palette = theme.apply(Palette::kanagawa());
        assert_eq!(palette.background, Color::Rgb(17, 34, 51));
        assert_eq!(palette.text, Color::Rgb(221, 238, 255));
    }

    #[test]
    fn parses_neovim_lua_palette() {
        let theme =
            ThemeFile::parse("local palette = { bg = '#010203', blue = '#aabbcc' }").unwrap();
        let palette = theme.apply(Palette::kanagawa());
        assert_eq!(palette.background, Color::Rgb(1, 2, 3));
        assert_eq!(palette.accent, Color::Rgb(170, 187, 204));
    }
}
