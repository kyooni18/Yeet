//! Native Foundation-style project memory. One committed generation owns one
//! embedding space across ALL projects and lifecycle states. No model fallback
//! is permitted after that space has been pinned.
use crate::core::{BridgeClient, ToolDefinition};
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use uuid::Uuid;

const CANARY: &str = "Yeet embedding index identity probe v1: durable project memory, exact evidence, and stable vector geometry.";
const CHUNK_CHARS: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingResult {
    pub model: String,
    pub resolved_model: String,
    pub source: String,
    pub vectors: Vec<Vec<f64>>,
}
pub trait Embeddings {
    fn candidates(&self, cancel: &AtomicBool) -> Result<Vec<String>>;
    fn embed(&self, model: &str, input: &[String], cancel: &AtomicBool) -> Result<EmbeddingResult>;
}
impl Embeddings for BridgeClient {
    fn candidates(&self, cancel: &AtomicBool) -> Result<Vec<String>> {
        self.embedding_models(cancel)
    }
    fn embed(&self, model: &str, input: &[String], cancel: &AtomicBool) -> Result<EmbeddingResult> {
        self.embeddings(model, input, cancel)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Binding {
    model: String,
    resolved_model: String,
    source: String,
    dimensions: usize,
    canary: Vec<f64>,
    chunk_chars: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Atom {
    id: String,
    project: String,
    content: String,
    key: Option<String>,
    status: String,
    created_at: String,
    updated_at: String,
    supersedes: Vec<String>,
    metadata: Value,
    vectors: Vec<Vec<f64>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Index {
    version: u32,
    generation: u64,
    binding: Option<Binding>,
    atoms: Vec<Atom>,
    relations: Vec<Value>,
}
impl Default for Index {
    fn default() -> Self {
        Self {
            version: 1,
            generation: 0,
            binding: None,
            atoms: Vec::new(),
            relations: Vec::new(),
        }
    }
}
#[derive(Clone)]
pub struct MemoryStore {
    root: PathBuf,
    cache: Arc<Mutex<Option<CachedIndex>>>,
}
struct CachedIndex {
    modified: SystemTime,
    len: u64,
    index: Arc<Index>,
    validated: bool,
}
impl Default for MemoryStore {
    fn default() -> Self {
        Self::new(&crate::config::ConfigStore::default().directory)
    }
}
impl MemoryStore {
    pub fn new(config: &Path) -> Self {
        Self {
            root: config.join("memory"),
            cache: Arc::new(Mutex::new(None)),
        }
    }
    fn read(&self) -> Result<Index> {
        Ok((*self.read_shared(true)?).clone())
    }
    fn read_index(&self, validate: bool) -> Result<Index> {
        Ok((*self.read_shared(validate)?).clone())
    }
    fn read_shared(&self, validate: bool) -> Result<Arc<Index>> {
        let path = self.root.join("index.json");
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Arc::new(Index::default()));
            }
            Err(e) => return Err(e.into()),
        };
        let modified = metadata.modified()?;
        let len = metadata.len();
        if let Ok(mut cache) = self.cache.lock()
            && let Some(cached) = cache.as_mut()
            && cached.modified == modified
            && cached.len == len
        {
            if validate && !cached.validated {
                validate_index(&cached.index)?;
                cached.validated = true;
            }
            return Ok(cached.index.clone());
        }
        let bytes = fs::read(&path)?;
        let index: Index = serde_json::from_slice(&bytes)
            .context("Invalid Yeet memory index; original data was not modified")?;
        if validate {
            validate_index(&index)?;
        } else {
            validate_index_shape(&index)?;
        }
        let index = Arc::new(index);
        if let Ok(mut cache) = self.cache.lock() {
            *cache = Some(CachedIndex {
                modified,
                len,
                index: index.clone(),
                validated: validate,
            });
        }
        Ok(index)
    }
    fn refresh_cache(&self, index: &Index) -> Result<()> {
        let metadata = fs::metadata(self.root.join("index.json"))?;
        let cached = CachedIndex {
            modified: metadata.modified()?,
            len: metadata.len(),
            index: Arc::new(index.clone()),
            validated: true,
        };
        if let Ok(mut cache) = self.cache.lock() {
            *cache = Some(cached);
        }
        Ok(())
    }
    fn commit(&self, index: &Index, cancel: &AtomicBool) -> Result<()> {
        check_cancel(cancel)?;
        fs::create_dir_all(&self.root)?;
        let mut temp = tempfile::NamedTempFile::new_in(&self.root)?;
        serde_json::to_writer(&mut temp, index)?;
        temp.flush()?;
        temp.as_file().sync_all()?;
        check_cancel(cancel)?;
        temp.persist(self.root.join("index.json"))?;
        fs::File::open(&self.root)?.sync_all()?;
        self.refresh_cache(index)?;
        Ok(())
    }
    pub fn status(&self) -> Result<Value> {
        let index = self.read()?;
        Ok(
            json!({"storage":"yeet","generation":index.generation,"total":index.atoms.len(),
            "active":index.atoms.iter().filter(|a| a.status=="active").count(),
            "embeddingModel":index.binding.as_ref().map(|b| &b.model),
            "dimensions":index.binding.as_ref().map(|b| b.dimensions),
            "pinned":index.binding.is_some(),"path":self.root.join("index.json")}),
        )
    }
    /// Explicit user administration only. A new model is not installed until
    /// every atom, including archived/superseded atoms in every project, embeds.
    pub fn reindex(
        &self,
        model: Option<&str>,
        api: &impl Embeddings,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let _lock = Lock::acquire(&self.root, cancel)?;
        let mut index = self.read_index(false)?;
        let selected = model.or_else(|| index.binding.as_ref().map(|b| b.model.as_str()));
        let binding = select_binding(selected, api, cancel)?;
        for atom in &mut index.atoms {
            atom.vectors = embed_text(&binding, &atom.content, api, cancel)?;
        }
        index.binding = Some(binding);
        index.generation += 1;
        self.commit(&index, cancel)?;
        Ok(
            json!({"reindexed":index.atoms.len(),"generation":index.generation,"embeddingModel":index.binding.as_ref().unwrap().model}),
        )
    }
    pub fn call(
        &self,
        operation: &str,
        project: &str,
        args: &Value,
        api: &impl Embeddings,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        ensure!(!project.trim().is_empty(), "A project identity is required");
        if operation == "memory_recall" {
            return self.recall(project, args, api, cancel);
        }
        let _lock = Lock::acquire(&self.root, cancel)?;
        let mut index = self.read()?;
        let id = args.get("id").and_then(Value::as_str);
        let position = if let Some(id) = id {
            Some(
                index
                    .atoms
                    .iter()
                    .position(|a| a.id == id && a.project == project)
                    .ok_or_else(|| anyhow::anyhow!("Memory does not exist in this project"))?,
            )
        } else {
            None
        };
        let now = chrono::Utc::now().to_rfc3339();
        let result = match operation {
            "memory_forget" | "memory_restore" => {
                let pos = position.ok_or_else(|| anyhow::anyhow!("id is required"))?;
                if operation == "memory_restore" {
                    ensure!(
                        matches!(index.atoms[pos].status.as_str(), "archived" | "deleted"),
                        "Only archived memories can be restored; superseded memories retain their history"
                    );
                    if let Some(key) = &index.atoms[pos].key {
                        ensure!(
                            !index.atoms.iter().any(|a| a.project == project
                                && a.status == "active"
                                && a.key.as_ref() == Some(key)),
                            "An active memory already owns this key"
                        );
                    }
                    index.atoms[pos].status = "active".into();
                } else {
                    ensure!(
                        index.atoms[pos].status != "superseded",
                        "Cannot archive superseded history; archive its active replacement"
                    );
                    index.atoms[pos].status = "archived".into();
                }
                index.atoms[pos].updated_at = now;
                json!({"id":index.atoms[pos].id,"status":index.atoms[pos].status})
            }
            "memory_remember" | "memory_update" | "memory_replace" => {
                let content = text_arg(args, "text", 100_000)?.to_owned();
                let mut key = args
                    .get("key")
                    .map(|_| text_arg(args, "key", 200).map(str::to_owned))
                    .transpose()?;
                if operation != "memory_remember" {
                    let pos = position.ok_or_else(|| anyhow::anyhow!("id is required"))?;
                    ensure!(
                        index.atoms[pos].status == "active",
                        "Only active memories can be updated or replaced"
                    );
                    key = index.atoms[pos].key.clone();
                }
                if operation == "memory_remember"
                    && let Some(existing) = index.atoms.iter().find(|a| {
                        a.project == project
                            && a.status == "active"
                            && a.key == key
                            && a.content == content
                    })
                {
                    return Ok(json!({"id":existing.id,"unchanged":true}));
                }
                if index.binding.is_none() {
                    index.binding = Some(select_binding(None, api, cancel)?);
                }
                let vectors = embed_text(index.binding.as_ref().unwrap(), &content, api, cancel)?;
                if operation == "memory_update" {
                    let atom = &mut index.atoms[position.unwrap()];
                    atom.content = content;
                    atom.vectors = vectors;
                    atom.updated_at = now;
                    json!({"id":atom.id,"content":atom.content})
                } else {
                    let mut supersedes = Vec::new();
                    for (pos, atom) in index.atoms.iter_mut().enumerate() {
                        if atom.project == project
                            && atom.status == "active"
                            && ((key.is_some() && atom.key == key)
                                || (operation == "memory_replace" && position == Some(pos)))
                        {
                            atom.status = "superseded".into();
                            atom.updated_at = now.clone();
                            supersedes.push(atom.id.clone());
                        }
                    }
                    let id = Uuid::new_v4().to_string();
                    index.atoms.push(Atom {
                        id: id.clone(),
                        project: project.into(),
                        content: content.clone(),
                        key,
                        status: "active".into(),
                        created_at: now.clone(),
                        updated_at: now,
                        supersedes,
                        metadata: json!({}),
                        vectors,
                    });
                    json!({"id":id,"content":content})
                }
            }
            _ => bail!("Unknown native memory operation: {operation}"),
        };
        index.generation += 1;
        self.commit(&index, cancel)?;
        Ok(result)
    }
    fn recall(
        &self,
        project: &str,
        args: &Value,
        api: &impl Embeddings,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        let query = text_arg(args, "query", 20_000)?;
        let index = self.read_shared(true)?;
        let atoms = index
            .atoms
            .iter()
            .filter(|a| a.project == project && a.status == "active")
            .collect::<Vec<_>>();
        if atoms.is_empty() {
            return Ok(json!({"context":"","memories":[],"generation":index.generation}));
        }
        let query_vectors = embed_text(index.binding.as_ref().unwrap(), query, api, cancel)?;
        let words = query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut ranked = atoms
            .into_iter()
            .map(|atom| {
                let semantic = query_vectors
                    .iter()
                    .flat_map(|q| atom.vectors.iter().map(move |v| dot(q, v)))
                    .fold(-1.0, f64::max);
                let content = atom.content.to_lowercase();
                let lexical = words
                    .iter()
                    .filter(|w| content.contains(w.as_str()))
                    .count() as f64
                    / words.len().max(1) as f64;
                (semantic * 0.85 + lexical * 0.15, atom)
            })
            .filter(|(score, _)| *score >= 0.25)
            .collect::<Vec<_>>();
        ranked.sort_by(|(sa, a), (sb, b)| sb.total_cmp(sa).then_with(|| a.id.cmp(&b.id)));
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(6)
            .clamp(1, 20) as usize;
        let mut context = String::new();
        let mut memories = Vec::new();
        for (score, atom) in ranked.into_iter().take(limit) {
            let remaining = 4096usize.saturating_sub(context.len());
            if remaining < 80 {
                break;
            }
            let line = format!("[{}] {}\n", atom.id, atom.content);
            let bounded = bounded_bytes(&line, remaining);
            context.push_str(bounded);
            memories.push(json!({"id":atom.id,"score":score,"key":atom.key}));
        }
        Ok(json!({"context":context,"memories":memories,"generation":index.generation}))
    }
    /// Portable Foundation JSONL import is additive, idempotent and project-
    /// scoped by the invoking user. Preserve source metadata/relations, but
    /// discard old vectors and embed every imported lifecycle state anew.
    pub fn import_foundation(
        &self,
        path: &Path,
        project: &str,
        api: &impl Embeddings,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        ensure!(!project.trim().is_empty(), "Project identity required");
        let records = fs::read_to_string(path)?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str::<Value>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ensure!(
            records
                .first()
                .is_some_and(|r| r["type"] == "foundation_export" && r["version"] == 1),
            "Expected Foundation portable JSONL export version 1"
        );
        let _lock = Lock::acquire(&self.root, cancel)?;
        let mut index = self.read()?;
        let mut imported = 0;
        for record in records.iter().filter(|r| r["type"] == "atom") {
            let raw = &record["atom"];
            let original = text_arg(raw, "id", 200)?;
            if index
                .atoms
                .iter()
                .any(|a| a.project == project && a.metadata["foundation"]["id"] == original)
            {
                continue;
            }
            let content = text_arg(raw, "content", 100_000)?.to_owned();
            if index.binding.is_none() {
                index.binding = Some(select_binding(None, api, cancel)?);
            }
            let vectors = embed_text(index.binding.as_ref().unwrap(), &content, api, cancel)?;
            let now = chrono::Utc::now().to_rfc3339();
            let mut portable = raw.clone();
            if let Some(object) = portable.as_object_mut() {
                object.remove("embedding");
                object.remove("search_document");
            }
            index.atoms.push(Atom {
                id: Uuid::new_v4().to_string(),
                project: project.into(),
                content,
                key: raw
                    .pointer("/metadata/memory/key")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                status: raw
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("active")
                    .into(),
                created_at: raw
                    .get("created_at")
                    .and_then(Value::as_str)
                    .unwrap_or(&now)
                    .into(),
                updated_at: now,
                supersedes: Vec::new(),
                metadata: json!({"foundation":portable}),
                vectors,
            });
            imported += 1;
        }
        for record in records.iter().filter(|r| r["type"] == "relation") {
            let relation = json!({"project":project,"foundation":record["relation"]});
            if !index.relations.contains(&relation) {
                index.relations.push(relation);
            }
        }
        index.generation += 1;
        self.commit(&index, cancel)?;
        Ok(json!({"imported":imported,"project":project,"generation":index.generation}))
    }
}
fn select_binding(
    model: Option<&str>,
    api: &impl Embeddings,
    cancel: &AtomicBool,
) -> Result<Binding> {
    let candidates = match model {
        Some(model) => vec![model.to_owned()],
        None => api.candidates(cancel)?,
    };
    let mut errors = Vec::new();
    for model in candidates {
        check_cancel(cancel)?;
        match api
            .embed(&model, &[CANARY.into()], cancel)
            .and_then(|result| binding_from_result(&model, result))
        {
            Ok(binding) => return Ok(binding),
            Err(error) => errors.push(format!("{model}: {error}")),
        }
    }
    bail!(
        "No usable embedding model. Configure a Yeet API provider or run `yeet memory reindex provider/model` with an explicit embedding model. {}",
        errors.join("; ")
    )
}
fn binding_from_result(model: &str, result: EmbeddingResult) -> Result<Binding> {
    ensure!(
        result.model == model && result.vectors.len() == 1,
        "Embedding provider returned an unexpected model or vector count"
    );
    let canary = normalize(&result.vectors[0])?;
    Ok(Binding {
        model: model.into(),
        resolved_model: result.resolved_model,
        source: result.source,
        dimensions: canary.len(),
        canary,
        chunk_chars: CHUNK_CHARS,
    })
}
fn embed_text(
    binding: &Binding,
    text: &str,
    api: &impl Embeddings,
    cancel: &AtomicBool,
) -> Result<Vec<Vec<f64>>> {
    let chars = text.chars().collect::<Vec<_>>();
    let chunks = chars
        .chunks(CHUNK_CHARS)
        .map(|s| s.iter().collect::<String>())
        .collect::<Vec<_>>();
    let mut output = Vec::new();
    for batch in chunks.chunks(63) {
        check_cancel(cancel)?;
        let mut input = vec![CANARY.to_owned()];
        input.extend_from_slice(batch);
        let result = api.embed(&binding.model, &input, cancel)?;
        ensure!(
            result.model == binding.model
                && result.resolved_model == binding.resolved_model
                && result.source == binding.source,
            "Embedding model or endpoint changed; full re-index required: yeet memory reindex {}",
            binding.model
        );
        ensure!(
            result.vectors.len() == input.len(),
            "Embedding vector count mismatch"
        );
        for vector in &result.vectors {
            validate_vector(vector, binding.dimensions)?;
        }
        let canary = normalize(&result.vectors[0])?;
        ensure!(
            dot(&binding.canary, &canary) >= 0.999999,
            "Embedding model behavior changed; full re-index required: yeet memory reindex {}",
            binding.model
        );
        for vector in &result.vectors[1..] {
            output.push(normalize(vector)?);
        }
    }
    Ok(output)
}
fn validate_vector(vector: &[f64], dimensions: usize) -> Result<()> {
    ensure!(
        dimensions > 0
            && dimensions <= 65536
            && vector.len() == dimensions
            && vector.iter().all(|v| v.is_finite())
            && vector.iter().any(|v| *v != 0.0),
        "Invalid embedding or changed vector dimensions; full re-index required"
    );
    Ok(())
}
fn validate_index_shape(index: &Index) -> Result<()> {
    ensure!(index.version == 1, "Unsupported memory index version");
    ensure!(
        index.binding.is_some() || index.atoms.is_empty(),
        "Memory index has unbound data; re-index required"
    );
    Ok(())
}
fn validate_index(index: &Index) -> Result<()> {
    validate_index_shape(index)?;
    if let Some(binding) = &index.binding {
        ensure!(
            binding.chunk_chars == CHUNK_CHARS,
            "Embedding chunk policy changed; full re-index required"
        );
        validate_vector(&binding.canary, binding.dimensions)?;
        for atom in &index.atoms {
            ensure!(
                !atom.vectors.is_empty(),
                "Memory {} has no embeddings",
                atom.id
            );
            for vector in &atom.vectors {
                validate_vector(vector, binding.dimensions)?;
            }
        }
    }
    Ok(())
}
fn normalize(vector: &[f64]) -> Result<Vec<f64>> {
    validate_vector(vector, vector.len())?;
    let scale = vector.iter().map(|v| v.abs()).fold(0.0, f64::max);
    let norm = vector
        .iter()
        .map(|v| (v / scale).powi(2))
        .sum::<f64>()
        .sqrt();
    Ok(vector.iter().map(|v| v / scale / norm).collect())
}
fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn text_arg<'a>(args: &'a Value, name: &str, max_chars: usize) -> Result<&'a str> {
    let text = args
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("{name} is required"))?;
    ensure!(
        !text.trim().is_empty() && text.chars().count() <= max_chars,
        "{name} must contain 1..{max_chars} characters"
    );
    Ok(text)
}
fn bounded_bytes(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Acquire), "cancelled");
    Ok(())
}
struct Lock {
    _file: fs::File,
}
impl Lock {
    fn acquire(root: &Path, cancel: &AtomicBool) -> Result<Self> {
        fs::create_dir_all(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        }
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(root.join(".lock"))?;
        let started = Instant::now();
        loop {
            check_cancel(cancel)?;
            match file.try_lock_exclusive() {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => bail!("Unable to lock memory: {error}"),
            }
            ensure!(
                started.elapsed() < Duration::from_secs(30),
                "Memory is busy; another write or re-index is running"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        Ok(Self { _file: file })
    }
}

pub fn tool_definitions() -> Vec<ToolDefinition> {
    let specs = [
        (
            "memory_recall",
            "Recall relevant active project memories.",
            json!({"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20}}),
            vec!["query"],
        ),
        (
            "memory_remember",
            "Save durable project knowledge. Reuse a stable key for mutable facts; previous values remain in history.",
            json!({"text":{"type":"string"},"key":{"type":"string"}}),
            vec!["text"],
        ),
        (
            "memory_update",
            "Correct an active project memory by id.",
            json!({"id":{"type":"string"},"text":{"type":"string"}}),
            vec!["id", "text"],
        ),
        (
            "memory_replace",
            "Replace an active memory by id, preserving the superseded record.",
            json!({"id":{"type":"string"},"text":{"type":"string"}}),
            vec!["id", "text"],
        ),
        (
            "memory_forget",
            "Archive an active project memory by id.",
            json!({"id":{"type":"string"}}),
            vec!["id"],
        ),
        (
            "memory_restore",
            "Restore an archived project memory by id when no active value owns its key.",
            json!({"id":{"type":"string"}}),
            vec!["id"],
        ),
    ];
    specs.into_iter().map(|(name,description,properties,required)|ToolDefinition::new(format!("project_{name}"),description,json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    struct Fake {
        candidates: Mutex<Vec<String>>,
        requests: Mutex<Vec<(String, Vec<String>)>>,
        dimensions: Mutex<usize>,
        source: Mutex<String>,
        canary_changed: AtomicBool,
        fail_on: Mutex<Option<String>>,
        cancel_on: Mutex<Option<String>>,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                candidates: Mutex::new(vec!["local/embed-a".into()]),
                requests: Mutex::new(Vec::new()),
                dimensions: Mutex::new(3),
                source: Mutex::new("endpoint-a".into()),
                canary_changed: AtomicBool::new(false),
                fail_on: Mutex::new(None),
                cancel_on: Mutex::new(None),
            }
        }
    }
    impl Embeddings for Fake {
        fn candidates(&self, _: &AtomicBool) -> Result<Vec<String>> {
            Ok(self.candidates.lock().unwrap().clone())
        }
        fn embed(
            &self,
            model: &str,
            input: &[String],
            cancel: &AtomicBool,
        ) -> Result<EmbeddingResult> {
            self.requests
                .lock()
                .unwrap()
                .push((model.into(), input.to_vec()));
            if self
                .fail_on
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|needle| input.iter().any(|text| text.contains(needle)))
            {
                bail!("injected provider failure");
            }
            if self
                .cancel_on
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|needle| input.iter().any(|text| text.contains(needle)))
            {
                cancel.store(true, Ordering::Release);
            }
            let dim = *self.dimensions.lock().unwrap();
            let vectors = input
                .iter()
                .map(|text| {
                    let mut vector = vec![0.1; dim];
                    vector[0] = 1.0;
                    if dim > 1 && text == CANARY && self.canary_changed.load(Ordering::Acquire) {
                        vector[1] = 1.0;
                    }
                    vector
                })
                .collect();
            Ok(EmbeddingResult {
                model: model.into(),
                resolved_model: model.into(),
                source: self.source.lock().unwrap().clone(),
                vectors,
            })
        }
    }
    fn setup() -> (tempfile::TempDir, MemoryStore, Fake, AtomicBool) {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(dir.path());
        (dir, store, Fake::default(), AtomicBool::new(false))
    }
    fn remember(
        store: &MemoryStore,
        api: &Fake,
        cancel: &AtomicBool,
        project: &str,
        text: &str,
        key: Option<&str>,
    ) -> String {
        let args = if let Some(key) = key {
            json!({"text":text,"key":key})
        } else {
            json!({"text":text})
        };
        store
            .call("memory_remember", project, &args, api, cancel)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    #[test]
    fn selection_is_pinned_across_restart_and_candidate_changes() {
        let (dir, store, api, cancel) = setup();
        remember(&store, &api, &cancel, "project-a", "known fact", None);
        *api.candidates.lock().unwrap() = vec!["other/new-default".into()];
        api.requests.lock().unwrap().clear();
        let restored = MemoryStore::new(dir.path());
        restored
            .call(
                "memory_recall",
                "project-a",
                &json!({"query":"known"}),
                &api,
                &cancel,
            )
            .unwrap();
        remember(&restored, &api, &cancel, "project-a", "another fact", None);
        assert!(
            api.requests
                .lock()
                .unwrap()
                .iter()
                .all(|(model, _)| model == "local/embed-a")
        );
        assert_eq!(
            restored.status().unwrap()["embeddingModel"],
            "local/embed-a"
        );
    }
    #[test]
    fn parsed_index_is_reused_until_a_committed_generation_changes() {
        let (_, store, api, cancel) = setup();
        remember(&store, &api, &cancel, "project-a", "known fact", None);
        let first = store.read_shared(true).unwrap();
        let second = store.read_shared(true).unwrap();
        assert!(Arc::ptr_eq(&first, &second));

        remember(&store, &api, &cancel, "project-a", "new fact", None);
        let third = store.read_shared(true).unwrap();
        assert!(!Arc::ptr_eq(&first, &third));
        assert_eq!(third.generation, first.generation + 1);
    }
    #[test]
    fn reindex_covers_every_project_and_lifecycle_state_before_atomic_switch() {
        let (_, store, api, cancel) = setup();
        remember(&store, &api, &cancel, "a", "old value", Some("setting"));
        remember(&store, &api, &cancel, "a", "current value", Some("setting"));
        let archived = remember(&store, &api, &cancel, "b", "archived value", None);
        store
            .call("memory_forget", "b", &json!({"id":archived}), &api, &cancel)
            .unwrap();
        *api.dimensions.lock().unwrap() = 5;
        api.requests.lock().unwrap().clear();
        store
            .reindex(Some("custom/pinned-embedding"), &api, &cancel)
            .unwrap();
        let index = store.read().unwrap();
        assert_eq!(index.binding.unwrap().model, "custom/pinned-embedding");
        assert_eq!(index.atoms.len(), 3);
        assert!(
            index
                .atoms
                .iter()
                .all(|atom| atom.vectors.iter().all(|v| v.len() == 5))
        );
        let requests = api.requests.lock().unwrap();
        for text in ["old value", "current value", "archived value"] {
            assert!(
                requests
                    .iter()
                    .any(|(_, inputs)| inputs.iter().any(|input| input == text))
            );
        }
    }
    #[test]
    fn failed_or_cancelled_reindex_leaves_original_bytes_and_model_usable() {
        let (_, store, api, cancel) = setup();
        remember(&store, &api, &cancel, "a", "first document", None);
        remember(&store, &api, &cancel, "b", "second document", None);
        let before = fs::read(store.root.join("index.json")).unwrap();
        *api.fail_on.lock().unwrap() = Some("second".into());
        assert!(
            store
                .reindex(Some("local/new-model"), &api, &cancel)
                .is_err()
        );
        assert_eq!(before, fs::read(store.root.join("index.json")).unwrap());
        *api.fail_on.lock().unwrap() = None;
        *api.cancel_on.lock().unwrap() = Some("second".into());
        assert!(
            store
                .reindex(Some("local/new-model"), &api, &cancel)
                .is_err()
        );
        assert_eq!(before, fs::read(store.root.join("index.json")).unwrap());
        cancel.store(false, Ordering::Release);
        *api.cancel_on.lock().unwrap() = None;
        assert!(
            store
                .call(
                    "memory_recall",
                    "a",
                    &json!({"query":"first"}),
                    &api,
                    &cancel
                )
                .unwrap()["context"]
                .as_str()
                .unwrap()
                .contains("first document")
        );
    }
    #[test]
    fn dimensions_endpoint_and_silent_model_drift_fail_closed() {
        let (_, store, api, cancel) = setup();
        remember(&store, &api, &cancel, "a", "existing", None);
        let before = fs::read(store.root.join("index.json")).unwrap();
        *api.dimensions.lock().unwrap() = 4;
        assert!(
            store
                .call(
                    "memory_remember",
                    "a",
                    &json!({"text":"new"}),
                    &api,
                    &cancel
                )
                .is_err()
        );
        *api.dimensions.lock().unwrap() = 3;
        *api.source.lock().unwrap() = "changed-endpoint".into();
        assert!(
            store
                .call(
                    "memory_recall",
                    "a",
                    &json!({"query":"existing"}),
                    &api,
                    &cancel
                )
                .is_err()
        );
        *api.source.lock().unwrap() = "endpoint-a".into();
        api.canary_changed.store(true, Ordering::Release);
        let error = store
            .call(
                "memory_recall",
                "a",
                &json!({"query":"existing"}),
                &api,
                &cancel,
            )
            .unwrap_err();
        assert!(error.to_string().contains("re-index required"));
        assert_eq!(before, fs::read(store.root.join("index.json")).unwrap());
        store.reindex(None, &api, &cancel).unwrap();
        assert!(
            store
                .call(
                    "memory_recall",
                    "a",
                    &json!({"query":"existing"}),
                    &api,
                    &cancel
                )
                .is_ok()
        );
    }
    #[test]
    fn keyed_history_and_id_mutations_remain_project_scoped() {
        let (_, store, api, cancel) = setup();
        let first = remember(&store, &api, &cancel, "a", "old", Some("setting"));
        let current = remember(&store, &api, &cancel, "a", "new", Some("setting"));
        assert_eq!(store.read().unwrap().atoms[0].status, "superseded");
        assert_eq!(store.read().unwrap().atoms[1].supersedes, vec![first]);
        assert!(
            store
                .call(
                    "memory_update",
                    "b",
                    &json!({"id":current,"text":"cross-project"}),
                    &api,
                    &cancel
                )
                .is_err()
        );
        assert_eq!(
            store
                .call("memory_recall", "b", &json!({"query":"new"}), &api, &cancel)
                .unwrap()["context"],
            ""
        );
        store
            .call("memory_forget", "a", &json!({"id":current}), &api, &cancel)
            .unwrap();
        assert_eq!(
            store
                .call("memory_recall", "a", &json!({"query":"new"}), &api, &cancel)
                .unwrap()["context"],
            ""
        );
        store
            .call("memory_restore", "a", &json!({"id":current}), &api, &cancel)
            .unwrap();
        assert_eq!(store.status().unwrap()["active"], 1);
    }
    #[test]
    fn import_is_additive_idempotent_and_reembeds_legacy_records() {
        let (dir, store, api, cancel) = setup();
        remember(&store, &api, &cancel, "a", "existing native memory", None);
        let export = dir.path().join("export.jsonl");
        let records = [
            json!({"type":"foundation_export","version":1}),
            json!({"type":"atom","atom":{"id":"legacy-id","content":"legacy fact","status":"archived","embedding":[999],"metadata":{"memory":{"key":"build.target"}}}}),
            json!({"type":"relation","relation":{"id":"relation-1","from_atom_id":"legacy-id","to_atom_id":"other"}}),
        ];
        fs::write(
            &export,
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        assert_eq!(
            store
                .import_foundation(&export, "a", &api, &cancel)
                .unwrap()["imported"],
            1
        );
        assert_eq!(
            store
                .import_foundation(&export, "a", &api, &cancel)
                .unwrap()["imported"],
            0
        );
        let index = store.read().unwrap();
        assert_eq!(index.atoms.len(), 2);
        assert_eq!(index.atoms[1].key.as_deref(), Some("build.target"));
        assert_eq!(index.atoms[1].status, "archived");
        assert_eq!(index.atoms[1].vectors[0].len(), 3);
        assert!(
            index.atoms[1].metadata["foundation"]
                .get("embedding")
                .is_none()
        );
        assert_eq!(index.relations.len(), 1);
    }
    #[test]
    fn chunking_indexes_entire_long_memories_and_schema_cannot_select_models() {
        let (_, store, api, cancel) = setup();
        remember(&store, &api, &cancel, "a", &"한".repeat(7000), None);
        assert_eq!(store.read().unwrap().atoms[0].vectors.len(), 4);
        for tool in tool_definitions() {
            assert!(tool.input_schema["properties"].get("model").is_none());
            assert!(tool.input_schema["properties"].get("project").is_none());
        }
    }
    #[test]
    fn malformed_store_is_not_overwritten_and_os_lock_releases_on_drop() {
        let (_, store, api, cancel) = setup();
        let lock = Lock::acquire(&store.root, &cancel).unwrap();
        cancel.store(true, Ordering::Release);
        assert!(Lock::acquire(&store.root, &cancel).is_err());
        drop(lock);
        cancel.store(false, Ordering::Release);
        let _lock = Lock::acquire(&store.root, &cancel).unwrap();
        fs::write(store.root.join("index.json"), "corrupt").unwrap();
        assert!(store.read().is_err());
        assert_eq!(
            fs::read_to_string(store.root.join("index.json")).unwrap(),
            "corrupt"
        );
        drop(_lock);
        assert!(store.reindex(Some("custom/model"), &api, &cancel).is_err());
    }
}
