//! Structured editing and shared workspace-read cache maintenance.
//!
//! Edits require read snapshots independently of permission mode and invalidate only the
//! cache entries that may have become stale. Large results are externalized
//! through the registry's artifact store.

use super::*;
use crate::edit::{EditFailure, ReadResult};

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
    pub(super) fn apply_file_edits(
        &mut self,
        arguments: &Value,
        cancel: &AtomicBool,
    ) -> Result<String> {
        self.apply_file_edits_inner(arguments, cancel)
            .map_err(|error| {
                if error.downcast_ref::<EditFailure>().is_some() {
                    error
                } else {
                    EditFailure::new("edit_rejected", "unchanged", error.to_string()).into()
                }
            })
    }

    fn apply_file_edits_inner(&mut self, arguments: &Value, cancel: &AtomicBool) -> Result<String> {
        self.sync_edit_state();
        let mut request = arguments.clone();
        if let Some(patch) = request.get("patch").and_then(Value::as_str) {
            if request.get("changes").is_some() {
                bail!("apply_file_edits takes either patch or changes, not both");
            }
            let changes = super::text_edit::parse_patch(patch)?;
            let object = request
                .as_object_mut()
                .ok_or_else(|| anyhow!("apply_file_edits arguments must be an object"))?;
            object.remove("patch");
            object.insert("changes".into(), json!(changes));
        }
        normalize_legacy_edit_shapes(&mut request);
        let unlimited = SandboxStore::new(&self.context.workspace_root)?
            .load()?
            .mode
            == SandboxMode::Unlimited;
        let mut match_modes = Vec::new();
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
                    .ok_or_else(|| {
                        anyhow!(
                            "Each change needs its own path: changes:[{{path:\"FILE\",edits:[...]}}]"
                        )
                    })?
                    .to_owned();
                let path = self
                    .context
                    .resolve_session_path(&requested_path)?
                    .to_string_lossy()
                    .into_owned();
                let cache_path =
                    super::paths::stable_workspace_path_key(&self.context.workspace_root, &path)?;
                object.insert("path".into(), json!(path.clone()));
                self.context.ensure_not_protected_write_path(&path)?;
                self.context.ensure_file_scope(&path, true)?;
                let requested_destination = object
                    .get("fileOp")
                    .and_then(Value::as_object)
                    .and_then(|op| op.get("destination"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(requested_destination) = requested_destination {
                    let destination = self
                        .context
                        .resolve_session_path(&requested_destination)?
                        .to_string_lossy()
                        .into_owned();
                    self.context.ensure_not_protected_write_path(&destination)?;
                    self.context.ensure_file_scope(&destination, true)?;
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
                        "File change for {path} has no operation. For an existing file use edits:[{{kind:\"replaceText\",oldText:\"...\",newText:\"...\"}}] (or a line-range edit); for a new file use fileOp:{{kind:\"create\",text:\"...\"}}."
                    );
                }
                if !is_create && let Some(provided) = object.get("snapshot").and_then(Value::as_str)
                {
                    if self.cached_snapshot(&cache_path).as_deref() != Some(provided) {
                        bail!(
                            "Snapshot {provided} is not the retained read revision for {path}; call read_file again before editing."
                        );
                    }
                }
                if !is_create
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
                if !is_create && object.get("snapshot").and_then(Value::as_str).is_none() {
                    bail!(
                        "Editing existing file {path} requires fresh read coverage. Call read_file first, then retry apply_file_edits; the runtime attaches the cached snapshot automatically."
                    );
                }
                let (has_text_edits, has_semantic_edits) = object
                    .get("edits")
                    .and_then(Value::as_array)
                    .map_or((false, false), |edits| {
                        (
                            super::text_edit::has_text_edits(edits),
                            super::syntax_edit::has_semantic_edits(edits),
                        )
                    });
                if has_text_edits || has_semantic_edits {
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
                    match_modes.push(super::text_edit::concretize_text_edits(
                        &path,
                        &snapshot_text,
                        edits,
                    )?);
                    super::syntax_edit::concretize_semantic_edits(
                        Path::new(&path),
                        &snapshot_text,
                        edits,
                    )?;
                } else {
                    match_modes.push(Vec::new());
                }
            }
            paths
        };
        // Approval and semantic preparation must not hold the cross-session
        // lock. The backend revalidates file identities and digests on commit.
        let edit_guard = edit_lock::WorkspaceEditLock::acquire(
            &crate::platform::default_config_directory(),
            &self.context.workspace_root,
            cancel,
        )?;
        let mut result: ApplyResult = match self.edit_mut()?.apply(&request, unlimited) {
            Ok(result) => result,
            Err(error) => {
                // A transport failure can happen after the daemon has
                // committed the transaction but before the response reaches
                // Rust. Drop all evidence in that ambiguous case; the next
                // read will restart the daemon if needed and obtain a fresh
                // snapshot instead of editing from stale cache state.
                if self.edit.as_ref().is_some_and(|edit| edit.is_broken())
                    || error
                        .downcast_ref::<EditFailure>()
                        .is_some_and(|failure| failure.mutation_outcome != "unchanged")
                {
                    self.invalidate_workspace_cache();
                }
                return Err(error);
            }
        };
        drop(edit_guard);
        for (file, modes) in result.files.iter_mut().zip(match_modes) {
            file.match_modes = modes;
            // Public handles remain content revisions, not daemon-local IDs.
            file.snapshot = file
                .revision
                .as_ref()
                .map(|revision| format!("r_{revision}"));
        }
        self.sync_edit_state();
        let error_count = result
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity.eq_ignore_ascii_case("error"))
            .count()
            + usize::from(result.diagnostics_error.is_some());
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
        self.record_post_edit_snapshots(&result);
        self.evidence.latest_mutation = Some(MutationValidation {
            error_count,
            changed_paths,
        });
        self.evidence.workspace_write_generation =
            self.evidence.workspace_write_generation.wrapping_add(1);
        serde_json::to_value(result)
            .map_err(anyhow::Error::from)
            .and_then(|value| self.externalize_if_large(value, 16 * 1024, None))
            .map_err(|error| {
                EditFailure::new(
                    "edit_result_failed",
                    "applied",
                    format!("Edit was applied but its response could not be rendered: {error}"),
                )
                .into()
            })
    }

    /// Converts one cached source range into the standard read-file payload.
    pub(super) fn read_payload(&self, entry: &ReadCacheEntry, replay: bool) -> Result<Value> {
        let entries = self
            .evidence
            .read_cache
            .get(&entry.path)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let complete = coverage_complete(entry.total, entries);
        let next = (entry.end < entry.total).then_some(entry.end + 1);
        let first_unread = next_uncovered(entry.total, entries);
        Ok(json!({
            "path":entry.path,"snapshot":entry.snapshot,"startLine":entry.start,"endLine":entry.end,"totalLines":entry.total,
            "lines":entry.anchored,"fileFullyRead":complete,"nextStartLine":next,"firstUnreadLine":first_unread,"refreshReplay":replay
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
        if let Some(outcome) = value.get("mutationOutcome").and_then(Value::as_str)
            && let Some(files) = value.get("files").and_then(Value::as_array)
        {
            // A short edit response must still say whether the write happened.
            // Its complete result remains in the artifact, including revisions,
            // coverage, diffs, and diagnostics omitted from this summary.
            metadata.insert("mutationOutcome".into(), json!(outcome));
            if let Some(status) = value.get("diagnosticsStatus").and_then(Value::as_str) {
                metadata.insert("diagnosticsStatus".into(), json!(status));
            }
            if let Some(error) = value.get("diagnosticsError").and_then(Value::as_str) {
                metadata.insert(
                    "diagnosticsError".into(),
                    json!(artifact_output::bounded_utf8_bytes(error, 1024)),
                );
                metadata.insert(
                    "diagnosticsErrorTruncated".into(),
                    json!(error.len() > 1024),
                );
            }
            let summaries = files
                .iter()
                .take(6)
                .map(|file| {
                    let mut summary = Map::new();
                    for key in ["path", "destination"] {
                        if let Some(path) = file.get(key).and_then(Value::as_str) {
                            summary.insert(
                                key.into(),
                                json!(artifact_output::bounded_utf8_bytes(path, 384)),
                            );
                            if path.len() > 384 {
                                summary.insert(format!("{key}Truncated"), json!(true));
                            }
                        }
                    }
                    if let Some(operation) = file.get("operation").and_then(Value::as_str) {
                        summary.insert("operation".into(), json!(operation));
                    }
                    Value::Object(summary)
                })
                .collect::<Vec<_>>();
            metadata.insert("fileCount".into(), json!(files.len()));
            metadata.insert("files".into(), json!(summaries));
            metadata.insert("filesSummaryOnly".into(), json!(true));
            metadata.insert("filesTruncated".into(), json!(files.len() > 6));
        }
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
        let empty_file = self
            .evidence
            .read_cache
            .get(path)
            .into_iter()
            .flatten()
            .any(|entry| entry.snapshot == snapshot && entry.total == 0);
        let ranges = self
            .evidence
            .read_cache
            .get(path)
            .into_iter()
            .flatten()
            .filter(|entry| entry.snapshot == snapshot && entry.end >= entry.start)
            .map(|entry| (entry.start, entry.end))
            .collect::<Vec<_>>();
        if ranges.is_empty() && !empty_file {
            return;
        }
        let evidence = self
            .evidence
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
        self.evidence
            .edit_snapshots
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
        let evidence = self.evidence.edit_read_coverage
            .get(cache_path)
            .cloned()
            .ok_or_else(|| {
                anyhow!("Snapshot {snapshot} has no retained read coverage for {path}; read the file again before editing.")
            })?;
        if evidence.snapshot != snapshot {
            bail!("Snapshot {snapshot} is stale for {path}; read the file again before editing.");
        }

        let mut daemon_snapshot = None::<String>;
        for &(start, end) in &evidence.ranges {
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
        if daemon_snapshot.is_none() {
            let (_, lines) = super::io::local_snapshot_for_path(Path::new(path))?;
            if lines != 0 {
                bail!(
                    "Snapshot {snapshot} has no retained read coverage for {path}; read the file again before editing."
                );
            }
            daemon_snapshot = Some(
                self.edit_mut()?
                    .read(path, None, None, unsafe_access)?
                    .snapshot,
            );
        }
        let daemon_snapshot = daemon_snapshot.expect("covered ranges or an empty file");
        // The daemon may remember a wider earlier read of the same digest.
        // Restrict that evidence to what survived the model-output boundary.
        self.edit_mut()?.limit_snapshot_coverage(
            &daemon_snapshot,
            snapshot.trim_start_matches("r_"),
            &evidence.ranges,
        )?;
        // edit_snapshots keeps the local content hash: daemon handles do not
        // survive a daemon restart and cannot be re-checked against the file.
        Ok(daemon_snapshot)
    }

    /// Preserve the backend's transformed prior coverage and supplied text,
    /// never promote a small successful edit into a full-file read.
    fn record_post_edit_snapshots(&mut self, result: &ApplyResult) {
        for file in &result.files {
            if file.operation == "delete" {
                continue;
            }
            let target = file.destination.as_deref().unwrap_or(&file.path);
            let Some(revision) = &file.revision else {
                continue;
            };
            let snapshot = format!("r_{revision}");
            let Ok(path) = self.context.resolve_session_path(target) else {
                continue;
            };
            if super::io::local_snapshot_handle_for_path(&path)
                .ok()
                .as_deref()
                != Some(snapshot.as_str())
            {
                // A later external write must not acquire this edit's coverage.
                continue;
            }
            let Ok(cache_path) = super::paths::stable_workspace_path_key(
                &self.context.workspace_root,
                &path.to_string_lossy(),
            ) else {
                continue;
            };
            self.evidence.edit_read_coverage.insert(
                cache_path.clone(),
                EditReadCoverage {
                    snapshot: snapshot.clone(),
                    ranges: file
                        .seen_ranges
                        .iter()
                        .map(|range| (range.start, range.end))
                        .collect(),
                },
            );
            self.evidence.edit_snapshots.insert(cache_path, snapshot);
        }
    }

    /// Returns the newest snapshot identifier established by read_file.
    /// This cache is task/edit safety state, not model-visible duplicate coverage.
    fn cached_snapshot(&self, path: &str) -> Option<String> {
        self.evidence.edit_snapshots.get(path).cloned()
    }

    /// Invalidates source/search/list caches affected by known changed paths.
    pub(super) fn invalidate_workspace_cache_for_paths(&mut self, changed_paths: &HashSet<String>) {
        self.evidence.invalidate_paths(changed_paths);
    }

    /// Invalidates all workspace evidence after a shell mutation with unknown scope.
    pub(super) fn invalidate_workspace_cache(&mut self) {
        self.evidence.invalidate_workspace();
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
    merge_same_path_edits(changes);
    for change in changes {
        let Some(edits) = change.get_mut("edits").and_then(Value::as_array_mut) else {
            continue;
        };
        for edit in edits {
            let Some(object) = edit.as_object_mut() else {
                continue;
            };
            // `insert` takes `at`; a range here means "insert at this line".
            if object.get("kind").and_then(Value::as_str) == Some("insert")
                && !object.contains_key("at")
                && let Some(line) = object
                    .remove("range")
                    .and_then(|range| range.get("start").cloned())
                    .or_else(|| object.get("line").cloned())
            {
                object.remove("line");
                object.insert("at".into(), json!({"kind": "before", "line": line}));
                continue;
            }
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

/// Folds several edit-only change objects for one path into the first, since
/// the daemon rejects multiple changes that target the same file.
fn merge_same_path_edits(changes: &mut Vec<Value>) {
    let edit_only =
        |value: &Value| value.get("fileOp").is_none() && value.get("snapshot").is_none();
    let mut merged: Vec<Value> = Vec::with_capacity(changes.len());
    for change in changes.drain(..) {
        let edits = change.get("edits").and_then(Value::as_array).cloned();
        let path = change
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let (Some(edits), Some(path)) = (edits, path)
            && edit_only(&change)
            && let Some(existing) = merged.iter_mut().find(|candidate| {
                candidate.get("path").and_then(Value::as_str) == Some(path.as_str())
                    && edit_only(candidate)
            })
            && let Some(list) = existing.get_mut("edits").and_then(Value::as_array_mut)
        {
            list.extend(edits);
            continue;
        }
        merged.push(change);
    }
    *changes = merged;
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn externalized_edit_preserves_outcome_and_full_retrievable_result() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut registry = ToolRegistry::new_with_bridge_handle(
            BridgeHandle::lazy_for_workspace(root.clone()),
            root,
            WorkerRegistry::new(Vec::new()).unwrap(),
            PermissionBroker::default(),
        )
        .unwrap();
        let result = json!({
            "mutationOutcome":"applied",
            "diagnosticsStatus":"failed",
            "diagnosticsError":"진단 오류 ".repeat(1000),
            "diagnostics":[],
            "files":(0..10).map(|index| json!({
                "path":format!("src/file-{index}.rs"), "operation":"update",
                "diff":{"text":"large change\n".repeat(2000)},
            })).collect::<Vec<_>>(),
        });
        let rendered = registry
            .externalize_if_large(result.clone(), 16 * 1024, None)
            .unwrap();
        let compact: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(compact["mutationOutcome"], "applied");
        assert_eq!(compact["diagnosticsStatus"], "failed");
        assert_eq!(compact["diagnosticsErrorTruncated"], true);
        assert!(compact["diagnosticsError"].as_str().unwrap().len() < 1200);
        assert_eq!(compact["fileCount"], 10);
        assert_eq!(compact["filesTruncated"], true);
        assert_eq!(compact["files"][0]["path"], "src/file-0.rs");
        assert_eq!(compact["files"][0]["operation"], "update");
        assert!(rendered.len() < 10 * 1024);
        let artifact = compact["artifactId"].as_str().unwrap();
        let retained: Value =
            serde_json::from_slice(&std::fs::read(registry.artifacts.path(artifact)).unwrap())
                .unwrap();
        assert_eq!(retained, result);
    }
}
