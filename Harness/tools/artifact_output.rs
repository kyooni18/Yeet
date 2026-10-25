//! Bounds artifact text sent to the model, including single-line data files.

use anyhow::Result;
use serde_json::{Value, json};

use super::support::ArtifactStore;

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
    // Source reads also store their bare numbered lines so read_artifact
    // ranges line up with the file rather than the JSON envelope.
    let (artifact_id, artifact_lines) = match logical_source_lines(tool_name, &content) {
        Some(lines) => (
            artifacts.store_typed(
                &lines,
                "tool-output-lines",
                "text/plain",
                Some(tool_name),
                json!({"tool":tool_name,"exactArtifactId":exact_artifact_id}),
            )?,
            lines.lines().count(),
        ),
        None => (exact_artifact_id, content.lines().count()),
    };
    let rendered =
        render_externalized_tool_output(&artifact_id, &content, max_visible_bytes, artifact_lines);
    Ok((rendered, true))
}

fn render_externalized_tool_output(
    artifact_id: &str,
    content: &str,
    max_visible_bytes: usize,
    artifact_lines: usize,
) -> String {
    let note = format!(
        "[output truncated; full {artifact_lines} lines in artifact {artifact_id}, use read_artifact to see more]"
    );
    let room = max_visible_bytes.saturating_sub(note.len() + 1);
    if room == 0 {
        return if note.len() <= max_visible_bytes {
            note
        } else {
            String::new()
        };
    }
    let preview = bounded_preview(content, room.min(MODEL_VISIBLE_TOOL_OUTPUT_PREVIEW_BYTES));
    format!("{preview}\n{note}")
}

/// Cuts at a line boundary within `max_bytes` when one exists.
fn bounded_preview(content: &str, max_bytes: usize) -> &str {
    if content.len() <= max_bytes {
        return content;
    }
    let mut end = max_bytes;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    let head = &content[..end];
    head.rfind('\n').map_or(head, |index| &head[..index])
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
