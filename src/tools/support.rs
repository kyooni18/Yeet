//! Shared state and helper routines used by the tool registry coordinator.

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
};
use tempfile::TempDir;
use uuid::Uuid;

use super::{BUILTIN_CAPABILITIES, BuiltinCapabilityDescriptor, artifact_output};
use crate::edit::ReadResult;

#[derive(Debug, Clone)]
pub(super) struct ReadCacheEntry {
    pub(super) path: String,
    pub(super) snapshot: String,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) total: usize,
    pub(super) anchored: String,
}

pub(super) struct ArtifactStore {
    pub(super) _directory: TempDir,
    pub(super) root: PathBuf,
}

impl ArtifactStore {
    pub(super) fn new() -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("yeet-artifacts-")
            .tempdir()?;
        let root = directory.path().to_path_buf();
        Ok(Self {
            _directory: directory,
            root,
        })
    }

    pub(super) fn store(&self, content: &str) -> Result<String> {
        self.store_typed(content, "tool-output", "text/plain", None, json!({}))
    }

    pub(super) fn store_typed(
        &self,
        content: &str,
        kind: &str,
        media_type: &str,
        source: Option<&str>,
        metadata: Value,
    ) -> Result<String> {
        fs::create_dir_all(&self.root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))?;
        }
        let id = Uuid::new_v4().to_string();
        fs::write(self.root.join(format!("{id}.txt")), content)?;
        fs::write(
            self.root.join(format!("{id}.meta.json")),
            serde_json::to_vec(&json!({
                "id": id,
                "kind": kind,
                "mediaType": media_type,
                "source": source,
                "characters": content.chars().count(),
                "lines": content.lines().count(),
                "metadata": metadata,
            }))?,
        )?;
        Ok(id)
    }

    pub(super) fn info(&self, id: &str) -> Result<Value> {
        Uuid::parse_str(id)?;
        let path = self.root.join(format!("{id}.meta.json"));
        if !path.is_file() {
            if self.path(id).is_file() {
                return Ok(json!({"id": id, "kind": "tool-output", "mediaType": "text/plain"}));
            }
            bail!("unknown artifact: {id}");
        }
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    pub(super) fn read(
        &self,
        id: &str,
        start: Option<usize>,
        end: Option<usize>,
    ) -> Result<String> {
        Uuid::parse_str(id)?;
        let text = fs::read_to_string(self.path(id))?;
        let lines: Vec<_> = text.lines().collect();
        let start = start.unwrap_or(1);
        let end = end.unwrap_or_else(|| (start + 159).min(lines.len()));
        if start == 0 || start > lines.len() || end < start || end > lines.len() {
            bail!(
                "artifact range {start}-{end} is out of bounds (1-{})",
                lines.len()
            );
        }
        Ok(lines[start - 1..end].join("\n"))
    }

    pub(super) fn search(&self, id: &str, query: &str, max_results: usize) -> Result<Value> {
        Uuid::parse_str(id)?;
        let text = fs::read_to_string(self.path(id))?;
        let mut matches = Vec::new();
        for (index, line) in text.lines().enumerate() {
            if let Some(position) = line.find(query) {
                if matches.len() >= max_results {
                    return Ok(json!({"artifactId": id, "matches": matches, "truncated": true}));
                }
                matches.push(artifact_output::search_match(index + 1, line, position));
            }
        }
        Ok(json!({"artifactId": id, "matches": matches, "truncated": false}))
    }

    pub(super) fn path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.txt"))
    }
}

pub(super) fn collect_web_source_urls(value: &Value, output: &mut HashSet<String>) {
    // Tool responses are external input. Walk them iteratively so a malicious or
    // unexpectedly deep JSON tree cannot overflow the MCP HTTP worker stack.
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(object) => {
                if let Some(url) = object.get("url").and_then(Value::as_str) {
                    let url = url.trim();
                    if url.starts_with("https://") || url.starts_with("http://") {
                        output.insert(canonical_web_source_key(url));
                    }
                }
                pending.extend(object.values());
            }
            Value::Array(values) => pending.extend(values),
            _ => {}
        }
    }
}

