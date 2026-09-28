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
        AgentMode, ConversationEntry, SessionSummary, WorkspaceSessionGroup, WorkspaceSummary,
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

    pub fn append_event(
        &self,
        id: &str,
        run_id: Option<&str>,
        event: &serde_json::Value,
    ) -> Result<()> {
        self.serialized_session(id, true, || self.append_event_unlocked(id, run_id, event))
    }

    fn append_event_unlocked(
        &self,
        id: &str,
        run_id: Option<&str>,
        event: &serde_json::Value,
    ) -> Result<()> {
        use std::io::Write;
        validate_id(id)?;
        let root = self.directory.join(id);
        create_private_dir(&root)?;
        let is_debate = event_is_debate(event);
        let category = if is_debate { DEBATES_DIR } else { TASKS_DIR };
        let event_directory = root.join(category);
        create_private_dir(&event_directory)?;
        let mut options = fs::OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let path = event_directory.join("events.jsonl");
        let seq = self.next_event_sequence(&root)?;
        let mut file = options.open(&path)?;
        let event_type = event
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let mut payload = event.clone();
        if let Some(object) = payload.as_object_mut() {
            object.remove("type");
        }
        let record = serde_json::json!({
            "schemaVersion": 1,
            "seq": seq,
            "eventId": Uuid::new_v4().to_string(),
            "timestamp": Utc::now(),
            "sessionId": id,
            "runId": run_id,
            "stream": if is_debate { "debate" } else { "agent" },
            "type": event_type,
            "payload": payload,
        });
        writeln!(file, "{}", serde_json::to_string(&record)?)?;
        file.flush()?;
        Ok(())
    }

    fn next_event_sequence(&self, root: &Path) -> Result<u64> {
        use std::io::{BufRead, BufReader};
        let counter_path = root.join(EVENT_SEQUENCE_FILE);
        let current = match fs::read_to_string(&counter_path) {
            Ok(value) => value.trim().parse::<u64>().unwrap_or(0),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut parsed_max = 0u64;
                let mut total_lines = 0u64;
                for stream in [TASKS_DIR, DEBATES_DIR] {
                    let path = root.join(stream).join("events.jsonl");
                    let Some(reader) = fs::File::open(path).ok().map(BufReader::new) else {
                        continue;
                    };
                    for line in reader.lines().map_while(Result::ok) {
                        total_lines = total_lines.saturating_add(1);
                        parsed_max = parsed_max.max(
                            serde_json::from_str::<serde_json::Value>(&line)
                                .ok()
                                .and_then(|value| value.get("seq")?.as_u64())
                                .unwrap_or(0),
                        );
                    }
                }
                parsed_max.max(total_lines)
            }
            Err(error) => return Err(error.into()),
        };
        let next = current.saturating_add(1);
        write_private_replace(&counter_path, next.to_string().as_bytes())?;
        Ok(next)
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

    pub fn export_session(
        &self,
        id: &str,
        destination: Option<&Path>,
        delete_source: bool,
        active_session_id: Option<&str>,
    ) -> Result<SessionExportResult> {
        self.serialized_session(id, true, || {
            self.export_session_unlocked(id, destination, delete_source, active_session_id)
        })
    }

    fn export_session_unlocked(
        &self,
        id: &str,
        destination: Option<&Path>,
        delete_source: bool,
        active_session_id: Option<&str>,
    ) -> Result<SessionExportResult> {
        validate_id(id)?;
        let root = self.directory.join(id);
        ensure!(root.is_dir(), "Session directory does not exist: {id}");
        if let Ok(bytes) = fs::read(root.join(MANIFEST_FILE))
            && let Ok(manifest) = serde_json::from_slice::<SemanticManifest>(&bytes)
        {
            materialize_manifest(&root, &manifest)?;
        }
        if delete_source && active_session_id == Some(id) {
            bail!("Refusing to delete the active session {id}");
        }

        let config_root = self
            .directory
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.directory.clone());
        let exports = config_root.join("exports");
        create_private_dir(&exports)?;
        let destination = destination
            .map(Path::to_path_buf)
            .unwrap_or_else(|| exports.join(format!("{id}.tar")));
        let destination = if destination.is_absolute() {
            destination
        } else {
            std::env::current_dir()?.join(destination)
        };
        let destination = normalize_lexical(&destination);
        let root_normalized = root
            .canonicalize()
            .unwrap_or_else(|_| normalize_lexical(&root));
        ensure!(
            !destination.starts_with(&root_normalized),
            "Session exports must be written outside the source session directory"
        );
        ensure!(
            !destination.exists(),
            "Export destination already exists: {}",
            destination.display()
        );
        let parent = destination
            .parent()
            .context("export destination has no parent")?;
        create_private_dir(parent)?;
        let temp = parent.join(format!(".{}.{}.tmp", id, Uuid::new_v4()));

        if let Err(error) = create_session_archive(&root, id, &temp) {
            let _ = fs::remove_file(&temp);
            return Err(error).context(format!("create session archive for {id}"));
        }
        let listing = match session_archive_listing(&temp) {
            Ok(listing) => listing,
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return Err(error).context(format!("verify session archive for {id}"));
            }
        };
        let required_metadata = format!("{id}/metadata.json");
        let required_conversation = format!("{id}/conversation.json");
        ensure!(
            listing.iter().any(|line| line == &required_metadata)
                && listing.iter().any(|line| line == &required_conversation),
            "Session archive verification failed: required session files are missing"
        );
        ensure!(
            !listing.iter().any(|line| {
                let name = Path::new(line)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default();
                name == ".DS_Store"
                    || name.starts_with("._")
                    || name.ends_with(".tar")
                    || name.ends_with(".tar.gz")
            }),
            "Session archive verification failed: ignored recursive/archive metadata leaked into export"
        );

        let mut file = fs::File::open(&temp)?;
        file.sync_all()?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        let bytes = file.metadata()?.len();
        let sha256 = format!("{:x}", digest.finalize());
        drop(file);
        fs::rename(&temp, &destination)?;

        let mut source_deleted = false;
        if delete_source {
            ensure!(
                active_session_id != Some(id),
                "Session became active again before deletion; export kept and source preserved"
            );
            let trash_root = config_root.join("trash");
            create_private_dir(&trash_root)?;
            let trash = trash_root.join(format!("{id}-{}", Uuid::new_v4()));
            fs::rename(&root, &trash)?;
            // Deletion happens only after archive creation, reopen/list
            // verification, fsync, checksum, and an inactive-session recheck.
            // If cleanup fails, the source remains recoverable under trash/.
            match fs::remove_dir_all(&trash) {
                Ok(()) => source_deleted = true,
                Err(error) => {
                    return Err(error).context(format!(
                        "Export succeeded at {}, but cleanup of quarantined source {} failed",
                        destination.display(),
                        trash.display()
                    ));
                }
            }
        }

        Ok(SessionExportResult {
            session_id: id.to_owned(),
            path: destination.display().to_string(),
            sha256,
            bytes,
            source_deleted,
        })
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

    fn load_semantic_layout_legacy(&self, root: &Path) -> Result<StoredSession> {
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("metadata.json"))?)?;
        value["conversation"] = serde_json::from_slice(&fs::read(root.join("conversation.json"))?)?;
        value["modelHistory"] =
            serde_json::from_slice(&fs::read(root.join("model-history.json"))?)?;
        let runs = root.join(RUNS_DIR).join("index.json");
        if runs.exists() {
            value["runs"] = serde_json::from_slice(&fs::read(runs)?)?;
        }
        let knowledge = root.join(DEBATES_DIR).join("knowledge.json");
        if knowledge.exists() {
            value["retainedDebateKnowledge"] = serde_json::from_slice(&fs::read(knowledge)?)?;
        }
        let debate_path = root.join(DEBATES_DIR).join("state.json");
        if debate_path.exists() {
            value["debate"] = serde_json::from_slice(&fs::read(debate_path)?)?;
        }
        Ok(serde_json::from_value(value)?)
    }

    fn load_revision_layout(&self, root: &Path) -> Result<StoredSession> {
        let revision = fs::read_to_string(root.join("current"))?;
        validate_id(&revision)?;
        let snapshot = root.join(revision);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(snapshot.join("metadata.json"))?)?;
        value["conversation"] =
            serde_json::from_slice(&fs::read(snapshot.join("conversation.json"))?)?;
        value["modelHistory"] =
            serde_json::from_slice(&fs::read(snapshot.join("model-history.json"))?)?;
        Ok(serde_json::from_value(value)?)
    }

    fn migrate_legacy_layouts(&self) {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return;
        };
        let entries = entries
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();

        // Revision-directory sessions first.
        for root in entries.iter().filter(|path| path.is_dir()) {
            if root.join("metadata.json").exists() {
                if root.join(MANIFEST_FILE).is_file() {
                    if let Ok(bytes) = fs::read(root.join(MANIFEST_FILE))
                        && let Ok(manifest) = serde_json::from_slice::<SemanticManifest>(&bytes)
                    {
                        let _ = materialize_manifest(root, &manifest);
                        let _ = gc_semantic_objects(root, &manifest);
                    }
                } else if let Ok(session) = self.load_semantic_layout_legacy(root) {
                    let _ = self.save_unlocked(&session);
                }
                let _ = self.migrate_legacy_runtime_log(root);
                let _ = cleanup_legacy_layout(root);
                continue;
            }
            if !root.join("current").exists() {
                continue;
            }
            let Ok(session) = self.load_revision_layout(root) else {
                continue;
            };
            if self.save_unlocked(&session).is_err() {
                continue;
            }
        }

        // Old single-file sessions become folders too.
        for path in entries.iter().filter(|path| {
            path.is_file() && path.extension().and_then(|value| value.to_str()) == Some("json")
        }) {
            let Some(id) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if validate_id(id).is_err() {
                continue;
            }
            let Ok(data) = fs::read(path) else {
                continue;
            };
            let Ok(session) = serde_json::from_slice::<StoredSession>(&data) else {
                continue;
            };
            if session.id != id {
                continue;
            }
            let root = self.directory.join(id);
            if !root.join("metadata.json").exists() && self.save_unlocked(&session).is_err() {
                continue;
            }
            let _ = fs::remove_file(path);
        }
    }

    /// Stamps pre-identity sessions with their workspace's identity while the
    /// recorded folder still exists, so they follow later moves.
    fn backfill_workspace_ids(&self) {
        let Ok(records) = self.scan_records() else {
            return;
        };
        for record in records
            .iter()
            .filter(|record| record.workspace_id.is_none())
        {
            let Ok(root) = Path::new(&record.workspace_root).canonicalize() else {
                continue;
            };
            if !root.is_dir() {
                continue;
            }
            let Ok(workspace_id) = ensure_workspace_id(&root) else {
                continue;
            };
            let Ok(mut session) = self.load_unlocked(&record.id) else {
                continue;
            };
            session.workspace_id = Some(workspace_id);
            session.working_directory = session
                .working_directory
                .map(|path| workspace_relative_path(&root, &path));
            for path in &mut session.context_roots {
                *path = workspace_relative_path(&root, path);
            }
            let _ = self.save_unlocked(&session);
        }
    }

    fn migrate_legacy_runtime_log(&self, root: &Path) -> Result<()> {
        use std::io::Write;

        let legacy = root.join("runtime.jsonl");
        if !legacy.exists() {
            return Ok(());
        }
        let target_directory = root.join(TASKS_DIR);
        create_private_dir(&target_directory)?;
        let target = target_directory.join("events.jsonl");
        if target.exists() {
            let data = fs::read(&legacy)?;
            let mut options = fs::OpenOptions::new();
            options.append(true);
            let mut file = options.open(&target)?;
            file.write_all(&data)?;
            file.flush()?;
        } else {
            fs::rename(&legacy, &target)?;
        }
        remove_file_if_exists(&legacy)?;
        Ok(())
    }

    fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        set_private_directory(&self.directory)?;
        Ok(())
    }
}

