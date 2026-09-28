use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

impl Appearance {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Rgb {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub fn mix(self, other: Self, amount: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);
        let blend = |left: u8, right: u8| {
            ((left as f32) + ((right as f32) - (left as f32)) * amount)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        Self::new(
            blend(self.red, other.red),
            blend(self.green, other.green),
            blend(self.blue, other.blue),
        )
    }

    fn luminance(self) -> f32 {
        let linear = |channel: u8| {
            let value = channel as f32 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(self.red) + 0.7152 * linear(self.green) + 0.0722 * linear(self.blue)
    }

    fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.red, self.green, self.blue)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub background: Rgb,
    pub surface: Rgb,
    pub surface_raised: Rgb,
    pub code_background: Rgb,
    pub selected: Rgb,
    pub muted: Rgb,
    pub border: Rgb,
    pub border_dim: Rgb,
    pub accent: Rgb,
    pub accent_hot: Rgb,
    pub accent_warm: Rgb,
    pub warning: Rgb,
    pub success: Rgb,
    pub text: Rgb,
    pub text_dim: Rgb,
    pub user: Rgb,
    pub user_surface: Rgb,
    pub error: Rgb,
}

impl Palette {
    pub const fn kanagawa() -> Self {
        // Source of truth: barklan/kanagawa.vscode
        // themes/kanagawa-color-theme.json. Yeet semantic roles are mapped
        // directly from the extension's editor/chrome/token colors.
        Self {
            background: rgb(31, 31, 40),      // editor.background #1F1F28
            surface: rgb(42, 42, 55),         // activityBar.background #2A2A37
            surface_raised: rgb(54, 54, 70),  // list.hoverBackground #363646
            code_background: rgb(22, 22, 29), // input/statusBar.background #16161D
            selected: rgb(34, 50, 73),        // editor.selectionBackground #223249
            muted: rgb(114, 113, 105),        // editorInlayHint.foreground #727169
            border: rgb(84, 84, 109),         // editorBracketMatch.border #54546D
            border_dim: rgb(22, 22, 29),      // editorGroup.border #16161D
            accent: rgb(126, 156, 216),       // list.highlightForeground #7E9CD8
            accent_hot: rgb(127, 180, 202),   // terminal.ansiBrightBlue #7FB4CA
            accent_warm: rgb(255, 160, 102),  // editorBracketHighlight.foreground2 #FFA066
            warning: rgb(255, 158, 59),       // editorWarning.foreground #FF9E3B
            success: rgb(152, 187, 108),      // terminal.ansiBrightGreen #98BB6C
            text: rgb(220, 215, 186),         // editor.foreground #DCD7BA
            text_dim: rgb(200, 192, 147),     // statusBar.foreground #C8C093
            user: rgb(230, 195, 132),         // terminal.ansiBrightYellow #E6C384
            user_surface: rgb(45, 79, 103),   // statusBarItem.remoteBackground #2D4F67
            error: rgb(232, 36, 36),          // editorError.foreground #E82424
        }
    }

    pub const fn quiet_night() -> Self {
        // Copied from ~/.config/nvim-custom/quiet-night.nvim/lua/quiet-night/palette.lua
        // dark palette and mapped into Yeet semantic UI roles. This is a
        // built-in copy, not a runtime dependency on the local Neovim theme path.
        Self {
            background: rgb(17, 17, 19),      // bg #111113
            surface: rgb(26, 26, 29),         // bg_cursor #1a1a1d
            surface_raised: rgb(39, 39, 44),  // bg_visual #27272c
            code_background: rgb(11, 11, 13), // bg_float #0b0b0d
            selected: rgb(39, 39, 44),        // bg_visual #27272c
            muted: rgb(134, 133, 145),        // comment #868591
            border: rgb(39, 39, 44),          // bg_visual #27272c
            border_dim: rgb(26, 26, 29),      // bg_cursor #1a1a1d
            accent: rgb(124, 167, 223),       // blue #7ca7df
            accent_hot: rgb(100, 194, 204),   // cyan #64c2cc
            accent_warm: rgb(220, 154, 108),  // orange #dc9a6c
            warning: rgb(211, 180, 101),      // yellow #d3b465
            success: rgb(135, 191, 129),      // green #87bf81
            text: rgb(202, 201, 211),         // fg #cac9d3
            text_dim: rgb(158, 157, 168),     // ui #9e9da8
            user: rgb(100, 194, 204),         // cyan #64c2cc
            user_surface: rgb(39, 39, 44),    // bg_visual #27272c
            error: rgb(216, 131, 129),        // red #d88381
        }
    }