#[cfg(test)]
mod web_source_url_tests {
    use super::*;

    #[test]
    fn deeply_nested_web_results_do_not_recurse_on_the_worker_stack() {
        let worker = std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(|| {
                let mut value = json!({"url":"https://example.com/source"});
                for _ in 0..2_000 {
                    value = Value::Array(vec![value]);
                }
                let mut urls = HashSet::new();
                collect_web_source_urls(&value, &mut urls);
                assert!(urls.contains("https://example.com/source"));

                // serde_json::Value itself drops recursively. Leak this synthetic
                // adversarial value so the test measures our walker rather than
                // serde_json's destructor on the deliberately tiny stack.
                std::mem::forget(value);
            })
            .expect("spawn tiny-stack worker");
        worker.join().expect("deep JSON walk should not overflow");
    }
}

pub(crate) fn canonical_web_source_key(value: &str) -> String {
    let value = value.trim();
    let Ok(mut parsed) = url::Url::parse(value) else {
        return value.to_owned();
    };
    parsed.set_fragment(None);
    parsed.to_string()
}

pub(super) fn web_search_queries(object: &Map<String, Value>) -> Result<Vec<String>> {
    let mut queries = Vec::new();
    if let Some(query) = object
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        queries.push(query.to_owned());
    }
    if let Some(values) = object.get("queries").and_then(Value::as_array) {
        if values.len() > 4 {
            bail!("web_search accepts at most 4 batched queries");
        }
        for value in values {
            let query = value
                .as_str()
                .ok_or_else(|| anyhow!("web_search queries must contain strings"))?
                .trim();
            if query.is_empty() {
                bail!("web_search queries must not contain empty strings");
            }
            if !queries
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(query))
            {
                queries.push(query.to_owned());
            }
        }
    }
    if queries.is_empty() {
        bail!("web_search requires query or queries");
    }
    if queries.len() > 4 {
        bail!("web_search accepts at most 4 total queries");
    }
    Ok(queries)
}

pub(super) fn builtin_capability_for_tool(
    tool_name: &str,
) -> Option<&'static BuiltinCapabilityDescriptor> {
    BUILTIN_CAPABILITIES
        .iter()
        .find(|capability| capability.tools.contains(&tool_name))
}

pub(super) fn string_arg<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing string argument {key}"))
}

#[cfg(test)]
pub(super) fn foundation_wrapper_schema(schema: &Map<String, Value>) -> Map<String, Value> {
    let mut schema = schema.clone();
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.remove("project");
    }
    if let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|value| value.as_str() != Some("project"));
    }
    schema
}

pub(super) fn foundation_tool_result_text(value: &Value) -> String {
    if let Some(text) = value
        .get("content")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find_map(|item| {
                (item.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| item.get("text").and_then(Value::as_str))
                    .flatten()
            })
        })
    {
        return text.to_owned();
    }
    value.to_string()
}

pub(super) fn foundation_context_from_tool_result(value: &Value) -> Option<String> {
    if value.get("isError").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let text = foundation_tool_result_text(value);
    let payload: Value = serde_json::from_str(&text).ok()?;
    let context = payload.get("context")?.as_str()?.trim();
    (!context.is_empty()).then(|| context.to_owned())
}

pub(super) fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".into();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn usize_arg(object: &Map<String, Value>, key: &str) -> Option<usize> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
}

pub(super) fn append_bounded_state_set(
    lines: &mut Vec<String>,
    label: &str,
    values: &HashSet<String>,
    limit: usize,
) {
    if values.is_empty() {
        return;
    }
    let mut values = values.iter().cloned().collect::<Vec<_>>();
    values.sort();
    let omitted = values.len().saturating_sub(limit);
    let mut rendered = values
        .into_iter()
        .take(limit)
        .map(|value| truncate_state_value(&value, 180))
        .collect::<Vec<_>>();
    if omitted > 0 {
        rendered.push(format!("... +{omitted}"));
    }
    lines.push(format!("{label}: {}", rendered.join(", ")));
}