fn normalize_lexical(path: &Path) -> PathBuf {
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

fn create_session_archive(source: &Path, id: &str, destination: &Path) -> Result<()> {
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let mut builder = tar::Builder::new(file);
    let archive_root = Path::new(id);
    builder.append_dir(archive_root, source)?;
    append_session_archive_tree(&mut builder, source, archive_root)?;
    builder.finish()?;
    builder.into_inner()?.sync_all()?;
    Ok(())
}

fn append_session_archive_tree(
    builder: &mut tar::Builder<fs::File>,
    source: &Path,
    archive_root: &Path,
) -> Result<()> {
    let mut entries = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        if ignored_archive_name(&name.to_string_lossy()) {
            continue;
        }
        let path = entry.path();
        let archive_path = archive_root.join(&name);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            builder.append_dir(&archive_path, &path)?;
            append_session_archive_tree(builder, &path, &archive_path)?;
        } else if file_type.is_file() {
            builder.append_path_with_name(&path, &archive_path)?;
        } else {
            bail!(
                "Refusing to export non-regular session entry {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn ignored_archive_name(name: &str) -> bool {
    name == ".DS_Store"
        || name.starts_with("._")
        || name == "exports"
        || name.ends_with(".tar")
        || name.ends_with(".tar.gz")
}

fn session_archive_listing(path: &Path) -> Result<Vec<String>> {
    let file = fs::File::open(path)?;
    let mut archive = tar::Archive::new(file);
    let mut listing = Vec::new();
    for entry in archive.entries()? {
        let entry = entry?;
        listing.push(entry.path()?.to_string_lossy().replace('\\', "/"));
    }
    Ok(listing)
}

fn lock_is_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error() == lock_contended_error().raw_os_error()
}

struct StoreFileLock {
    file: fs::File,
}

impl StoreFileLock {
    fn acquire(path: &Path, exclusive: bool) -> Result<Self> {
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        if exclusive {
            file.lock_exclusive().context("lock session store")?;
        } else {
            file.lock_shared().context("lock session store")?;
        }
        Ok(Self { file })
    }

    fn try_acquire_exclusive(path: &Path) -> Result<Option<Self>> {
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { file })),
            Err(error) if lock_is_contended(&error) => Ok(None),
            Err(error) => Err(error).context("lock session store"),
        }
    }
}

