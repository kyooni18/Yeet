//! Built-in workspace, web, document, data, and Skill-script tool handlers.
//!
//! These handlers translate validated tool arguments into concrete reads and
//! bounded artifacts while reusing the registry's shared caches and sandbox.

use super::*;

impl ToolRegistry {
    /// Reads one bounded file range or dispatches a small batch of independent reads.
    pub(super) fn read_file(&mut self, object: &Map<String, Value>) -> Result<String> {
        self.sync_edit_state();
        if object.contains_key("requests") {
            if object.contains_key("path")
                || object.contains_key("startLine")
                || object.contains_key("endLine")
                || object.contains_key("refresh")
            {
                bail!("read_file accepts either path/range fields or requests, not both");
            }
            return self.read_file_batch(object);
        }

        let path = string_arg(object, "path")?.to_owned();
        self.ensure_file_scope(&path, false)?;
        let cache_path = super::paths::stable_workspace_path_key(&self.workspace_root, &path)?;
        let unsafe_access =
            SandboxStore::new(&self.workspace_root)?.load()?.mode == SandboxMode::Unlimited;
        let requested_start = usize_arg(object, "startLine").unwrap_or(1).max(1);
        let explicit_end = usize_arg(object, "endLine");
        if explicit_end.is_some_and(|end| end < requested_start) {
            bail!("read_file endLine must be greater than or equal to startLine");
        }
        let limit = if explicit_end.is_some() {
            EXPLICIT_READ_LINES
        } else {
            DEFAULT_READ_LINES
        };
        let max_end = requested_start.saturating_add(limit - 1);
        let requested_end = explicit_end.unwrap_or(max_end).min(max_end);
        let refresh = object
            .get("refresh")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let threshold = if explicit_end.is_some() {
            EXPLICIT_INLINE_BYTES
        } else {
            DEFAULT_INLINE_BYTES
        };
        let cached_before = self
            .read_cache
            .get(&cache_path)
            .cloned()
            .unwrap_or_default();
        let cached_total = cached_before.first().map(|entry| entry.total);
        let bounded_end = cached_total.map_or(requested_end, |total| requested_end.min(total));
        let covered_before = covered_ranges_within(&cached_before, requested_start, bounded_end);

        if refresh {
            let mut read = self.edit_mut()?.read(
                &path,
                Some(requested_start),
                Some(requested_end),
                unsafe_access,
            )?;
            self.sync_edit_state();
            read.path = cache_path.clone();
            let refreshed_snapshot = read.snapshot.clone();
            let refreshed_total = read.total_lines;
            let actual_end = requested_end.min(refreshed_total);
            let unchanged_and_covered = refresh_matches_cached_coverage(
                &cached_before,
                &refreshed_snapshot,
                requested_start,
                actual_end,
            );
            let entry = cache_read_result(&mut self.read_cache, read.clone());
            let payload = self.read_payload(&entry, true)?;
            if unchanged_and_covered {
                let avoided_bytes = payload.to_string().len();
                let mut duplicate =
                    self.duplicate_read_payload(&cache_path, requested_start, actual_end);
                if let Value::Object(object) = &mut duplicate {
                    object.insert("refreshVerified".into(), json!(true));
                    object.insert("snapshot".into(), json!(refreshed_snapshot));
                    object.insert("duplicateReadBytesAvoided".into(), json!(avoided_bytes));
                    object.insert("hint".into(), json!("Refresh verified the file snapshot is unchanged and this range was already returned. Reuse the earlier anchored source; no source text is repeated."));
                }
                return Ok(duplicate.to_string());
            }
            return self.externalize_if_large(payload, threshold, Some(&read));
        }

        let missing = uncovered_ranges(requested_start, bounded_end, &covered_before);
        if missing.is_empty() {
            return Ok(self
                .duplicate_read_payload(&cache_path, requested_start, bounded_end)
                .to_string());
        }

        let mut new_entries = Vec::new();
        let mut total = cached_total;
        for (start, mut end) in missing {
            if let Some(total) = total {
                if start > total {
                    break;
                }
                end = end.min(total);
            }
            let mut read = self
                .edit_mut()?
                .read(&path, Some(start), Some(end), unsafe_access)?;
            self.sync_edit_state();
            read.path = cache_path.clone();
            total = Some(read.total_lines);
            new_entries.push(cache_read_result(&mut self.read_cache, read));
        }
        if new_entries.is_empty() {
            return Ok(self
                .duplicate_read_payload(&cache_path, requested_start, bounded_end)
                .to_string());
        }

        let total = total.unwrap_or_else(|| new_entries[0].total);
        let actual_requested_end = bounded_end.min(total);
        let reused_ranges =
            covered_ranges_within(&cached_before, requested_start, actual_requested_end);
        if new_entries.len() == 1 {
            let entry = &new_entries[0];
            let mut payload = self.read_payload(entry, false)?;
            if let Value::Object(object) = &mut payload {
                object.insert("requestedStartLine".into(), json!(requested_start));
                object.insert("requestedEndLine".into(), json!(actual_requested_end));
                if !reused_ranges.is_empty() {
                    object.insert("incremental".into(), json!(true));
                    object.insert("reusedCoveredRanges".into(), json!(reused_ranges));
                    object.insert("hint".into(), json!("Only previously unseen source is returned. Reuse the covered ranges already present in context."));
                }
            }
            return self.externalize_if_large(payload, threshold, None);
        }

        let entries = self
            .read_cache
            .get(&cache_path)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let segments = new_entries
            .iter()
            .map(
                |entry| json!({"startLine":entry.start,"endLine":entry.end,"lines":entry.anchored}),
            )
            .collect::<Vec<_>>();
        let payload = json!({
            "path": cache_path,
            "snapshot": new_entries.last().map(|entry| entry.snapshot.as_str()),
            "requestedStartLine": requested_start,
            "requestedEndLine": actual_requested_end,
            "totalLines": total,
            "incremental": true,
            "reusedCoveredRanges": reused_ranges,
            "segments": segments,
            "fileFullyRead": coverage_complete(total, entries),
            "nextStartLine": next_uncovered(total, entries),
            "hint": "Only previously unseen source segments are returned. Reuse covered ranges already present in context."
        });
        self.externalize_if_large(payload, threshold, None)
    }

