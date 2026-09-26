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

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceBackend {
    #[default]
    Builtin,
    Mcp,
}

impl ServiceBackend {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "builtin" | "internal" | "native" => Ok(Self::Builtin),
            "mcp" | "external" => Ok(Self::Mcp),
            other => bail!("Unknown backend '{other}'; expected builtin or mcp"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Mcp => "mcp",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FoundationMemorySettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub backend: ServiceBackend,
    #[serde(default = "default_foundation_server")]
    pub server: String,
}

impl Default for FoundationMemorySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: ServiceBackend::Builtin,
            server: default_foundation_server(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebBackendSettings {
    #[serde(default)]
    pub backend: ServiceBackend,
    #[serde(default = "default_web_server")]
    pub server: String,
}

impl Default for WebBackendSettings {
    fn default() -> Self {
        Self {
            backend: ServiceBackend::Builtin,
            server: default_web_server(),
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
    pub web: WebBackendSettings,
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
            web: WebBackendSettings::default(),
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

fn default_web_server() -> String {
    "web".into()
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
        normalize_web_backend(&mut settings.web);
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
        normalize_web_backend(&mut normalized.web);
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

    pub fn save_foundation_backend(
        &self,
        backend: ServiceBackend,
        server: Option<&str>,
    ) -> Result<()> {
        let mut settings = self.load()?;
        settings.foundation_memory.backend = backend;
        if let Some(server) = server {
            settings.foundation_memory.server = server.to_owned();
        }
        normalize_foundation_memory(&mut settings.foundation_memory);
        self.save(&settings)
    }

    pub fn save_web_backend(&self, backend: ServiceBackend, server: Option<&str>) -> Result<()> {
        let mut settings = self.load()?;
        settings.web.backend = backend;
        if let Some(server) = server {
            settings.web.server = server.to_owned();
        }
        normalize_web_backend(&mut settings.web);
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
        attached.retain(|value| !retired_capability(value));
        attached.sort();
        attached.dedup();
    }
    capabilities
        .disabled
        .retain(|value| !retired_capability(value));
    capabilities.disabled.sort();
    capabilities.disabled.dedup();
}

fn retired_capability(value: &str) -> bool {
    matches!(value, "lead" | "context-mode" | "builtin:artifacts")
}

fn normalize_foundation_memory(settings: &mut FoundationMemorySettings) {
    settings.server = settings.server.trim().to_owned();
    if settings.server.is_empty() {
        settings.server = default_foundation_server();
    }
}

fn normalize_web_backend(settings: &mut WebBackendSettings) {
    settings.server = settings.server.trim().to_owned();
    if settings.server.is_empty() {
        settings.server = default_web_server();
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
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn service_backend_parse_accepts_aliases() {
        assert_eq!(
            ServiceBackend::parse("builtin").unwrap(),
            ServiceBackend::Builtin
        );
        assert_eq!(
            ServiceBackend::parse("internal").unwrap(),
            ServiceBackend::Builtin
        );
        assert_eq!(
            ServiceBackend::parse("native").unwrap(),
            ServiceBackend::Builtin
        );
        assert_eq!(ServiceBackend::parse("mcp").unwrap(), ServiceBackend::Mcp);
        assert_eq!(
            ServiceBackend::parse("external").unwrap(),
            ServiceBackend::Mcp
        );
        assert!(ServiceBackend::parse("remote-http").is_err());
    }

    #[test]
    fn legacy_project_settings_default_services_to_builtin() {
        let directory = tempdir().unwrap();
        let store = ProjectSettingsStore::new(directory.path()).unwrap();
        fs::create_dir_all(store.directory()).unwrap();
        fs::write(
            store.path(),
            serde_json::to_vec_pretty(&json!({
                "version": 1,
                "foundationMemory": {
                    "enabled": true,
                    "server": " foundation "
                },
                "context": ContextSettings::default()
            }))
            .unwrap(),
        )
        .unwrap();

        let settings = store.load().unwrap();
        assert_eq!(settings.foundation_memory.backend, ServiceBackend::Builtin);
        assert_eq!(settings.foundation_memory.server, "foundation");
        assert_eq!(settings.web.backend, ServiceBackend::Builtin);
        assert_eq!(settings.web.server, "web");
    }

    #[test]
    fn service_backend_selection_round_trips_and_preserves_other_settings() {
        let directory = tempdir().unwrap();
        let store = ProjectSettingsStore::new(directory.path()).unwrap();
        store.ensure().unwrap();

        let mut initial = store.load().unwrap();
        initial
            .extra
            .insert("customFeature".into(), json!({"enabled": true}));
        store.save(&initial).unwrap();

        store
            .save_foundation_backend(ServiceBackend::Mcp, Some(" foundation-external "))
            .unwrap();
        store
            .save_web_backend(ServiceBackend::Mcp, Some(" web-external "))
            .unwrap();

        let settings = store.load().unwrap();
        assert_eq!(settings.foundation_memory.backend, ServiceBackend::Mcp);
        assert_eq!(settings.foundation_memory.server, "foundation-external");
        assert_eq!(settings.web.backend, ServiceBackend::Mcp);
        assert_eq!(settings.web.server, "web-external");
        assert_eq!(
            settings.extra.get("customFeature"),
            Some(&json!({"enabled": true}))
        );
    }
}
