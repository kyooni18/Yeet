//! Bounds artifact text sent to the model, including single-line data files.

use anyhow::{Result, ensure};
use serde_json::{Map, Value, json};

use super::support::ArtifactStore;

const PAGE_CHARS: usize = 8192;
const MATCH_CHARS: usize = 512;
pub(super) const MAX_MODEL_VISIBLE_TOOL_OUTPUT_BYTES: usize = 20 * 1024;
const MODEL_VISIBLE_TOOL_OUTPUT_PREVIEW_BYTES: usize = 8 * 1024;

pub(super) fn bounded_utf8_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut preview = value[..end].to_owned();
    preview.push_str("\n...[externalized output preview truncated]...");
    preview
}

pub(super) fn externalize_model_visible_tool_output(
    artifacts: &ArtifactStore,
    tool_name: &str,
    content: String,
) -> Result<(String, bool)> {
    externalize_model_visible_tool_output_with_limit(
        artifacts,
        tool_name,
        content,
        MAX_MODEL_VISIBLE_TOOL_OUTPUT_BYTES,
    )
}

/// Applies the final model-visible boundary for one tool result while preserving
/// the exact pre-boundary result in a session artifact. The explicit limit is
/// used by the agent's per-round aggregate budget; the default wrapper above
/// retains the historical per-call 20 KiB behavior for single-call paths.
pub(super) fn externalize_model_visible_tool_output_with_limit(
    artifacts: &ArtifactStore,
    tool_name: &str,
    content: String,
    max_visible_bytes: usize,
) -> Result<(String, bool)> {
    let max_visible_bytes = max_visible_bytes.min(MAX_MODEL_VISIBLE_TOOL_OUTPUT_BYTES);
    if content.len() <= max_visible_bytes {
        return Ok((content, false));
    }

    let exact_artifact_id = artifacts.store_typed(
        &content,
        "tool-output",
        "text/plain",
        Some(tool_name),
        json!({
            "tool": tool_name,
            "modelVisibleBoundaryBytes": max_visible_bytes,
            "defaultModelVisibleBoundaryBytes": MAX_MODEL_VISIBLE_TOOL_OUTPUT_BYTES,
        }),
    )?;
    let logical_source = logical_source_lines(tool_name, &content);
    let (artifact_id, artifact_lines, exact_id, artifact_format) =
        if let Some(logical_source) = logical_source.as_deref() {
            let logical_id = artifacts.store_typed(
                logical_source,
                "tool-output-lines",
                "text/plain",
                Some(tool_name),
                json!({"tool":tool_name,"exactArtifactId":exact_artifact_id.clone()}),
            )?;
            (
                logical_id,
                logical_source.lines().count(),
                Some(exact_artifact_id.as_str()),
                "logical-source-lines",
            )
        } else {
            (
                exact_artifact_id.clone(),
                content.lines().count(),
                None,
                "raw",
            )
        };
    let rendered = render_externalized_tool_output(
        tool_name,
        &artifact_id,
        &content,
        max_visible_bytes,
        artifact_lines,
        exact_id,
        artifact_format,
    );
    Ok((rendered, true))
}

