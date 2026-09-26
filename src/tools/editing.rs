//! Structured editing and shared workspace-read cache maintenance.
//!
//! Edits require fresh snapshots in sandboxed mode and invalidate only the
//! cache entries that may have become stale. Large results are externalized
//! through the registry's artifact store.

use super::*;
use crate::edit::ReadResult;

impl ToolRegistry {
    /// Drops Rust-side evidence when the edit daemon has been replaced.
    /// Snapshot handles are daemon-local and must never survive a restart.
    pub(super) fn sync_edit_state(&mut self) {
        let Some(edit) = self.edit.as_ref() else {
            return;
        };
        let generation = edit.generation();
        if generation == self.edit_generation {
            return;
        }
        self.edit_generation = generation;
        self.invalidate_workspace_cache();
    }

    /// Applies validated structured file changes and records mutation diagnostics.
    pub(super) fn apply_file_edits(&mut self, arguments: &Value) -> Result<String> {
        let _mutation_guard = self.workspace_mutation_guard()?;
        self.sync_edit_state();
        let mut request = arguments.clone();
        normalize_legacy_edit_shapes(&mut request);
        let unlimited =
            SandboxStore::new(&self.workspace_root)?.load()?.mode == SandboxMode::Unlimited;
        let paths = {
            let changes = request
                .get_mut("changes")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| anyhow!("apply_file_edits requires changes"))?;
            if changes.is_empty() {
                bail!("apply_file_edits requires at least one change");
            }
            let mut paths = Vec::new();
            for change in changes.iter_mut() {
                let object = change
                    .as_object_mut()
                    .ok_or_else(|| anyhow!("changes must be objects"))?;
                let requested_path = object
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("change requires path"))?
                    .to_owned();
                let path = self
                    .resolve_session_path(&requested_path)?
                    .to_string_lossy()
                    .into_owned();
                let cache_path =
                    super::paths::stable_workspace_path_key(&self.workspace_root, &path)?;
                object.insert("path".into(), json!(path.clone()));
                self.ensure_not_protected_write_path(&path)?;
                self.ensure_file_scope(&path, true)?;
                let requested_destination = object
                    .get("fileOp")
                    .and_then(Value::as_object)
                    .and_then(|op| op.get("destination"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(requested_destination) = requested_destination {
                    let destination = self
                        .resolve_session_path(&requested_destination)?
                        .to_string_lossy()
                        .into_owned();
                    self.ensure_not_protected_write_path(&destination)?;
                    self.ensure_file_scope(&destination, true)?;
                    if let Some(operation) = object.get_mut("fileOp").and_then(Value::as_object_mut)
                    {
                        operation.insert("destination".into(), json!(destination));
                    }
                }
                paths.push(path.clone());
                let is_create = object
                    .get("fileOp")
                    .and_then(Value::as_object)
                    .and_then(|op| op.get("kind"))
                    .and_then(Value::as_str)
                    == Some("create");
                let has_edits = object
                    .get("edits")
                    .and_then(Value::as_array)
                    .is_some_and(|edits| !edits.is_empty());
                let has_file_operation = object.get("fileOp").and_then(Value::as_object).is_some();
                if !has_edits && !has_file_operation {
                    bail!(
                        "File change for {path} has no operation. For an existing file use edits:[{{kind:\"replace\",range:{{start:LINE,end:LINE}},text:\"...\"}}] (or another structured edit); for a new file use fileOp:{{kind:\"create\",text:\"...\"}}."
                    );
                }
                if !unlimited
                    && !is_create
                    && object.get("snapshot").is_none()
                    && let Some(snapshot) = self.cached_snapshot(&cache_path)
                {
                    object.insert("snapshot".into(), json!(snapshot));
                }
                if !is_create
                    && let Some(snapshot) = object
                        .get("snapshot")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                    && super::io::is_local_snapshot(&snapshot)
                {
                    let daemon_snapshot =
                        self.rehydrate_local_snapshot(&cache_path, &path, &snapshot, unlimited)?;
                    object.insert("snapshot".into(), json!(daemon_snapshot));
                }
                if !unlimited
                    && !is_create
                    && object.get("snapshot").and_then(Value::as_str).is_none()
                {
                    bail!(
                        "Editing existing file {path} requires fresh read coverage. Call read_file first, then retry apply_file_edits; the runtime attaches the cached snapshot automatically."
                    );
                }
                let has_semantic_edits = object
                    .get("edits")
                    .and_then(Value::as_array)
                    .is_some_and(|edits| super::syntax_edit::has_semantic_edits(edits));
                if has_semantic_edits {
                    let snapshot_text = if let Some(snapshot) = object
                        .get("snapshot")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                    {
                        self.edit_mut()?.snapshot_text(&snapshot).with_context(|| {
                            format!("load edit snapshot for semantic targeting in {path}")
                        })?
                    } else {
                        std::fs::read_to_string(&path)
                            .with_context(|| format!("read {path} for semantic targeting"))?
                    };
                    let edits = object
                        .get_mut("edits")
                        .and_then(Value::as_array_mut)
                        .expect("semantic edit array exists");
                    super::syntax_edit::concretize_semantic_edits(
                        Path::new(&path),
                        &snapshot_text,
                        edits,
                    )?;
                }
            }
            paths
        };
        let result: ApplyResult = match self.edit_mut()?.apply(&request, unlimited) {
            Ok(result) => result,
            Err(error) => {
                // A transport failure can happen after the daemon has
                // committed the transaction but before the response reaches
                // Rust. Drop all evidence in that ambiguous case; the next
                // read will restart the daemon if needed and obtain a fresh
                // snapshot instead of editing from stale cache state.
                if self.edit.as_ref().is_some_and(|edit| edit.is_broken()) {
                    self.invalidate_workspace_cache();
                }
                return Err(error);
            }
        };
        self.sync_edit_state();
        let error_count = result
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity.eq_ignore_ascii_case("error"))
            .count();
        let mut changed_paths = paths.into_iter().collect::<HashSet<_>>();
        for file in &result.files {
            changed_paths.insert(file.path.clone());
            if let Some(destination) = &file.destination {
                changed_paths.insert(destination.clone());
            }
        }
        let changed_path_set = changed_paths;
        let mut changed_paths = changed_path_set.iter().cloned().collect::<Vec<_>>();
        changed_paths.sort();
        self.invalidate_workspace_cache_for_paths(&changed_path_set);
        self.latest_mutation = Some(MutationValidation {
            error_count,
            changed_paths,
        });
        self.workspace_write_generation = self.workspace_write_generation.wrapping_add(1);
        self.externalize_if_large(serde_json::to_value(result)?, 16 * 1024, None)
    }

    /// Rejects writes into active Yeet runtime/session state directories.
    fn ensure_not_protected_write_path(&self, path: &str) -> Result<()> {
        if self.protected_write_paths.is_empty() {
            return Ok(());
        }
        let candidate = PathBuf::from(path);
        let candidate = if candidate.is_absolute() {
            candidate
        } else {
            self.workspace_root.join(candidate)
        };
        let normalized = canonicalize_existing_ancestor(&candidate)?;
        for protected in &self.protected_write_paths {
            let protected = protected
                .canonicalize()
                .unwrap_or_else(|_| protected.to_path_buf());
            if normalized == protected || normalized.starts_with(&protected) {
                bail!(
                    "Refusing to mutate active Yeet runtime state at {}. Active session state is protected even in unlimited mode.",
                    protected.display()
                );
            }
        }
        Ok(())
    }

    /// Renders a compact response when a requested source range is already cached.
    pub(super) fn duplicate_read_payload(&self, path: &str, start: usize, end: usize) -> Value {
        let entries = self.read_cache.get(path).map(Vec::as_slice).unwrap_or(&[]);
        let total = entries.first().map(|entry| entry.total).unwrap_or(end);
        json!({
            "path": path,
            "snapshot": self.cached_snapshot(path),
            "duplicate": true,
            "contentAlreadyReturned": true,
            "startLine": start,
            "endLine": end.min(total),
            "totalLines": total,
            "fileFullyRead": coverage_complete(total, entries),
            "nextStartLine": next_uncovered(total, entries),
            "hint": "This range is already covered by earlier reads. Reuse it or request only uncovered source."
        })
    }

    /// Converts one cached source range into the standard read-file payload.
    pub(super) fn read_payload(&self, entry: &ReadCacheEntry, replay: bool) -> Result<Value> {
        let entries = self
            .read_cache
            .get(&entry.path)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let complete = coverage_complete(entry.total, entries);
        let next = next_uncovered(entry.total, entries);
        Ok(json!({
            "path":entry.path,"snapshot":entry.snapshot,"startLine":entry.start,"endLine":entry.end,"totalLines":entry.total,
            "lines":entry.anchored,"fileFullyRead":complete,"nextStartLine":next,"refreshReplay":replay
        }))
    }

    /// Stores oversized tool output as an artifact and returns compact metadata.
    pub(super) fn externalize_if_large(
        &mut self,
        value: Value,
        threshold: usize,
        read: Option<&ReadResult>,
    ) -> Result<String> {
        let encoded = serde_json::to_string(&value)?;
        if encoded.len() <= threshold || !self.artifacts_enabled {
            return Ok(encoded);
        }
        let content = read
            .map(|read| read.anchored.as_str())
            .unwrap_or(encoded.as_str());
        let artifact = self.artifacts.store(content)?;
        let preview = artifact_output::bounded_utf8_bytes(content, 4 * 1024);
        let preview_bytes = preview.len();
        let mut metadata = Map::new();
        metadata.insert("artifactId".into(), json!(artifact));
        metadata.insert("externalized".into(), json!(true));
        metadata.insert("bytes".into(), json!(content.len()));
        metadata.insert("preview".into(), json!(preview));
        metadata.insert("previewBytes".into(), json!(preview_bytes));
        metadata.insert("hint".into(), json!("Large output was externalized. Reuse this bounded preview; use a narrow read_artifact range for specific missing evidence, and search_artifact only when the needed text cannot be located by range."));
        if let Some(read) = read {
            metadata.insert("path".into(), json!(read.path));
            metadata.insert("snapshot".into(), json!(read.snapshot));
            metadata.insert("startLine".into(), json!(read.start_line));
            metadata.insert("endLine".into(), json!(read.end_line));
            metadata.insert("totalLines".into(), json!(read.total_lines));
        }
        Ok(Value::Object(metadata).to_string())
    }

    pub(super) fn record_edit_read_coverage(&mut self, path: &str, snapshot: &str) {
        let ranges = self
            .read_cache
            .get(path)
            .into_iter()
            .flatten()
            .filter(|entry| entry.snapshot == snapshot)
            .map(|entry| (entry.start, entry.end))
            .collect::<Vec<_>>();
        if ranges.is_empty() {
            return;
        }
        let evidence = self
            .edit_read_coverage
            .entry(path.to_owned())
            .or_insert_with(|| EditReadCoverage {
                snapshot: snapshot.to_owned(),
                ranges: Vec::new(),
            });
        if evidence.snapshot != snapshot {
            evidence.snapshot = snapshot.to_owned();
            evidence.ranges.clear();
        }
        evidence.ranges.extend(ranges);
        evidence.ranges = merged_ranges(std::mem::take(&mut evidence.ranges));
        self.edit_snapshots
            .insert(path.to_owned(), snapshot.to_owned());
    }

    fn rehydrate_local_snapshot(
        &mut self,
        cache_path: &str,
        path: &str,
        snapshot: &str,
        unsafe_access: bool,
    ) -> Result<String> {
        let cached_snapshot = self.cached_snapshot(cache_path).ok_or_else(|| {
            anyhow!("Snapshot {snapshot} is no longer backed by read_file coverage for {path}; read the file again before editing")
        })?;
        if cached_snapshot != snapshot {
            bail!(
                "Snapshot {snapshot} is stale for {path}; the newest read snapshot is {cached_snapshot}. Read the file again before editing."
            );
        }
        let current = super::io::local_snapshot_handle_for_path(Path::new(path))?;
        if current != snapshot {
            bail!(
                "File {path} changed after it was read. Call read_file again before applying edits."
            );
        }
        let evidence = self
            .edit_read_coverage
            .get(cache_path)
            .cloned()
            .ok_or_else(|| {
                anyhow!("Snapshot {snapshot} has no retained read coverage for {path}; read the file again before editing.")
            })?;
        if evidence.snapshot != snapshot {
            bail!("Snapshot {snapshot} is stale for {path}; read the file again before editing.");
        }

        let mut daemon_snapshot = None::<String>;
        for (start, end) in evidence.ranges {
            let read = self
                .edit_mut()?
                .read(path, Some(start), Some(end), unsafe_access)?;
            match daemon_snapshot.as_deref() {
                Some(existing) if existing != read.snapshot => {
                    bail!("Edit snapshot changed while rehydrating {path}; read the file again")
                }
                Some(_) => {}
                None => daemon_snapshot = Some(read.snapshot),
            }
        }
        let daemon_snapshot = daemon_snapshot.expect("read coverage is non-empty");
        self.edit_snapshots
            .insert(cache_path.to_owned(), daemon_snapshot.clone());
        Ok(daemon_snapshot)
    }

    /// Returns the newest snapshot identifier established by read_file.
    /// This cache is task/edit safety state, not model-visible duplicate coverage.
    fn cached_snapshot(&self, path: &str) -> Option<String> {
        self.edit_snapshots.get(path).cloned()
    }

    /// Invalidates source/search/list caches affected by known changed paths.
    fn invalidate_workspace_cache_for_paths(&mut self, changed_paths: &HashSet<String>) {
        self.read_cache
            .retain(|path, _| !changed_paths.contains(path));
        self.edit_snapshots
            .retain(|path, _| !changed_paths.contains(path));
        self.edit_read_coverage
            .retain(|path, _| !changed_paths.contains(path));
        self.searches.clear();
        self.listings.clear();
        self.shell_inspections.clear();
        self.workspace_generation = self.workspace_generation.wrapping_add(1);
    }

    /// Invalidates all workspace evidence after a shell mutation with unknown scope.
    pub(super) fn invalidate_workspace_cache(&mut self) {
        self.read_cache.clear();
        self.edit_snapshots.clear();
        self.edit_read_coverage.clear();
        self.searches.clear();
        self.listings.clear();
        self.shell_inspections.clear();
        self.workspace_generation = self.workspace_generation.wrapping_add(1);
    }
}

