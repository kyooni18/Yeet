use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::platform::{replace_file, set_private_directory, set_private_file};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attached: Option<Vec<String>>,
    #[serde(default)]
    pub disabled: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FoundationMemorySettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_foundation_server")]
    pub server: String,
}

impl Default for FoundationMemorySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            server: default_foundation_server(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ContextSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_set_tokens: Option<u64>,
    pub unknown_model_tokens: u64,
    pub warning_percent: u64,
    pub rollover_percent: u64,
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            working_set_tokens: None,
            unknown_model_tokens: 32_768,
            warning_percent: 70,
            rollover_percent: 90,
        }
    }
}

impl ContextSettings {
    pub fn validate(&self) -> Result<()> {
        if self
            .working_set_tokens
            .is_some_and(|tokens| !(4096..=2_000_000).contains(&tokens))
            || !(4096..=2_000_000).contains(&self.unknown_model_tokens)
            || !(1..self.rollover_percent).contains(&self.warning_percent)
            || !(2..=90).contains(&self.rollover_percent)
        {
            bail!(
                "Invalid context settings: explicit working-set caps and unknown-model budgets must be 4096..2000000 and 0 < warningPercent < rolloverPercent <= 90"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettings {
    #[serde(default = "version_one")]
    pub version: u64,
    #[serde(default)]
    pub capabilities: ProjectCapabilities,
    #[serde(default)]
    pub openai_flex: bool,
    #[serde(default, rename = "memory", alias = "foundationMemory")]
    pub foundation_memory: FoundationMemorySettings,
    #[serde(default)]
    pub context: ContextSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    // Project settings are intentionally extensible. Capability toggles are
    // owned by this Rust store, but other project-scoped features may persist
    // their own keys in the same document. Preserve those keys whenever the
    // capability UI updates its slice of the file.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            version: 1,
            capabilities: ProjectCapabilities::default(),
            openai_flex: true,
            foundation_memory: FoundationMemorySettings::default(),
            context: ContextSettings::default(),
            project_id: None,
            extra: BTreeMap::new(),
        }
    }
}

fn version_one() -> u64 {
    1
}

fn default_true() -> bool {
    true
}

fn default_foundation_server() -> String {
    "foundation".into()
}

#[derive(Debug, Clone)]
pub struct ProjectSettingsStore {
    workspace_root: PathBuf,
}

impl ProjectSettingsStore {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Result<Self> {
        let root = workspace_root.into();
        let root = root.canonicalize().with_context(|| {
            format!(
                "Project settings workspace root is not a directory: {}",
                root.display()
            )
        })?;
        if !root.is_dir() {
            bail!(
                "Project settings workspace root is not a directory: {}",
                root.display()
            );
        }
        Ok(Self {
            workspace_root: root,
        })
    }

    pub fn directory(&self) -> PathBuf {
        self.workspace_root.join(".yeet")
    }
    pub fn path(&self) -> PathBuf {
        self.directory().join("settings.json")
    }

    pub fn ensure(&self) -> Result<()> {
        let directory = self.directory();
        if directory.exists() {
            reject_symlink(&directory)?;
        }
        fs::create_dir_all(&directory)?;
        set_private_directory(&directory)?;
        if !self.path().exists() {
            self.save(&ProjectSettings::default())?;
        } else {
            reject_symlink(&self.path())?;
            set_private_file(&self.path())?;
        }
        Ok(())
    }

    pub fn load(&self) -> Result<ProjectSettings> {
        self.ensure_directory()?;
        let path = self.path();
        if !path.exists() {
            return Ok(ProjectSettings::default());
        }
        reject_symlink(&path)?;
        let data = fs::read(&path)?;
        let mut settings: ProjectSettings = serde_json::from_slice(&data)
            .context("Invalid project settings: document does not match version 1 schema")?;
        if settings.version == 0 {
            settings.version = 1;
        }
        normalize_capabilities(&mut settings.capabilities);
        normalize_foundation_memory(&mut settings.foundation_memory);
        settings.context.validate()?;
        Ok(settings)
    }

    pub fn save(&self, settings: &ProjectSettings) -> Result<()> {
        self.ensure_directory()?;
        let path = self.path();
        if path.exists() {
            reject_symlink(&path)?;
        }
        let mut normalized = settings.clone();
        normalized.version = 1;
        normalize_capabilities(&mut normalized.capabilities);
        normalize_foundation_memory(&mut normalized.foundation_memory);
        normalized.context.validate()?;
        let tmp = self.directory().join(format!(
            ".settings.json.{}.{}.tmp",
            std::process::id(),
            Uuid::new_v4()
        ));
        let mut data = serde_json::to_vec_pretty(&normalized)?;
        data.push(b'\n');
        fs::write(&tmp, data)?;
        set_private_file(&tmp)?;
        replace_file(&tmp, &path)?;
        set_private_file(&path)?;
        Ok(())
    }

    pub fn save_capabilities(
        &self,
        attached: Option<Vec<String>>,
        disabled: Vec<String>,
    ) -> Result<()> {
        let mut settings = self.load()?;
        settings.capabilities = ProjectCapabilities { attached, disabled };
        self.save(&settings)
    }

    pub fn save_openai_flex(&self, enabled: bool) -> Result<()> {
        let mut settings = self.load()?;
        settings.openai_flex = enabled;
        self.save(&settings)
    }

    pub fn save_foundation_memory(&self, enabled: bool, server: Option<&str>) -> Result<()> {
        let mut settings = self.load()?;
        settings.foundation_memory.enabled = enabled;
        if let Some(server) = server {
            settings.foundation_memory.server = server.to_owned();
        }
        normalize_foundation_memory(&mut settings.foundation_memory);
        self.save(&settings)
    }

    pub fn project_identity(&self) -> Result<String> {
        let mut settings = self.load()?;
        if let Some(existing) = settings
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.contains('\n') && value.len() <= 200)
        {
            return Ok(existing.to_owned());
        }
        let id = format!("yeet-project:{}", Uuid::new_v4());
        settings.project_id = Some(id.clone());
        self.save(&settings)?;
        Ok(id)
    }

    fn ensure_directory(&self) -> Result<()> {
        let directory = self.directory();
        if directory.exists() {
            reject_symlink(&directory)?;
        }
        fs::create_dir_all(&directory)?;
        set_private_directory(&directory)?;
        Ok(())
    }
}

fn normalize_capabilities(capabilities: &mut ProjectCapabilities) {
    if let Some(attached) = capabilities.attached.as_mut() {
        attached.sort();
        attached.dedup();
    }
    capabilities.disabled.sort();
    capabilities.disabled.dedup();
}

fn normalize_foundation_memory(settings: &mut FoundationMemorySettings) {
    settings.server = settings.server.trim().to_owned();
    if settings.server.is_empty() {
        settings.server = default_foundation_server();
    }
}

fn reject_symlink(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        bail!("Refusing project settings symlink: {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_project_settings_and_round_trips_capabilities() {
        let workspace = tempfile::tempdir().unwrap();
        let store = ProjectSettingsStore::new(workspace.path()).unwrap();
        store.ensure().unwrap();
        assert!(workspace.path().join(".yeet/settings.json").is_file());

        store
            .save_capabilities(
                Some(vec![
                    "web-search".into(),
                    "context-mode".into(),
                    "web-search".into(),
                ]),
                vec![
                    "builtin:file-write".into(),
                    "skill:test".into(),
                    "builtin:file-write".into(),
                ],
            )
            .unwrap();
        let settings = store.load().unwrap();
        assert_eq!(
            settings.capabilities.attached,
            Some(vec!["context-mode".into(), "web-search".into()])
        );
        assert_eq!(
            settings.capabilities.disabled,
            vec!["builtin:file-write", "skill:test"]
        );
    }

    #[test]
    fn capability_updates_preserve_other_project_settings() {
        let workspace = tempfile::tempdir().unwrap();
        let store = ProjectSettingsStore::new(workspace.path()).unwrap();
        store.ensure().unwrap();
        fs::write(
            store.path(),
            r#"{
  "version": 1,
  "capabilities": { "disabled": [] },
  "providers": {
    "local": {
      "baseUrl": "http://127.0.0.1:1234/v1"
    }
  },
  "customFeature": { "enabled": true }
}
"#,
        )
        .unwrap();

        store
            .save_capabilities(
                Some(vec!["context-mode".into()]),
                vec!["builtin:file-write".into()],
            )
            .unwrap();

        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(
            value["providers"]["local"]["baseUrl"],
            "http://127.0.0.1:1234/v1"
        );
        assert_eq!(value["customFeature"]["enabled"], true);
        assert_eq!(value["capabilities"]["attached"][0], "context-mode");
        assert_eq!(value["capabilities"]["disabled"][0], "builtin:file-write");
    }