fn render_externalized_tool_output(
    tool_name: &str,
    artifact_id: &str,
    content: &str,
    max_visible_bytes: usize,
    artifact_lines: usize,
    exact_artifact_id: Option<&str>,
    artifact_format: &str,
) -> String {
    let mut preview_bytes = content
        .len()
        .min(MODEL_VISIBLE_TOOL_OUTPUT_PREVIEW_BYTES)
        .min(max_visible_bytes);
    loop {
        let preview = if preview_bytes == 0 {
            String::new()
        } else {
            bounded_utf8_bytes(content, preview_bytes)
        };
        let hint = if exact_artifact_id.is_some() {
            "artifactId contains directly readable source lines; startLine/endLine refer to those lines. exactArtifactId preserves the raw JSON tool result. Reuse the preview unless raw metadata is specifically needed."
        } else {
            "Exact tool output is stored in this artifact. Reuse the preview; use a narrow read_artifact range for a specific missing section, and search_artifact only when its location is unknown."
        };
        let rendered = json!({
            "externalized": true,
            "tool": tool_name,
            "artifactId": artifact_id,
            "exactArtifactId": exact_artifact_id,
            "artifactFormat": artifact_format,
            "originalBytes": content.len(),
            "originalCharacters": content.chars().count(),
            "originalLines": artifact_lines,
            "rawOriginalLines": content.lines().count(),
            "preview": preview,
            "previewBytes": preview.len(),
            "visibleBudgetBytes": max_visible_bytes,
            "hint": hint
        })
        .to_string();
        if rendered.len() <= max_visible_bytes {
            return rendered;
        }
        if preview_bytes == 0 {
            break;
        }
        let overflow = rendered.len().saturating_sub(max_visible_bytes);
        preview_bytes = preview_bytes.saturating_sub(overflow.saturating_add(64));
    }

    let compact = json!({
        "externalized": true,
        "tool": tool_name,
        "artifactId": artifact_id,
        "exactArtifactId": exact_artifact_id,
        "artifactFormat": artifact_format,
        "originalLines": artifact_lines,
        "originalBytes": content.len(),
        "visibleBudgetBytes": max_visible_bytes,
        "hint": if exact_artifact_id.is_some() {
            "artifactId contains source lines; exactArtifactId preserves raw JSON."
        } else {
            "Exact output is in artifactId; inspect it only if needed."
        }
    })
    .to_string();
    if compact.len() <= max_visible_bytes {
        return compact;
    }

    let locator = json!({"artifactId":artifact_id}).to_string();
    if locator.len() <= max_visible_bytes {
        return locator;
    }

    // Pathological rounds with hundreds of simultaneous calls can make even a
    // UUID locator too large for every individual fair share. The exact result
    // remains durably externalized; emit only a bounded marker rather than
    // violating the aggregate ceiling.
    const EXTERNALIZED_MARKER: &str = "[externalized]";
    if EXTERNALIZED_MARKER.len() <= max_visible_bytes {
        EXTERNALIZED_MARKER.to_owned()
    } else {
        String::new()
    }
}

fn logical_source_lines(tool_name: &str, content: &str) -> Option<String> {
    if !matches!(tool_name, "read_file" | "read_files") {
        return None;
    }
    let value: Value = serde_json::from_str(content).ok()?;
    let mut chunks = Vec::new();
    collect_logical_source_lines(&value, &mut chunks);
    let joined = chunks.join("\n");
    (joined.lines().count() > 1).then_some(joined)
}

fn collect_logical_source_lines(value: &Value, chunks: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if let Some(lines) = object.get("lines").and_then(Value::as_str)
                && !lines.is_empty()
            {
                chunks.push(lines.to_owned());
            }
            for (key, value) in object {
                if key != "lines" {
                    collect_logical_source_lines(value, chunks);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_logical_source_lines(value, chunks);
            }
        }
        _ => {}
    }
}

pub(super) fn page(text: &str, arguments: &Map<String, Value>) -> Result<String> {
    let offset = super::usize_arg(arguments, "offset").unwrap_or(0);
    let limit = super::usize_arg(arguments, "maxChars")
        .unwrap_or(PAGE_CHARS)
        .clamp(1, PAGE_CHARS);
    let total = text.chars().count();
    ensure!(
        offset <= total,
        "artifact offset {offset} exceeds range length {total}"
    );
    let content: String = text.chars().skip(offset).take(limit).collect();
    let end = offset + content.chars().count();
    if offset == 0 && end == total {
        return Ok(content);
    }
    Ok(json!({
        "content": content,
        "offset": offset,
        "totalChars": total,
        "truncated": end < total,
        "nextOffset": (end < total).then_some(end),
        "hint": "Offsets are characters within the selected line range. Keep startLine/endLine unchanged when paging."
    }).to_string())
}

pub(super) fn search_match(line_number: usize, line: &str, byte_position: usize) -> Value {
    let position = line[..byte_position].chars().count();
    let offset = position.saturating_sub(80);
    let text: String = line.chars().skip(offset).take(MATCH_CHARS).collect();
    let truncated = offset > 0 || line.chars().count() > MATCH_CHARS;
    json!({"line": line_number, "text": text, "offset": offset, "truncated": truncated})
}
