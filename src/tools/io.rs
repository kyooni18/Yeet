//! Built-in workspace, web, document, data, and Skill-script tool handlers.
//!
//! These handlers translate validated tool arguments into concrete reads and
//! bounded artifacts while reusing the registry's shared caches and sandbox.

use super::*;

use std::fs;

use regex::RegexBuilder;
use sha2::{Digest, Sha256};

use crate::edit::{ListFilesResult, ReadResult, SearchMatch, SearchResult, WorkspaceEntry};

const WORKSPACE_IGNORED_DIRECTORIES: &[&str] = &[
    ".git",
    ".yeet",
    ".transactions",
    ".build",
    ".swiftpm",
    ".cache",
    ".next",
    ".venv",
    ".astro",
    ".turbo",
    ".vite",
    "node_modules",
    "dist",
    "build",
    "built",
    "out",
    "coverage",
    "DerivedData",
    "Pods",
    "target",
    "vendor",
    "venv",
];
const WORKSPACE_SOURCE_DIRECTORIES: &[&str] = &[
    "src",
    "source",
    "sources",
    "packages",
    "tests",
    "test",
    "runtimesource",
];
const WORKSPACE_SEARCH_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const WORKSPACE_SEARCH_MAX_FILES: usize = 10_000;

fn normalize_edit_text(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw.as_ref());
    raw.replace("\r\n", "\n").replace('\r', "\n")
}

fn addressable_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines = text.split('\n').collect::<Vec<_>>();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

fn local_snapshot_handle(text: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(text.as_bytes());
    format!("r_{:x}", digest.finalize())
}

pub(super) fn is_local_snapshot(snapshot: &str) -> bool {
    snapshot.starts_with("r_")
}

pub(super) fn local_snapshot_handle_for_path(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(local_snapshot_handle(&normalize_edit_text(&bytes)))
}

fn anchored_line(line_number: usize, line: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(line.as_bytes());
    let hash = format!("{:x}", digest.finalize());
    format!("{line_number}:{}|{line}", &hash[..4])
}

pub(super) fn read_file_in_process(
    path: &Path,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<ReadResult> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let text = normalize_edit_text(&bytes);
    let lines = addressable_lines(&text);
    let snapshot = local_snapshot_handle(&text);
    if lines.is_empty() {
        return Ok(ReadResult {
            path: path.to_string_lossy().into_owned(),
            snapshot,
            start_line: 1,
            end_line: 0,
            total_lines: 0,
            content: String::new(),
            numbered: String::new(),
            anchored: String::new(),
        });
    }

    let start_line = start_line.unwrap_or(1);
    let requested_end = end_line.unwrap_or_else(|| start_line.saturating_add(159));
    if start_line < 1 || start_line > lines.len() || requested_end < start_line {
        bail!(
            "Invalid read range {start_line}..{requested_end}; file has {} lines.",
            lines.len()
        );
    }
    let end_line = requested_end.min(lines.len());
    let selected = &lines[start_line - 1..end_line];
    let content = selected.join("\n");
    let numbered = selected
        .iter()
        .enumerate()
        .map(|(offset, line)| format!("{}:{line}", start_line + offset))
        .collect::<Vec<_>>()
        .join("\n");
    let anchored = selected
        .iter()
        .enumerate()
        .map(|(offset, line)| anchored_line(start_line + offset, line))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ReadResult {
        path: path.to_string_lossy().into_owned(),
        snapshot,
        start_line,
        end_line,
        total_lines: lines.len(),
        content,
        numbered,
        anchored,
    })
}

pub(super) fn ignored_workspace_directory(name: &str) -> bool {
    WORKSPACE_IGNORED_DIRECTORIES.contains(&name)
}

fn requested_path_hits_ignored_directory(path: &str) -> bool {
    path.split(['/', '\\']).any(ignored_workspace_directory)
}

fn workspace_entry_priority(name: &str) -> u8 {
    let normalized = name.to_ascii_lowercase();
    if WORKSPACE_SOURCE_DIRECTORIES.contains(&normalized.as_str()) {
        0
    } else if matches!(normalized.as_str(), "docs" | "examples") {
        1
    } else {
        2
    }
}

