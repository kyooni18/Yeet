use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::{
    core::{Message, Usage},
    model::{ConversationEntry, SessionSummary},
};

const SESSION_LAYOUT_VERSION: u64 = 4;
const TASKS_DIR: &str = "tasks";
const DEBATES_DIR: &str = "debates";
const RUNS_DIR: &str = "runs";
const EVENT_SEQUENCE_FILE: &str = ".event-seq";
const OBJECTS_DIR: &str = ".objects";
const MANIFEST_FILE: &str = ".current.json";

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
    pub workspace_root: String,
    pub model: String,
    #[serde(default = "default_agent_mode")]
    pub agent_mode: String,
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
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Completed,
    CompletedUnverified,
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
    model: String,
    #[serde(default)]
    message_count: Option<usize>,
}

struct SessionListRecord {
    id: String,
    title: String,
    updated_at: DateTime<Utc>,
    workspace_root: String,
    model: String,
    message_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SemanticManifest {
    version: u64,
    components: BTreeMap<String, Option<String>>,
}

fn default_agent_mode() -> String {
    "auto".into()
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

    /// Prepare the on-disk store and collapse older revision-based sessions
    /// into the current semantic layout. Migration is intentionally done once
    /// at backend startup rather than on every save.
    pub fn prepare(&self) -> Result<()> {
        self.ensure()?;
        self.migrate_legacy_layouts();
        Ok(())
    }

    pub fn save(&self, session: &StoredSession) -> Result<()> {
        self.serialized(true, || self.save_unlocked(session))
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
        Ok(())
    }

    pub fn append_event(
        &self,
        id: &str,
        run_id: Option<&str>,
        event: &serde_json::Value,
    ) -> Result<()> {
        self.serialized(true, || self.append_event_unlocked(id, run_id, event))
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
        self.serialized(false, || self.load_unlocked(id))
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
        self.serialized(false, || self.list_unlocked(workspace_root))
    }

    fn list_unlocked(&self, workspace_root: &Path) -> Result<Vec<SessionSummary>> {
        self.ensure()?;
        let canonical = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let mut result = Vec::new();
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
            if result.iter().any(|s: &SessionSummary| s.id == id) {
                continue;
            }
            let summary = if path.is_dir()
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
                let Some(metadata) = metadata else {
                    continue;
                };
                let Ok(metadata) = serde_json::from_slice::<SessionListMetadata>(&metadata) else {
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
                    model: session.model,
                    message_count: session.conversation.len(),
                }
            };
            let root = PathBuf::from(&summary.workspace_root)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(&summary.workspace_root));
            if root != canonical {
                continue;
            }
            result.push(SessionSummary {
                id: summary.id,
                title: summary.title,
                updated_at: summary.updated_at.to_rfc3339(),
                model: summary.model,
                message_count: summary.message_count,
            });
        }
        result.sort_by(|lhs, rhs| rhs.updated_at.cmp(&lhs.updated_at));
        Ok(result)
    }

    pub fn is_debate_session(&self, id: &str) -> bool {
        self.serialized(false, || Ok(self.is_debate_session_unlocked(id)))
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
        self.serialized(true, || {
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
                    let _ = self.save(&session);
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
            if self.save(&session).is_err() {
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
            if !root.join("metadata.json").exists() && self.save(&session).is_err() {
                continue;
            }
            let _ = fs::remove_file(path);
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
        #[cfg(unix)]
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
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

fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
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
    if let Err(error) = fs::rename(&tmp, target) {
        if target.exists() {
            fs::remove_file(target)?;
            fs::rename(&tmp, target)?;
        } else {
            let _ = fs::remove_file(&tmp);
            return Err(error.into());
        }
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
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = fs::rename(&tmp, path) {
        if path.exists() {
            fs::remove_file(path)?;
            fs::rename(&tmp, path)?;
        } else {
            let _ = fs::remove_file(&tmp);
            return Err(error.into());
        }
    }
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{Message, Usage},
        model::{ConversationEntry, ConversationKind},
    };

    #[test]
    fn session_store_round_trips_and_lists_newest_first() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let now = Utc::now();
        for (index, title) in ["older", "newer"].into_iter().enumerate() {
            let mut debate = crate::debate::DebateState::default();
            debate.models = crate::debate::DebateModels {
                pro: "p/pro".into(),
                con: "p/con".into(),
                jury: "p/jury".into(),
            };
            let session = StoredSession {
                version: 1,
                debate: Some(debate),
                id: format!("session-{index}"),
                title: title.into(),
                created_at: now,
                updated_at: now + chrono::Duration::seconds(index as i64),
                workspace_root: workspace.path().display().to_string(),
                model: "openai/test".into(),
                agent_mode: "auto".into(),
                token_usage: Usage::default(),
                credit_usage: 0,
                conversation: vec![ConversationEntry {
                    id: "entry".into(),
                    kind: ConversationKind::User {
                        content: "hello".into(),
                    },
                }],
                model_history: vec![Message::user("hello")],
                runs: vec![],
                retained_debate_knowledge: vec![],
                attached_harness_capabilities: Some(vec![]),
                disabled_capabilities: vec!["skill:example".into()],
            };
            store.save(&session).unwrap();
        }
        let root = store.directory.join("session-0");
        for file in ["metadata.json", "conversation.json", "model-history.json"] {
            assert!(root.join(file).is_file());
        }
        assert!(!root.join("tasks/entries.json").exists());
        assert!(root.join("debates/state.json").is_file());
        assert!(root.join("runs/index.json").is_file());
        assert!(!root.join("current").exists());
        let directories = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            directories,
            [
                ".objects".to_owned(),
                "debates".to_owned(),
                "runs".to_owned(),
                "tasks".to_owned(),
            ]
            .into_iter()
            .collect()
        );
        assert!(!store.file_path("session-0").exists());
        let saved = store.load("session-0").unwrap();
        assert_eq!(saved.debate.as_ref().unwrap().models.jury, "p/jury");
        assert_eq!(saved.debate.as_ref().unwrap().models.pro, "p/pro");
        assert_eq!(saved.debate.as_ref().unwrap().models.con, "p/con");
        fs::write(
            store.file_path("legacy-copy"),
            serde_json::to_vec(&StoredSession {
                id: "legacy-copy".into(),
                ..saved.clone()
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(store.load("legacy-copy").unwrap().title, "older");
        fs::remove_file(store.file_path("legacy-copy")).unwrap();
        fs::create_dir(root.join("unfinished-revision")).unwrap();
        assert_eq!(store.load("session-0").unwrap().title, "older");
        assert!(store.load("../escape").is_err());
        store
            .append_event(
                "session-0",
                Some("run-1"),
                &serde_json::json!({"type":"agent-tool-started", "tool":"read_file"}),
            )
            .unwrap();
        assert!(
            fs::read_to_string(root.join("tasks/events.jsonl"))
                .unwrap()
                .contains("agent-tool-started")
        );
        let task_event: serde_json::Value = serde_json::from_str(
            fs::read_to_string(root.join("tasks/events.jsonl"))
                .unwrap()
                .lines()
                .last()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(task_event["seq"], 1);
        assert_eq!(task_event["runId"], "run-1");
        assert_eq!(task_event["type"], "agent-tool-started");
        assert_eq!(task_event["stream"], "agent");
        store
            .append_event(
                "session-0",
                Some("run-2"),
                &serde_json::json!({"type":"debate-research", "stage":1}),
            )
            .unwrap();
        assert!(
            fs::read_to_string(root.join("debates/events.jsonl"))
                .unwrap()
                .contains("debate-research")
        );
        let debate_event: serde_json::Value = serde_json::from_str(
            fs::read_to_string(root.join("debates/events.jsonl"))
                .unwrap()
                .lines()
                .last()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(debate_event["seq"], 2);
        assert_eq!(debate_event["runId"], "run-2");
        assert_eq!(debate_event["type"], "debate-research");
        assert_eq!(debate_event["stream"], "debate");
        assert_eq!(
            fs::read_to_string(root.join(EVENT_SEQUENCE_FILE))
                .unwrap()
                .trim(),
            "2"
        );
        assert_eq!(store.load("session-0").unwrap().title, "older");
        assert_eq!(
            store.load("session-0").unwrap().disabled_capabilities,
            vec!["skill:example"]
        );
        let listed = store.list(workspace.path()).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|value| value.title.as_str())
                .collect::<Vec<_>>(),
            vec!["newer", "older"]
        );
    }

    #[test]
    fn semantic_manifest_is_authoritative_over_stale_root_mirrors() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let now = Utc::now();
        let session = StoredSession {
            version: SESSION_LAYOUT_VERSION,
            debate: None,
            id: "manifest-authority".into(),
            title: "committed".into(),
            created_at: now,
            updated_at: now,
            workspace_root: workspace.path().display().to_string(),
            model: "openai/test".into(),
            agent_mode: "auto".into(),
            token_usage: Usage::default(),
            credit_usage: 0,
            conversation: vec![ConversationEntry {
                id: "user".into(),
                kind: ConversationKind::User {
                    content: "committed conversation".into(),
                },
            }],
            model_history: vec![Message::user("committed history")],
            runs: vec![],
            retained_debate_knowledge: vec![],
            attached_harness_capabilities: None,
            disabled_capabilities: vec![],
        };
        store.save(&session).unwrap();
        let root = store.directory.join(&session.id);

        // Simulate a crash after root compatibility files moved to a newer
        // generation but before the manifest commit. The authoritative object
        // graph must still load the previous complete generation.
        fs::remove_file(root.join("metadata.json")).unwrap();
        fs::write(
            root.join("metadata.json"),
            br#"{"id":"wrong","title":"uncommitted"}"#,
        )
        .unwrap();
        fs::remove_file(root.join("conversation.json")).unwrap();
        fs::write(root.join("conversation.json"), b"[]").unwrap();

        let loaded = store.load(&session.id).unwrap();
        assert_eq!(loaded.title, "committed");
        assert_eq!(loaded.conversation.len(), 1);
        assert_eq!(
            loaded.model_history[0].content.as_deref(),
            Some("committed history")
        );

        store.prepare().unwrap();
        let repaired: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("metadata.json")).unwrap()).unwrap();
        assert_eq!(repaired["title"], "committed");
    }

    #[test]
    fn session_export_is_verified_hygienic_and_never_deletes_active_session() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let now = Utc::now();
        let mut debate = crate::debate::DebateState::default();
        debate.topic = "Current implementation is safe".into();
        let session = StoredSession {
            version: SESSION_LAYOUT_VERSION,
            debate: Some(debate),
            id: "debate-export".into(),
            title: "debate export".into(),
            created_at: now,
            updated_at: now,
            workspace_root: workspace.path().display().to_string(),
            model: "openai/test".into(),
            agent_mode: "auto".into(),
            token_usage: Usage::default(),
            credit_usage: 0,
            conversation: vec![],
            model_history: vec![],
            runs: vec![],
            retained_debate_knowledge: vec![],
            attached_harness_capabilities: None,
            disabled_capabilities: vec![],
        };
        store.save(&session).unwrap();
        assert!(store.is_debate_session(&session.id));

        let source = store.directory.join(&session.id);
        fs::write(source.join(".DS_Store"), b"junk").unwrap();
        fs::write(source.join("._metadata.json"), b"junk").unwrap();
        fs::write(source.join("old.tar"), b"junk").unwrap();

        let exported = store
            .export_session(&session.id, None, false, Some(&session.id))
            .unwrap();
        assert!(!exported.source_deleted);
        assert_eq!(exported.sha256.len(), 64);
        assert!(Path::new(&exported.path).is_file());
        assert!(source.is_dir());

        let listing = session_archive_listing(Path::new(&exported.path)).unwrap();
        assert!(
            listing
                .iter()
                .any(|path| path == "debate-export/metadata.json")
        );
        assert!(!listing.iter().any(|path| path.contains(".DS_Store")));
        assert!(!listing.iter().any(|path| path.contains("._metadata.json")));
        assert!(!listing.iter().any(|path| path.contains("old.tar")));

        let active_delete = store.export_session(
            &session.id,
            Some(&temp.path().join("active-delete.tar")),
            true,
            Some(&session.id),
        );
        assert!(active_delete.is_err());
        assert!(source.is_dir());

        let deleted = store
            .export_session(
                &session.id,
                Some(&temp.path().join("inactive-delete.tar")),
                true,
                None,
            )
            .unwrap();
        assert!(deleted.source_deleted);
        assert!(!source.exists());
        assert!(Path::new(&deleted.path).is_file());
    }

    #[test]
    fn retained_debate_knowledge_round_trips_as_typed_debate_state() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let now = Utc::now();
        let knowledge = crate::debate::RetainedDebateKnowledge {
            subject_ref: crate::debate::DebateSubject {
                workspace_bound: true,
                workspace_root: workspace.path().display().to_string(),
                revision: Some("rev-a".into()),
                anchor_files: vec!["src".into()],
            },
            source_debate_run_id: "debate-run".into(),
            supported_claims: vec![crate::debate::RetainedKnowledgeClaim {
                text: "bounded controller".into(),
                evidence_ids: vec!["file:src/controller.rs".into()],
            }],
            strong_inferences: vec![],
            unresolved: vec!["flight test".into()],
            confidence: "evidence_backed".into(),
        };
        let session = StoredSession {
            version: SESSION_LAYOUT_VERSION,
            debate: None,
            id: "typed-memory".into(),
            title: "typed memory".into(),
            created_at: now,
            updated_at: now,
            workspace_root: workspace.path().display().to_string(),
            model: "openai/test".into(),
            agent_mode: "auto".into(),
            token_usage: Usage::default(),
            credit_usage: 0,
            conversation: vec![],
            model_history: vec![],
            runs: vec![],
            retained_debate_knowledge: vec![knowledge.clone()],
            attached_harness_capabilities: None,
            disabled_capabilities: vec![],
        };
        store.save(&session).unwrap();
        let root = store.directory.join(&session.id);
        assert!(root.join("debates/knowledge.json").is_file());
        assert!(
            !fs::read_to_string(root.join("metadata.json"))
                .unwrap()
                .contains("retainedDebateKnowledge")
        );
        let restored = store.load(&session.id).unwrap();
        assert_eq!(restored.retained_debate_knowledge, vec![knowledge]);
    }

    #[test]
    fn concurrent_save_and_load_never_mix_semantic_generations() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let now = Utc::now();
        let base = StoredSession {
            version: SESSION_LAYOUT_VERSION,
            debate: None,
            id: "concurrent".into(),
            title: "A".into(),
            created_at: now,
            updated_at: now,
            workspace_root: workspace.path().display().to_string(),
            model: "openai/test".into(),
            agent_mode: "auto".into(),
            token_usage: Usage::default(),
            credit_usage: 0,
            conversation: vec![ConversationEntry {
                id: "entry".into(),
                kind: ConversationKind::User {
                    content: "A".into(),
                },
            }],
            model_history: vec![Message::user("A")],
            runs: vec![],
            retained_debate_knowledge: vec![],
            attached_harness_capabilities: None,
            disabled_capabilities: vec![],
        };
        store.save(&base).unwrap();

        // Use a distinct store instance so this exercises the cross-instance
        // file lock rather than only the shared in-memory mutex on Clone.
        let writer_store = SessionStore::new(temp.path());
        let writer_base = base.clone();
        let writer = std::thread::spawn(move || {
            for index in 0..80 {
                let marker = if index % 2 == 0 { "A" } else { "B" };
                let mut session = writer_base.clone();
                session.title = marker.into();
                session.updated_at = Utc::now();
                session.conversation = vec![ConversationEntry {
                    id: "entry".into(),
                    kind: ConversationKind::User {
                        content: marker.into(),
                    },
                }];
                session.model_history = vec![Message::user(marker)];
                writer_store.save(&session).unwrap();
            }
        });

        for _ in 0..160 {
            let loaded = store.load("concurrent").unwrap();
            let content = match &loaded.conversation[0].kind {
                ConversationKind::User { content } => content.as_str(),
                other => panic!("unexpected conversation entry: {other:?}"),
            };
            assert_eq!(loaded.title, content);
            assert_eq!(loaded.model_history[0].content.as_deref(), Some(content));
        }
        writer.join().unwrap();
    }

    #[test]
    fn prepare_migrates_revision_and_single_file_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        fs::create_dir_all(&store.directory).unwrap();
        let now = Utc::now();
        let session = StoredSession {
            version: 2,
            debate: Some(crate::debate::DebateState::default()),
            id: "revision-session".into(),
            title: "revision".into(),
            created_at: now,
            updated_at: now,
            workspace_root: workspace.path().display().to_string(),
            model: "openai/test".into(),
            agent_mode: "auto".into(),
            token_usage: Usage::default(),
            credit_usage: 0,
            conversation: vec![],
            model_history: vec![],
            runs: vec![],
            retained_debate_knowledge: vec![],
            attached_harness_capabilities: None,
            disabled_capabilities: vec![],
        };

        let root = store.directory.join(&session.id);
        let revision = "old-revision";
        let snapshot = root.join(revision);
        fs::create_dir_all(&snapshot).unwrap();
        let mut metadata = serde_json::to_value(&session).unwrap();
        metadata.as_object_mut().unwrap().remove("conversation");
        metadata.as_object_mut().unwrap().remove("modelHistory");
        fs::write(
            snapshot.join("metadata.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        fs::write(snapshot.join("conversation.json"), b"[]").unwrap();
        fs::write(snapshot.join("model-history.json"), b"[]").unwrap();
        fs::write(root.join("current"), revision).unwrap();
        fs::write(root.join("runtime.jsonl"), "legacy runtime\n").unwrap();

        let single = StoredSession {
            id: "single-session".into(),
            title: "single".into(),
            ..session.clone()
        };
        fs::write(
            store.file_path("single-session"),
            serde_json::to_vec(&single).unwrap(),
        )
        .unwrap();

        store.prepare().unwrap();

        assert!(root.join("metadata.json").is_file());
        assert!(root.join("tasks/events.jsonl").is_file());
        assert!(!root.join("current").exists());
        assert!(!snapshot.exists());
        assert_eq!(store.load("revision-session").unwrap().title, "revision");
        assert!(
            store
                .directory
                .join("single-session/metadata.json")
                .is_file()
        );
        assert!(!store.file_path("single-session").exists());
        assert_eq!(store.load("single-session").unwrap().title, "single");
    }

    #[test]
    fn stored_session_serializes_swift_compatible_keys() {
        let now = Utc::now();
        let session = StoredSession {
            version: 1,
            debate: None,
            id: "id".into(),
            title: "title".into(),
            created_at: now,
            updated_at: now,
            workspace_root: "/tmp/project".into(),
            model: "openai/test".into(),
            agent_mode: "auto".into(),
            token_usage: Usage {
                input_tokens: Some(1),
                ..Usage::default()
            },
            credit_usage: 1,
            conversation: vec![],
            model_history: vec![],
            runs: vec![],
            retained_debate_knowledge: vec![],
            attached_harness_capabilities: None,
            disabled_capabilities: vec![],
        };
        let value = serde_json::to_value(session).unwrap();
        assert!(value.get("workspaceRoot").is_some());
        assert!(value.get("modelHistory").is_some());
        assert!(value.get("disabledCapabilities").is_none());
        assert!(value.get("attachedHarnessCapabilities").is_none());
        assert!(value.get("disabledLazyCapabilities").is_none());
        assert_eq!(value["tokenUsage"]["inputTokens"], 1);
    }

    #[test]
    fn stored_session_reads_legacy_disabled_lazy_capabilities_key() {
        let value = serde_json::json!({
            "version": 1,
            "id": "legacy",
            "title": "legacy",
            "createdAt": Utc::now(),
            "updatedAt": Utc::now(),
            "workspaceRoot": "/tmp/project",
            "model": "openai/test",
            "tokenUsage": {},
            "creditUsage": 0,
            "conversation": [],
            "modelHistory": [],
            "attachedHarnessCapabilities": null,
            "disabledLazyCapabilities": ["skill:example", "builtin:file-write"]
        });
        let session: StoredSession = serde_json::from_value(value).unwrap();
        assert_eq!(
            session.disabled_capabilities,
            vec!["skill:example", "builtin:file-write"]
        );
    }

    #[test]
    fn legacy_session_without_agent_mode_defaults_to_auto() {
        let raw = r#"{
          "version": 1,
          "id": "legacy-mode",
          "title": "legacy",
          "createdAt": "2026-01-01T00:00:00Z",
          "updatedAt": "2026-01-01T00:00:00Z",
          "workspaceRoot": "/tmp/example",
          "model": "openai/test",
          "tokenUsage": {},
          "creditUsage": 0,
          "conversation": [],
          "modelHistory": []
        }"#;
        let session: StoredSession = serde_json::from_str(raw).unwrap();
        assert_eq!(session.agent_mode, "auto");
    }
}
