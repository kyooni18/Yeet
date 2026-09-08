use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::platform::replace_file;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    #[default]
    Sandboxed,
    Unlimited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceRead {
    None,
    All,
    Paths(BTreeSet<String>),
}

impl WorkspaceRead {
    pub fn allows(&self, path: &str) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Paths(paths) => paths
                .iter()
                .any(|root| path == root || path.starts_with(&format!("{root}/"))),
        }
    }

    pub fn restricted_to(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::None, _) | (_, Self::None) => Self::None,
            (Self::All, rhs) => rhs.clone(),
            (lhs, Self::All) => lhs.clone(),
            (Self::Paths(lhs), Self::Paths(rhs)) => {
                let mut result = BTreeSet::new();
                for left in lhs {
                    for right in rhs {
                        if relative_contains(left, right) {
                            result.insert(right.clone());
                        } else if relative_contains(right, left) {
                            result.insert(left.clone());
                        }
                    }
                }
                if result.is_empty() {
                    Self::None
                } else {
                    Self::Paths(result)
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct NetworkEndpoint {
    pub host: String,
    pub port: Option<u16>,
}

impl NetworkEndpoint {
    pub fn new(host: &str, port: Option<u16>) -> Result<Self> {
        let host = host.to_ascii_lowercase();
        validate_host(&host)?;
        Ok(Self { host, port })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SandboxLimits {
    pub wall_time_seconds: u64,
    pub max_stdout_bytes: usize,
    pub max_stderr_bytes: usize,
    pub max_memory_bytes: u64,
    pub max_processes: usize,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            wall_time_seconds: 30,
            max_stdout_bytes: 1_048_576,
            max_stderr_bytes: 1_048_576,
            max_memory_bytes: 536_870_912,
            max_processes: 32,
        }
    }
}

impl SandboxLimits {
    pub fn validate(&self) -> Result<()> {
        if !(1..=86_400).contains(&self.wall_time_seconds) {
            bail!("invalid wallTimeSeconds");
        }
        if !(1..=67_108_864).contains(&self.max_stdout_bytes) {
            bail!("invalid maxStdoutBytes");
        }
        if !(1..=67_108_864).contains(&self.max_stderr_bytes) {
            bail!("invalid maxStderrBytes");
        }
        if !(67_108_864..=34_359_738_368).contains(&self.max_memory_bytes) {
            bail!("invalid maxMemoryBytes");
        }
        if !(1..=1024).contains(&self.max_processes) {
            bail!("invalid maxProcesses");
        }
        Ok(())
    }

    pub fn restricted_to(&self, other: &Self) -> Self {
        Self {
            wall_time_seconds: self.wall_time_seconds.min(other.wall_time_seconds),
            max_stdout_bytes: self.max_stdout_bytes.min(other.max_stdout_bytes),
            max_stderr_bytes: self.max_stderr_bytes.min(other.max_stderr_bytes),
            max_memory_bytes: self.max_memory_bytes.min(other.max_memory_bytes),
            max_processes: self.max_processes.min(other.max_processes),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub mode: SandboxMode,
    pub auto_approve: bool,
    pub workspace_read: WorkspaceRead,
    pub workspace_writable: bool,
    pub scratch_writable: bool,
    pub network_allow: BTreeSet<NetworkEndpoint>,
    pub environment: BTreeMap<String, String>,
    pub secret_ids: BTreeSet<String>,
    pub limits: SandboxLimits,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            mode: SandboxMode::Sandboxed,
            auto_approve: false,
            workspace_read: WorkspaceRead::None,
            workspace_writable: false,
            scratch_writable: true,
            network_allow: BTreeSet::new(),
            environment: BTreeMap::new(),
            secret_ids: BTreeSet::new(),
            limits: SandboxLimits::default(),
        }
    }
}

impl SandboxPolicy {
    pub fn restricted_to(&self, other: &Self) -> Self {
        let network_allow = self
            .network_allow
            .iter()
            .filter_map(|lhs| {
                other.network_allow.iter().find_map(|rhs| {
                    if lhs.host != rhs.host {
                        return None;
                    }
                    match (lhs.port, rhs.port) {
                        (None, None) => Some(lhs.clone()),
                        (None, Some(_)) => Some(rhs.clone()),
                        (Some(_), None) => Some(lhs.clone()),
                        (Some(a), Some(b)) if a == b => Some(lhs.clone()),
                        _ => None,
                    }
                })
            })
            .collect();
        let environment = self
            .environment
            .iter()
            .filter(|(key, value)| other.environment.get(*key) == Some(*value))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Self {
            mode: if self.mode == SandboxMode::Unlimited && other.mode == SandboxMode::Unlimited {
                SandboxMode::Unlimited
            } else {
                SandboxMode::Sandboxed
            },
            auto_approve: self.auto_approve && other.auto_approve,
            workspace_read: self.workspace_read.restricted_to(&other.workspace_read),
            workspace_writable: self.workspace_writable && other.workspace_writable,
            scratch_writable: self.scratch_writable && other.scratch_writable,
            network_allow,
            environment,
            secret_ids: self
                .secret_ids
                .intersection(&other.secret_ids)
                .cloned()
                .collect(),
            limits: self.limits.restricted_to(&other.limits),
        }
    }

    pub fn normalize_network(&mut self) {
        let wildcards: BTreeSet<String> = self
            .network_allow
            .iter()
            .filter(|endpoint| endpoint.port.is_none())
            .map(|endpoint| endpoint.host.clone())
            .collect();
        self.network_allow
            .retain(|endpoint| endpoint.port.is_none() || !wildcards.contains(&endpoint.host));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PolicyDocument {
    version: u64,
    #[serde(default)]
    mode: SandboxMode,
    #[serde(default)]
    auto_approve: bool,
    workspace_read: WorkspaceReadDocument,
    scratch_writable: bool,
    network_allow: Vec<NetworkEndpointDocument>,
    environment: BTreeMap<String, String>,
    #[serde(rename = "secretIDs", alias = "secretIds")]
    secret_ids: Vec<String>,
    limits: SandboxLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceReadDocument {
    mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NetworkEndpointDocument {
    host: String,
    port: Option<u16>,
}

impl TryFrom<PolicyDocument> for SandboxPolicy {
    type Error = anyhow::Error;
    fn try_from(document: PolicyDocument) -> Result<Self> {
        if document.version != 1 {
            bail!("Unsupported sandbox policy version: {}", document.version);
        }
        let workspace_read = match document.workspace_read.mode.as_str() {
            "none" => {
                if document.workspace_read.paths.is_some() {
                    bail!("workspaceRead.paths is only valid for paths mode");
                }
                WorkspaceRead::None
            }
            "all" => {
                if document.workspace_read.paths.is_some() {
                    bail!("workspaceRead.paths is only valid for paths mode");
                }
                WorkspaceRead::All
            }
            "paths" => {
                let paths = document
                    .workspace_read
                    .paths
                    .ok_or_else(|| anyhow!("workspaceRead.paths is required"))?;
                if paths.is_empty() {
                    bail!("workspaceRead.paths must contain at least one path");
                }
                let mut validated = BTreeSet::new();
                for path in paths {
                    validated.insert(validate_relative_path(&path)?);
                }
                WorkspaceRead::Paths(validated)
            }
            other => bail!("invalid workspaceRead.mode: {other}"),
        };
        let mut network_allow = BTreeSet::new();
        for endpoint in document.network_allow {
            network_allow.insert(NetworkEndpoint::new(&endpoint.host, endpoint.port)?);
        }
        validate_environment(&document.environment)?;
        for id in &document.secret_ids {
            validate_secret_id(id)?;
        }
        document.limits.validate()?;
        let mut policy = Self {
            mode: document.mode,
            auto_approve: document.auto_approve,
            workspace_read,
            workspace_writable: false,
            scratch_writable: document.scratch_writable,
            network_allow,
            environment: document.environment,
            secret_ids: document.secret_ids.into_iter().collect(),
            limits: document.limits,
        };
        policy.normalize_network();
        Ok(policy)
    }
}

impl From<&SandboxPolicy> for PolicyDocument {
    fn from(policy: &SandboxPolicy) -> Self {
        let workspace_read = match &policy.workspace_read {
            WorkspaceRead::None => WorkspaceReadDocument {
                mode: "none".into(),
                paths: None,
            },
            WorkspaceRead::All => WorkspaceReadDocument {
                mode: "all".into(),
                paths: None,
            },
            WorkspaceRead::Paths(paths) if paths.is_empty() => WorkspaceReadDocument {
                mode: "none".into(),
                paths: None,
            },
            WorkspaceRead::Paths(paths) => WorkspaceReadDocument {
                mode: "paths".into(),
                paths: Some(paths.iter().cloned().collect()),
            },
        };
        Self {
            version: 1,
            mode: policy.mode,
            auto_approve: policy.auto_approve,
            workspace_read,
            scratch_writable: policy.scratch_writable,
            network_allow: policy
                .network_allow
                .iter()
                .map(|e| NetworkEndpointDocument {
                    host: e.host.clone(),
                    port: e.port,
                })
                .collect(),
            environment: policy.environment.clone(),
            secret_ids: policy.secret_ids.iter().cloned().collect(),
            limits: policy.limits.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SandboxStore {
    pub workspace_root: PathBuf,
}

impl SandboxStore {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Result<Self> {
        let root = workspace_root.into();
        let root = root.canonicalize().with_context(|| {
            format!(
                "Sandbox workspace root is not a directory: {}",
                root.display()
            )
        })?;
        if !root.is_dir() {
            bail!(
                "Sandbox workspace root is not a directory: {}",
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
        self.directory().join("sandbox.json")
    }

    pub fn load(&self) -> Result<SandboxPolicy> {
        let directory = self.directory();
        if !directory.exists() {
            return Ok(SandboxPolicy::default());
        }
        reject_symlink(&directory)?;
        let path = self.path();
        if !path.exists() {
            return Ok(SandboxPolicy::default());
        }
        reject_symlink(&path)?;
        let data = fs::read(&path)?;
        let document: PolicyDocument = serde_json::from_slice(&data)
            .context("Invalid sandbox policy: document does not match version 1 schema")?;
        document.try_into()
    }

    pub fn save(&self, policy: &SandboxPolicy) -> Result<()> {
        policy.limits.validate()?;
        validate_environment(&policy.environment)?;
        for id in &policy.secret_ids {
            validate_secret_id(id)?;
        }
        let directory = self.directory();
        if directory.exists() {
            reject_symlink(&directory)?;
        }
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let path = self.path();
        if path.exists() {
            reject_symlink(&path)?;
        }
        let tmp = directory.join(format!(
            ".sandbox.json.{}.{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let mut data = serde_json::to_vec_pretty(&PolicyDocument::from(policy))?;
        data.push(b'\n');
        fs::write(&tmp, data)?;
        #[cfg(unix)]
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        replace_file(&tmp, &path)?;
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    pub fn reset(&self) -> Result<()> {
        let path = self.path();
        if path.exists() {
            reject_symlink(&path)?;
            fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn render(&self, policy: &SandboxPolicy) -> Result<String> {
        Ok(serde_json::to_string_pretty(&PolicyDocument::from(policy))?)
    }
}

pub fn validate_relative_path(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 4096
        || value.contains('\0')
        || value.contains('\\')
        || value.starts_with('/')
        || value.ends_with('/')
    {
        bail!("invalid workspace-relative path: {value}");
    }
    let path = Path::new(value);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("invalid workspace-relative path: {value}");
    }
    Ok(value.to_owned())
}

pub fn validate_environment(values: &BTreeMap<String, String>) -> Result<()> {
    for (key, value) in values {
        let mut bytes = key.bytes();
        let Some(first) = bytes.next() else {
            bail!("invalid environment key: {key}");
        };
        if !(first == b'_' || first.is_ascii_alphabetic())
            || !bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        {
            bail!("invalid environment key: {key}");
        }
        if value.contains('\0') {
            bail!("invalid environment value for {key}");
        }
    }
    Ok(())
}

pub fn validate_secret_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
    {
        bail!("invalid secret id: {value}");
    }
    Ok(())
}

fn validate_host(host: &str) -> Result<()> {
    if host.is_empty() || host.len() > 253 || host.starts_with('.') || host.ends_with('.') {
        bail!("invalid network host: {host}");
    }
    for label in host.split('.') {
        let bytes = label.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 63
            || !bytes[0].is_ascii_alphanumeric()
            || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
            || !bytes
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
        {
            bail!("invalid network host: {host}");
        }
    }
    Ok(())
}

fn relative_contains(root: &str, path: &str) -> bool {
    path == root || path.starts_with(&format!("{root}/"))
}

fn reject_symlink(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        bail!("Refusing sandbox policy symlink: {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_is_deny_by_default_except_scratch() {
        let policy = SandboxPolicy::default();
        assert_eq!(policy.mode, SandboxMode::Sandboxed);
        assert!(!policy.auto_approve);
        assert_eq!(policy.workspace_read, WorkspaceRead::None);
        assert!(policy.scratch_writable);
        assert!(policy.network_allow.is_empty());
        assert!(policy.environment.is_empty());
    }

    #[test]
    fn wildcard_endpoint_normalizes_specific_ports() {
        let mut policy = SandboxPolicy::default();
        policy
            .network_allow
            .insert(NetworkEndpoint::new("example.com", Some(443)).unwrap());
        policy
            .network_allow
            .insert(NetworkEndpoint::new("example.com", None).unwrap());
        policy.normalize_network();
        assert_eq!(policy.network_allow.len(), 1);
        assert!(policy.network_allow.iter().next().unwrap().port.is_none());
    }

    #[test]
    fn store_writes_version_one_schema_and_round_trips() {
        let workspace = tempfile::tempdir().unwrap();
        let store = SandboxStore::new(workspace.path()).unwrap();
        let mut policy = SandboxPolicy {
            workspace_read: WorkspaceRead::Paths(BTreeSet::from(["src".into()])),
            ..SandboxPolicy::default()
        };
        policy.secret_ids.insert("OPENAI_API_KEY".into());
        policy.environment.insert("RUST_LOG".into(), "debug".into());
        store.save(&policy).unwrap();
        let data = fs::read_to_string(store.path()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["workspaceRead"]["mode"], "paths");
        assert_eq!(value["secretIDs"][0], "OPENAI_API_KEY");
        assert!(value.get("secretIds").is_none());
        assert_eq!(store.load().unwrap(), policy);
    }

    #[test]
    fn store_reads_swift_era_secret_ids_key() {
        let workspace = tempfile::tempdir().unwrap();
        let store = SandboxStore::new(workspace.path()).unwrap();
        fs::create_dir_all(store.directory()).unwrap();
        fs::write(
            store.path(),
            r#"{
  "environment": {},
  "limits": {
    "maxMemoryBytes": 536870912,
    "maxProcesses": 32,
    "maxStderrBytes": 1048576,
    "maxStdoutBytes": 1048576,
    "wallTimeSeconds": 30
  },
  "networkAllow": [],
  "scratchWritable": true,
  "secretIDs": [],
  "version": 1,
  "workspaceRead": { "mode": "all" }
}"#,
        )
        .unwrap();

        let policy = store.load().unwrap();
        assert_eq!(policy.workspace_read, WorkspaceRead::All);
        assert!(policy.scratch_writable);
    }

    #[test]
    fn store_accepts_transitional_rust_secret_ids_key() {
        let workspace = tempfile::tempdir().unwrap();
        let store = SandboxStore::new(workspace.path()).unwrap();
        fs::create_dir_all(store.directory()).unwrap();
        fs::write(
            store.path(),
            r#"{
  "version": 1,
  "workspaceRead": { "mode": "none" },
  "scratchWritable": true,
  "networkAllow": [],
  "environment": {},
  "secretIds": ["TOKEN"],
  "limits": {
    "wallTimeSeconds": 30,
    "maxStdoutBytes": 1048576,
    "maxStderrBytes": 1048576,
    "maxMemoryBytes": 536870912,
    "maxProcesses": 32
  }
}"#,
        )
        .unwrap();

        let policy = store.load().unwrap();
        assert!(policy.secret_ids.contains("TOKEN"));
    }
}
