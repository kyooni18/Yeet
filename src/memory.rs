//! Native Foundation-style project memory. One committed generation owns one
//! embedding space across ALL projects and lifecycle states. No model fallback
//! is permitted after that space has been pinned.
use crate::{
    core::{BridgeClient, ToolDefinition},
    platform::{set_private_directory, set_private_file},
};
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
const INDEX_VERSION: u32 = 2;
const GLOBAL_PROJECT: &str = "__yeet_global__";
const DEFAULT_RECALL_BUDGET_CHARS: usize = 4096;
const MAX_RECALL_BUDGET_CHARS: usize = 24 * 1024;
const MAX_SUMMARY_CHARS: usize = 360;

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
    #[serde(default = "default_project_scope")]
    scope: String,
    #[serde(default = "default_memory_kind")]
    kind: String,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    summary: String,
    #[serde(default = "default_importance")]
    importance: f64,
    #[serde(default = "default_confidence")]
    confidence: f64,
    #[serde(default)]
    observed_at: Option<String>,
    #[serde(default)]
    valid_from: Option<String>,
    #[serde(default)]
    valid_until: Option<String>,
    #[serde(default)]
    source_refs: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Relation {
    id: String,
    project: String,
    from_atom_id: String,
    relation: String,
    to_atom_id: String,
    confidence: f64,
    created_at: String,
    #[serde(default)]
    metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum RelationRecord {
    Typed(Relation),
    Legacy(Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Index {
    version: u32,
    generation: u64,
    binding: Option<Binding>,
    atoms: Vec<Atom>,
    #[serde(default)]
    relations: Vec<RelationRecord>,
}
impl Default for Index {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
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
        let mut index: Index = serde_json::from_slice(&bytes)
            .context("Invalid Yeet memory index; original data was not modified")?;
        upgrade_index(&mut index)?;
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
            json!({"storage":"foundation-v2","indexVersion":index.version,"generation":index.generation,"total":index.atoms.len(),
            "active":index.atoms.iter().filter(|a| a.status=="active").count(),
            "global":index.atoms.iter().filter(|a| a.status=="active" && a.project==GLOBAL_PROJECT).count(),
            "relations":index.relations.len(),
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
        match operation {
            "memory_recall" => return self.recall(project, args, api, cancel),
            "memory_get" => return self.get(project, args),
            "memory_connections" => return self.connections(project, args),
            _ => {}
        }

        let _lock = Lock::acquire(&self.root, cancel)?;
        let mut index = self.read()?;
        let id = args.get("id").and_then(Value::as_str);
        let position = if let Some(id) = id {
            Some(
                index
                    .atoms
                    .iter()
                    .position(|a| a.id == id && atom_visible_to_project(a, project))
                    .ok_or_else(|| {
                        anyhow::anyhow!("Memory does not exist in this project or global scope")
                    })?,
            )
        } else {
            None
        };
        let now = chrono::Utc::now().to_rfc3339();
        let result = match operation {
            "memory_forget" | "memory_restore" => {
                let pos = position.ok_or_else(|| anyhow::anyhow!("id is required"))?;
                let owner = index.atoms[pos].project.clone();
                if operation == "memory_restore" {
                    ensure!(
                        matches!(index.atoms[pos].status.as_str(), "archived" | "deleted"),
                        "Only archived memories can be restored; superseded memories retain their history"
                    );
                    if let Some(key) = &index.atoms[pos].key {
                        ensure!(
                            !index.atoms.iter().any(|a| a.project == owner
                                && a.status == "active"
                                && a.key.as_ref() == Some(key)),
                            "An active memory already owns this key in the same scope"
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
                json!({"id":index.atoms[pos].id,"status":index.atoms[pos].status,"scope":index.atoms[pos].scope})
            }
            "memory_remember" | "memory_update" | "memory_replace" => {
                let content = text_arg(args, "text", 100_000)?.to_owned();
                let mut key = args
                    .get("key")
                    .map(|_| text_arg(args, "key", 200).map(str::to_owned))
                    .transpose()?;

                let (owner, scope, base) = if operation == "memory_remember" {
                    let scope = memory_scope_arg(args)?;
                    let owner = owner_for_scope(project, scope).to_owned();
                    (owner, scope.to_owned(), None)
                } else {
                    let pos = position.ok_or_else(|| anyhow::anyhow!("id is required"))?;
                    ensure!(
                        index.atoms[pos].status == "active",
                        "Only active memories can be updated or replaced"
                    );
                    key = index.atoms[pos].key.clone();
                    (
                        index.atoms[pos].project.clone(),
                        index.atoms[pos].scope.clone(),
                        Some(index.atoms[pos].clone()),
                    )
                };

                if operation == "memory_remember"
                    && let Some(existing) = index.atoms.iter().find(|a| {
                        a.project == owner
                            && a.status == "active"
                            && a.key == key
                            && a.content == content
                    })
                {
                    return Ok(json!({"id":existing.id,"unchanged":true,"scope":existing.scope}));
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
                    apply_atom_attributes(atom, args, true)?;
                    public_atom(atom, true)
                } else {
                    let mut supersedes = Vec::new();
                    for (pos, atom) in index.atoms.iter_mut().enumerate() {
                        if atom.project == owner
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
                    let mut atom = if let Some(mut atom) = base {
                        atom.id = id.clone();
                        atom.project = owner;
                        atom.scope = scope;
                        atom.content = content;
                        atom.vectors = vectors;
                        atom.status = "active".into();
                        atom.created_at = now.clone();
                        atom.updated_at = now.clone();
                        atom.supersedes = supersedes;
                        atom.metadata = atom.metadata.clone();
                        atom
                    } else {
                        Atom {
                            id: id.clone(),
                            project: owner,
                            content,
                            key,
                            status: "active".into(),
                            created_at: now.clone(),
                            updated_at: now.clone(),
                            supersedes,
                            metadata: json!({}),
                            vectors,
                            scope,
                            kind: default_memory_kind(),
                            subject: None,
                            summary: String::new(),
                            importance: default_importance(),
                            confidence: default_confidence(),
                            observed_at: None,
                            valid_from: None,
                            valid_until: None,
                            source_refs: Vec::new(),
                            tags: Vec::new(),
                        }
                    };
                    apply_atom_attributes(&mut atom, args, true)?;
                    let result = public_atom(&atom, true);
                    index.atoms.push(atom);
                    result
                }
            }
            "memory_relate" => {
                let from = text_arg(args, "fromId", 200)?;
                let to = text_arg(args, "toId", 200)?;
                ensure!(from != to, "A memory cannot relate to itself");
                let relation = text_arg(args, "relation", 100)?.trim();
                ensure!(!relation.is_empty(), "relation must not be empty");
                let confidence = bounded_number_arg(args, "confidence", default_confidence())?;
                let from_atom = index
                    .atoms
                    .iter()
                    .find(|a| {
                        a.id == from && a.status == "active" && atom_visible_to_project(a, project)
                    })
                    .ok_or_else(|| anyhow::anyhow!("fromId is not an active visible memory"))?;
                let to_atom = index
                    .atoms
                    .iter()
                    .find(|a| {
                        a.id == to && a.status == "active" && atom_visible_to_project(a, project)
                    })
                    .ok_or_else(|| anyhow::anyhow!("toId is not an active visible memory"))?;
                let owner =
                    if from_atom.project == GLOBAL_PROJECT && to_atom.project == GLOBAL_PROJECT {
                        GLOBAL_PROJECT
                    } else {
                        project
                    };
                if let Some(existing) = index.relations.iter().find_map(|record| match record {
                    RelationRecord::Typed(r)
                        if r.project == owner
                            && r.from_atom_id == from
                            && r.to_atom_id == to
                            && r.relation == relation =>
                    {
                        Some(r)
                    }
                    _ => None,
                }) {
                    return Ok(json!({"id":existing.id,"unchanged":true}));
                }
                let relation_record = Relation {
                    id: Uuid::new_v4().to_string(),
                    project: owner.to_owned(),
                    from_atom_id: from.to_owned(),
                    relation: relation.to_owned(),
                    to_atom_id: to.to_owned(),
                    confidence,
                    created_at: now,
                    metadata: json!({}),
                };
                let result = serde_json::to_value(&relation_record)?;
                index.relations.push(RelationRecord::Typed(relation_record));
                result
            }
            _ => bail!("Unknown native memory operation: {operation}"),
        };
        index.version = INDEX_VERSION;
        index.generation += 1;
        self.commit(&index, cancel)?;
        Ok(result)
    }

    fn get(&self, project: &str, args: &Value) -> Result<Value> {
        let id = text_arg(args, "id", 200)?;
        let index = self.read_shared(true)?;
        let atom = index
            .atoms
            .iter()
            .find(|a| a.id == id && atom_visible_to_project(a, project))
            .ok_or_else(|| {
                anyhow::anyhow!("Memory does not exist in this project or global scope")
            })?;
        Ok(public_atom(atom, true))
    }

    fn connections(&self, project: &str, args: &Value) -> Result<Value> {
        let id = text_arg(args, "id", 200)?;
        let index = self.read_shared(true)?;
        ensure!(
            index
                .atoms
                .iter()
                .any(|a| a.id == id && atom_visible_to_project(a, project)),
            "Memory does not exist in this project or global scope"
        );
        let connections = index
            .relations
            .iter()
            .filter_map(|record| typed_relation(record))
            .filter(|relation| relation_visible_to_project(relation, project))
            .filter(|relation| relation.from_atom_id == id || relation.to_atom_id == id)
            .map(|relation| {
                let other_id = if relation.from_atom_id == id {
                    &relation.to_atom_id
                } else {
                    &relation.from_atom_id
                };
                let other = index
                    .atoms
                    .iter()
                    .find(|a| &a.id == other_id && atom_visible_to_project(a, project));
                json!({
                    "id": relation.id,
                    "relation": relation.relation,
                    "direction": if relation.from_atom_id == id { "out" } else { "in" },
                    "otherId": other_id,
                    "confidence": relation.confidence,
                    "other": other.map(|atom| public_atom(atom, false)),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({"id":id,"connections":connections}))
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
        let requested_scope = args.get("scope").and_then(Value::as_str).unwrap_or("all");
        ensure!(
            matches!(requested_scope, "all" | "project" | "global"),
            "scope must be all, project, or global"
        );
        let detail = args
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("compact");
        ensure!(
            matches!(detail, "compact" | "full"),
            "detail must be compact or full"
        );
        let budget_chars = args
            .get("budgetChars")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_RECALL_BUDGET_CHARS as u64)
            .clamp(512, MAX_RECALL_BUDGET_CHARS as u64) as usize;
        let now = chrono::Utc::now();
        let atoms = index
            .atoms
            .iter()
            .filter(|a| a.status == "active")
            .filter(|a| atom_visible_to_project(a, project))
            .filter(|a| match requested_scope {
                "project" => a.project == project,
                "global" => a.project == GLOBAL_PROJECT,
                _ => true,
            })
            .filter(|a| atom_currently_valid(a, &now))
            .collect::<Vec<_>>();
        if atoms.is_empty() {
            return Ok(
                json!({"context":"","memories":[],"generation":index.generation,"detail":detail}),
            );
        }
        let query_vectors = embed_text(index.binding.as_ref().unwrap(), query, api, cancel)?;
        let query_lower = query.to_lowercase();
        let words = search_words(query);
        let mut ranked = atoms
            .into_iter()
            .map(|atom| {
                let semantic = query_vectors
                    .iter()
                    .flat_map(|q| atom.vectors.iter().map(move |v| dot(q, v)))
                    .fold(-1.0, f64::max);
                let searchable = format!(
                    "{} {} {} {}",
                    atom.summary,
                    atom.content,
                    atom.subject.as_deref().unwrap_or(""),
                    atom.tags.join(" ")
                )
                .to_lowercase();
                let lexical = lexical_overlap(&words, &searchable);
                let subject = atom
                    .subject
                    .as_deref()
                    .map(|subject| lexical_overlap(&words, &subject.to_lowercase()))
                    .unwrap_or(0.0);
                let key = atom
                    .key
                    .as_deref()
                    .map(|key| {
                        let key = key.to_lowercase();
                        if query_lower.contains(&key) || key.contains(&query_lower) {
                            1.0
                        } else {
                            lexical_overlap(&words, &key)
                        }
                    })
                    .unwrap_or(0.0);
                let recency = recency_score(&atom.updated_at, &now);
                let score = semantic * 0.67
                    + lexical * 0.13
                    + subject * 0.07
                    + key * 0.06
                    + atom.importance * 0.025
                    + atom.confidence * 0.025
                    + recency * 0.04;
                (score, 0.0_f64, atom)
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|(sa, _, a), (sb, _, b)| sb.total_cmp(sa).then_with(|| a.id.cmp(&b.id)));
        let anchors = ranked
            .iter()
            .take(4)
            .filter(|(score, _, _)| *score >= 0.22)
            .map(|(_, _, atom)| atom.id.as_str())
            .collect::<Vec<_>>();
        for (_, graph_boost, atom) in &mut ranked {
            if anchors.iter().any(|id| *id == atom.id) {
                continue;
            }
            *graph_boost = anchors
                .iter()
                .map(|anchor| relation_strength(&index, project, &atom.id, anchor) * 0.18)
                .fold(0.0, f64::max);
        }
        ranked.sort_by(|(sa, ga, a), (sb, gb, b)| {
            (sb + gb)
                .total_cmp(&(sa + ga))
                .then_with(|| a.id.cmp(&b.id))
        });
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(6)
            .clamp(1, 20) as usize;
        let mut context = String::new();
        let mut memories = Vec::new();
        for (base_score, graph_boost, atom) in ranked
            .into_iter()
            .filter(|(score, graph, _)| score + graph >= 0.22)
            .take(limit)
        {
            let remaining = budget_chars.saturating_sub(context.len());
            if remaining < 80 {
                break;
            }
            let line = render_recall_line(atom, detail);
            let bounded = bounded_bytes(&line, remaining);
            context.push_str(bounded);
            memories.push(json!({
                "id":atom.id,
                "score":base_score + graph_boost,
                "semanticScore":base_score,
                "graphBoost":graph_boost,
                "key":atom.key,
                "kind":atom.kind,
                "scope":atom.scope,
                "subject":atom.subject,
                "summary":normalized_summary(atom),
                "confidence":atom.confidence,
                "importance":atom.importance,
            }));
        }
        Ok(json!({
            "context":context,
            "memories":memories,
            "generation":index.generation,
            "detail":detail,
            "budgetChars":budget_chars,
        }))
    }

    /// Portable Foundation JSONL import is additive and idempotent. Version 1
    /// and version 2 exports are accepted; vectors are always rebuilt in Yeet's
    /// pinned embedding space and imported records remain project-scoped.
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
        let version = records
            .first()
            .filter(|r| r["type"] == "foundation_export")
            .and_then(|r| r["version"].as_u64())
            .ok_or_else(|| anyhow::anyhow!("Expected a Foundation portable JSONL export"))?;
        ensure!(
            matches!(version, 1 | 2),
            "Unsupported Foundation export version {version}"
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
                object.remove("vectors");
                object.remove("project");
            }
            let mut atom = Atom {
                id: Uuid::new_v4().to_string(),
                project: project.into(),
                content,
                key: raw
                    .get("key")
                    .and_then(Value::as_str)
                    .or_else(|| raw.pointer("/metadata/memory/key").and_then(Value::as_str))
                    .map(str::to_owned),
                status: raw
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("active")
                    .into(),
                created_at: field_str(raw, "createdAt", "created_at")
                    .unwrap_or(&now)
                    .to_owned(),
                updated_at: now,
                supersedes: string_array_field(raw, "supersedes"),
                metadata: json!({"foundation":portable}),
                vectors,
                scope: "project".into(),
                kind: raw
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("fact")
                    .to_owned(),
                subject: raw
                    .get("subject")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                summary: raw
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                importance: raw
                    .get("importance")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(default_importance),
                confidence: raw
                    .get("confidence")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(default_confidence),
                observed_at: field_str(raw, "observedAt", "observed_at").map(str::to_owned),
                valid_from: field_str(raw, "validFrom", "valid_from").map(str::to_owned),
                valid_until: field_str(raw, "validUntil", "valid_until").map(str::to_owned),
                source_refs: string_array_field_alias(raw, "sourceRefs", "source_refs"),
                tags: string_array_field(raw, "tags"),
            };
            normalize_atom(&mut atom)?;
            index.atoms.push(atom);
            imported += 1;
        }
        let mut imported_relations = 0;
        for record in records.iter().filter(|r| r["type"] == "relation") {
            let raw = &record["relation"];
            let from_original = field_str(raw, "fromAtomId", "from_atom_id");
            let to_original = field_str(raw, "toAtomId", "to_atom_id");
            let predicate = raw
                .get("relation")
                .and_then(Value::as_str)
                .or_else(|| raw.get("predicate").and_then(Value::as_str))
                .or_else(|| raw.get("type").and_then(Value::as_str));
            let local_id = |original: &str| {
                index
                    .atoms
                    .iter()
                    .find(|atom| {
                        atom.project == project && atom.metadata["foundation"]["id"] == original
                    })
                    .map(|atom| atom.id.clone())
            };
            let relation = match (from_original, to_original, predicate) {
                (Some(from), Some(to), Some(predicate)) => match (local_id(from), local_id(to)) {
                    (Some(from_atom_id), Some(to_atom_id)) => RelationRecord::Typed(Relation {
                        id: raw
                            .get("id")
                            .and_then(Value::as_str)
                            .map(|id| format!("foundation:{id}"))
                            .unwrap_or_else(|| Uuid::new_v4().to_string()),
                        project: project.to_owned(),
                        from_atom_id,
                        relation: predicate.to_owned(),
                        to_atom_id,
                        confidence: raw
                            .get("confidence")
                            .and_then(Value::as_f64)
                            .unwrap_or_else(default_confidence)
                            .clamp(0.0, 1.0),
                        created_at: chrono::Utc::now().to_rfc3339(),
                        metadata: json!({"foundation":raw}),
                    }),
                    _ => RelationRecord::Legacy(json!({"project":project,"foundation":raw})),
                },
                _ => RelationRecord::Legacy(json!({"project":project,"foundation":raw})),
            };
            let duplicate = match &relation {
                RelationRecord::Typed(candidate) => index.relations.iter().any(|existing| {
                    matches!(existing, RelationRecord::Typed(existing)
                        if existing.project == candidate.project
                            && existing.from_atom_id == candidate.from_atom_id
                            && existing.to_atom_id == candidate.to_atom_id
                            && existing.relation == candidate.relation)
                }),
                RelationRecord::Legacy(candidate) => index.relations.iter().any(|existing| {
                    matches!(existing, RelationRecord::Legacy(existing) if existing == candidate)
                }),
            };
            if !duplicate {
                index.relations.push(relation);
                imported_relations += 1;
            }
        }
        index.version = INDEX_VERSION;
        index.generation += 1;
        self.commit(&index, cancel)?;
        Ok(json!({
            "imported":imported,
            "relations":imported_relations,
            "sourceVersion":version,
            "project":project,
            "generation":index.generation
        }))
    }

    /// Export the current project's Foundation v2 records without embeddings.
    /// Global memories are deliberately excluded from a project export.
    pub fn export_foundation(&self, path: &Path, project: &str) -> Result<Value> {
        ensure!(!project.trim().is_empty(), "Project identity required");
        let index = self.read_shared(true)?;
        let atoms = index
            .atoms
            .iter()
            .filter(|atom| atom.project == project)
            .collect::<Vec<_>>();
        let atom_ids = atoms
            .iter()
            .map(|atom| atom.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let mut records = vec![json!({
            "type":"foundation_export",
            "version":2,
            "generator":"yeet",
            "generation":index.generation
        })];
        for atom in atoms {
            let mut value = serde_json::to_value(atom)?;
            if let Some(object) = value.as_object_mut() {
                object.remove("project");
                object.remove("vectors");
            }
            records.push(json!({"type":"atom","atom":value}));
        }
        for record in &index.relations {
            match record {
                RelationRecord::Typed(relation)
                    if relation.project == project
                        && atom_ids.contains(relation.from_atom_id.as_str())
                        && atom_ids.contains(relation.to_atom_id.as_str()) =>
                {
                    let mut value = serde_json::to_value(relation)?;
                    if let Some(object) = value.as_object_mut() {
                        object.remove("project");
                    }
                    records.push(json!({"type":"relation","relation":value}));
                }
                RelationRecord::Legacy(value)
                    if value.get("project").and_then(Value::as_str) == Some(project) =>
                {
                    records.push(json!({
                        "type":"relation",
                        "relation":value.get("foundation").unwrap_or(value)
                    }));
                }
                _ => {}
            }
        }
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::File::create(path)?;
        for record in &records {
            writeln!(file, "{}", serde_json::to_string(record)?)?;
        }
        file.sync_all()?;
        Ok(json!({
            "exported":records.iter().filter(|record| record["type"] == "atom").count(),
            "relations":records.iter().filter(|record| record["type"] == "relation").count(),
            "version":2,
            "path":path
        }))
    }
}

fn default_project_scope() -> String {
    "project".into()
}
fn default_memory_kind() -> String {
    "fact".into()
}
fn default_importance() -> f64 {
    0.5
}
fn default_confidence() -> f64 {
    0.8
}

fn upgrade_index(index: &mut Index) -> Result<()> {
    ensure!(
        matches!(index.version, 1 | INDEX_VERSION),
        "Unsupported memory index version {}",
        index.version
    );
    for atom in &mut index.atoms {
        normalize_atom(atom)?;
    }
    index.version = INDEX_VERSION;
    Ok(())
}

fn normalize_atom(atom: &mut Atom) -> Result<()> {
    if atom.project == GLOBAL_PROJECT {
        atom.scope = "global".into();
    }
    if atom.scope.trim().is_empty() {
        atom.scope = "project".into();
    }
    ensure!(
        matches!(atom.scope.as_str(), "project" | "global"),
        "Memory {} has invalid scope {}",
        atom.id,
        atom.scope
    );
    if atom.scope == "global" {
        atom.project = GLOBAL_PROJECT.into();
    }
    if atom.kind.trim().is_empty() {
        atom.kind = default_memory_kind();
    }
    ensure!(
        atom.kind.chars().count() <= 64,
        "Memory {} has an overlong kind",
        atom.id
    );
    if atom.summary.trim().is_empty() {
        atom.summary = summarize(&atom.content);
    } else if atom.summary.chars().count() > MAX_SUMMARY_CHARS {
        atom.summary = summarize(&atom.summary);
    }
    ensure!(
        atom.importance.is_finite() && (0.0..=1.0).contains(&atom.importance),
        "Memory {} has invalid importance",
        atom.id
    );
    ensure!(
        atom.confidence.is_finite() && (0.0..=1.0).contains(&atom.confidence),
        "Memory {} has invalid confidence",
        atom.id
    );
    for value in [&atom.observed_at, &atom.valid_from, &atom.valid_until]
        .into_iter()
        .flatten()
    {
        validate_timestamp(value)?;
    }
    if let (Some(from), Some(until)) = (&atom.valid_from, &atom.valid_until) {
        ensure!(
            chrono::DateTime::parse_from_rfc3339(from)?
                <= chrono::DateTime::parse_from_rfc3339(until)?,
            "Memory {} has an inverted validity range",
            atom.id
        );
    }
    ensure!(
        atom.source_refs.len() <= 64 && atom.tags.len() <= 64,
        "Memory {} has too many sourceRefs or tags",
        atom.id
    );
    ensure!(
        atom.source_refs.iter().all(|v| v.chars().count() <= 1000)
            && atom.tags.iter().all(|v| v.chars().count() <= 200),
        "Memory {} has an overlong sourceRef or tag",
        atom.id
    );
    Ok(())
}

fn apply_atom_attributes(atom: &mut Atom, args: &Value, content_changed: bool) -> Result<()> {
    if let Some(v) = optional_text_arg(args, "kind", 64)? {
        atom.kind = v.to_owned();
    }
    if let Some(v) = optional_text_arg(args, "subject", 500)? {
        atom.subject = Some(v.to_owned());
    }
    if let Some(v) = optional_text_arg(args, "summary", MAX_SUMMARY_CHARS)? {
        atom.summary = v.to_owned();
    } else if content_changed || atom.summary.trim().is_empty() {
        atom.summary = summarize(&atom.content);
    }
    if args.get("importance").is_some() {
        atom.importance = bounded_number_arg(args, "importance", atom.importance)?;
    }
    if args.get("confidence").is_some() {
        atom.confidence = bounded_number_arg(args, "confidence", atom.confidence)?;
    }
    if let Some(v) = optional_text_arg(args, "observedAt", 100)? {
        validate_timestamp(v)?;
        atom.observed_at = Some(v.to_owned());
    }
    if let Some(v) = optional_text_arg(args, "validFrom", 100)? {
        validate_timestamp(v)?;
        atom.valid_from = Some(v.to_owned());
    }
    if let Some(v) = optional_text_arg(args, "validUntil", 100)? {
        validate_timestamp(v)?;
        atom.valid_until = Some(v.to_owned());
    }
    if args.get("sourceRefs").is_some() {
        atom.source_refs = string_array_arg(args, "sourceRefs", 64, 1000)?;
    }
    if args.get("tags").is_some() {
        atom.tags = string_array_arg(args, "tags", 64, 200)?;
    }
    normalize_atom(atom)
}

fn memory_scope_arg(args: &Value) -> Result<&str> {
    let scope = args
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("project");
    ensure!(
        matches!(scope, "project" | "global"),
        "scope must be project or global"
    );
    Ok(scope)
}
fn owner_for_scope<'a>(project: &'a str, scope: &str) -> &'a str {
    if scope == "global" {
        GLOBAL_PROJECT
    } else {
        project
    }
}
fn atom_visible_to_project(atom: &Atom, project: &str) -> bool {
    atom.project == project || atom.project == GLOBAL_PROJECT
}
fn relation_visible_to_project(relation: &Relation, project: &str) -> bool {
    relation.project == project || relation.project == GLOBAL_PROJECT
}
fn typed_relation(record: &RelationRecord) -> Option<&Relation> {
    match record {
        RelationRecord::Typed(r) => Some(r),
        RelationRecord::Legacy(_) => None,
    }
}

fn relation_strength(index: &Index, project: &str, a: &str, b: &str) -> f64 {
    index
        .relations
        .iter()
        .filter_map(typed_relation)
        .filter(|r| relation_visible_to_project(r, project))
        .filter(|r| {
            (r.from_atom_id == a && r.to_atom_id == b) || (r.from_atom_id == b && r.to_atom_id == a)
        })
        .map(|r| r.confidence)
        .fold(0.0, f64::max)
}

fn atom_currently_valid(atom: &Atom, now: &chrono::DateTime<chrono::Utc>) -> bool {
    let after_start = atom
        .valid_from
        .as_deref()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .is_none_or(|v| v.with_timezone(&chrono::Utc) <= *now);
    let before_end = atom
        .valid_until
        .as_deref()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .is_none_or(|v| v.with_timezone(&chrono::Utc) >= *now);
    after_start && before_end
}

fn recency_score(updated_at: &str, now: &chrono::DateTime<chrono::Utc>) -> f64 {
    let Ok(updated) = chrono::DateTime::parse_from_rfc3339(updated_at) else {
        return 0.0;
    };
    let seconds = (*now - updated.with_timezone(&chrono::Utc))
        .num_seconds()
        .max(0) as f64;
    1.0 / (1.0 + (seconds / 86_400.0) / 30.0)
}

fn search_words(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut words = lower
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|w| w.chars().count() >= 2)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    words.sort();
    words.dedup();
    words
}

fn lexical_overlap(words: &[String], text: &str) -> f64 {
    if words.is_empty() {
        return 0.0;
    }
    words.iter().filter(|w| text.contains(w.as_str())).count() as f64 / words.len() as f64
}

fn summarize(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = collapsed.chars().count();
    let mut output = collapsed
        .chars()
        .take(MAX_SUMMARY_CHARS)
        .collect::<String>();
    if count > MAX_SUMMARY_CHARS {
        output.push('…');
    }
    output
}

fn normalized_summary(atom: &Atom) -> String {
    if atom.summary.trim().is_empty() {
        summarize(&atom.content)
    } else {
        atom.summary.clone()
    }
}

fn render_recall_line(atom: &Atom, detail: &str) -> String {
    let mut descriptors = vec![
        format!("kind={}", atom.kind),
        format!("scope={}", atom.scope),
    ];
    if let Some(key) = atom.key.as_deref() {
        descriptors.push(format!("key={key}"));
    }
    if let Some(subject) = atom.subject.as_deref() {
        descriptors.push(format!("subject={subject}"));
    }
    descriptors.push(format!("confidence={:.2}", atom.confidence));
    let body = if detail == "full" {
        atom.content.clone()
    } else {
        normalized_summary(atom)
    };
    format!("[{}] {} :: {}\n", atom.id, descriptors.join(" "), body)
}

fn public_atom(atom: &Atom, include_content: bool) -> Value {
    let mut value = json!({
        "id":atom.id,"key":atom.key,"status":atom.status,"scope":atom.scope,"kind":atom.kind,
        "subject":atom.subject,"summary":normalized_summary(atom),"importance":atom.importance,
        "confidence":atom.confidence,"createdAt":atom.created_at,"updatedAt":atom.updated_at,
        "observedAt":atom.observed_at,"validFrom":atom.valid_from,"validUntil":atom.valid_until,
        "sourceRefs":atom.source_refs,"tags":atom.tags,"supersedes":atom.supersedes,"metadata":atom.metadata
    });
    if include_content {
        value["content"] = json!(atom.content);
    }
    value
}

fn optional_text_arg<'a>(args: &'a Value, name: &str, max_chars: usize) -> Result<Option<&'a str>> {
    let Some(value) = args.get(name) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("{name} must be a string"))?;
    ensure!(
        !text.trim().is_empty() && text.chars().count() <= max_chars,
        "{name} must contain 1..{max_chars} characters"
    );
    Ok(Some(text))
}

fn bounded_number_arg(args: &Value, name: &str, default: f64) -> Result<f64> {
    let value = args.get(name).and_then(Value::as_f64).unwrap_or(default);
    ensure!(
        value.is_finite() && (0.0..=1.0).contains(&value),
        "{name} must be a finite number from 0 to 1"
    );
    Ok(value)
}

fn string_array_arg(
    args: &Value,
    name: &str,
    max_items: usize,
    max_chars: usize,
) -> Result<Vec<String>> {
    let values = args
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("{name} must be an array"))?;
    ensure!(values.len() <= max_items, "{name} has too many values");
    values
        .iter()
        .map(|value| {
            let value = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{name} values must be strings"))?;
            ensure!(
                !value.trim().is_empty() && value.chars().count() <= max_chars,
                "{name} contains an invalid value"
            );
            Ok(value.to_owned())
        })
        .collect()
}

fn validate_timestamp(value: &str) -> Result<()> {
    chrono::DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("Invalid RFC3339 timestamp: {value}"))?;
    Ok(())
}

fn field_str<'a>(value: &'a Value, camel: &str, snake: &str) -> Option<&'a str> {
    value
        .get(camel)
        .and_then(Value::as_str)
        .or_else(|| value.get(snake).and_then(Value::as_str))
}