/// Accepts the flat range shape used by older Yeet prompts and sessions.
///
/// Current edits place line anchors under `range`. Models can retain the older
/// `{kind:"replace",start,end,startHash,endHash,text}` shape across context
/// windows, so normalize that shape before the edit daemon validates it. This
/// is intentionally limited to unambiguous replace/delete ranges.
fn normalize_legacy_edit_shapes(request: &mut Value) {
    let Some(changes) = request.get_mut("changes").and_then(Value::as_array_mut) else {
        return;
    };
    for change in changes {
        let Some(edits) = change.get_mut("edits").and_then(Value::as_array_mut) else {
            continue;
        };
        for edit in edits {
            let Some(object) = edit.as_object_mut() else {
                continue;
            };
            if object.contains_key("range")
                || !matches!(
                    object.get("kind").and_then(Value::as_str),
                    Some("replace" | "delete")
                )
            {
                continue;
            }
            let (Some(start), Some(end)) =
                (object.get("start").cloned(), object.get("end").cloned())
            else {
                continue;
            };
            let mut range = serde_json::Map::new();
            range.insert("start".into(), start);
            range.insert("end".into(), end);
            if let Some(value) = object.get("startHash").cloned() {
                range.insert("startHash".into(), value);
            }
            if let Some(value) = object.get("endHash").cloned() {
                range.insert("endHash".into(), value);
            }
            object.remove("start");
            object.remove("end");
            object.remove("startHash");
            object.remove("endHash");
            object.insert("range".into(), Value::Object(range));
        }
    }
}
