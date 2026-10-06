//! Stable workspace identity and resolution.
//!
//! A workspace is identified by the id in `<root>/.yeet/workspace.json`, not by
//! its path, so sessions follow a moved workspace. The registry maps ids to
//! last-known roots; stored paths are kept workspace-relative.

use super::*;

pub(super) fn normalize_lexical(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                output.pop();
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

pub(super) fn workspace_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WorkspaceRegistry {
    #[serde(default)]
    pub(super) workspaces: BTreeMap<String, RegisteredWorkspace>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RegisteredWorkspace {
    pub(super) path: String,
    pub(super) last_seen: DateTime<Utc>,
}

/// Maps workspace identities to their current location for one listing pass.
/// A workspace is found at its registered path or the session's last known
/// path, but only if the marker there still carries the same identity.
/// Entries whose folder vanished are pruned; the sessions themselves are kept
/// and reappear once the workspace is opened from its new location.
pub(super) struct WorkspaceResolver {
    pub(super) registry_path: PathBuf,
    pub(super) registry: WorkspaceRegistry,
    pub(super) dirty: bool,
    pub(super) current_id: String,
    pub(super) current_path: PathBuf,
    pub(super) located: BTreeMap<String, Option<PathBuf>>,
    pub(super) legacy: BTreeMap<PathBuf, Option<String>>,
}

impl WorkspaceResolver {
    pub(super) fn new(store: &SessionStore, current: &Path) -> Result<Self> {
        let current_path = current
            .canonicalize()
            .unwrap_or_else(|_| current.to_path_buf());
        let current_id = ensure_workspace_id(&current_path)?;
        let registry_path = store.registry_path();
        let mut registry = fs::read(&registry_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<WorkspaceRegistry>(&bytes).ok())
            .unwrap_or_default();
        let current_string = current_path.to_string_lossy().into_owned();
        // Whatever used to live at this path has been replaced by the current
        // workspace, so no other identity may keep claiming it.
        registry
            .workspaces
            .retain(|id, entry| *id == current_id || entry.path != current_string);
        registry.workspaces.insert(
            current_id.clone(),
            RegisteredWorkspace {
                path: current_string,
                last_seen: Utc::now(),
            },
        );
        let mut located = BTreeMap::new();
        located.insert(current_id.clone(), Some(current_path.clone()));
        Ok(Self {
            registry_path,
            registry,
            dirty: true,
            current_id,
            current_path,
            located,
            legacy: BTreeMap::new(),
        })
    }

    pub(super) fn resolve(&mut self, record: &SessionListRecord) -> Option<(String, PathBuf)> {
        let hint = PathBuf::from(&record.workspace_root);
        let id = match &record.workspace_id {
            Some(id) => id.clone(),
            None => self.legacy_id(&hint)?,
        };
        let path = self.locate(&id, &hint)?;
        Some((id, path))
    }

    pub(super) fn legacy_id(&mut self, root: &Path) -> Option<String> {
        let root = root.canonicalize().ok()?;
        if let Some(cached) = self.legacy.get(&root) {
            return cached.clone();
        }
        let id = root
            .is_dir()
            .then(|| ensure_workspace_id(&root).ok())
            .flatten();
        self.legacy.insert(root, id.clone());
        id
    }

    pub(super) fn locate(&mut self, id: &str, hint: &Path) -> Option<PathBuf> {
        if let Some(cached) = self.located.get(id) {
            return cached.clone();
        }
        let registered = self
            .registry
            .workspaces
            .get(id)
            .map(|entry| PathBuf::from(&entry.path));
        let found = registered
            .iter()
            .map(PathBuf::as_path)
            .chain([hint])
            .filter_map(|candidate| candidate.canonicalize().ok())
            .find(|candidate| read_workspace_id(candidate).as_deref() == Some(id));
        match &found {
            Some(path) if registered.as_ref() != Some(path) => {
                let last_seen = self
                    .registry
                    .workspaces
                    .get(id)
                    .map_or_else(Utc::now, |entry| entry.last_seen);
                self.registry.workspaces.insert(
                    id.to_owned(),
                    RegisteredWorkspace {
                        path: path.to_string_lossy().into_owned(),
                        last_seen,
                    },
                );
                self.dirty = true;
            }
            None if registered.is_some() => {
                self.registry.workspaces.remove(id);
                self.dirty = true;
            }
            _ => {}
        }
        self.located.insert(id.to_owned(), found.clone());
        found
    }

    pub(super) fn finish(self) -> Result<()> {
        if self.dirty {
            write_private_replace(
                &self.registry_path,
                &serde_json::to_vec_pretty(&self.registry)?,
            )?;
        }
        Ok(())
    }
}

pub(super) fn workspace_marker_path(root: &Path) -> PathBuf {
    root.join(WORKSPACE_MARKER_DIR).join(WORKSPACE_MARKER_FILE)
}

/// Reads the stable identity stored inside a workspace, if it has one.
pub fn read_workspace_id(root: &Path) -> Option<String> {
    let bytes = fs::read(workspace_marker_path(root)).ok()?;
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    let id = value.get("id")?.as_str()?;
    Uuid::parse_str(id).ok()?;
    Some(id.to_owned())
}

/// Returns the workspace's identity, creating `.yeet/workspace.json` on first use.
pub fn ensure_workspace_id(root: &Path) -> Result<String> {
    use std::io::Write;
    if let Some(id) = read_workspace_id(root) {
        return Ok(id);
    }
    let marker = workspace_marker_path(root);
    let directory = marker.parent().context("workspace marker has no parent")?;
    ensure!(
        !directory.is_symlink(),
        "Refusing to use symlinked workspace directory {}",
        directory.display()
    );
    create_private_dir(directory)?;
    let id = Uuid::new_v4().to_string();
    let data = serde_json::to_vec_pretty(&serde_json::json!({ "id": id }))?;
    // create_new so concurrent first opens agree on one identity.
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    match options.open(&marker) {
        Ok(mut file) => {
            file.write_all(&data)?;
            file.sync_all()?;
            Ok(id)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            read_workspace_id(root).context("workspace marker exists but is unreadable")
        }
        Err(error) => Err(error).context("create workspace marker"),
    }
}

/// Stores `path` relative to `root` when it lives inside the workspace, so the
/// session stays valid after the workspace moves.
pub fn workspace_relative_path(root: &Path, path: &str) -> String {
    match Path::new(path).strip_prefix(root) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".into(),
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_owned(),
    }
}

/// Resolves a stored session path against the workspace's current root.
/// Legacy absolute paths under the session's old root are rebased as well.
pub fn resolve_workspace_path(root: &Path, stored_root: &str, path: &str) -> String {
    let stored = Path::new(path);
    let resolved = if stored.is_relative() {
        root.join(stored)
    } else if let Some(relative) = (!stored_root.is_empty())
        .then(|| stored.strip_prefix(stored_root).ok())
        .flatten()
    {
        root.join(relative)
    } else {
        stored.to_path_buf()
    };
    normalize_lexical(&resolved).to_string_lossy().into_owned()
}