    #[test]
    fn openai_flex_round_trips_without_overwriting_other_settings() {
        let workspace = tempfile::tempdir().unwrap();
        let store = ProjectSettingsStore::new(workspace.path()).unwrap();
        store.ensure().unwrap();
        fs::write(
            store.path(),
            r#"{
  "version": 1,
  "capabilities": { "disabled": ["skill:test"] },
  "customFeature": { "enabled": true }
}
"#,
        )
        .unwrap();

        store.save_openai_flex(true).unwrap();

        let settings = store.load().unwrap();
        assert!(settings.openai_flex);
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(value["openaiFlex"], true);
        assert_eq!(value["customFeature"]["enabled"], true);
        assert_eq!(value["capabilities"]["disabled"][0], "skill:test");
    }

    #[test]
    fn foundation_memory_defaults_on_and_round_trips_per_project() {
        let workspace = tempfile::tempdir().unwrap();
        let store = ProjectSettingsStore::new(workspace.path()).unwrap();
        let settings = store.load().unwrap();
        assert!(settings.foundation_memory.enabled);
        assert_eq!(settings.foundation_memory.server, "foundation");

        store
            .save_foundation_memory(false, Some("foundation-local"))
            .unwrap();
        let settings = store.load().unwrap();
        assert!(!settings.foundation_memory.enabled);
        assert_eq!(settings.foundation_memory.server, "foundation-local");
    }

    #[test]
    fn project_identity_is_persistent_and_distinct() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_store = ProjectSettingsStore::new(first.path()).unwrap();
        let second_store = ProjectSettingsStore::new(second.path()).unwrap();

        let identity = first_store.project_identity().unwrap();
        assert!(identity.starts_with("yeet-project:"));
        assert_eq!(first_store.project_identity().unwrap(), identity);
        assert_ne!(second_store.project_identity().unwrap(), identity);
    }
}

#[cfg(test)]
mod context_settings_tests {
    use super::*;
    #[test]
    fn context_policy_defaults_merges_and_rejects_unsafe_thresholds() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectSettingsStore::new(dir.path()).unwrap();
        let defaults = ProjectSettings::default();
        assert_eq!(defaults.context.working_set_tokens, None);
        assert_eq!(defaults.context.warning_percent, 70);
        assert_eq!(defaults.context.rollover_percent, 90);
        let mut settings: ProjectSettings =
            serde_json::from_value(serde_json::json!({"context":{"workingSetTokens":12000}}))
                .unwrap();
        assert_eq!(settings.context.working_set_tokens, Some(12000));
        assert_eq!(settings.context.warning_percent, 70);
        store.save(&settings).unwrap();
        assert_eq!(
            store.load().unwrap().context.working_set_tokens,
            Some(12000)
        );
        settings.context.warning_percent = 90;
        assert!(store.save(&settings).is_err());
        assert_eq!(store.load().unwrap().context.warning_percent, 70);
    }
}