pub(super) fn truncate_state_value(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    value
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>()
        + "…"
}

pub(super) fn merged_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut()
            && start <= last.1 + 1
        {
            last.1 = last.1.max(end);
            continue;
        }
        merged.push((start, end));
    }
    merged
}

pub(super) fn covered_ranges_within(
    entries: &[ReadCacheEntry],
    start: usize,
    end: usize,
) -> Vec<(usize, usize)> {
    if end < start {
        return Vec::new();
    }
    merged_ranges(
        entries
            .iter()
            .filter_map(|entry| {
                let overlap_start = entry.start.max(start);
                let overlap_end = entry.end.min(end);
                (overlap_start <= overlap_end).then_some((overlap_start, overlap_end))
            })
            .collect(),
    )
}

pub(super) fn uncovered_ranges(
    start: usize,
    end: usize,
    covered: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    if end < start {
        return Vec::new();
    }
    let mut missing = Vec::new();
    let mut cursor = start;
    for &(covered_start, covered_end) in covered {
        if covered_end < cursor {
            continue;
        }
        if covered_start > cursor {
            missing.push((cursor, covered_start - 1));
        }
        cursor = cursor.max(covered_end.saturating_add(1));
        if cursor > end {
            break;
        }
    }
    if cursor <= end {
        missing.push((cursor, end));
    }
    missing
}

pub(super) fn cache_read_result(
    cache: &mut HashMap<String, Vec<ReadCacheEntry>>,
    read: ReadResult,
) -> ReadCacheEntry {
    let entry = ReadCacheEntry {
        path: read.path.clone(),
        snapshot: read.snapshot,
        start: read.start_line,
        end: read.end_line,
        total: read.total_lines,
        anchored: read.anchored,
    };
    let entries = cache.entry(entry.path.clone()).or_default();
    entries.retain(|cached| cached.snapshot == entry.snapshot);
    entries.push(entry.clone());
    entry
}

pub(super) fn coverage_complete(total: usize, entries: &[ReadCacheEntry]) -> bool {
    let ranges = merged_ranges(
        entries
            .iter()
            .map(|entry| (entry.start, entry.end))
            .collect(),
    );
    ranges.first().is_some_and(|first| first.0 == 1)
        && ranges.last().is_some_and(|last| last.1 >= total)
        && ranges.windows(2).all(|pair| pair[1].0 <= pair[0].1 + 1)
}

pub(super) fn next_uncovered(total: usize, entries: &[ReadCacheEntry]) -> Option<usize> {
    let ranges = merged_ranges(
        entries
            .iter()
            .map(|entry| (entry.start, entry.end))
            .collect(),
    );
    let mut next = 1;
    for (start, end) in ranges {
        if start > next {
            return Some(next);
        }
        next = next.max(end + 1);
    }
    (next <= total).then_some(next)
}

pub(super) fn allocate_tool_name<'a>(
    prefix: &str,
    parts: &[&str],
    occupied: impl Iterator<Item = &'a String>,
) -> String {
    let occupied: HashSet<String> = occupied.cloned().collect();
    let sanitize = |value: &str| {
        let mut result = value
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect::<String>();
        while result.contains("__") {
            result = result.replace("__", "_");
        }
        result.trim_matches('_').to_owned()
    };
    let base = format!(
        "{}_{}",
        prefix,
        parts
            .iter()
            .map(|part| sanitize(part))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("_")
    );
    if !occupied.contains(&base) {
        return base;
    }
    for index in 2..1000 {
        let candidate = format!("{base}_{index}");
        if !occupied.contains(&candidate) {
            return candidate;
        }
    }
    format!("{base}_{}", Uuid::new_v4().simple())
}