    pub const fn yeet() -> Self {
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

    pub const fn nord() -> Self {
        Self {
            background: rgb(46, 52, 64),
            surface: rgb(59, 66, 82),
            surface_raised: rgb(67, 76, 94),
            code_background: rgb(36, 41, 51),
            selected: rgb(62, 82, 107),
            muted: rgb(136, 146, 162),
            border: rgb(76, 86, 106),
            border_dim: rgb(59, 66, 82),
            accent: rgb(136, 192, 208),
            accent_hot: rgb(143, 188, 187),
            accent_warm: rgb(180, 142, 173),
            warning: rgb(235, 203, 139),
            success: rgb(163, 190, 140),
            text: rgb(236, 239, 244),
            text_dim: rgb(216, 222, 233),
            user: rgb(129, 161, 193),
            user_surface: rgb(53, 68, 92),
            error: rgb(191, 97, 106),
        }
    }

    pub const fn catppuccin_mocha() -> Self {
        Self {
            background: rgb(30, 30, 46),
            surface: rgb(49, 50, 68),
            surface_raised: rgb(69, 71, 90),
            code_background: rgb(24, 24, 37),
            selected: rgb(69, 71, 90),
            muted: rgb(127, 132, 156),
            border: rgb(88, 91, 112),
            border_dim: rgb(49, 50, 68),
            accent: rgb(137, 180, 250),
            accent_hot: rgb(116, 199, 236),
            accent_warm: rgb(203, 166, 247),
            warning: rgb(249, 226, 175),
            success: rgb(166, 227, 161),
            text: rgb(205, 214, 244),
            text_dim: rgb(186, 194, 222),
            user: rgb(148, 226, 213),
            user_surface: rgb(40, 55, 62),
            error: rgb(243, 139, 168),
        }
    }

    pub const fn rose_pine() -> Self {
        Self {
            background: rgb(25, 23, 36),
            surface: rgb(31, 29, 46),
            surface_raised: rgb(38, 35, 58),
            code_background: rgb(20, 18, 29),
            selected: rgb(64, 61, 82),
            muted: rgb(110, 106, 134),
            border: rgb(82, 79, 103),
            border_dim: rgb(38, 35, 58),
            accent: rgb(196, 167, 231),
            accent_hot: rgb(235, 188, 186),
            accent_warm: rgb(235, 111, 146),
            warning: rgb(246, 193, 119),
            success: rgb(156, 207, 216),
            text: rgb(224, 222, 244),
            text_dim: rgb(144, 140, 170),
            user: rgb(49, 116, 143),
            user_surface: rgb(29, 46, 60),
            error: rgb(235, 111, 146),
        }
    }

    pub const fn adwaita() -> Self {
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

    pub const fn solarized_light() -> Self {
        Self {
            background: rgb(253, 246, 227),
            surface: rgb(238, 232, 213),
            surface_raised: rgb(232, 225, 207),
            code_background: rgb(245, 239, 221),
            selected: rgb(215, 229, 229),
            muted: rgb(131, 148, 150),
            border: rgb(147, 161, 161),
            border_dim: rgb(211, 203, 183),
            accent: rgb(38, 139, 210),
            accent_hot: rgb(42, 161, 152),
            accent_warm: rgb(108, 113, 196),
            warning: rgb(181, 137, 0),
            success: rgb(133, 153, 0),
            text: rgb(88, 110, 117),
            text_dim: rgb(101, 123, 131),
            user: rgb(42, 161, 152),
            user_surface: rgb(225, 238, 233),
            error: rgb(220, 50, 47),
        }
    }
}

const fn rgb(red: u8, green: u8, blue: u8) -> Rgb {
    Rgb::new(red, green, blue)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct PaletteState {
    pub background: String,
    pub surface: String,
    pub surface_raised: String,
    pub code_background: String,
    pub selected: String,
    pub muted: String,
    pub border: String,
    pub border_dim: String,
    pub accent: String,
    pub accent_hot: String,
    pub accent_warm: String,
    pub warning: String,
    pub success: String,
    pub text: String,
    pub text_dim: String,
    pub user: String,
    pub user_surface: String,
    pub error: String,
}

impl From<Palette> for PaletteState {
    fn from(palette: Palette) -> Self {
        Self {
            background: palette.background.hex(),
            surface: palette.surface.hex(),
            surface_raised: palette.surface_raised.hex(),
            code_background: palette.code_background.hex(),
            selected: palette.selected.hex(),
            muted: palette.muted.hex(),
            border: palette.border.hex(),
            border_dim: palette.border_dim.hex(),
            accent: palette.accent.hex(),
            accent_hot: palette.accent_hot.hex(),
            accent_warm: palette.accent_warm.hex(),
            warning: palette.warning.hex(),
            success: palette.success.hex(),
            text: palette.text.hex(),
            text_dim: palette.text_dim.hex(),
            user: palette.user.hex(),
            user_surface: palette.user_surface.hex(),
            error: palette.error.hex(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeCatalogItem {
    pub id: String,
    pub label: String,
    pub appearance: String,
    pub background: String,
    pub surface: String,
    pub accent: String,
    pub text: String,
}

impl ThemeCatalogItem {
    fn new(id: &str, label: &str, appearance: Appearance, palette: Palette) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            appearance: appearance.as_str().into(),
            background: palette.background.hex(),
            surface: palette.surface.hex(),
            accent: palette.accent.hex(),
            text: palette.text.hex(),
        }
    }
}

pub fn catalog() -> Vec<ThemeCatalogItem> {
    vec![
        ThemeCatalogItem::new(
            "quiet-night",
            "Quiet Night",
            Appearance::Dark,
            Palette::quiet_night(),
        ),
        ThemeCatalogItem::new(
            "kanagawa",
            "Kanagawa",
            Appearance::Dark,
            Palette::kanagawa(),
        ),
        ThemeCatalogItem::new("yeet", "Yeet Ember", Appearance::Dark, Palette::yeet()),
        ThemeCatalogItem::new("nord", "Nord", Appearance::Dark, Palette::nord()),
        ThemeCatalogItem::new(
            "catppuccin-mocha",
            "Catppuccin Mocha",
            Appearance::Dark,
            Palette::catppuccin_mocha(),
        ),
        ThemeCatalogItem::new(
            "rose-pine",
            "Rosé Pine",
            Appearance::Dark,
            Palette::rose_pine(),
        ),
        ThemeCatalogItem::new("adwaita", "Adwaita", Appearance::Light, Palette::adwaita()),
        ThemeCatalogItem::new(
            "solarized-light",
            "Solarized Light",
            Appearance::Light,
            Palette::solarized_light(),
        ),
    ]
}

#[derive(Debug, Clone)]
pub struct ResolvedTheme {
    pub palette: Palette,
    pub requested: String,
    pub resolved: String,
    pub warning: Option<String>,
}

pub fn default_theme_id(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Dark => "kanagawa",
        Appearance::Light => "adwaita",
    }
}

pub fn resolve_palette(appearance: Appearance, name_or_path: Option<&str>) -> ResolvedTheme {
    let requested = name_or_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_theme_id(appearance));
    let default = default_palette(appearance);

    if let Some((resolved, palette)) = builtin_palette(requested) {
        return ResolvedTheme {
            palette,
            requested: requested.into(),
            resolved: resolved.into(),
            warning: None,
        };
    }

    match ThemeFile::load(Path::new(requested)) {
        Ok(file) => ResolvedTheme {
            palette: file.apply(default),
            requested: requested.into(),
            resolved: requested.into(),
            warning: None,
        },
        Err(error) => ResolvedTheme {
            palette: default,
            requested: requested.into(),
            resolved: default_theme_id(appearance).into(),
            warning: Some(error),
        },
    }
}

pub fn validate_reference(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Theme name or path cannot be empty".into());
    }
    if builtin_palette(value).is_some() {
        return Ok(());
    }
    ThemeFile::load(Path::new(value)).map(|_| ())
}

fn default_palette(appearance: Appearance) -> Palette {
    match appearance {
        Appearance::Dark => Palette::kanagawa(),
        Appearance::Light => Palette::adwaita(),
    }
}

fn builtin_palette(value: &str) -> Option<(&'static str, Palette)> {
    match value.trim().to_ascii_lowercase().as_str() {
        "quiet-night" | "quietnight" | "quiet_night" => {
            Some(("quiet-night", Palette::quiet_night()))
        }
        "kanagawa" | "kanagawa-wave" | "wave" => Some(("kanagawa", Palette::kanagawa())),
        "yeet" | "ember" | "yeet-ember" => Some(("yeet", Palette::yeet())),
        "nord" => Some(("nord", Palette::nord())),
        "catppuccin" | "catppuccin-mocha" | "mocha" => {
            Some(("catppuccin-mocha", Palette::catppuccin_mocha()))
        }
        "rose-pine" | "rosepine" => Some(("rose-pine", Palette::rose_pine())),
        "adwaita" => Some(("adwaita", Palette::adwaita())),
        "solarized-light" | "solarized" => Some(("solarized-light", Palette::solarized_light())),
        _ => None,
    }
}

#[derive(Debug, Default)]
struct ThemeFile {
    colors: Vec<(String, Rgb)>,
}

impl ThemeFile {
    fn load(path: &Path) -> Result<Self, String> {
        let path = expand_home(path);
        if path.is_dir() {
            return Self::load_directory(&path);
        }

        let source = fs::read_to_string(&path)
            .map_err(|error| format!("Could not read theme {}: {error}", path.display()))?;
        let theme = Self::parse(&source)?;
        if theme.colors.is_empty() {
            return Err(format!(
                "Theme {} does not contain readable colors",
                path.display()
            ));
        }
        Ok(theme)
    }

    fn load_directory(path: &Path) -> Result<Self, String> {
        let mut files = Vec::new();
        collect_theme_files(path, &mut files).map_err(|error| {
            format!("Could not scan theme directory {}: {error}", path.display())
        })?;
        files.sort();

        let mut colors = Vec::new();
        for file in files {
            let source = fs::read_to_string(&file)
                .map_err(|error| format!("Could not read theme {}: {error}", file.display()))?;
            if let Ok(theme) = Self::parse(&source) {
                colors.extend(theme.colors);
            }
        }

        if colors.is_empty() {
            return Err(format!(
                "Theme directory {} does not contain readable JSON or Lua colors",
                path.display()
            ));
        }

        Ok(Self { colors })
    }

    fn parse(source: &str) -> Result<Self, String> {
        if let Ok(value) = serde_json::from_str::<Value>(source) {
            let theme = Self::from_json(&value);
            if theme.colors.is_empty() {
                return Err("JSON theme does not contain readable colors".into());
            }
            return Ok(theme);
        }

        let theme = Self::from_lua(source);
        if theme.colors.is_empty() {
            Err("Theme is neither supported JSON nor a Lua palette with hex colors".into())
        } else {
            Ok(theme)
        }
    }

    fn from_json(value: &Value) -> Self {
        let mut colors = Vec::new();
        collect_json_colors(value, String::new(), &mut colors);
        if let Some(tokens) = value.get("tokenColors").and_then(Value::as_array) {
            for token in tokens {
                let scopes = match token.get("scope") {
                    Some(Value::String(scope)) => vec![scope.clone()],
                    Some(Value::Array(items)) => items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                    _ => Vec::new(),
                };
                if let Some(color) = token
                    .get("settings")
                    .and_then(|settings| settings.get("foreground"))
                    .and_then(Value::as_str)
                    .and_then(parse_color)
                {
                    for scope in scopes {
                        colors.push((format!("token.{scope}"), color));
                    }
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

                let key = lua_color_key(line, index);
                if !key.is_empty() {
                    colors.push((key, color));
                }
            }
        }
        add_neovim_palette_aliases(&mut colors);
        Self { colors }
    }

    fn apply(&self, mut palette: Palette) -> Palette {
        let mut touched = BTreeSet::new();
        for (key, color) in &self.colors {
            if let Some(role) = role_for(key) {
                set_role(&mut palette, role, *color);
                touched.insert(role);
            }
        }
        harmonize(&mut palette, &touched);
        palette
    }
}

fn expand_home(path: &Path) -> PathBuf {
    let raw = path.as_os_str().to_string_lossy();
    if raw == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_path_buf());
    }
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    path.to_path_buf()
}

fn collect_theme_files(path: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_theme_files(&path, files)?;
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "json" | "lua"))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn lua_color_key(line: &str, color_index: usize) -> String {
    let before = &line[..color_index];
    let assignment_head = before
        .split_once('=')
        .map(|(prefix, _)| prefix)
        .or_else(|| before.split_once(':').map(|(prefix, _)| prefix))
        .unwrap_or(before);
    let assignment_key = trailing_identifier(assignment_head);
    if !assignment_key.is_empty() && !matches!(assignment_key.as_str(), "gui" | "fg" | "bg" | "sp")
    {
        return assignment_key;
    }

    let quoted_group = before
        .rfind(['"', '\''])
        .and_then(|end| {
            before[..end]
                .rfind(['"', '\''])
                .map(|start| before[start + 1..end].to_owned())
        })
        .unwrap_or_default();
    if !quoted_group.is_empty() {
        return quoted_group;
    }

    assignment_key
}

fn trailing_identifier(source: &str) -> String {
    source
        .trim()
        .rsplit(|character: char| {
            character.is_whitespace()
                || matches!(character, '=' | ':' | '{' | '}' | ',' | '(' | ')')
        })
        .find(|part| !part.is_empty())
        .unwrap_or_default()
        .trim_matches(['"', '\''])
        .to_owned()
}

fn add_neovim_palette_aliases(colors: &mut Vec<(String, Rgb)>) {
    let snapshot = colors.clone();
    let find = |name: &str| -> Option<Rgb> {
        snapshot
            .iter()
            .rev()
            .find_map(|(key, color)| key.eq_ignore_ascii_case(name).then_some(*color))
    };
    let mut push = |role: &str, color: Option<Rgb>| {
        if let Some(color) = color {
            colors.push((role.to_owned(), color));
        }
    };

    push("background", find("bg"));
    push("surface", find("bg_cursor"));
    push("surfaceRaised", find("bg_visual"));
    push("codeBackground", find("bg_float"));
    push("selected", find("bg_visual"));
    push("border", find("bg_visual"));
    push("borderDim", find("bg_cursor"));
    push("muted", find("comment"));
    push("textDim", find("ui"));
    push("text", find("fg"));
    push("accent", find("blue"));
    push("accentHot", find("cyan"));
    push("accentWarm", find("orange"));
    push("warning", find("yellow"));
    push("success", find("green"));
    push("user", find("cyan").or_else(|| find("blue")));
    push("userSurface", find("bg_visual"));
    push("error", find("red"));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

fn role_for(raw_key: &str) -> Option<Role> {
    let key = raw_key.to_ascii_lowercase().replace(['_', '-'], ".");
    let compact = key.replace('.', "");

    // TextMate scopes describe syntax, not UI roles.  Applying them here lets
    // a large token palette overwrite the actual chrome colors merely because
    // a scope contains words such as `string`, `keyword`, or `blue`.
    if key.starts_with("token.") {
        return None;
    }

    let semantic = [
        ("background", Role::Background),
        ("surface", Role::Surface),
        ("surfaceraised", Role::SurfaceRaised),
        ("codebackground", Role::CodeBackground),
        ("selected", Role::Selected),
        ("muted", Role::Muted),
        ("border", Role::Border),
        ("borderdim", Role::BorderDim),
        ("accent", Role::Accent),
        ("accenthot", Role::AccentHot),
        ("accentwarm", Role::AccentWarm),
        ("warning", Role::Warning),
        ("success", Role::Success),
        ("text", Role::Text),
        ("textdim", Role::TextDim),
        ("user", Role::User),
        ("usersurface", Role::UserSurface),
        ("error", Role::Error),
    ];
    if let Some((_, role)) = semantic
        .iter()
        .find(|(name, _)| compact == *name || compact.ends_with(&format!("yeet{name}")))
    {
        return Some(*role);
    }

    let leaf = key.rsplit('.').next().unwrap_or(key.as_str());
    let leaf_compact = leaf.replace('.', "");

    if key.contains("user.background")
        || key.contains("user.surface")
        || key.contains("user.bubble")
    {
        return Some(Role::UserSurface);
    }
    if key.contains("border.dim") || key.contains("inactiveborder") {
        return Some(Role::BorderDim);
    }
    if key.contains("text.dim")
        || key.contains("foreground.dim")
        || key.contains("inactiveforeground")
    {
        return Some(Role::TextDim);
    }
    if key == "bg" || key.ends_with(".bg") || key.contains("editor.background") {
        return Some(Role::Background);
    }
    if key.ends_with(".background") {
        if key.contains("sidebar")
            || key.contains("panel")
            || key.contains("activitybar")
            || key.contains("titlebar")
            || key.contains("toolbar")
        {
            return Some(Role::Surface);
        }
        return Some(Role::Background);
    }
    if (key.contains("hover") || key.contains("suggest")) && key.contains("background")
        || key == "bg.light"
    {
        return Some(Role::SurfaceRaised);
    }
    if key
        .split('.')
        .any(|part| matches!(part, "code" | "preformat"))
    {
        return Some(Role::CodeBackground);
    }
    if key.contains("selection") || key.contains("selected") {
        return Some(Role::Selected);
    }
    if key.contains("warning") || leaf_compact == "yellow" {
        return Some(Role::Warning);
    }
    if key.contains("error") || key.contains("invalid") || leaf_compact == "red" {
        return Some(Role::Error);
    }
    if key.contains("success") || key.contains("passed") || leaf_compact == "green" {
        return Some(Role::Success);
    }
    if leaf_compact == "cyan" || leaf_compact == "user" || key.contains("modifiedresource") {
        return Some(Role::User);
    }
    if matches!(leaf_compact.as_str(), "magenta" | "purple" | "violet") {
        return Some(Role::AccentWarm);
    }
    if matches!(
        leaf_compact.as_str(),
        "comment" | "muted" | "description" | "linenumber"
    ) {
        return Some(Role::Muted);
    }
    if matches!(leaf_compact.as_str(), "hot" | "bright" | "link" | "badge") {
        return Some(Role::AccentHot);
    }
    if key.contains("focus") || matches!(leaf_compact.as_str(), "blue" | "accent" | "button") {
        return Some(Role::Accent);
    }
    if matches!(leaf_compact.as_str(), "foreground" | "fg" | "text") {
        return Some(Role::Text);
    }
    if matches!(leaf_compact.as_str(), "border" | "outline" | "split") || key.contains("border") {
        return Some(Role::Border);
    }
    None
}

fn set_role(palette: &mut Palette, role: Role, color: Rgb) {
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

fn harmonize(palette: &mut Palette, touched: &BTreeSet<Role>) {
    let dark = palette.background.luminance() < 0.45;
    let base = palette.background;
    let text = palette.text;

    if touched.contains(&Role::Background) {
        if !touched.contains(&Role::Surface) {
            palette.surface = base.mix(text, if dark { 0.055 } else { 0.035 });
        }
        if !touched.contains(&Role::SurfaceRaised) {
            palette.surface_raised = base.mix(text, if dark { 0.10 } else { 0.065 });
        }
        if !touched.contains(&Role::CodeBackground) {
            palette.code_background = if dark {
                base.mix(Rgb::new(0, 0, 0), 0.18)
            } else {
                base.mix(text, 0.03)
            };
        }
        if !touched.contains(&Role::Border) {
            palette.border = base.mix(text, if dark { 0.26 } else { 0.22 });
        }
        if !touched.contains(&Role::BorderDim) {
            palette.border_dim = base.mix(text, if dark { 0.14 } else { 0.11 });
        }
    }

    if touched.contains(&Role::Text) || touched.contains(&Role::Background) {
        if !touched.contains(&Role::TextDim) {
            palette.text_dim = text.mix(base, if dark { 0.28 } else { 0.18 });
        }
        if !touched.contains(&Role::Muted) {
            palette.muted = text.mix(base, if dark { 0.47 } else { 0.38 });
        }
    }

    if touched.contains(&Role::Accent) || touched.contains(&Role::Background) {
        if !touched.contains(&Role::Selected) {
            palette.selected = base.mix(palette.accent, if dark { 0.28 } else { 0.18 });
        }
        if !touched.contains(&Role::AccentHot) {
            palette.accent_hot = palette.accent.mix(text, if dark { 0.20 } else { 0.12 });
        }
    }

    if (touched.contains(&Role::User) || touched.contains(&Role::Background))
        && !touched.contains(&Role::UserSurface)
    {
        palette.user_surface = base.mix(palette.user, if dark { 0.18 } else { 0.12 });
    }
}

fn collect_json_colors(value: &Value, prefix: String, colors: &mut Vec<(String, Rgb)>) {
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

fn parse_color(value: &str) -> Option<Rgb> {
    let value = value.trim().trim_matches(['"', '\'']);
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Rgb::new(red, green, blue))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kanagawa_matches_barklan_vscode_theme_roles() {
        let palette = Palette::kanagawa();
        assert_eq!(palette.background, Rgb::new(31, 31, 40)); // editor.background
        assert_eq!(palette.surface, Rgb::new(42, 42, 55)); // activityBar.background
        assert_eq!(palette.surface_raised, Rgb::new(54, 54, 70)); // list.hoverBackground
        assert_eq!(palette.code_background, Rgb::new(22, 22, 29)); // input.background
        assert_eq!(palette.selected, Rgb::new(34, 50, 73)); // editor.selectionBackground
        assert_eq!(palette.muted, Rgb::new(114, 113, 105)); // editorInlayHint.foreground
        assert_eq!(palette.border, Rgb::new(84, 84, 109)); // editorBracketMatch.border
        assert_eq!(palette.border_dim, Rgb::new(22, 22, 29)); // editorGroup.border
        assert_eq!(palette.accent, Rgb::new(126, 156, 216)); // list.highlightForeground
        assert_eq!(palette.accent_hot, Rgb::new(127, 180, 202)); // terminal.ansiBrightBlue
        assert_eq!(palette.accent_warm, Rgb::new(255, 160, 102)); // warm bracket accent
        assert_eq!(palette.warning, Rgb::new(255, 158, 59)); // editorWarning.foreground
        assert_eq!(palette.success, Rgb::new(152, 187, 108)); // terminal.ansiBrightGreen
        assert_eq!(palette.text, Rgb::new(220, 215, 186)); // editor.foreground
        assert_eq!(palette.text_dim, Rgb::new(200, 192, 147)); // statusBar.foreground
        assert_eq!(palette.user, Rgb::new(230, 195, 132)); // terminal.ansiBrightYellow
        assert_eq!(palette.user_surface, Rgb::new(45, 79, 103)); // remote background
        assert_eq!(palette.error, Rgb::new(232, 36, 36)); // editorError.foreground
    }

    #[test]
    fn quiet_night_builtin_matches_embedded_dark_palette() {
        let palette = Palette::quiet_night();
        assert_eq!(palette.background, Rgb::new(17, 17, 19));
        assert_eq!(palette.surface, Rgb::new(26, 26, 29));
        assert_eq!(palette.surface_raised, Rgb::new(39, 39, 44));
        assert_eq!(palette.code_background, Rgb::new(11, 11, 13));
        assert_eq!(palette.selected, Rgb::new(39, 39, 44));
        assert_eq!(palette.muted, Rgb::new(134, 133, 145));
        assert_eq!(palette.border, Rgb::new(39, 39, 44));
        assert_eq!(palette.border_dim, Rgb::new(26, 26, 29));
        assert_eq!(palette.accent, Rgb::new(124, 167, 223));
        assert_eq!(palette.accent_hot, Rgb::new(100, 194, 204));
        assert_eq!(palette.accent_warm, Rgb::new(220, 154, 108));
        assert_eq!(palette.warning, Rgb::new(211, 180, 101));
        assert_eq!(palette.success, Rgb::new(135, 191, 129));
        assert_eq!(palette.text, Rgb::new(202, 201, 211));
        assert_eq!(palette.text_dim, Rgb::new(158, 157, 168));
        assert_eq!(palette.user, Rgb::new(100, 194, 204));
        assert_eq!(palette.user_surface, Rgb::new(39, 39, 44));
        assert_eq!(palette.error, Rgb::new(216, 131, 129));
    }

    #[test]
    fn quiet_night_resolves_as_builtin_theme() {
        let resolved = resolve_palette(Appearance::Dark, Some("quiet-night"));
        assert_eq!(resolved.resolved, "quiet-night");
        assert_eq!(resolved.palette, Palette::quiet_night());
        assert!(resolved.warning.is_none());

        let alias = resolve_palette(Appearance::Dark, Some("quiet_night"));
        assert_eq!(alias.resolved, "quiet-night");
        assert_eq!(alias.palette, Palette::quiet_night());
        assert!(alias.warning.is_none());
    }

    #[test]
    fn yeet_ember_remains_a_distinct_builtin() {
        let resolved = resolve_palette(Appearance::Dark, Some("yeet"));
        assert_eq!(resolved.resolved, "yeet");
        assert_eq!(resolved.palette, Palette::yeet());
        assert_ne!(resolved.palette, Palette::kanagawa());
        assert!(resolved.warning.is_none());
    }

    #[test]
    fn vscode_import_maps_semantic_colors_and_derives_surfaces() {
        let theme = ThemeFile::parse(
            r##"{"colors":{"editor.background":"#112233","editor.foreground":"#ddeeff","focusBorder":"#6699cc"}}"##,
        )
        .unwrap();
        let palette = theme.apply(Palette::kanagawa());
        assert_eq!(palette.background, Rgb::new(17, 34, 51));
        assert_eq!(palette.text, Rgb::new(221, 238, 255));
        assert_eq!(palette.accent, Rgb::new(102, 153, 204));
        assert_ne!(palette.surface, Palette::kanagawa().surface);
    }

    #[test]
    fn neovim_palette_lua_maps_quiet_night_roles() {
        let theme = ThemeFile::parse(
            r##"
M.dark = {
  bg_float = { gui = "#0b0b0d" },
  bg = { gui = "#111113" },
  bg_cursor = { gui = "#1a1a1d" },
  bg_visual = { gui = "#27272c" },
  comment = { gui = "#868591" },
  ui = { gui = "#9e9da8" },
  fg = { gui = "#cac9d3" },
  red = { gui = "#d88381" },
  orange = { gui = "#dc9a6c" },
  yellow = { gui = "#d3b465" },
  green = { gui = "#87bf81" },
  cyan = { gui = "#64c2cc" },
  blue = { gui = "#7ca7df" },
  magenta = { gui = "#b995ca" },
}
"##,
        )
        .unwrap();
        let palette = theme.apply(Palette::kanagawa());
        assert_eq!(palette.background, Rgb::new(17, 17, 19));
        assert_eq!(palette.surface, Rgb::new(26, 26, 29));
        assert_eq!(palette.surface_raised, Rgb::new(39, 39, 44));
        assert_eq!(palette.code_background, Rgb::new(11, 11, 13));
        assert_eq!(palette.selected, Rgb::new(39, 39, 44));
        assert_eq!(palette.muted, Rgb::new(134, 133, 145));
        assert_eq!(palette.border, Rgb::new(39, 39, 44));
        assert_eq!(palette.border_dim, Rgb::new(26, 26, 29));
        assert_eq!(palette.accent, Rgb::new(124, 167, 223));
        assert_eq!(palette.accent_hot, Rgb::new(100, 194, 204));
        assert_eq!(palette.accent_warm, Rgb::new(220, 154, 108));
        assert_eq!(palette.warning, Rgb::new(211, 180, 101));
        assert_eq!(palette.success, Rgb::new(135, 191, 129));
        assert_eq!(palette.text, Rgb::new(202, 201, 211));
        assert_eq!(palette.text_dim, Rgb::new(158, 157, 168));
        assert_eq!(palette.user, Rgb::new(100, 194, 204));
        assert_eq!(palette.user_surface, Rgb::new(39, 39, 44));
        assert_eq!(palette.error, Rgb::new(216, 131, 129));
    }

    #[test]
    fn theme_directory_loads_lua_palette_files() {
        let root = tempfile::tempdir().unwrap();
        let palette_dir = root.path().join("lua/quiet-night");
        std::fs::create_dir_all(&palette_dir).unwrap();
        std::fs::write(
            palette_dir.join("palette.lua"),
            r##"
local M = {}
M.dark = {
  bg_float = { gui = "#0b0b0d" },
  bg = { gui = "#111113" },
  bg_cursor = { gui = "#1a1a1d" },
  bg_visual = { gui = "#27272c" },
  comment = { gui = "#868591" },
  ui = { gui = "#9e9da8" },
  fg = { gui = "#cac9d3" },
  red = { gui = "#d88381" },
  orange = { gui = "#dc9a6c" },
  yellow = { gui = "#d3b465" },
  green = { gui = "#87bf81" },
  cyan = { gui = "#64c2cc" },
  blue = { gui = "#7ca7df" },
}
return M
"##,
        )
        .unwrap();

        let theme = ThemeFile::load(root.path()).unwrap();
        let palette = theme.apply(Palette::kanagawa());
        assert_eq!(palette.background, Rgb::new(17, 17, 19));
        assert_eq!(palette.accent_warm, Rgb::new(220, 154, 108));
        assert_eq!(palette.accent, Rgb::new(124, 167, 223));
        assert_eq!(palette.error, Rgb::new(216, 131, 129));
    }

    #[test]
    fn explicit_semantic_roles_are_supported() {
        let theme = ThemeFile::parse(
            r##"{"yeet":{"background":"#101010","surfaceRaised":"#222222","borderDim":"#333333"}}"##,
        )
        .unwrap();
        let palette = theme.apply(Palette::kanagawa());
        assert_eq!(palette.surface_raised, Rgb::new(34, 34, 34));
        assert_eq!(palette.border_dim, Rgb::new(51, 51, 51));
    }
}