fn string_array_field(value: &Value, name: &str) -> Vec<String> {
    value
        .get(name)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn string_array_field_alias(value: &Value, camel: &str, snake: &str) -> Vec<String> {
    value
        .get(camel)
        .or_else(|| value.get(snake))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
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
    ensure!(
        index.version == INDEX_VERSION,
        "Unsupported memory index version"
    );
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
    for record in &index.relations {
        let RelationRecord::Typed(relation) = record else {
            continue;
        };
        ensure!(
            !relation.id.trim().is_empty()
                && !relation.relation.trim().is_empty()
                && relation.confidence.is_finite()
                && (0.0..=1.0).contains(&relation.confidence),
            "Invalid typed memory relation"
        );
        let from = index
            .atoms
            .iter()
            .find(|atom| atom.id == relation.from_atom_id)
            .ok_or_else(|| {
                anyhow::anyhow!("Memory relation {} has a missing source", relation.id)
            })?;
        let to = index
            .atoms
            .iter()
            .find(|atom| atom.id == relation.to_atom_id)
            .ok_or_else(|| {
                anyhow::anyhow!("Memory relation {} has a missing target", relation.id)
            })?;
        let endpoint_visible = |atom: &Atom| {
            if relation.project == GLOBAL_PROJECT {
                atom.project == GLOBAL_PROJECT
            } else {
                atom.project == relation.project || atom.project == GLOBAL_PROJECT
            }
        };
        ensure!(
            endpoint_visible(from) && endpoint_visible(to),
            "Memory relation {} crosses an invalid project boundary",
            relation.id
        );
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
        set_private_directory(root)?;
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(root.join(".lock"))?;

        set_private_file(&root.join(".lock"))?;
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
            "Recall relevant active project and global memories with hybrid semantic, lexical, temporal, and relation-aware ranking. Compact summaries are returned by default; hydrate an id with project_memory_get when exact content is needed.",
            json!({
                "query":{"type":"string"},
                "limit":{"type":"integer","minimum":1,"maximum":20},
                "scope":{"type":"string","enum":["all","project","global"]},
                "detail":{"type":"string","enum":["compact","full"]},
                "budgetChars":{"type":"integer","minimum":512,"maximum":24576}
            }),
            vec!["query"],
        ),
        (
            "memory_get",
            "Hydrate one visible memory by id, including full content, provenance, validity, lifecycle, and structured metadata.",
            json!({"id":{"type":"string"}}),
            vec!["id"],
        ),
        (
            "memory_remember",
            "Save structured durable knowledge. Default scope is project; use global only for genuinely cross-project user knowledge. Reuse a stable key for mutable facts so previous values remain as superseded history.",
            json!({
                "text":{"type":"string"},
                "key":{"type":"string"},
                "scope":{"type":"string","enum":["project","global"]},
                "kind":{"type":"string","enum":["fact","decision","preference","procedure","constraint","failure","discovery","goal","observation"]},
                "subject":{"type":"string"},
                "summary":{"type":"string"},
                "importance":{"type":"number","minimum":0.0,"maximum":1.0},
                "confidence":{"type":"number","minimum":0.0,"maximum":1.0},
                "observedAt":{"type":"string","description":"RFC3339 timestamp"},
                "validFrom":{"type":"string","description":"RFC3339 timestamp"},
                "validUntil":{"type":"string","description":"RFC3339 timestamp"},
                "sourceRefs":{"type":"array","maxItems":64,"items":{"type":"string"}},
                "tags":{"type":"array","maxItems":64,"items":{"type":"string"}}
            }),
            vec!["text"],
        ),
        (
            "memory_update",
            "Correct an active memory in place while preserving its identity and scope.",
            json!({
                "id":{"type":"string"},"text":{"type":"string"},
                "kind":{"type":"string"},"subject":{"type":"string"},"summary":{"type":"string"},
                "importance":{"type":"number","minimum":0.0,"maximum":1.0},
                "confidence":{"type":"number","minimum":0.0,"maximum":1.0},
                "observedAt":{"type":"string"},"validFrom":{"type":"string"},"validUntil":{"type":"string"},
                "sourceRefs":{"type":"array","maxItems":64,"items":{"type":"string"}},
                "tags":{"type":"array","maxItems":64,"items":{"type":"string"}}
            }),
            vec!["id", "text"],
        ),
        (
            "memory_replace",
            "Replace an active memory while preserving its prior record as superseded history and retaining scope/metadata unless explicitly changed.",
            json!({
                "id":{"type":"string"},"text":{"type":"string"},
                "kind":{"type":"string"},"subject":{"type":"string"},"summary":{"type":"string"},
                "importance":{"type":"number","minimum":0.0,"maximum":1.0},
                "confidence":{"type":"number","minimum":0.0,"maximum":1.0},
                "observedAt":{"type":"string"},"validFrom":{"type":"string"},"validUntil":{"type":"string"},
                "sourceRefs":{"type":"array","maxItems":64,"items":{"type":"string"}},
                "tags":{"type":"array","maxItems":64,"items":{"type":"string"}}
            }),
            vec!["id", "text"],
        ),
        (
            "memory_relate",
            "Create a typed directed relation between two active visible memories. Relations participate in recall reranking.",
            json!({
                "fromId":{"type":"string"},
                "relation":{"type":"string"},
                "toId":{"type":"string"},
                "confidence":{"type":"number","minimum":0.0,"maximum":1.0}
            }),
            vec!["fromId", "relation", "toId"],
        ),
        (
            "memory_connections",
            "List typed incoming and outgoing relations for one visible memory.",
            json!({"id":{"type":"string"}}),
            vec!["id"],
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
    fn global_scope_is_cross_project_but_project_scope_stays_isolated() {
        let (_, store, api, cancel) = setup();
        let global = store
            .call(
                "memory_remember",
                "a",
                &json!({
                    "text":"Prefer compact memory recall across every Yeet project.",
                    "scope":"global",
                    "kind":"preference",
                    "subject":"memory recall",
                    "summary":"Prefer compact memory recall globally."
                }),
                &api,
                &cancel,
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let local = remember(&store, &api, &cancel, "a", "project-a-only memory", None);

        let recalled = store
            .call(
                "memory_recall",
                "b",
                &json!({"query":"compact memory recall"}),
                &api,
                &cancel,
            )
            .unwrap();
        assert!(recalled["context"].as_str().unwrap().contains("globally"));
        assert_eq!(
            store
                .call("memory_get", "b", &json!({"id":global}), &api, &cancel)
                .unwrap()["scope"],
            "global"
        );
        assert!(
            store
                .call("memory_get", "b", &json!({"id":local}), &api, &cancel)
                .is_err()
        );
        assert_eq!(store.status().unwrap()["global"], 1);
    }

    #[test]
    fn compact_recall_hydrates_full_content_only_on_demand() {
        let (_, store, api, cancel) = setup();
        let id = store
            .call(
                "memory_remember",
                "a",
                &json!({
                    "text":"The durable architecture decision contains exact implementation evidence. FULL_ONLY_MARKER",
                    "kind":"decision",
                    "summary":"Durable architecture decision summary."
                }),
                &api,
                &cancel,
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let compact = store
            .call(
                "memory_recall",
                "a",
                &json!({"query":"architecture decision"}),
                &api,
                &cancel,
            )
            .unwrap();
        let context = compact["context"].as_str().unwrap();
        assert!(context.contains("Durable architecture decision summary."));
        assert!(!context.contains("FULL_ONLY_MARKER"));

        let hydrated = store
            .call("memory_get", "a", &json!({"id":id}), &api, &cancel)
            .unwrap();
        assert!(
            hydrated["content"]
                .as_str()
                .unwrap()
                .contains("FULL_ONLY_MARKER")
        );
        let full = store
            .call(
                "memory_recall",
                "a",
                &json!({"query":"architecture decision","detail":"full"}),
                &api,
                &cancel,
            )
            .unwrap();
        assert!(
            full["context"]
                .as_str()
                .unwrap()
                .contains("FULL_ONLY_MARKER")
        );
    }

    #[test]
    fn expired_memories_are_not_recalled() {
        let (_, store, api, cancel) = setup();
        store
            .call(
                "memory_remember",
                "a",
                &json!({
                    "text":"This deployment fact is expired.",
                    "kind":"fact",
                    "validUntil":"2000-01-01T00:00:00Z"
                }),
                &api,
                &cancel,
            )
            .unwrap();
        assert_eq!(
            store
                .call(
                    "memory_recall",
                    "a",
                    &json!({"query":"deployment fact"}),
                    &api,
                    &cancel,
                )
                .unwrap()["context"],
            ""
        );
    }

    #[test]
    fn typed_relations_are_queryable_and_foundation_v2_round_trips() {
        let (dir, store, api, cancel) = setup();
        let first = store
            .call(
                "memory_remember",
                "a",
                &json!({
                    "text":"Remote transport uses WebSockets.",
                    "kind":"decision",
                    "subject":"remote transport",
                    "summary":"Remote uses WebSockets.",
                    "confidence":0.95,
                    "sourceRefs":["repo:src/remote/websocket.rs"],
                    "tags":["remote","transport"]
                }),
                &api,
                &cancel,
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let second = remember(&store, &api, &cancel, "a", "HTML polling transport", None);
        store
            .call(
                "memory_relate",
                "a",
                &json!({
                    "fromId":first,
                    "relation":"supersedes",
                    "toId":second,
                    "confidence":0.9
                }),
                &api,
                &cancel,
            )
            .unwrap();
        let connections = store
            .call(
                "memory_connections",
                "a",
                &json!({"id":first}),
                &api,
                &cancel,
            )
            .unwrap();
        assert_eq!(connections["connections"][0]["relation"], "supersedes");
        assert_eq!(connections["connections"][0]["otherId"], second);

        let export = dir.path().join("foundation-v2.jsonl");
        let exported = store.export_foundation(&export, "a").unwrap();
        assert_eq!(exported["version"], 2);
        assert_eq!(exported["exported"], 2);
        assert_eq!(exported["relations"], 1);
        assert!(!fs::read_to_string(&export).unwrap().contains("\"vectors\""));

        let imported_root = tempfile::tempdir().unwrap();
        let imported = MemoryStore::new(imported_root.path());
        let result = imported
            .import_foundation(&export, "b", &api, &cancel)
            .unwrap();
        assert_eq!(result["sourceVersion"], 2);
        assert_eq!(result["imported"], 2);
        assert_eq!(result["relations"], 1);
        let index = imported.read().unwrap();
        assert_eq!(index.version, INDEX_VERSION);
        assert_eq!(index.atoms[0].kind, "decision");
        assert_eq!(index.atoms[0].subject.as_deref(), Some("remote transport"));
        assert_eq!(
            index.atoms[0].source_refs,
            vec!["repo:src/remote/websocket.rs"]
        );
        assert!(matches!(index.relations[0], RelationRecord::Typed(_)));
    }

    #[test]
    fn version_one_index_is_upgraded_in_memory_and_committed_on_next_write() {
        let (dir, store, api, cancel) = setup();
        remember(&store, &api, &cancel, "a", "legacy-compatible memory", None);
        let path = store.root.join("index.json");
        let mut raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        raw["version"] = json!(1);
        let atom = raw["atoms"][0].as_object_mut().unwrap();
        for field in [
            "scope",
            "kind",
            "subject",
            "summary",
            "importance",
            "confidence",
            "observedAt",
            "validFrom",
            "validUntil",
            "sourceRefs",
            "tags",
        ] {
            atom.remove(field);
        }
        fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();

        let restored = MemoryStore::new(dir.path());
        assert_eq!(restored.status().unwrap()["indexVersion"], INDEX_VERSION);
        let still_v1: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(still_v1["version"], 1);

        remember(&restored, &api, &cancel, "a", "next write commits v2", None);
        let committed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(committed["version"], INDEX_VERSION);
        assert_eq!(committed["atoms"][0]["scope"], "project");
        assert_eq!(committed["atoms"][0]["kind"], "fact");
        assert!(
            committed["atoms"][0]["summary"]
                .as_str()
                .unwrap()
                .contains("legacy-compatible")
        );
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
