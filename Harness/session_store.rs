use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use fs2::{FileExt, lock_contended_error};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::{
    core::{Message, Usage},
    model::{
        AgentMode, AutonomyMode, ConversationEntry, SessionSummary, WorkspaceSessionGroup,
        WorkspaceSummary,
    },
    platform::{replace_file, set_private_directory, set_private_file, sync_directory},
};

const SESSION_LAYOUT_VERSION: u64 = 4;
const TASKS_DIR: &str = "tasks";
const DEBATES_DIR: &str = "debates";
const RUNS_DIR: &str = "runs";
const EVENT_SEQUENCE_FILE: &str = ".event-seq";
const OBJECTS_DIR: &str = ".objects";
const MANIFEST_FILE: &str = ".current.json";
const GOAL_STATE_FILE: &str = ".goal.json";
const LEGACY_GOAL_STATE_FILE: &str = ".infinity.json";
const WORKSPACE_REGISTRY_FILE: &str = "workspaces.json";
const WORKSPACE_MARKER_DIR: &str = ".yeet";
const WORKSPACE_MARKER_FILE: &str = "workspace.json";

mod events;
mod export;
mod locking;
mod migration;
mod objects;
mod workspace;

use locking::*;
use migration::*;
use objects::*;
use workspace::*;
pub use workspace::{
    ensure_workspace_id, read_workspace_id, resolve_workspace_path, workspace_relative_path,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSession {
    #[serde(default)]
    pub debate: Option<crate::debate::DebateState>,
    pub version: u64,
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Last known location only. Identity is `workspace_id`, which lives in
    /// the workspace itself so sessions follow the folder when it moves.
    pub workspace_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// Stored relative to the workspace root when inside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_roots: Vec<String>,
    pub model: String,
    #[serde(default)]
    pub agent_mode: AgentMode,
    #[serde(default)]
    pub autonomy_mode: AutonomyMode,
    pub token_usage: Usage,
    pub credit_usage: u64,
    pub conversation: Vec<ConversationEntry>,
    pub model_history: Vec<Message>,
    #[serde(default)]
    pub runs: Vec<StoredRun>,
    #[serde(default)]
    pub retained_debate_knowledge: Vec<crate::debate::RetainedDebateKnowledge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attached_harness_capabilities: Option<Vec<String>>,
    #[serde(
        default,
        alias = "disabledLazyCapabilities",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub disabled_capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredRun {
    pub id: String,
    pub kind: String,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_start: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_start: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Completed,
    CompletedUnverified,
    Paused,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExportResult {
    pub session_id: String,
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub source_deleted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionListMetadata {
    id: String,
    title: String,
    updated_at: DateTime<Utc>,
    workspace_root: String,
    #[serde(default)]
    workspace_id: Option<String>,
    model: String,
    #[serde(default)]
    message_count: Option<usize>,
}

struct SessionListRecord {
    id: String,
    title: String,
    updated_at: DateTime<Utc>,
    workspace_root: String,
    workspace_id: Option<String>,
    model: String,
    message_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SemanticManifest {
    version: u64,
    components: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    pub directory: PathBuf,
    io_lock: Arc<Mutex<()>>,
}

impl SessionStore {
    pub fn new(config_directory: &Path) -> Self {
        Self {
            directory: config_directory.join("sessions"),
            io_lock: Arc::new(Mutex::new(())),
        }
    }

    fn serialized<T>(&self, exclusive: bool, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let _process_guard = self
            .io_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("session store lock poisoned"))?;
        self.ensure()?;
        let _file_guard = StoreFileLock::acquire(&self.directory.join(".store.lock"), exclusive)?;
        operation()
    }

    fn serialized_session<T>(
        &self,
        id: &str,
        exclusive: bool,
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        validate_id(id)?;
        let _process_guard = self
            .io_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("session store lock poisoned"))?;
        self.ensure()?;
        // Session operations take the store lock in shared mode so unrelated
        // sessions can proceed concurrently while store-wide maintenance still
        // has an exclusive barrier against every active session operation.
        let _store_guard = StoreFileLock::acquire(&self.directory.join(".store.lock"), false)?;
        let lock_path = self.directory.join(format!(".session-{id}.lock"));
        let _session_guard = StoreFileLock::acquire(&lock_path, exclusive)?;
        operation()
    }

    /// Prepare the on-disk store and collapse older revision-based sessions
    /// into the current semantic layout. Migration is opportunistic: current
    /// readers understand every legacy layout, so backend startup must not wait
    /// behind an unrelated save/export that already owns the store lock.
    pub fn prepare(&self) -> Result<()> {
        let _process_guard = self
            .io_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("session store lock poisoned"))?;
        self.ensure()?;
        let Some(_file_guard) =
            StoreFileLock::try_acquire_exclusive(&self.directory.join(".store.lock"))?
        else {
            return Ok(());
        };
        self.migrate_legacy_layouts();
        self.backfill_workspace_ids();
        cleanup_stale_materialization_links_in_store(&self.directory)
    }

    pub fn save(&self, session: &StoredSession) -> Result<()> {
        self.serialized_session(&session.id, true, || self.save_unlocked(session))
    }

    fn save_unlocked(&self, session: &StoredSession) -> Result<()> {
        self.ensure()?;
        validate_id(&session.id)?;
        let root = self.directory.join(&session.id);
        create_private_dir(&root)?;
        create_private_dir(&root.join(TASKS_DIR))?;
        create_private_dir(&root.join(DEBATES_DIR))?;
        create_private_dir(&root.join(RUNS_DIR))?;

        let mut metadata = serde_json::to_value(session)?;
        metadata.as_object_mut().unwrap().remove("conversation");
        metadata.as_object_mut().unwrap().remove("modelHistory");
        metadata.as_object_mut().unwrap().remove("debate");
        metadata.as_object_mut().unwrap().remove("runs");
        metadata
            .as_object_mut()
            .unwrap()
            .remove("retainedDebateKnowledge");
        metadata["version"] = SESSION_LAYOUT_VERSION.into();
        metadata["messageCount"] = session.conversation.len().into();
        let components = [
            ("metadata.json", Some(serde_json::to_vec_pretty(&metadata)?)),
            (
                "conversation.json",
                Some(serde_json::to_vec_pretty(&session.conversation)?),
            ),
            (
                "model-history.json",
                Some(serde_json::to_vec_pretty(&session.model_history)?),
            ),
            (
                "runs/index.json",
                Some(serde_json::to_vec_pretty(&session.runs)?),
            ),
            (
                "debates/knowledge.json",
                (!session.retained_debate_knowledge.is_empty())
                    .then(|| serde_json::to_vec_pretty(&session.retained_debate_knowledge))
                    .transpose()?,
            ),
            (
                "debates/state.json",
                session
                    .debate
                    .as_ref()
                    .map(serde_json::to_vec_pretty)
                    .transpose()?,
            ),
        ];
        let objects = root.join(OBJECTS_DIR);
        create_private_dir(&objects)?;
        let mut manifest = SemanticManifest {
            version: 1,
            components: BTreeMap::new(),
        };
        for (component, data) in &components {
            let hash = match data {
                Some(data) => {
                    let hash = sha256_bytes(data);
                    let object = objects.join(&hash);
                    if !object.exists() {
                        write_private_replace(&object, data)?;
                    }
                    #[cfg(unix)]
                    fs::set_permissions(&object, fs::Permissions::from_mode(0o400))?;
                    materialize_component(&object, &root.join(component))?;
                    Some(hash)
                }
                None => {
                    remove_file_if_exists(&root.join(component))?;
                    None
                }
            };
            manifest.components.insert((*component).to_owned(), hash);
        }
        // All objects exist before this point. Updating the tiny manifest last
        // makes a save an atomic generation switch: after a crash, readers see
        // either the complete previous generation or the complete new one.
        write_private_replace(
            &root.join(MANIFEST_FILE),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        // The manifest switch is the commit point. Cleanup is deliberately
        // best-effort so a stale object cannot make a successfully committed
        // generation look like a failed save to callers.
        let _ = gc_semantic_objects(&root, &manifest);

        // Conversation history is stored exactly once in conversation.json.
        // `tasks/` is the append-only agent event stream; run lifecycle lives
        // in runs/index.json. Retire the old duplicate task snapshot if present.
        remove_file_if_exists(&root.join(TASKS_DIR).join("entries.json"))?;

        // A successful semantic save is enough to retire all redundant UUID
        // snapshots from the old layout.
        self.migrate_legacy_runtime_log(&root)?;
        cleanup_legacy_layout(&root)?;
        cleanup_stale_materialization_links(&root)?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<StoredSession> {
        self.serialized_session(id, false, || self.load_unlocked(id))
    }

    fn load_unlocked(&self, id: &str) -> Result<StoredSession> {
        validate_id(id)?;
        let root = self.directory.join(id);

        if root.join(MANIFEST_FILE).is_file() || root.join("metadata.json").is_file() {
            return self.load_semantic_layout(&root);
        }

        if root.join("current").exists() {
            return self.load_revision_layout(&root);
        }
        let data = fs::read(self.file_path(id)).with_context(|| format!("load session {id}"))?;
        Ok(serde_json::from_slice(&data)?)
    }

    pub fn list(&self, workspace_root: &Path) -> Result<Vec<SessionSummary>> {
        self.serialized(false, || {
            let records = self.scan_records()?;
            let mut resolver = WorkspaceResolver::new(self, workspace_root)?;
            let current = resolver.current_id.clone();
            let sessions = session_summaries(&records, &mut resolver, &current);
            resolver.finish()?;
            Ok(sessions)
        })
    }

    pub fn list_workspaces(&self, current_workspace: &Path) -> Result<Vec<WorkspaceSummary>> {
        Ok(self.list_workspace_catalog(current_workspace)?.0)
    }

    /// Groups sessions by workspace identity rather than absolute path, so a
    /// moved workspace keeps its sessions and a deleted one drops out.
    pub fn list_workspace_catalog(
        &self,
        current_workspace: &Path,
    ) -> Result<(Vec<WorkspaceSummary>, Vec<WorkspaceSessionGroup>)> {
        self.serialized(false, || {
            let records = self.scan_records()?;
            let mut resolver = WorkspaceResolver::new(self, current_workspace)?;
            let mut grouped: BTreeMap<String, (PathBuf, Option<DateTime<Utc>>, usize)> =
                BTreeMap::new();
            grouped.insert(
                resolver.current_id.clone(),
                (resolver.current_path.clone(), None, 0),
            );
            for record in &records {
                let Some((id, path)) = resolver.resolve(record) else {
                    continue;
                };
                let (_, latest, count) = grouped.entry(id).or_insert((path, None, 0));
                *count += 1;
                match latest {
                    Some(existing) if *existing >= record.updated_at => {}
                    _ => *latest = Some(record.updated_at),
                }
            }
            let current_id = resolver.current_id.clone();
            let mut workspaces = grouped
                .into_iter()
                .map(|(id, (path, updated_at, session_count))| WorkspaceSummary {
                    is_current: id == current_id,
                    id,
                    path: path.to_string_lossy().into_owned(),
                    display_name: workspace_display_name(&path),
                    updated_at: updated_at.map(|value| value.to_rfc3339()),
                    session_count,
                })
                .collect::<Vec<_>>();
            workspaces.sort_by(|lhs, rhs| {
                rhs.is_current
                    .cmp(&lhs.is_current)
                    .then_with(|| rhs.updated_at.cmp(&lhs.updated_at))
                    .then_with(|| lhs.display_name.cmp(&rhs.display_name))
                    .then_with(|| lhs.path.cmp(&rhs.path))
            });
            let session_groups = workspaces
                .iter()
                .map(|workspace| WorkspaceSessionGroup {
                    workspace_id: workspace.id.clone(),
                    sessions: session_summaries(&records, &mut resolver, &workspace.id),
                })
                .collect();
            resolver.finish()?;
            Ok((workspaces, session_groups))
        })
    }

    fn scan_records(&self) -> Result<Vec<SessionListRecord>> {
        self.ensure()?;
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let path = entry.path();
            let id = if path.is_dir() {
                path.file_name()
            } else if path.extension().and_then(|v| v.to_str()) == Some("json") {
                path.file_stem()
            } else {
                None
            };
            let Some(id) = id.and_then(|v| v.to_str()) else {
                continue;
            };
            if validate_id(id).is_err() || !seen.insert(id.to_owned()) {
                continue;
            }
            let record = if path.is_dir()
                && (path.join(MANIFEST_FILE).is_file() || path.join("metadata.json").is_file())
            {
                let manifest = fs::read(path.join(MANIFEST_FILE))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<SemanticManifest>(&bytes).ok());
                let metadata = if let Some(manifest) = manifest.as_ref() {
                    read_manifest_component(&path, manifest, "metadata.json")
                        .ok()
                        .flatten()
                } else {
                    fs::read(path.join("metadata.json")).ok()
                };
                let Some(metadata) = metadata
                    .and_then(|bytes| serde_json::from_slice::<SessionListMetadata>(&bytes).ok())
                else {
                    continue;
                };
                let message_count = metadata.message_count.unwrap_or_else(|| {
                    manifest
                        .as_ref()
                        .and_then(|manifest| {
                            read_manifest_component(&path, manifest, "conversation.json")
                                .ok()
                                .flatten()
                        })
                        .or_else(|| fs::read(path.join("conversation.json")).ok())
                        .and_then(|data| {
                            serde_json::from_slice::<Vec<ConversationEntry>>(&data).ok()
                        })
                        .map_or(0, |conversation| conversation.len())
                });
                SessionListRecord {
                    id: metadata.id,
                    title: metadata.title,
                    updated_at: metadata.updated_at,
                    workspace_root: metadata.workspace_root,
                    workspace_id: metadata.workspace_id,
                    model: metadata.model,
                    message_count,
                }
            } else {
                let Ok(session) = self.load_unlocked(id) else {
                    continue;
                };
                SessionListRecord {
                    id: session.id,
                    title: session.title,
                    updated_at: session.updated_at,
                    workspace_root: session.workspace_root,
                    workspace_id: session.workspace_id,
                    model: session.model,
                    message_count: session.conversation.len(),
                }
            };
            result.push(record);
        }
        Ok(result)
    }

    fn registry_path(&self) -> PathBuf {
        self.directory
            .parent()
            .unwrap_or(&self.directory)
            .join(WORKSPACE_REGISTRY_FILE)
    }

    pub fn is_debate_session(&self, id: &str) -> bool {
        self.serialized_session(id, false, || Ok(self.is_debate_session_unlocked(id)))
            .unwrap_or(false)
    }

    fn is_debate_session_unlocked(&self, id: &str) -> bool {
        let root = self.directory.join(id);
        if let Ok(bytes) = fs::read(root.join(MANIFEST_FILE))
            && let Ok(manifest) = serde_json::from_slice::<SemanticManifest>(&bytes)
            && let Ok(Some(bytes)) = read_manifest_component(&root, &manifest, "debates/state.json")
        {
            return serde_json::from_slice::<crate::debate::DebateState>(&bytes)
                .ok()
                .is_some_and(|debate| !debate.topic.trim().is_empty());
        }
        if root.join(DEBATES_DIR).join("state.json").is_file() {
            return fs::read(root.join(DEBATES_DIR).join("state.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<crate::debate::DebateState>(&bytes).ok())
                .is_some_and(|debate| !debate.topic.trim().is_empty());
        }
        self.load_unlocked(id)
            .ok()
            .and_then(|session| session.debate)
            .is_some_and(|debate| !debate.topic.trim().is_empty())
    }

    pub fn set_goal_mode(&self, id: &str, enabled: bool) -> Result<()> {
        self.serialized_session(id, true, || {
            let root = self.directory.join(id);
            create_private_dir(&root)?;
            let data = serde_json::to_vec_pretty(&serde_json::json!({ "enabled": enabled }))?;
            write_private_replace(&root.join(GOAL_STATE_FILE), &data)
        })
    }

    pub fn goal_mode(&self, id: &str) -> Result<bool> {
        self.serialized_session(id, false, || {
            let root = self.directory.join(id);
            let path = [
                root.join(GOAL_STATE_FILE),
                root.join(LEGACY_GOAL_STATE_FILE),
            ]
            .into_iter()
            .find(|path| path.is_file());
            let Some(path) = path else {
                return Ok(false);
            };
            let value: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
            Ok(value
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false))
        })
    }

    pub fn new_id() -> String {
        Uuid::new_v4().to_string()
    }

    fn file_path(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.json"))
    }

    fn load_semantic_layout(&self, root: &Path) -> Result<StoredSession> {
        let manifest_path = root.join(MANIFEST_FILE);
        if manifest_path.is_file() {
            let manifest: SemanticManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
            ensure!(
                manifest.version == 1,
                "unsupported session manifest version"
            );
            let mut value: serde_json::Value = serde_json::from_slice(
                &read_manifest_component(root, &manifest, "metadata.json")?
                    .context("session manifest is missing metadata.json")?,
            )?;
            value["conversation"] = serde_json::from_slice(
                &read_manifest_component(root, &manifest, "conversation.json")?
                    .context("session manifest is missing conversation.json")?,
            )?;
            value["modelHistory"] = serde_json::from_slice(
                &read_manifest_component(root, &manifest, "model-history.json")?
                    .context("session manifest is missing model-history.json")?,
            )?;
            if let Some(bytes) = read_manifest_component(root, &manifest, "runs/index.json")? {
                value["runs"] = serde_json::from_slice(&bytes)?;
            }
            if let Some(bytes) = read_manifest_component(root, &manifest, "debates/knowledge.json")?
            {
                value["retainedDebateKnowledge"] = serde_json::from_slice(&bytes)?;
            }
            if let Some(bytes) = read_manifest_component(root, &manifest, "debates/state.json")? {
                value["debate"] = serde_json::from_slice(&bytes)?;
            }
            return Ok(serde_json::from_value(value)?);
        }
        self.load_semantic_layout_legacy(root)
    }

    fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        set_private_directory(&self.directory)?;
        Ok(())
    }
}

fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    set_private_directory(path)?;
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn validate_id(id: &str) -> Result<()> {
    anyhow::ensure!(
        !id.is_empty()
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
        "invalid session identifier"
    );
    Ok(())
}

fn session_summaries(
    records: &[SessionListRecord],
    resolver: &mut WorkspaceResolver,
    workspace_id: &str,
) -> Vec<SessionSummary> {
    let mut result = records
        .iter()
        .filter(|record| {
            resolver
                .resolve(record)
                .is_some_and(|(id, _)| id == workspace_id)
        })
        .map(|record| SessionSummary {
            id: record.id.clone(),
            title: record.title.clone(),
            updated_at: record.updated_at.to_rfc3339(),
            model: record.model.clone(),
            message_count: record.message_count,
        })
        .collect::<Vec<_>>();
    result.sort_by(|lhs, rhs| rhs.updated_at.cmp(&lhs.updated_at));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, root: &Path, workspace_id: Option<String>) -> StoredSession {
        StoredSession {
            debate: None,
            version: SESSION_LAYOUT_VERSION,
            id: id.into(),
            title: id.into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            workspace_root: root.display().to_string(),
            workspace_id,
            working_directory: Some("src".into()),
            context_roots: vec![".".into()],
            model: "test".into(),
            agent_mode: AgentMode::Single,
            autonomy_mode: AutonomyMode::Manual,
            token_usage: Usage::default(),
            credit_usage: 0,
            conversation: Vec::new(),
            model_history: Vec::new(),
            runs: Vec::new(),
            retained_debate_knowledge: Vec::new(),
            attached_harness_capabilities: None,
            disabled_capabilities: Vec::new(),
        }
    }

    #[test]
    fn read_events_merges_streams_in_sequence_order() {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        store.prepare().unwrap();
        let root = store.directory.join("session-events");
        fs::create_dir_all(root.join(TASKS_DIR)).unwrap();
        fs::create_dir_all(root.join(DEBATES_DIR)).unwrap();
        fs::write(
            root.join(TASKS_DIR).join("events.jsonl"),
            r#"{"seq":1,"type":"task"}"#,
        )
        .unwrap();
        fs::write(
            root.join(DEBATES_DIR).join("events.jsonl"),
            r#"{"seq":2,"type":"debate"}"#,
        )
        .unwrap();

        let events = store.read_events("session-events").unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event["seq"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn read_events_skips_truncated_trailing_record() {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        store.prepare().unwrap();
        let root = store.directory.join("session-events");
        fs::create_dir_all(root.join(TASKS_DIR)).unwrap();
        fs::write(
            root.join(TASKS_DIR).join("events.jsonl"),
            "{\"seq\":1,\"type\":\"task\"}\n{\"seq\":2,\"type\":",
        )
        .unwrap();

        let events = store.read_events("session-events").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["seq"], 1);
    }

    #[test]
    fn group_checkpoint_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        store.prepare().unwrap();
        let checkpoint = serde_json::json!({
            "tasks": [{ "id": "task-1", "status": "running" }],
            "revision": 3,
        });

        store
            .save_group_checkpoint("session-checkpoint", &checkpoint)
            .unwrap();
        let loaded: Option<serde_json::Value> =
            store.load_group_checkpoint("session-checkpoint").unwrap();

        assert_eq!(loaded, Some(checkpoint));
    }

    #[test]
    fn old_session_without_group_checkpoint_loads_as_none() {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        store.prepare().unwrap();

        let loaded: Option<serde_json::Value> = store
            .load_group_checkpoint("session-without-checkpoint")
            .unwrap();

        assert_eq!(loaded, None);
    }

    #[test]
    fn group_events_replay_with_member_attribution_and_group_sequence() {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        store.prepare().unwrap();
        store
            .append_event(
                "session-group-events",
                None,
                &serde_json::json!({
                    "type":"agent_group_event",
                    "groupId":"group-1",
                    "groupSequence":1,
                    "memberId":"member-1",
                    "taskId":"task-1",
                    "kind":"reasoning_started"
                }),
            )
            .unwrap();
        store
            .append_event(
                "session-group-events",
                None,
                &serde_json::json!({
                    "type":"agent_group_event",
                    "groupId":"group-1",
                    "groupSequence":2,
                    "memberId":"member-2",
                    "taskId":"task-2",
                    "kind":"task_completed"
                }),
            )
            .unwrap();

        let events = store.read_events("session-group-events").unwrap();
        let events = events
            .iter()
            .filter(|event| event["type"] == "agent_group_event")
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["payload"]["groupSequence"], 1);
        assert_eq!(events[0]["payload"]["memberId"], "member-1");
        assert_eq!(events[1]["payload"]["groupSequence"], 2);
        assert_eq!(events[1]["payload"]["taskId"], "task-2");
    }

    #[test]
    fn manifest_is_the_commit_point_for_semantic_saves() {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        store.prepare().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let mut saved = session("session-a", &workspace, None);
        saved.model_history = vec![Message::user("committed")];
        saved.runs = vec![StoredRun {
            id: "run-1".into(),
            kind: "agent".into(),
            status: RunStatus::Interrupted,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            model: "test".into(),
            history_start: Some(0),
            conversation_start: Some(0),
            user_entry_id: None,
            error: Some("cancelled".into()),
        }];
        store.save(&saved).unwrap();

        // A save that crashed after writing objects and materialized views but
        // before the manifest switch must be invisible to readers.
        let root = store.directory.join("session-a");
        let uncommitted = serde_json::to_vec_pretty(&vec![Message::user("uncommitted")]).unwrap();
        fs::write(
            root.join(OBJECTS_DIR).join(sha256_bytes(&uncommitted)),
            &uncommitted,
        )
        .unwrap();
        let materialized = root.join("model-history.json");
        fs::remove_file(&materialized).unwrap();
        fs::write(&materialized, &uncommitted).unwrap();

        let loaded = store.load("session-a").unwrap();
        assert_eq!(loaded.model_history, saved.model_history);
        assert_eq!(loaded.runs.len(), 1);
        assert_eq!(loaded.runs[0].status, RunStatus::Interrupted);

        // Objects are content-addressed and verified on read.
        let manifest: SemanticManifest =
            serde_json::from_slice(&fs::read(root.join(MANIFEST_FILE)).unwrap()).unwrap();
        let hash = manifest.components["model-history.json"].clone().unwrap();
        let object = root.join(OBJECTS_DIR).join(&hash);
        #[cfg(unix)]
        fs::set_permissions(&object, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&object, b"tampered").unwrap();
        assert!(store.load("session-a").is_err());
    }

    #[test]
    fn pre_manifest_semantic_layout_remains_readable() {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        store.prepare().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let mut saved = session("legacy-a", &workspace, None);
        saved.model_history = vec![Message::user("old turn")];
        store.save(&saved).unwrap();

        // The layout before manifests: plain component files, no
        // `.current.json` and no content-addressed objects.
        let root = store.directory.join("legacy-a");
        fs::remove_file(root.join(MANIFEST_FILE)).unwrap();
        fs::remove_dir_all(root.join(OBJECTS_DIR)).unwrap();

        let loaded = store.load("legacy-a").unwrap();
        assert_eq!(loaded.model_history, saved.model_history);
        assert_eq!(loaded.id, "legacy-a");

        // The next save converts it to the manifest layout in place.
        store.save(&loaded).unwrap();
        assert!(root.join(MANIFEST_FILE).is_file());
        assert_eq!(
            store.load("legacy-a").unwrap().model_history,
            saved.model_history
        );
    }

    #[test]
    fn missing_agent_mode_defaults_to_single() {
        let temp = tempfile::tempdir().unwrap();
        let mut value = serde_json::to_value(session("legacy", temp.path(), None)).unwrap();
        value.as_object_mut().unwrap().remove("agentMode");

        let restored: StoredSession = serde_json::from_value(value).unwrap();
        assert_eq!(restored.agent_mode, AgentMode::Single);
    }

    #[test]
    fn missing_autonomy_mode_defaults_to_manual() {
        let temp = tempfile::tempdir().unwrap();
        let mut value =
            serde_json::to_value(session("legacy-autonomy", temp.path(), None)).unwrap();
        value.as_object_mut().unwrap().remove("autonomyMode");

        let restored: StoredSession = serde_json::from_value(value).unwrap();
        assert_eq!(restored.autonomy_mode, AutonomyMode::Manual);
    }

    #[test]
    fn sessions_follow_moved_workspace_and_drop_deleted_one() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        let store = SessionStore::new(&config);
        let original = temp.path().join("project");
        let other = temp.path().join("other");
        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&other).unwrap();
        let original = original.canonicalize().unwrap();
        let id = ensure_workspace_id(&original).unwrap();
        store
            .save(&session("a", &original, Some(id.clone())))
            .unwrap();

        let moved = temp.path().join("renamed");
        fs::rename(&original, &moved).unwrap();
        let moved = moved.canonicalize().unwrap();
        assert!(store.list(&other).unwrap().is_empty());
        let listed = store.list(&moved).unwrap();
        assert_eq!(listed.len(), 1);

        let (workspaces, groups) = store.list_workspace_catalog(&other).unwrap();
        let entry = workspaces.iter().find(|w| w.id == id).unwrap();
        assert_eq!(Path::new(&entry.path), moved);
        assert!(
            groups
                .iter()
                .any(|g| g.workspace_id == id && g.sessions.len() == 1)
        );

        fs::remove_dir_all(&moved).unwrap();
        let (workspaces, _) = store.list_workspace_catalog(&other).unwrap();
        assert!(workspaces.iter().all(|w| w.id != id));
    }

    #[test]
    fn legacy_sessions_are_adopted_by_identity() {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(&temp.path().join("config"));
        let root = temp.path().join("legacy");
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let mut legacy = session("b", &root, None);
        legacy.working_directory = Some(root.join("src").display().to_string());
        store.save(&legacy).unwrap();
        store.prepare().unwrap();
        let loaded = store.load("b").unwrap();
        assert_eq!(loaded.workspace_id, read_workspace_id(&root));
        assert_eq!(loaded.working_directory.as_deref(), Some("src"));

        let moved = temp.path().join("moved");
        fs::rename(&root, &moved).unwrap();
        assert_eq!(store.list(&moved).unwrap().len(), 1);
    }

    #[test]
    fn stored_paths_rebase_onto_current_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("new").join("place");
        let old = temp.path().join("old");
        let elsewhere = temp.path().join("elsewhere");
        let root_text = root.to_string_lossy().into_owned();
        let old_text = old.to_string_lossy().into_owned();
        let elsewhere_text = elsewhere.to_string_lossy().into_owned();

        assert_eq!(
            PathBuf::from(resolve_workspace_path(&root, &old_text, "src")),
            root.join("src")
        );
        assert_eq!(
            PathBuf::from(resolve_workspace_path(&root, &old_text, ".")),
            root
        );
        assert_eq!(
            PathBuf::from(resolve_workspace_path(
                &root,
                &old_text,
                &old.join("lib").to_string_lossy()
            )),
            root.join("lib")
        );
        assert_eq!(
            PathBuf::from(resolve_workspace_path(&root, &old_text, &elsewhere_text)),
            elsewhere
        );
        assert_eq!(workspace_relative_path(&root, &root_text), ".");
        assert_eq!(
            workspace_relative_path(&root, &root.join("a").join("b").to_string_lossy()),
            "a/b"
        );
    }
}
