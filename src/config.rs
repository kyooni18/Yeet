use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    model::{ModelCatalogItem, normalize_reasoning_level},
    platform::{default_config_directory, replace_file, set_private_directory, set_private_file},
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigDocument {
    #[serde(default = "version_one")]
    version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    theme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    theme_dark: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    theme_light: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    appearance: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    context_lengths: std::collections::BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    context_length_overrides: std::collections::BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    jev_loop_mode: Option<String>,
    // config.json is shared with the TypeScript runtime. Keep fields owned by
    // that side (notably `providers`) intact whenever Rust updates its own
    // settings instead of silently deleting them on the next write.
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelCatalogCacheDocument {
    #[serde(default = "version_one")]
    version: u64,
    #[serde(default)]
    models: Vec<ModelCatalogItem>,
}

fn version_one() -> u64 {
    1
}

#[derive(Debug, Clone, Default)]
pub struct ThemeSettings {
    pub theme: Option<String>,
    pub dark: Option<String>,
    pub light: Option<String>,
    pub appearance: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    pub directory: PathBuf,
}

impl Default for ConfigStore {
    fn default() -> Self {
        let directory = default_config_directory();
        Self { directory }
    }
}

impl ConfigStore {
    #[cfg(test)]
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn config_path(&self) -> PathBuf {
        self.directory.join("config.json")
    }

    pub fn model_catalog_cache_path(&self) -> PathBuf {
        self.directory.join("model-catalog.json")
    }

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.directory)
            .with_context(|| format!("create {}", self.directory.display()))?;
        set_private_directory(&self.directory)?;
        if !self.config_path().exists() {
            self.write(&ConfigDocument {
                version: 1,
                ..Default::default()
            })?;
        } else {
            set_private_file(&self.config_path())?;
        }
        Ok(())
    }

    pub fn model(&self) -> Result<Option<String>> {
        Ok(self.read()?.model)
    }

    /// Theme overrides are kept in config.json so the Rust TUI and the
    /// TypeScript runtime can share the same user choice.
    pub fn theme_settings(&self) -> Result<ThemeSettings> {
        let document = self.read()?;
        let theme = document.theme;
        Ok(ThemeSettings {
            dark: document.theme_dark.or_else(|| theme.clone()),
            light: document.theme_light.or_else(|| theme.clone()),
            theme,
            appearance: document.appearance,
        })
    }

    pub fn set_theme(&self, mode: &str, value: &str) -> Result<()> {
        let value = value.trim();
        if value.is_empty() {
            bail!("Theme name or path cannot be empty");
        }
        let mut document = self.read()?;
        document.version = 1;
        match mode {
            "dark" => document.theme_dark = Some(value.to_owned()),
            "light" => document.theme_light = Some(value.to_owned()),
            "both" => document.theme = Some(value.to_owned()),
            _ => bail!("Theme mode must be dark, light, or both"),
        }
        self.write(&document)
    }

    pub fn set_appearance(&self, appearance: &str) -> Result<()> {
        let appearance = appearance.trim().to_ascii_lowercase();
        if !matches!(appearance.as_str(), "auto" | "dark" | "light") {
            bail!("Appearance must be auto, dark, or light");
        }
        let mut document = self.read()?;
        document.version = 1;
        document.appearance = (appearance != "auto").then_some(appearance);
        self.write(&document)
    }

    pub fn set_model(&self, model: &str) -> Result<String> {
        let model = validate_model_id(model)?;
        let mut document = self.read()?;
        document.version = 1;
        document.model = Some(model.clone());
        self.write(&document)?;
        Ok(model)
    }

    pub fn reasoning_level(&self) -> Result<Option<String>> {
        Ok(self
            .read()?
            .reasoning_level
            .and_then(|value| normalize_reasoning_level(&value).map(str::to_owned)))
    }

    pub fn set_reasoning_level(&self, level: &str) -> Result<String> {
        let level = normalize_reasoning_level(level)
            .ok_or_else(|| {
                anyhow::anyhow!("Invalid reasoning level: {level}. Use auto, low, medium, or high.")
            })?
            .to_owned();
        let mut document = self.read()?;
        document.version = 1;
        document.reasoning_level = Some(level.clone());
        self.write(&document)?;
        Ok(level)
    }

    pub fn context_length(&self, model: &str) -> Result<Option<u64>> {
        Ok(self
            .read()?
            .context_lengths
            .get(model)
            .copied()
            .filter(|value| *value > 0))
    }

    pub fn set_context_length(&self, model: &str, length: Option<u64>) -> Result<()> {
        if model.is_empty() {
            return Ok(());
        }
        let mut document = self.read()?;
        if let Some(length) = length.filter(|value| *value > 0) {
            document.context_lengths.insert(model.to_owned(), length);
        } else {
            document.context_lengths.remove(model);
        }
        self.write(&document)
    }

    pub fn context_length_override(&self, model: &str) -> Result<Option<u64>> {
        Ok(self
            .read()?
            .context_length_overrides
            .get(model)
            .copied()
            .filter(|value| *value > 0))
    }

    pub fn set_context_length_override(&self, model: &str, length: Option<u64>) -> Result<()> {
        if model.is_empty() {
            return Ok(());
        }
        let mut document = self.read()?;
        if let Some(length) = length.filter(|value| *value > 0) {
            document
                .context_length_overrides
                .insert(model.to_owned(), length);
        } else {
            document.context_length_overrides.remove(model);
        }
        self.write(&document)
    }

    pub fn jev_loop_mode(&self) -> Result<String> {
        Ok(self
            .read()?
            .jev_loop_mode
            .filter(|mode| matches!(mode.as_str(), "shadow" | "enforce"))
            .unwrap_or_else(|| "off".into()))
    }

    pub fn set_jev_loop_mode(&self, mode: &str) -> Result<String> {
        let mode = mode.trim().to_ascii_lowercase();
        if !matches!(mode.as_str(), "off" | "shadow" | "enforce") {
            bail!("Jev loop mode must be off, shadow, or enforce");
        }
        let mut document = self.read()?;
        document.version = 1;
        document.jev_loop_mode = (mode != "off").then_some(mode.clone());
        self.write(&document)?;
        Ok(mode)
    }

    pub fn model_catalog_cache(&self) -> Result<Vec<ModelCatalogItem>> {
        self.ensure_no_recurse()?;
        let path = self.model_catalog_cache_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        set_private_file(&path)?;
        let data = fs::read(&path)?;
        let mut document: ModelCatalogCacheDocument = match serde_json::from_slice(&data) {
            Ok(document) => document,
            Err(_) => return Ok(Vec::new()),
        };
        if document.version == 0 {
            document.version = 1;
        }
        document.models.retain(|item| {
            !item.id.is_empty()
                && !item.provider.is_empty()
                && !item.model.is_empty()
                && item.id == format!("{}/{}", item.provider, item.model)
        });
        document
            .models
            .sort_by_key(|item| item.id.to_ascii_lowercase());
        document.models.dedup_by(|lhs, rhs| lhs.id == rhs.id);
        Ok(document.models)
    }

    pub fn set_model_catalog_cache(&self, models: &[ModelCatalogItem]) -> Result<()> {
        self.ensure_no_recurse()?;
        let path = self.model_catalog_cache_path();
        let tmp = self
            .directory
            .join(format!(".model-catalog.json.{}.tmp", std::process::id()));
        let mut data = serde_json::to_vec_pretty(&ModelCatalogCacheDocument {
            version: 1,
            models: models.to_vec(),
        })?;
        data.push(b'\n');
        fs::write(&tmp, data)?;
        set_private_file(&tmp)?;
        replace_file(&tmp, &path)?;
        set_private_file(&path)?;
        Ok(())
    }

    fn read(&self) -> Result<ConfigDocument> {
        self.ensure_no_recurse()?;
        let path = self.config_path();
        if !path.exists() {
            return Ok(ConfigDocument {
                version: 1,
                ..Default::default()
            });
        }
        let data = fs::read(&path)?;
        let mut document: ConfigDocument =
            serde_json::from_slice(&data).unwrap_or(ConfigDocument {
                version: 1,
                ..Default::default()
            });
        if document.version == 0 {
            document.version = 1;
        }
        // Agent mode no longer exists. Do not carry the legacy code/general
        // selector forward through the flattened compatibility fields.
        document.extra.remove("agentMode");
        Ok(document)
    }

    fn ensure_no_recurse(&self) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        set_private_directory(&self.directory)?;
        Ok(())
    }

    fn write(&self, document: &ConfigDocument) -> Result<()> {
        self.ensure_no_recurse()?;
        let path = self.config_path();
        let tmp = self
            .directory
            .join(format!(".config.json.{}.tmp", std::process::id()));
        let mut data = serde_json::to_vec_pretty(document)?;
        data.push(b'\n');
        fs::write(&tmp, data)?;
        set_private_file(&tmp)?;
        replace_file(&tmp, &path)?;
        set_private_file(&path)?;
        Ok(())
    }
}

