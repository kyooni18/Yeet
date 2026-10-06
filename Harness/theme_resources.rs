//! Filesystem theme resources and neutral imported colors; no application palette rules.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}
impl ResourceColor {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThemePreferences {
    pub appearance: Option<String>,
    pub dark: Option<String>,
    pub light: Option<String>,
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

pub fn builtin_id(value: &str) -> Option<&'static str> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "quiet-night" | "quietnight" | "quiet_night" => "quiet-night",
        "kanagawa" | "kanagawa-wave" | "wave" => "kanagawa",
        "yeet" | "ember" | "yeet-ember" => "yeet",
        "nord" => "nord",
        "catppuccin" | "catppuccin-mocha" | "mocha" => "catppuccin-mocha",
        "rose-pine" | "rosepine" => "rose-pine",
        "adwaita" => "adwaita",
        "solarized-light" | "solarized" => "solarized-light",
        _ => return None,
    })
}
#[derive(Debug, Default)]
pub struct ThemeFile {
    pub colors: Vec<(String, ResourceColor)>,
}

impl ThemeFile {
    pub fn load(path: &Path) -> Result<Self, String> {
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

    pub fn parse(source: &str) -> Result<Self, String> {
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

fn add_neovim_palette_aliases(colors: &mut Vec<(String, ResourceColor)>) {
    let snapshot = colors.clone();
    let find = |name: &str| -> Option<ResourceColor> {
        snapshot
            .iter()
            .rev()
            .find_map(|(key, color)| key.eq_ignore_ascii_case(name).then_some(*color))
    };
    let mut push = |role: &str, color: Option<ResourceColor>| {
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

fn collect_json_colors(value: &Value, prefix: String, colors: &mut Vec<(String, ResourceColor)>) {
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

pub fn parse_color(value: &str) -> Option<ResourceColor> {
    let value = value.trim().trim_matches(['"', '\'']);
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(ResourceColor::new(red, green, blue))
}
pub fn validate_reference(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("Theme name or path cannot be empty".into());
    }
    if builtin_id(value).is_some() {
        return Ok(());
    }
    ThemeFile::load(Path::new(value)).map(|_| ())
}
