use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{model::normalize_reasoning_level, platform::replace_file};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigDocument {
    #[serde(default = "version_one")]
    version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_mode: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    context_lengths: std::collections::BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    context_length_overrides: std::collections::BTreeMap<String, u64>,
    // config.json is shared with the TypeScript runtime. Keep fields owned by
    // that side (notably `providers`) intact whenever Rust updates its own
    // settings instead of silently deleting them on the next write.
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

fn version_one() -> u64 {
    1
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    pub directory: PathBuf,
}

impl Default for ConfigStore {
    fn default() -> Self {
        let directory = env::var_os("YEET_CONFIG_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".yeet")))
            .unwrap_or_else(|| PathBuf::from(".yeet"));
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

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.directory)
            .with_context(|| format!("create {}", self.directory.display()))?;
        #[cfg(unix)]
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        if !self.config_path().exists() {
            self.write(&ConfigDocument {
                version: 1,
                ..Default::default()
            })?;
        } else {
            #[cfg(unix)]
            fs::set_permissions(self.config_path(), fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn model(&self) -> Result<Option<String>> {
        Ok(self.read()?.model)
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

    pub fn agent_mode(&self) -> Result<Option<String>> {
        Ok(self
            .read()?
            .agent_mode
            .and_then(|value| crate::model::normalize_agent_mode(&value).map(str::to_owned)))
    }

    pub fn set_agent_mode(&self, mode: &str) -> Result<String> {
        let mode = crate::model::normalize_agent_mode(mode)
            .ok_or_else(|| {
                anyhow::anyhow!("Invalid agent mode: {mode}. Use auto, code, or general.")
            })?
            .to_owned();
        let mut document = self.read()?;
        document.version = 1;
        document.agent_mode = Some(mode.clone());
        self.write(&document)?;
        Ok(mode)
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
        Ok(document)
    }

    fn ensure_no_recurse(&self) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        #[cfg(unix)]
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
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
        #[cfg(unix)]
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        replace_file(&tmp, &path)?;
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_model_ids() {
        assert_eq!(validate_model_id("openai/gpt-5").unwrap(), "openai/gpt-5");
        assert!(validate_model_id("gpt-5").is_err());
        assert!(validate_model_id("bad provider/model").is_err());
    }

    #[test]
    fn config_round_trips_model_and_context_length() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(temp.path());
        assert_eq!(store.model().unwrap(), None);
        assert_eq!(store.set_model("openai/test").unwrap(), "openai/test");
        store
            .set_context_length("openai/test", Some(128_000))
            .unwrap();
        store
            .set_context_length_override("openai/test", Some(96_000))
            .unwrap();
        assert_eq!(store.model().unwrap().as_deref(), Some("openai/test"));
        assert_eq!(store.context_length("openai/test").unwrap(), Some(128_000));
        assert_eq!(
            store.context_length_override("openai/test").unwrap(),
            Some(96_000)
        );
        store
            .set_context_length_override("openai/test", None)
            .unwrap();
        assert_eq!(store.context_length_override("openai/test").unwrap(), None);
    }

    #[test]
    fn parses_context_length_suffixes() {
        assert_eq!(parse_context_length("128k").unwrap(), 128_000);
        assert_eq!(parse_context_length("1M").unwrap(), 1_000_000);
        assert_eq!(parse_context_length("128_000").unwrap(), 128_000);
        assert_eq!(parse_context_length("128,000").unwrap(), 128_000);
        assert!(parse_context_length("0").is_err());
        assert!(parse_context_length("128kb").is_err());
    }

    #[test]
    fn config_round_trips_reasoning_level() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(temp.path());
        assert_eq!(store.reasoning_level().unwrap(), None);
        assert_eq!(store.set_reasoning_level("HIGH").unwrap(), "high");
        assert_eq!(store.reasoning_level().unwrap().as_deref(), Some("high"));
        assert!(store.set_reasoning_level("extreme").is_err());
        assert_eq!(store.set_agent_mode("CODE").unwrap(), "code");
        assert_eq!(store.agent_mode().unwrap().as_deref(), Some("code"));
        assert!(store.set_agent_mode("invalid").is_err());
    }

    #[test]
    fn config_preserves_runtime_owned_provider_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(temp.path());
        store.ensure().unwrap();
        fs::write(
            store.config_path(),
            r#"{
  "version": 1,
  "providers": {
    "local": {
      "id": "local",
      "baseUrl": "http://127.0.0.1:1234/v1",
      "requireApiKey": false
    }
  }
}
"#,
        )
        .unwrap();

        store.set_model("local/test-model").unwrap();
        store.set_reasoning_level("medium").unwrap();
        store
            .set_context_length("local/test-model", Some(32_768))
            .unwrap();
        store
            .set_context_length_override("local/test-model", Some(24_000))
            .unwrap();

        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(store.config_path()).unwrap()).unwrap();
        assert_eq!(value["providers"]["local"]["id"], "local");
        assert_eq!(
            value["providers"]["local"]["baseUrl"],
            "http://127.0.0.1:1234/v1"
        );
        assert_eq!(value["providers"]["local"]["requireApiKey"], false);
        assert_eq!(value["model"], "local/test-model");
        assert_eq!(value["reasoningLevel"], "medium");
        assert_eq!(value["contextLengths"]["local/test-model"], 32_768);
        assert_eq!(value["contextLengthOverrides"]["local/test-model"], 24_000);
    }
}