impl Drop for StoreFileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn event_is_debate(event: &serde_json::Value) -> bool {
    event
        .get("type")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|kind| kind.starts_with("debate"))
}

fn cleanup_legacy_layout(root: &Path) -> Result<()> {
    remove_file_if_exists(&root.join("current"))?;
    remove_file_if_exists(&root.join("tasks.json"))?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if matches!(name, TASKS_DIR | DEBATES_DIR | RUNS_DIR) {
            continue;
        }
        let looks_like_revision = path.join("metadata.json").is_file()
            && path.join("conversation.json").is_file()
            && path.join("model-history.json").is_file();
        if looks_like_revision {
            fs::remove_dir_all(path)?;
        }
    }
    Ok(())
}

fn cleanup_stale_materialization_links_in_store(directory: &Path) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            cleanup_stale_materialization_links(&entry.path())?;
        }
    }
    Ok(())
}

fn cleanup_stale_materialization_links(root: &Path) -> Result<()> {
    for directory in [
        root.to_path_buf(),
        root.join(RUNS_DIR),
        root.join(DEBATES_DIR),
    ] {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if is_stale_materialization_link(name) {
                remove_file_if_exists(&entry.path())?;
            }
        }
    }
    Ok(())
}

fn is_stale_materialization_link(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".link") else {
        return false;
    };
    let Some((base, id)) = stem.rsplit_once('.') else {
        return false;
    };
    matches!(
        base,
        ".metadata.json"
            | ".conversation.json"
            | ".model-history.json"
            | ".index.json"
            | ".knowledge.json"
            | ".state.json"
    ) && Uuid::parse_str(id).is_ok()
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