    /// Executes up to eight independent file reads and returns one batched result.
    fn read_file_batch(&mut self, object: &Map<String, Value>) -> Result<String> {
        let requests = object
            .get("requests")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("read_file requires path or a non-empty requests array"))?;
        if requests.is_empty() || requests.len() > 8 {
            bail!("read_file accepts 1 to 8 batch requests");
        }
        let mut values = Vec::new();
        for request in requests {
            match request.as_object() {
                Some(object) => match self.read_file(object) {
                    Ok(value) => {
                        values.push(serde_json::from_str(&value).unwrap_or_else(|_| json!(value)))
                    }
                    Err(error) => values.push(
                        json!({"error":error.to_string(),"path":object.get("path").cloned()}),
                    ),
                },
                None => values.push(json!({"error":"read_file requests must be objects"})),
            }
        }
        Ok(serde_json::to_string(&values)?)
    }

    /// Compatibility path for restored transcripts that still contain read_files calls.
    pub(super) fn read_files(&mut self, object: &Map<String, Value>) -> Result<String> {
        self.read_file_batch(object)
    }

    /// Lists a bounded workspace subtree and suppresses exact replay requests.
    pub(super) fn list_files(&mut self, object: &Map<String, Value>) -> Result<String> {
        let path = root_capable_workspace_path(object.get("path").and_then(Value::as_str));
        self.ensure_file_scope(path, false)?;
        let cache_path = super::paths::stable_workspace_path_key(&self.workspace_root, path)?;
        let max_results = usize_arg(object, "maxResults").unwrap_or(50).clamp(1, 500);
        let max_depth = usize_arg(object, "maxDepth").unwrap_or(2).min(12);
        let key = format!("{cache_path}:{max_results}:{max_depth}");
        if !self.listings.insert(key) {
            return Ok(json!({"duplicate":true,"contentAlreadyReturned":true,"hint":"This directory listing was already returned. Reuse it or expand a different subtree."}).to_string());
        }
        let result =
            self.edit_mut()?
                .list_files((path != ".").then_some(path), max_results, max_depth)?;
        self.sync_edit_state();
        self.externalize_if_large(serde_json::to_value(result)?, 16 * 1024, None)
    }

    /// Searches workspace text with bounded results and duplicate-query suppression.
    pub(super) fn search_workspace(&mut self, object: &Map<String, Value>) -> Result<String> {
        let query = string_arg(object, "query")?;
        if query.is_empty() {
            bail!("search_workspace requires query");
        }
        let path = root_capable_workspace_path(object.get("path").and_then(Value::as_str));
        self.ensure_file_scope(path, false)?;
        let cache_path = super::paths::stable_workspace_path_key(&self.workspace_root, path)?;
        let max_results = usize_arg(object, "maxResults").unwrap_or(20).clamp(1, 100);
        let case_sensitive = object
            .get("caseSensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let regex = object
            .get("regex")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let key = format!(
            "{}:{}:{}:{}",
            cache_path,
            case_sensitive,
            regex,
            if case_sensitive {
                query.into()
            } else {
                query.to_ascii_lowercase()
            }
        );
        if !self.searches.insert(key) {
            return Ok(json!({"duplicate":true,"contentAlreadyReturned":true,"hint":"This search was already returned. Reuse it or change query/path."}).to_string());
        }
        let result = self.edit_mut()?.search(
            query,
            (path != ".").then_some(path),
            max_results,
            case_sensitive,
            regex,
        )?;
        self.sync_edit_state();
        self.externalize_if_large(serde_json::to_value(result)?, 16 * 1024, None)
    }

    /// Executes one or more web searches concurrently and records discovered source URLs.
    pub(super) fn web_search(
        &mut self,
        object: &Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let queries = web_search_queries(object)?;
        let requested_max_results = usize_arg(object, "maxResults").unwrap_or(4);
        // Discovery snippets are only a source-selection surface. Bound the total
        // result volume aggressively so each follow-up can reuse most of the prior
        // prompt; primary evidence belongs in web_read artifacts.
        let max_results = effective_web_search_max_results(requested_max_results, queries.len());
        let language = object.get("language").and_then(Value::as_str);
        let category = object.get("category").and_then(Value::as_str);
        let time_range = object.get("timeRange").and_then(Value::as_str);
        let safe_search = object.get("safeSearch").and_then(Value::as_u64);
        let page = object.get("page").and_then(Value::as_u64);
        let backend = WebSearchBackend::parse(object.get("backend").and_then(Value::as_str))?;
        let mut slots = vec![None; queries.len()];
        let mut pending = Vec::new();
        for (index, query) in queries.iter().enumerate() {
            let key = format!(
                "{}:{}:{}:{}:{}:{}:{}:{backend:?}",
                query.to_ascii_lowercase(),
                max_results,
                language.unwrap_or(""),
                category.unwrap_or(""),
                time_range.unwrap_or(""),
                safe_search.unwrap_or(0),
                page.unwrap_or(1)
            );
            if self.web_searches.contains(&key) {
                slots[index] = Some(
                    json!({"query":query,"duplicate":true,"contentAlreadyReturned":true,"hint":"This web search was already returned. Reuse it or materially change the query/filters."}),
                );
            } else {
                pending.push((index, query.clone(), key));
            }
        }
        if !pending.is_empty() {
            let client = &self.web_search;
            let completed = thread::scope(|scope| {
                let handles = pending
                    .iter()
                    .map(|(_, query, _)| {
                        scope.spawn(move || {
                            client.search_cancellable(
                                WebSearchRequest {
                                    query,
                                    max_results,
                                    language,
                                    category,
                                    time_range,
                                    safe_search,
                                    page,
                                    backend,
                                },
                                cancel,
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .map(|handle| {
                        handle
                            .join()
                            .unwrap_or_else(|_| Err(anyhow!("web search worker panicked")))
                    })
                    .collect::<Vec<_>>()
            });
            for ((index, query, key), outcome) in pending.into_iter().zip(completed) {
                match outcome {
                    Ok(value) => {
                        self.web_searches.insert(key);
                        slots[index] = Some(value);
                    }
                    Err(error) => {
                        if cancel.load(std::sync::atomic::Ordering::Acquire)
                            || error.to_string() == "cancelled"
                        {
                            bail!("cancelled");
                        }
                        slots[index] =
                            Some(json!({"query":query,"failed":true,"error":error.to_string()}));
                    }
                }
            }
        }
        let mut results = slots.into_iter().flatten().collect::<Vec<_>>();
        let result = if results.len() == 1 {
            results.pop().unwrap()
        } else {
            json!({"searches":results})
        };
        collect_web_source_urls(&result, &mut self.web_sources);
        self.externalize_if_large(result, 16 * 1024, None)
    }

    /// Reads a full source page only when it was discovered by the current web search task.
    pub(super) fn web_read(
        &mut self,
        object: &Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let url = string_arg(object, "url")?.trim();
        if url.is_empty() {
            bail!("web_read requires a non-empty URL");
        }
        let source_key = canonical_web_source_key(url);
        if !self.web_sources.contains(&source_key) {
            bail!(
                "web_read may only open URLs returned by web_search in the current research task; search for this source first"
            );
        }
        if self.web_reads.contains(&source_key) {
            return Ok(json!({"url":url,"duplicate":true,"contentAlreadyReturned":true,"hint":"This source page was already read. Reuse its prior preview/artifact evidence instead of reopening it."}).to_string());
        }
        let max_chars = usize_arg(object, "maxChars")
            .unwrap_or(3_000)
            .clamp(2_000, 4_000);
        // Fetch enough source text to preserve useful follow-up evidence, but
        // expose only a bounded preview to the model. Larger fetched text is
        // stored once as a session artifact instead of being replayed verbatim
        // through every subsequent agent round.
        let result = self.web_search.read_url(url, 48_000, cancel)?;
        let rendered = self.bound_web_read_evidence(url, result, max_chars)?;
        self.web_reads.insert(source_key);
        Ok(rendered)
    }

    fn bound_web_read_evidence(
        &mut self,
        url: &str,
        mut result: Value,
        max_chars: usize,
    ) -> Result<String> {
        let Some(object) = result.as_object_mut() else {
            return Ok(serde_json::to_string(&result)?);
        };
        let Some(content) = object
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return Ok(serde_json::to_string(&result)?);
        };
        let total_chars = content.chars().count();
        if total_chars <= max_chars {
            return Ok(serde_json::to_string(&result)?);
        }

        let artifact = self.artifacts.store(&content)?;
        let preview = content.chars().take(max_chars).collect::<String>();
        object.insert("content".into(), json!(preview));
        object.insert("artifactId".into(), json!(artifact));
        object.insert("externalized".into(), json!(true));
        object.insert("characters".into(), json!(total_chars));
        object.insert("previewChars".into(), json!(max_chars));
        object.insert("previewTruncated".into(), json!(true));
        object.insert(
            "hint".into(),
            json!("The fetched source text is stored as an artifact. Reuse this preview; use search_artifact and then a narrow read_artifact range only if a specific missing section is needed."),
        );
        object.entry("url").or_insert_with(|| json!(url));
        Ok(serde_json::to_string(&result)?)
    }

    /// Extracts a supported document and stores the full text as a typed artifact.
    pub(super) fn read_document_tool(&mut self, object: &Map<String, Value>) -> Result<String> {
        let path = string_arg(object, "path")?;
        self.ensure_file_scope(path, false)?;
        let resolved = self.resolve_local_path(path);
        let document = general::read_document(&resolved)?;
        let artifact = self.artifacts.store_typed(
            &document.text,
            &document.kind,
            &document.media_type,
            Some(path),
            document.metadata.clone(),
        )?;
        let max_chars = usize_arg(object, "maxChars")
            .unwrap_or(8_000)
            .clamp(1_000, 64_000);
        let total_chars = document.text.chars().count();
        let truncated = total_chars > max_chars;
        let content = if truncated {
            document.text.chars().take(max_chars).collect::<String>()
        } else {
            document.text.clone()
        };
        Ok(json!({
            "path":path,"artifactId":artifact,"artifactKind":document.kind,"mediaType":document.media_type,
            "metadata":document.metadata,"characters":total_chars,"content":content,"truncated":truncated,
            "hint":if truncated { "The full extracted document is stored as a typed artifact. Use search_artifact or read_artifact for additional sections." } else { "The extracted document is also stored as a typed artifact for follow-up inspection." }
        }).to_string())
    }

    /// Runs a bounded data-analysis operation and stores its full result as JSON artifact.
    pub(super) fn analyze_data_tool(&mut self, object: &Map<String, Value>) -> Result<String> {
        let path = string_arg(object, "path")?;
        self.ensure_file_scope(path, false)?;
        let resolved = self.resolve_local_path(path);
        let result = general::analyze_data(&resolved, object)?;
        let rendered = serde_json::to_string_pretty(&result)?;
        let artifact = self.artifacts.store_typed(&rendered, "data-analysis", "application/json", Some(path), json!({
            "operation":object.get("operation").and_then(Value::as_str).unwrap_or("describe"),"sheet":object.get("sheet"),
        }))?;
        let mut payload = result.as_object().cloned().unwrap_or_default();
        payload.insert("path".into(), json!(path));
        payload.insert("artifactId".into(), json!(artifact));
        payload.insert("artifactKind".into(), json!("data-analysis"));
        self.externalize_if_large(Value::Object(payload), 12 * 1024, None)
    }

    /// Resolves a user path relative to the active workspace when needed.
    fn resolve_local_path(&self, path: &str) -> PathBuf {
        let candidate = PathBuf::from(path);
        if candidate.is_absolute() {
            candidate
        } else {
            self.workspace_root.join(candidate)
        }
    }

    /// Executes a helper script from an activated Skill through the normal shell policy.
    pub(super) fn run_skill_script(
        &mut self,
        skill_name: &str,
        object: &Map<String, Value>,
        model: &str,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let relative = string_arg(object, "path")?.trim();
        if !relative.starts_with("scripts/") {
            bail!("Skill script path must be under scripts/");
        }
        let skill = self.bridge.load_skill(skill_name)?;
        if !skill.files.iter().any(|path| path == relative) {
            bail!("Unknown script for Skill {skill_name}: {relative}");
        }
        if Path::new(relative).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            bail!("Skill script path escapes its root: {relative}");
        }
        let script = Path::new(&skill.root).join(relative);
        let mut command_parts = Vec::new();
        command_parts.push(format!(
            "YEET_WORKSPACE_ROOT={}",
            shell_quote(self.workspace_root.to_string_lossy().as_ref())
        ));
        match script
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
        {
            "py" => command_parts.push("python3".to_owned()),
            "sh" | "bash" => command_parts.push("bash".to_owned()),
            "js" | "mjs" | "cjs" => command_parts.push("node".to_owned()),
            _ => {}
        }
        command_parts.push(shell_quote(script.to_string_lossy().as_ref()));
        if let Some(args) = object.get("args").and_then(Value::as_array) {
            for value in args {
                let arg = value
                    .as_str()
                    .ok_or_else(|| anyhow!("Skill script args must all be strings"))?;
                command_parts.push(shell_quote(arg));
            }
        }
        let mut shell_object = Map::new();
        shell_object.insert("command".into(), json!(command_parts.join(" ")));
        shell_object.insert("workingDirectory".into(), json!(skill.root));
        shell_object.insert(
            "purpose".into(),
            json!(format!("Run {relative} from activated Skill {skill_name}.")),
        );
        if let Some(timeout) = object.get("timeoutSeconds") {
            shell_object.insert("timeoutSeconds".into(), timeout.clone());
        }
        self.run_shell_tool(&shell_object, model, cancel)
    }
}

fn root_capable_workspace_path(path: Option<&str>) -> &str {
    match path {
        Some(path) if !path.trim().is_empty() => path,
        _ => ".",
    }
}

fn refresh_matches_cached_coverage(
    entries: &[ReadCacheEntry],
    snapshot: &str,
    start: usize,
    end: usize,
) -> bool {
    if !entries.iter().any(|entry| entry.snapshot == snapshot) {
        return false;
    }
    let covered = covered_ranges_within(entries, start, end);
    uncovered_ranges(start, end, &covered).is_empty()
}

fn effective_web_search_max_results(requested: usize, query_count: usize) -> usize {
    let requested = requested.clamp(1, 8);
    let per_query_budget = (8 / query_count.max(1)).max(1);
    requested.min(per_query_budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batched_web_search_budget_caps_total_result_volume() {
        assert_eq!(effective_web_search_max_results(8, 1), 8);
        assert_eq!(effective_web_search_max_results(8, 2), 4);
        assert_eq!(effective_web_search_max_results(10, 3), 2);
        assert_eq!(effective_web_search_max_results(8, 4), 2);
    }

    #[test]
    fn refresh_reuses_fully_covered_unchanged_snapshot_only() {
        let entries = vec![
            ReadCacheEntry {
                path: "src/lib.rs".into(),
                snapshot: "same".into(),
                start: 1,
                end: 80,
                total: 120,
                anchored: String::new(),
            },
            ReadCacheEntry {
                path: "src/lib.rs".into(),
                snapshot: "same".into(),
                start: 81,
                end: 120,
                total: 120,
                anchored: String::new(),
            },
        ];
        assert!(refresh_matches_cached_coverage(&entries, "same", 20, 100));
        assert!(!refresh_matches_cached_coverage(
            &entries, "changed", 20, 100
        ));
        assert!(!refresh_matches_cached_coverage(
            &entries[..1],
            "same",
            20,
            100
        ));
    }

    #[test]
    fn root_capable_workspace_paths_normalize_empty_root_intent() {
        assert_eq!(root_capable_workspace_path(None), ".");
        assert_eq!(root_capable_workspace_path(Some("")), ".");
        assert_eq!(root_capable_workspace_path(Some("   ")), ".");
        assert_eq!(root_capable_workspace_path(Some("src")), "src");
    }
}