pub fn validate_model_id(value: &str) -> Result<String> {
    let trimmed = value.trim();
    let Some((provider, model)) = trimmed.split_once('/') else {
        bail!("Invalid model id: {value}. Use provider/model.");
    };
    if provider.is_empty() || model.is_empty() || provider.chars().any(char::is_whitespace) {
        bail!("Invalid model id: {value}. Use provider/model.");
    }
    Ok(trimmed.to_owned())
}

pub fn parse_context_length(value: &str) -> Result<u64> {
    let compact = value.trim().replace(['_', ','], "").to_ascii_lowercase();
    if compact.is_empty() {
        bail!("Context length cannot be empty");
    }
    let (number, multiplier) = match compact.chars().last() {
        Some('k') => (&compact[..compact.len() - 1], 1_000u64),
        Some('m') => (&compact[..compact.len() - 1], 1_000_000u64),
        _ => (compact.as_str(), 1u64),
    };
    let base = number.parse::<u64>().map_err(|_| {
        anyhow::anyhow!(
            "Invalid context length: {value}. Use a positive integer such as 128000, 128k, or 1m."
        )
    })?;
    let length = base
        .checked_mul(multiplier)
        .ok_or_else(|| anyhow::anyhow!("Context length is too large: {value}"))?;
    if length == 0 {
        bail!("Context length must be greater than zero");
    }
    Ok(length)
}