fn sha256_bytes(data: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(data);
    format!("{:x}", digest.finalize())
}

fn read_manifest_component(
    root: &Path,
    manifest: &SemanticManifest,
    component: &str,
) -> Result<Option<Vec<u8>>> {
    let Some(hash) = manifest.components.get(component).cloned().flatten() else {
        return Ok(None);
    };
    ensure!(
        hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid session object hash for {component}"
    );
    let path = root.join(OBJECTS_DIR).join(&hash);
    let bytes = fs::read(&path)
        .with_context(|| format!("read session object {} for {component}", path.display()))?;
    ensure!(
        sha256_bytes(&bytes) == hash,
        "session object checksum mismatch for {component}"
    );
    Ok(Some(bytes))
}

fn materialize_component(object: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        create_private_dir(parent)?;
    }
    let file_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("component");
    let tmp = target.with_file_name(format!(".{file_name}.{}.link", Uuid::new_v4()));
    fs::hard_link(object, &tmp)?;
    if let Err(error) = replace_file(&tmp, target) {
        let _ = fs::remove_file(&tmp);
        return Err(error.into());
    }
    Ok(())
}

fn gc_semantic_objects(root: &Path, manifest: &SemanticManifest) -> Result<()> {
    let referenced = manifest
        .components
        .values()
        .filter_map(|value| value.as_ref())
        .cloned()
        .collect::<HashSet<_>>();
    let objects = root.join(OBJECTS_DIR);
    let Ok(entries) = fs::read_dir(&objects) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !referenced.contains(&name) {
            remove_file_if_exists(&path)?;
        }
    }
    Ok(())
}