fn sorted_directory_entries(path: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(path)
        .with_context(|| format!("read directory {}", path.display()))?
        .filter_map(std::result::Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        let left_name = left.file_name().to_string_lossy().into_owned();
        let right_name = right.file_name().to_string_lossy().into_owned();
        workspace_entry_priority(&left_name)
            .cmp(&workspace_entry_priority(&right_name))
            .then_with(|| left_name.cmp(&right_name))
    });
    Ok(entries)
}

fn stable_display_path(root: &Path, path: &Path) -> Result<String> {
    super::paths::stable_workspace_path_key(root, &path.to_string_lossy())
}

fn list_files_in_process(
    root: &Path,
    start: &Path,
    max_results: usize,
    max_depth: usize,
) -> Result<ListFilesResult> {
    #[allow(clippy::too_many_arguments)] // Recursive traversal carries bounded accumulator state.
    fn walk(
        root: &Path,
        path: &Path,
        depth: usize,
        max_depth: usize,
        max_results: usize,
        entries: &mut Vec<WorkspaceEntry>,
        result_limit_reached: &mut bool,
        depth_limited: &mut bool,
    ) -> Result<()> {
        if *result_limit_reached {
            return Ok(());
        }
        let metadata =
            fs::symlink_metadata(path).with_context(|| format!("inspect {}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Ok(());
        }
        if metadata.is_file() {
            if entries.len() >= max_results {
                *result_limit_reached = true;
            } else {
                entries.push(WorkspaceEntry {
                    path: stable_display_path(root, path)?,
                    kind: "file".into(),
                });
            }
            return Ok(());
        }
        if !metadata.is_dir() {
            return Ok(());
        }

        for entry in sorted_directory_entries(path)? {
            if *result_limit_reached {
                break;
            }
            let file_type = match entry.file_type() {
                Ok(value) => value,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if file_type.is_dir() && ignored_workspace_directory(&name) {
                continue;
            }
            let child = entry.path();
            if entries.len() >= max_results {
                *result_limit_reached = true;
                break;
            }
            entries.push(WorkspaceEntry {
                path: stable_display_path(root, &child)?,
                kind: if file_type.is_dir() {
                    "directory"
                } else {
                    "file"
                }
                .into(),
            });
            if file_type.is_dir() {
                if depth < max_depth {
                    walk(
                        root,
                        &child,
                        depth + 1,
                        max_depth,
                        max_results,
                        entries,
                        result_limit_reached,
                        depth_limited,
                    )?;
                } else if fs::read_dir(&child)
                    .ok()
                    .and_then(|mut entries| entries.next())
                    .is_some()
                {
                    *depth_limited = true;
                }
            }
        }
        Ok(())
    }

    let mut entries = Vec::new();
    let mut result_limit_reached = false;
    let mut depth_limited = false;
    walk(
        root,
        start,
        0,
        max_depth,
        max_results,
        &mut entries,
        &mut result_limit_reached,
        &mut depth_limited,
    )?;
    Ok(ListFilesResult {
        entries,
        truncated: result_limit_reached || depth_limited,
        result_limit_reached,
        depth_limited,
    })
}

fn search_workspace_in_process(
    root: &Path,
    start: &Path,
    query: &str,
    max_results: usize,
    case_sensitive: bool,
    regex: bool,
) -> Result<SearchResult> {
    let expression = if regex {
        if query.len() > 1024 {
            bail!("Search regex is too long (maximum 1024 characters).");
        }
        Some(
            RegexBuilder::new(query)
                .case_insensitive(!case_sensitive)
                .build()
                .map_err(|error| anyhow!("Invalid search regex: {error}"))?,
        )
    } else {
        None
    };
    let needle = if case_sensitive {
        query.to_owned()
    } else {
        query.to_lowercase()
    };
    let mut matches = Vec::new();
    let mut files_scanned = 0usize;
    let mut truncated = false;

    #[allow(clippy::too_many_arguments)] // Recursive traversal carries bounded accumulator state.
    fn walk(
        root: &Path,
        path: &Path,
        expression: Option<&regex::Regex>,
        needle: &str,
        case_sensitive: bool,
        max_results: usize,
        matches: &mut Vec<SearchMatch>,
        files_scanned: &mut usize,
        truncated: &mut bool,
    ) -> Result<()> {
        if *truncated {
            return Ok(());
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(value) => value,
            Err(_) => return Ok(()),
        };
        if metadata.file_type().is_symlink() {
            return Ok(());
        }
        if metadata.is_file() {
            if *files_scanned >= WORKSPACE_SEARCH_MAX_FILES || matches.len() >= max_results {
                *truncated = true;
                return Ok(());
            }
            if metadata.len() > WORKSPACE_SEARCH_MAX_FILE_BYTES {
                return Ok(());
            }
            *files_scanned += 1;
            let bytes = match fs::read(path) {
                Ok(value) => value,
                Err(_) => return Ok(()),
            };
            if bytes.contains(&0) {
                return Ok(());
            }
            let text = normalize_edit_text(&bytes);
            for (index, line) in text.split('\n').enumerate() {
                let matched = if let Some(expression) = expression {
                    expression.is_match(line)
                } else if case_sensitive {
                    line.contains(needle)
                } else {
                    line.to_lowercase().contains(needle)
                };
                if !matched {
                    continue;
                }
                let text = if line.chars().count() <= 320 {
                    line.to_owned()
                } else {
                    format!("{}...", line.chars().take(317).collect::<String>())
                };
                matches.push(SearchMatch {
                    path: stable_display_path(root, path)?,
                    line: index + 1,
                    text,
                });
                if matches.len() >= max_results {
                    *truncated = true;
                    break;
                }
            }
            return Ok(());
        }
        if !metadata.is_dir() {
            return Ok(());
        }
        for entry in sorted_directory_entries(path)? {
            if *truncated {
                break;
            }
            let file_type = match entry.file_type() {
                Ok(value) => value,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if file_type.is_dir() && ignored_workspace_directory(&name) {
                continue;
            }
            walk(
                root,
                &entry.path(),
                expression,
                needle,
                case_sensitive,
                max_results,
                matches,
                files_scanned,
                truncated,
            )?;
        }
        Ok(())
    }

    walk(
        root,
        start,
        expression.as_ref(),
        &needle,
        case_sensitive,
        max_results,
        &mut matches,
        &mut files_scanned,
        &mut truncated,
    )?;
    matches.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.line.cmp(&right.line))
    });
    Ok(SearchResult {
        matches,
        files_scanned,
        truncated,
    })
}

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

        let requested_path = string_arg(object, "path")?.to_owned();
        let path = self
            .resolve_session_path(&requested_path)?
            .to_string_lossy()
            .into_owned();
        self.ensure_file_scope(&path, false)?;
        let cache_path = super::paths::stable_workspace_path_key(&self.workspace_root, &path)?;
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
        // Other sessions and external tools may mutate a previously read file.
        // Verify its content before suppressing an apparently duplicate read.
        if !refresh
            && let Some(cached) = self
                .read_cache
                .get(&cache_path)
                .and_then(|entries| entries.first())
            && local_snapshot_handle_for_path(Path::new(&path))? != cached.snapshot
        {
            self.invalidate_workspace_cache_for_paths(&HashSet::from([cache_path.clone()]));
        }
        let cached_before = self
            .read_cache
            .get(&cache_path)
            .cloned()
            .unwrap_or_default();
        let cached_total = cached_before.first().map(|entry| entry.total);
        let bounded_end = cached_total.map_or(requested_end, |total| requested_end.min(total));
        let covered_before = covered_ranges_within(&cached_before, requested_start, bounded_end);

        if refresh {
            let mut read =
                read_file_in_process(Path::new(&path), Some(requested_start), Some(requested_end))?;
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
            let result = self.externalize_if_large(payload, threshold, Some(&read))?;
            if result.contains("\"externalized\":true") {
                trim_cache_to_preview(&mut self.read_cache, &cache_path, &result);
            }
            self.record_edit_read_coverage(&cache_path, &read.snapshot);
            return Ok(result);
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
            let mut read = read_file_in_process(Path::new(&path), Some(start), Some(end))?;
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
            let result = self.externalize_if_large(payload, threshold, None)?;
            if result.contains("\"externalized\":true") {
                trim_cache_to_preview(&mut self.read_cache, &cache_path, &result);
            }
            self.record_edit_read_coverage(&cache_path, &entry.snapshot);
            return Ok(result);
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
        let result = self.externalize_if_large(payload, threshold, None)?;
        if result.contains("\"externalized\":true") {
            trim_cache_to_preview(&mut self.read_cache, &cache_path, &result);
        }
        if let Some(snapshot) = new_entries.last().map(|entry| entry.snapshot.clone()) {
            self.record_edit_read_coverage(&cache_path, &snapshot);
        }
        Ok(result)
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
        let requested_path =
            root_capable_workspace_path(object.get("path").and_then(Value::as_str));
        if requested_path_hits_ignored_directory(requested_path) {
            bail!("List path points into generated or internal workspace state.");
        }
        let path = self
            .resolve_session_path(requested_path)?
            .to_string_lossy()
            .into_owned();
        self.ensure_file_scope(&path, false)?;
        let cache_path = super::paths::stable_workspace_path_key(&self.workspace_root, &path)?;
        let max_results = usize_arg(object, "maxResults").unwrap_or(50).clamp(1, 500);
        let max_depth = usize_arg(object, "maxDepth").unwrap_or(2).min(12);
        let key = format!("{cache_path}:{max_results}:{max_depth}");
        if !self.listings.insert(key) {
            return Ok(json!({"duplicate":true,"contentAlreadyReturned":true,"hint":"This directory listing was already returned. Reuse it or expand a different subtree."}).to_string());
        }
        let result = list_files_in_process(
            &self.workspace_root,
            Path::new(&path),
            max_results,
            max_depth,
        )?;
        self.externalize_if_large(serde_json::to_value(result)?, 16 * 1024, None)
    }

    /// Searches workspace text with bounded results and duplicate-query suppression.
    pub(super) fn search_workspace(&mut self, object: &Map<String, Value>) -> Result<String> {
        let query = string_arg(object, "query")?.trim();
        if query.is_empty() {
            bail!("search_workspace requires query");
        }
        let requested_path =
            root_capable_workspace_path(object.get("path").and_then(Value::as_str));
        if requested_path_hits_ignored_directory(requested_path) {
            bail!("Search path points into generated or internal workspace state.");
        }
        let path = self
            .resolve_session_path(requested_path)?
            .to_string_lossy()
            .into_owned();
        self.ensure_file_scope(&path, false)?;
        let cache_path = super::paths::stable_workspace_path_key(&self.workspace_root, &path)?;
        let max_results = usize_arg(object, "maxResults").unwrap_or(20).clamp(1, 100);
        let case_sensitive = object
            .get("caseSensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let regex = object
            .get("regex")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let key =
            workspace_search_cache_key(&cache_path, max_results, case_sensitive, regex, query);
        if !self.searches.insert(key) {
            return Ok(json!({"duplicate":true,"contentAlreadyReturned":true,"hint":"This search was already returned. Reuse it or change query/path."}).to_string());
        }
        let result = search_workspace_in_process(
            &self.workspace_root,
            Path::new(&path),
            query,
            max_results,
            case_sensitive,
            regex,
        )?;
        self.externalize_if_large(serde_json::to_value(result)?, 16 * 1024, None)
    }

    /// Executes one or more web searches concurrently and records discovered source URLs.
    pub(super) fn web_search(
        &mut self,
        object: &Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<String> {
        if self.web_backend == ServiceBackend::Mcp {
            return self.web_search_mcp(object, cancel);
        }
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

    fn web_search_mcp(
        &mut self,
        object: &Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let server = self
            .web_server
            .clone()
            .ok_or_else(|| anyhow!("Web MCP server is not configured"))?;
        if self.capability_disabled("mcp", &server) {
            bail!("MCP server {server} is disabled for this session");
        }
        let result = self.bridge_client()?.call_mcp_tool_cancellable(
            &server,
            "web_search",
            object,
            cancel,
        )?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            bail!(
                "Web MCP {server}/web_search: {}",
                foundation_tool_result_text(&result)
            );
        }
        collect_web_source_urls(&result, &mut self.web_sources);
        let text = foundation_tool_result_text(&result);
        if let Ok(payload) = serde_json::from_str::<Value>(&text) {
            collect_web_source_urls(&payload, &mut self.web_sources);
            return self.externalize_if_large(payload, 16 * 1024, None);
        }
        Ok(text)
    }

    /// Reads a full source page only when it was discovered by the current web search task.
    pub(super) fn web_read(
        &mut self,
        object: &Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<String> {
        if self.web_backend == ServiceBackend::Mcp {
            return self.web_read_mcp(object, cancel);
        }
        let url = string_arg(object, "url")?.trim();
        if url.is_empty() {
            bail!("web_read requires a non-empty URL");
        }
        let source_key = canonical_web_source_key(url);
        if !self.web_sources.contains(&source_key) {
            let message = if self.artifacts_enabled {
                "web_read may only open URLs returned by web_search in the current research task; search for this source first"
            } else {
                "web action=read may only open URLs returned by web action=search in the current research task; search for this source first"
            };
            bail!("{message}");
        }
        if self.web_reads.contains(&source_key) {
            return Ok(json!({"url":url,"duplicate":true,"contentAlreadyReturned":true,"hint":"This source page was already read. Reuse the evidence already returned instead of reopening it."}).to_string());
        }
        let max_chars = usize_arg(object, "maxChars")
            .unwrap_or(3_000)
            .clamp(2_000, 4_000);
        // Fetch enough source text to preserve useful evidence while returning a
        // bounded preview. Interactive sessions may retain the remainder as an
        // artifact; direct MCP runtimes disable artifact storage.
        let result = self.web_search.read_url(url, 48_000, cancel)?;
        let rendered = self.bound_web_read_evidence(url, result, max_chars)?;
        self.web_reads.insert(source_key);
        Ok(rendered)
    }

    fn web_read_mcp(&mut self, object: &Map<String, Value>, cancel: &AtomicBool) -> Result<String> {
        let url = string_arg(object, "url")?.trim();
        if url.is_empty() {
            bail!("web_read requires a non-empty URL");
        }
        crate::web_search::validate_public_http_url(url)?;
        let source_key = canonical_web_source_key(url);
        if !self.web_sources.contains(&source_key) {
            let message = if self.artifacts_enabled {
                "web_read may only open URLs returned by web_search in the current research task; search for this source first"
            } else {
                "web action=read may only open URLs returned by web action=search in the current research task; search for this source first"
            };
            bail!("{message}");
        }
        if self.web_reads.contains(&source_key) {
            return Ok(json!({"url":url,"duplicate":true,"contentAlreadyReturned":true,"hint":"This source page was already read. Reuse the evidence already returned instead of reopening it."}).to_string());
        }
        let server = self
            .web_server
            .clone()
            .ok_or_else(|| anyhow!("Web MCP server is not configured"))?;
        if self.capability_disabled("mcp", &server) {
            bail!("MCP server {server} is disabled for this session");
        }
        let result = self
            .bridge_client()?
            .call_mcp_tool_cancellable(&server, "web_read", object, cancel)?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            bail!(
                "Web MCP {server}/web_read: {}",
                foundation_tool_result_text(&result)
            );
        }
        let text = foundation_tool_result_text(&result);
        let max_chars = usize_arg(object, "maxChars")
            .unwrap_or(3_000)
            .clamp(2_000, 4_000);
        let rendered = if let Ok(payload) = serde_json::from_str::<Value>(&text) {
            self.bound_web_read_evidence(url, payload, max_chars)?
        } else {
            let total_chars = text.chars().count();
            if total_chars <= max_chars {
                text
            } else {
                let preview = text.chars().take(max_chars).collect::<String>();
                if self.artifacts_enabled {
                    let artifact = self.artifacts.store(&text)?;
                    json!({
                        "url": url,
                        "content": preview,
                        "artifactId": artifact,
                        "externalized": true,
                        "characters": total_chars,
                        "previewChars": max_chars,
                        "previewTruncated": true
                    })
                    .to_string()
                } else {
                    json!({
                        "url": url,
                        "content": preview,
                        "characters": total_chars,
                        "previewChars": max_chars,
                        "previewTruncated": true
                    })
                    .to_string()
                }
            }
        };
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

        let preview = content.chars().take(max_chars).collect::<String>();
        object.insert("content".into(), json!(preview));
        object.insert("characters".into(), json!(total_chars));
        object.insert("previewChars".into(), json!(max_chars));
        object.insert("previewTruncated".into(), json!(true));
        if self.artifacts_enabled {
            let artifact = self.artifacts.store(&content)?;
            object.insert("artifactId".into(), json!(artifact));
            object.insert("externalized".into(), json!(true));
            object.insert(
                "hint".into(),
                json!("The fetched source text is stored as an artifact. Reuse this preview; retrieve the artifact only for a specific missing section."),
            );
        }
        object.entry("url").or_insert_with(|| json!(url));
        Ok(serde_json::to_string(&result)?)
    }

    /// Extracts a supported document and returns bounded readable content.
    pub(super) fn read_document_tool(&mut self, object: &Map<String, Value>) -> Result<String> {
        let path = string_arg(object, "path")?;
        self.ensure_file_scope(path, false)?;
        let resolved = self.resolve_local_path(path)?;
        let document = general::read_document(&resolved)?;
        let artifact = if self.artifacts_enabled {
            Some(self.artifacts.store_typed(
                &document.text,
                &document.kind,
                &document.media_type,
                Some(path),
                document.metadata.clone(),
            )?)
        } else {
            None
        };
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
        let mut payload = json!({
            "path":path,"artifactKind":document.kind,"mediaType":document.media_type,
            "metadata":document.metadata,"characters":total_chars,"content":content,"truncated":truncated
        });
        if let Some(artifact) = artifact
            && let Some(object) = payload.as_object_mut()
        {
            object.insert("artifactId".into(), json!(artifact));
        }
        Ok(payload.to_string())
    }

    /// Runs a bounded data-analysis operation.
    pub(super) fn analyze_data_tool(&mut self, object: &Map<String, Value>) -> Result<String> {
        let path = string_arg(object, "path")?;
        self.ensure_file_scope(path, false)?;
        let resolved = self.resolve_local_path(path)?;
        let result = general::analyze_data(&resolved, object)?;
        let mut payload = result.as_object().cloned().unwrap_or_default();
        payload.insert("path".into(), json!(path));
        if self.artifacts_enabled {
            let rendered = serde_json::to_string_pretty(&result)?;
            let artifact = self.artifacts.store_typed(&rendered, "data-analysis", "application/json", Some(path), json!({
                "operation":object.get("operation").and_then(Value::as_str).unwrap_or("describe"),"sheet":object.get("sheet"),
            }))?;
            payload.insert("artifactId".into(), json!(artifact));
            payload.insert("artifactKind".into(), json!("data-analysis"));
        }
        self.externalize_if_large(Value::Object(payload), 12 * 1024, None)
    }

    /// Resolves a user path relative to the persisted session cwd.
    fn resolve_local_path(&self, path: &str) -> Result<PathBuf> {
        self.resolve_session_path(path)
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
        let skill = self.bridge_client()?.load_skill(skill_name)?;
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

fn workspace_search_cache_key(
    cache_path: &str,
    max_results: usize,
    case_sensitive: bool,
    regex: bool,
    query: &str,
) -> String {
    let query = if case_sensitive {
        query.to_owned()
    } else {
        query.to_ascii_lowercase()
    };
    format!("{cache_path}:{max_results}:{case_sensitive}:{regex}:{query}")
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
mod tests;
