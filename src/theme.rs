//! Compatibility host adapter for theme resource loading and shared UI projection.
#[cfg(test)]
use crate::harness::theme_resources::ThemeFile;
pub use crate::harness::theme_resources::validate_reference;
pub use crate::shared_ui::theme::{
    Appearance, Palette, PaletteState, ResolvedTheme, Rgb, ThemeCatalogItem, apply_resource,
    catalog, default_theme_id,
};
pub fn resolve_palette(appearance: Appearance, name_or_path: Option<&str>) -> ResolvedTheme {
    let requested = name_or_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_theme_id(appearance));
    if crate::harness::theme_resources::builtin_id(requested).is_some() {
        return crate::shared_ui::theme::resolve_palette(appearance, requested, Ok(None));
    }
    let source = crate::harness::theme_resources::ThemeFile::load(std::path::Path::new(requested));
    crate::shared_ui::theme::resolve_palette(
        appearance,
        requested,
        source.as_ref().map(Some).map_err(Clone::clone),
    )
}
/// Host-only compatibility enrichment. Harness publishes preferences; UI owns
/// their palette projection. Cache avoids reading custom resources per token.
#[derive(Default)]
pub struct ThemeProjectionCache {
    key: Option<(String, String, String)>,
    resolved: Option<(ResolvedTheme, ResolvedTheme, Vec<ThemeCatalogItem>)>,
}
impl ThemeProjectionCache {
    pub fn invalidate(&mut self) {
        self.key = None;
    }
    pub fn invalidate_for_command(&mut self, command: &crate::harness::HarnessCommand) {
        if matches!(
            command,
            crate::harness::HarnessCommand::RequestSettings
                | crate::harness::HarnessCommand::SetAppearance { .. }
                | crate::harness::HarnessCommand::SetTheme { .. }
        ) {
            self.invalidate();
        }
    }
    pub fn hydrate(&mut self, settings: &mut crate::model::RuntimeSettingsState) {
        let key = (
            settings.appearance.clone(),
            settings.theme_dark.clone(),
            settings.theme_light.clone(),
        );
        if self.key.as_ref() != Some(&key) {
            let dark = resolve_palette(Appearance::Dark, Some(&settings.theme_dark));
            let light = resolve_palette(Appearance::Light, Some(&settings.theme_light));
            self.resolved = Some((dark, light, catalog()));
            self.key = Some(key);
        }
        if let Some((dark, light, catalog)) = &self.resolved {
            settings.theme_dark_resolved = dark.resolved.clone();
            settings.theme_light_resolved = light.resolved.clone();
            settings.theme_dark_palette = dark.palette.into();
            settings.theme_light_palette = light.palette.into();
            settings.theme_dark_warning = dark.warning.clone();
            settings.theme_light_warning = light.warning.clone();
            settings.theme_catalog = catalog.clone();
        }
    }
}
pub fn hydrate_runtime_settings(settings: &mut crate::model::RuntimeSettingsState) {
    ThemeProjectionCache::default().hydrate(settings);
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_cache_preserves_wire_projection_and_refreshes_same_resource_on_request() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("theme.json");
        std::fs::write(&path, r##"{"yeet":{"background":"#112233"}}"##).unwrap();
        let mut settings = crate::model::RuntimeSettingsState::default();
        assert!(settings.theme_catalog.is_empty());
        settings.theme_dark = path.to_string_lossy().into_owned();
        let mut cache = ThemeProjectionCache::default();
        cache.hydrate(&mut settings);
        assert_eq!(settings.theme_dark_palette.background, "#112233");
        assert!(!settings.theme_catalog.is_empty());
        std::fs::write(&path, r##"{"yeet":{"background":"#445566"}}"##).unwrap();
        cache.hydrate(&mut settings);
        assert_eq!(settings.theme_dark_palette.background, "#112233");
        cache.invalidate_for_command(&crate::harness::HarnessCommand::RequestSettings);
        cache.hydrate(&mut settings);
        assert_eq!(settings.theme_dark_palette.background, "#445566");
        std::fs::remove_file(path).unwrap();
        cache.invalidate();
        cache.hydrate(&mut settings);
        assert_eq!(settings.theme_dark_resolved, "kanagawa");
        assert!(settings.theme_dark_warning.is_some());
    }
    #[test]
    fn shared_preference_projection_respects_system_appearance_and_explicit_choice() {
        use crate::shared_ui::theme::{palette_for_settings, project_preferences};
        let mut preferences = crate::harness::theme_resources::ThemePreferences::default();
        let auto = project_preferences(&preferences, Appearance::Light, Ok(None), Ok(None));
        assert_eq!(auto.effective, Appearance::Light);
        assert_eq!(auto.light.palette, Palette::adwaita());
        preferences.appearance = Some("dark".into());
        let explicit = project_preferences(&preferences, Appearance::Light, Ok(None), Ok(None));
        assert_eq!(explicit.effective, Appearance::Dark);
        let mut settings = crate::model::RuntimeSettingsState::default();
        hydrate_runtime_settings(&mut settings);
        assert_eq!(
            palette_for_settings(&settings, Appearance::Light),
            Palette::adwaita()
        );
    }

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
        let palette = apply_resource(&theme, Palette::kanagawa());
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
        let palette = apply_resource(&theme, Palette::kanagawa());
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
        let palette = apply_resource(&theme, Palette::kanagawa());
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
        let palette = apply_resource(&theme, Palette::kanagawa());
        assert_eq!(palette.surface_raised, Rgb::new(34, 34, 34));
        assert_eq!(palette.border_dim, Rgb::new(51, 51, 51));
    }
}