fn materialize_manifest(root: &Path, manifest: &SemanticManifest) -> Result<()> {
    for component in [
        "metadata.json",
        "conversation.json",
        "model-history.json",
        "runs/index.json",
        "debates/knowledge.json",
        "debates/state.json",
    ] {
        match manifest.components.get(component).cloned().flatten() {
            Some(hash) => {
                let object = root.join(OBJECTS_DIR).join(hash);
                ensure!(object.is_file(), "missing session object for {component}");
                materialize_component(&object, &root.join(component))?;
            }
            None => remove_file_if_exists(&root.join(component))?,
        }
    }
    Ok(())
}

fn write_private_replace(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("session");
    let tmp = path.with_file_name(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;

    set_private_file(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = replace_file(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error.into());
    }

    set_private_file(path)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn workspace_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceRegistry {
    #[serde(default)]
    workspaces: BTreeMap<String, RegisteredWorkspace>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisteredWorkspace {
    path: String,
    last_seen: DateTime<Utc>,
}

/// Maps workspace identities to their current location for one listing pass.
/// A workspace is found at its registered path or the session's last known
/// path, but only if the marker there still carries the same identity.
/// Entries whose folder vanished are pruned; the sessions themselves are kept
/// and reappear once the workspace is opened from its new location.
struct WorkspaceResolver {
    registry_path: PathBuf,
    registry: WorkspaceRegistry,
    dirty: bool,
    current_id: String,
    current_path: PathBuf,
    located: BTreeMap<String, Option<PathBuf>>,
    legacy: BTreeMap<PathBuf, Option<String>>,
}

impl WorkspaceResolver {
    fn new(store: &SessionStore, current: &Path) -> Result<Self> {
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

    fn resolve(&mut self, record: &SessionListRecord) -> Option<(String, PathBuf)> {
        let hint = PathBuf::from(&record.workspace_root);
        let id = match &record.workspace_id {
            Some(id) => id.clone(),
            None => self.legacy_id(&hint)?,
        };
        let path = self.locate(&id, &hint)?;
        Some((id, path))
    }

    fn legacy_id(&mut self, root: &Path) -> Option<String> {
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

    fn locate(&mut self, id: &str, hint: &Path) -> Option<PathBuf> {
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

    fn finish(self) -> Result<()> {
        if self.dirty {
            write_private_replace(
                &self.registry_path,
                &serde_json::to_vec_pretty(&self.registry)?,
            )?;
        }
        Ok(())
    }
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

fn workspace_marker_path(root: &Path) -> PathBuf {
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
    fn missing_agent_mode_defaults_to_single() {
        let temp = tempfile::tempdir().unwrap();
        let mut value = serde_json::to_value(session("legacy", temp.path(), None)).unwrap();
        value.as_object_mut().unwrap().remove("agentMode");

        let restored: StoredSession = serde_json::from_value(value).unwrap();
        assert_eq!(restored.agent_mode, AgentMode::Single);
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
