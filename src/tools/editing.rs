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
                let path = object
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("change requires path"))?
                    .to_owned();
                self.ensure_not_protected_write_path(&path)?;
                self.ensure_file_scope(&path, true)?;
                if let Some(destination) = object
                    .get("fileOp")
                    .and_then(Value::as_object)
                    .and_then(|op| op.get("destination"))
                    .and_then(Value::as_str)
                {
                    self.ensure_not_protected_write_path(destination)?;
                    self.ensure_file_scope(destination, true)?;
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
                    && let Some(snapshot) = self.cached_snapshot(&path)
                {
                    object.insert("snapshot".into(), json!(snapshot));
                }
                if !unlimited
                    && !is_create
                    && object.get("snapshot").and_then(Value::as_str).is_none()
                {
                    bail!(
                        "Editing existing file {path} requires a snapshot. Call read_file first, then retry apply_file_edits using the returned snapshot."
                    );
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
        self.externalize_if_large(serde_json::to_value(result)?, 48 * 1024, None)
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
        if encoded.len() <= threshold {
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
        metadata.insert("hint".into(), json!("Large output was externalized. Reuse this bounded preview; use search_artifact first, then a narrow read_artifact range only for specific missing evidence."));
        if let Some(read) = read {
            metadata.insert("path".into(), json!(read.path));
            metadata.insert("snapshot".into(), json!(read.snapshot));
            metadata.insert("startLine".into(), json!(read.start_line));
            metadata.insert("endLine".into(), json!(read.end_line));
            metadata.insert("totalLines".into(), json!(read.total_lines));
        }
        Ok(Value::Object(metadata).to_string())
    }

    /// Returns the newest cached snapshot identifier for one path.
    fn cached_snapshot(&self, path: &str) -> Option<String> {
        self.read_cache
            .get(path)
            .and_then(|entries| entries.last())
            .map(|entry| entry.snapshot.clone())
    }

    /// Invalidates source/search/list caches affected by known changed paths.
    fn invalidate_workspace_cache_for_paths(&mut self, changed_paths: &HashSet<String>) {
        self.read_cache
            .retain(|path, _| !changed_paths.contains(path));
        self.searches.clear();
        self.listings.clear();
        self.shell_inspections.clear();
        self.workspace_generation = self.workspace_generation.wrapping_add(1);
    }

    /// Invalidates all workspace evidence after a shell mutation with unknown scope.
    pub(super) fn invalidate_workspace_cache(&mut self) {
        self.read_cache.clear();
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

#[cfg(test)]
mod legacy_edit_tests {
    use super::*;

    #[test]
    fn normalizes_flat_legacy_replace_ranges() {
        let mut request = json!({
            "changes": [{
                "path": "src/lib.rs",
                "edits": [{
                    "kind": "replace",
                    "start": 12,
                    "end": 14,
                    "startHash": "abcd",
                    "endHash": "ef01",
                    "text": "replacement"
                }]
            }]
        });
        normalize_legacy_edit_shapes(&mut request);
        let edit = &request["changes"][0]["edits"][0];
        assert_eq!(edit["range"]["start"], 12);
        assert_eq!(edit["range"]["end"], 14);
        assert_eq!(edit["range"]["startHash"], "abcd");
        assert_eq!(edit["range"]["endHash"], "ef01");
        assert!(edit.get("start").is_none());
        assert!(edit.get("end").is_none());
    }

    #[test]
    fn preserves_current_range_shape() {
        let mut request = json!({
            "changes": [{
                "path": "src/lib.rs",
                "edits": [{
                    "kind": "replace",
                    "range": {"start": 3, "end": 4},
                    "text": "current"
                }]
            }]
        });
        let before = request.clone();
        normalize_legacy_edit_shapes(&mut request);
        assert_eq!(request, before);
    }
}
