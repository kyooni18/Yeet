//! Normalization and recovery for model/tool protocol messages.
//!
//! Providers and MCP servers can stream partial calls, embed image resources,
//! or emit malformed textual tool calls. This module converts those variants
//! into Yeet's canonical `ToolCall` and model-visible output forms.

use std::collections::HashMap;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::core::{ImageAttachment, ToolCall};

/// Incrementally assembled tool call received from a streaming provider.
#[derive(Debug, Clone)]
pub(super) struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl PartialToolCall {
    /// Creates an empty partial call using deterministic fallbacks for missing fields.
    pub(super) fn new(_index: usize, id: Option<String>, name: Option<String>) -> Self {
        Self {
            id: id
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("tool-{}", Uuid::new_v4().simple())),
            name: name
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "tool".into()),
            arguments: String::new(),
        }
    }

    /// Applies one streamed update to the partial call.
    pub(super) fn apply(
        &mut self,
        id: Option<String>,
        name: Option<String>,
        delta: Option<String>,
    ) {
        if let Some(id) = id.filter(|value| !value.trim().is_empty()) {
            self.id = id;
        }
        if let Some(name) = name.filter(|value| !value.trim().is_empty()) {
            self.name = name;
        }
        if let Some(delta) = delta {
            self.arguments.push_str(&delta);
        }
    }

    /// Finalizes streamed JSON arguments without fabricating an empty object when
    /// the provider stopped mid-call. Invalid raw arguments are preserved so the
    /// coordinator can repair the model turn instead of executing the wrong call.
    fn finish(self) -> ToolCall {
        let raw = self.arguments.trim();
        let arguments = if raw.is_empty() {
            json!({})
        } else {
            serde_json::from_str(raw).unwrap_or(Value::String(self.arguments))
        };
        ToolCall {
            id: self.id,
            name: self.name,
            arguments,
        }
    }
}

/// Converts structured tool output into compact model text plus supported images.
///
/// Raw tool results remain untouched for execution bookkeeping, evidence, replay, and UI
/// events. This is the model-facing projection boundary: transport metadata, duplicated
/// MCP structured payloads, and opaque cache handles are removed here.
pub(super) fn normalize_tool_output_for_model(
    content: &str,
    vision_enabled: bool,
) -> (String, Vec<ImageAttachment>) {
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return (content.to_owned(), Vec::new());
    };
    let Some(object) = value.as_object() else {
        return (render_model_value(&value), Vec::new());
    };
    let Some(items) = object.get("content").and_then(Value::as_array) else {
        return (render_model_value(&value), Vec::new());
    };

    let mut lines = Vec::new();
    let mut images = Vec::new();
    for item in items {
        let Some(item) = item.as_object() else {
            continue;
        };
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "text" => {
                if let Some(text) = item
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    let rendered = serde_json::from_str::<Value>(text)
                        .map(|value| render_model_value(&value))
                        .unwrap_or_else(|_| text.to_owned());
                    if !rendered.is_empty() {
                        lines.push(rendered);
                    }
                }
            }
            "image" => {
                let media_type = item
                    .get("mimeType")
                    .or_else(|| item.get("mediaType"))
                    .or_else(|| item.get("mime_type"))
                    .and_then(Value::as_str)
                    .unwrap_or("image/png");
                let data = item.get("data").and_then(Value::as_str).unwrap_or_default();
                if vision_enabled && is_supported_image_media_type(media_type) && !data.is_empty() {
                    images.push(ImageAttachment {
                        media_type: media_type.to_owned(),
                        data: data.to_owned(),
                        name: None,
                    });
                    lines.push(format!("[image output: {media_type}]"));
                } else {
                    lines.push(format!("[image output: {media_type}; vision unavailable]"));
                }
            }
            "resource" => {
                let resource = item.get("resource").and_then(Value::as_object);
                if let Some(text) = resource
                    .and_then(|value| value.get("text"))
                    .and_then(Value::as_str)
                {
                    lines.push(text.to_owned());
                } else if let Some(uri) = resource
                    .and_then(|value| value.get("uri"))
                    .and_then(Value::as_str)
                {
                    lines.push(format!("[resource output: {uri}]"));
                }
            }
            other if !other.is_empty() => lines.push(format!("[{other} output]")),
            _ => {}
        }
    }

    // content is the canonical model-facing representation. MCP structuredContent
    // is frequently the same payload wrapped in workspace/transport metadata, so it
    // is only a fallback when the server supplied no usable content at all.
    if lines.is_empty()
        && let Some(structured) = object.get("structuredContent")
    {
        let rendered = render_model_value(structured);
        if !rendered.is_empty() {
            lines.push(rendered);
        }
    }
    if lines.is_empty() && object.get("isError").and_then(Value::as_bool) == Some(true) {
        lines.push("Tool reported an error.".into());
    }

    if lines.is_empty() {
        (render_model_value(&value), images)
    } else {
        (lines.join("\n"), images)
    }
}

/// Produces a semantic model-facing JSON projection without mutating the raw result.
fn project_tool_json_for_model(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(project_tool_json_for_model)
                .filter(|value| !value.is_null())
                .collect(),
        ),
        Value::Object(object) => {
            if let Some(payload) = mcp_structured_payload(object) {
                return project_tool_json_for_model(payload);
            }

            let synchronous_shell = looks_like_synchronous_shell_result(object);
            let mut projected = serde_json::Map::new();
            for (key, value) in object {
                if value.is_null()
                    || matches!(
                        key.as_str(),
                        "_meta"
                            | "resultType"
                            | "route"
                            | "durationMilliseconds"
                            | "stdoutBytes"
                            | "stderrBytes"
                            | "requestedStartLine"
                            | "requestedEndLine"
                            | "refreshReplay"
                            | "snapshot"
                            | "duplicateReadBytesAvoided"
                            | "contentAlreadyReturned"
                            | "fileFullyRead"
                            | "filesScanned"
                            | "anchors"
                            | "supersedesSnapshot"
                            | "observationAgeSeconds"
                            | "updateHint"
                    )
                    || (synchronous_shell
                        && matches!(key.as_str(), "command" | "workingDirectory" | "succeeded"))
                    || (matches!(key.as_str(), "stdoutTruncated" | "stderrTruncated")
                        && value.as_bool() == Some(false))
                    || (key == "data" && value.as_str().is_some_and(|text| text.len() > 512))
                {
                    continue;
                }

                let projected_value = project_tool_json_for_model(value);
                if projected_value.is_null()
                    || projected_value
                        .as_array()
                        .is_some_and(|values| values.is_empty())
                    || projected_value
                        .as_object()
                        .is_some_and(|values| values.is_empty())
                {
                    continue;
                }
                projected.insert(key.clone(), projected_value);
            }
            Value::Object(projected)
        }
        _ => value.clone(),
    }
}

/// Renders the built-in tools' results as plain text: file reads as numbered
/// lines, shell runs as output plus a non-zero exit code, searches as
/// `path:line: text`, edits as diffs, and errors as one line.
fn render_known_result(value: &Value) -> Option<String> {
    if let Value::Array(items) = value {
        let rendered = items
            .iter()
            .map(render_known_result)
            .collect::<Option<Vec<_>>>()?;
        return (!rendered.is_empty()).then(|| rendered.join("\n\n"));
    }
    let object = value.as_object()?;
    let text = |key: &str| object.get(key).and_then(Value::as_str);
    let number = |key: &str| object.get(key).and_then(Value::as_u64);

    if let Some(error) = text("error")
        && object.keys().all(|key| matches!(key.as_str(), "error" | "reason" | "hint"))
    {
        return Some(match text("hint") {
            Some(hint) => format!("Error: {error}\n{hint}"),
            None => format!("Error: {error}"),
        });
    }

    if let (Some(path), Some(lines)) = (text("path"), text("lines")) {
        let mut out = path.to_owned();
        if let (Some(start), Some(end), Some(total)) =
            (number("startLine"), number("endLine"), number("totalLines"))
            && !(start <= 1 && end >= total)
        {
            out.push_str(&format!(" (lines {start}-{end} of {total})"));
        }
        for line in lines.lines() {
            out.push('\n');
            out.push_str(&strip_line_anchor(line));
        }
        if let Some(next) = number("nextStartLine") {
            out.push_str(&format!("\n[more from line {next}]"));
        }
        return Some(out);
    }

    if let Some(code) = object.get("exitCode").and_then(Value::as_i64)
        && !object.contains_key("status")
    {
        let mut out = String::new();
        if let Some(stdout) = text("stdout") {
            out.push_str(stdout.trim_end());
        }
        if let Some(stderr) = text("stderr").filter(|value| !value.trim().is_empty()) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(stderr.trim_end());
        }
        if object.get("stdoutTruncated").and_then(Value::as_bool) == Some(true)
            || object.get("stderrTruncated").and_then(Value::as_bool) == Some(true)
        {
            out.push_str("\n[output truncated]");
        }
        if code != 0 {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("[exit code {code}]"));
        }
        if object.get("timedOut").and_then(Value::as_bool) == Some(true) {
            out.push_str("\n[timed out]");
        }
        return Some(if out.is_empty() { "(no output)".into() } else { out });
    }

    if let Some(matches) = object.get("matches").and_then(Value::as_array) {
        let mut out = matches
            .iter()
            .filter_map(|item| {
                Some(format!(
                    "{}:{}: {}",
                    item.get("path")?.as_str()?,
                    item.get("line")?,
                    item.get("text").and_then(Value::as_str).unwrap_or_default().trim()
                ))
            })
            .collect::<Vec<_>>()
            .join("\n");
        if out.is_empty() {
            out.push_str("No matches.");
        }
        if object.get("truncated").and_then(Value::as_bool) == Some(true) {
            out.push_str("\n[more matches not shown]");
        }
        return Some(out);
    }

    if let Some(files) = object.get("files").and_then(Value::as_array)
        && files.iter().all(|file| file.get("operation").is_some())
    {
        let mut out = Vec::new();
        for file in files {
            let operation = file.get("operation").and_then(Value::as_str).unwrap_or("update");
            let path = file.get("path").and_then(Value::as_str).unwrap_or_default();
            let mut section = match file.get("destination").and_then(Value::as_str) {
                Some(destination) => format!("{operation} {path} -> {destination}"),
                None => format!("{operation} {path}"),
            };
            let hunks = file
                .pointer("/diff/hunks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten();
            for hunk in hunks {
                section.push_str(&format!(
                    "\n@@ -{} +{} @@",
                    hunk.get("oldStart").unwrap_or(&Value::Null),
                    hunk.get("newStart").unwrap_or(&Value::Null)
                ));
                for line in hunk.get("lines").and_then(Value::as_array).into_iter().flatten() {
                    section.push('\n');
                    section.push_str(line.as_str().unwrap_or_default());
                }
            }
            for warning in file.get("warnings").and_then(Value::as_array).into_iter().flatten() {
                section.push_str(&format!("\nwarning: {}", render_inline(warning)));
            }
            out.push(section);
        }
        for diagnostic in object
            .get("diagnostics")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            out.push(format!("diagnostic: {}", render_inline(diagnostic)));
        }
        return Some(out.join("\n"));
    }
    None
}

fn render_inline(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

/// `12:af16|code` becomes `12|code`; edits address lines by number only.
fn strip_line_anchor(line: &str) -> String {
    if let Some((head, rest)) = line.split_once('|')
        && let Some((number, hash)) = head.split_once(':')
        && number.chars().all(|ch| ch.is_ascii_digit())
        && hash.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        return format!("{number}|{rest}");
    }
    line.to_owned()
}

/// Unwraps Yeet's legacy MCP structuredContent envelope when it contains no
/// information beyond the actual result plus transport metadata.
fn mcp_structured_payload<'a>(object: &'a serde_json::Map<String, Value>) -> Option<&'a Value> {
    let result = object.get("result")?;
    object
        .keys()
        .all(|key| {
            matches!(
                key.as_str(),
                "result" | "workspace" | "status" | "resultType" | "_meta"
            )
        })
        .then_some(result)
}

fn looks_like_synchronous_shell_result(object: &serde_json::Map<String, Value>) -> bool {
    !object.contains_key("status")
        && object.contains_key("exitCode")
        && (object.contains_key("stdout")
            || object.contains_key("stderr")
            || object.contains_key("stdoutBytes")
            || object.contains_key("durationMilliseconds")
            || object.contains_key("route"))
}

fn render_model_value(value: &Value) -> String {
    let projected = project_tool_json_for_model(value);
    if let Some(text) = render_known_result(&projected) {
        return text;
    }
    match projected {
        Value::Null => String::new(),
        Value::String(text) => text,
        Value::Object(ref object) if object.is_empty() => String::new(),
        Value::Array(ref values) if values.is_empty() => String::new(),
        other => other.to_string(),
    }
}

/// Returns whether a media type can be passed through the model image interface.
fn is_supported_image_media_type(value: &str) -> bool {
    matches!(
        value,
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    )
}

/// Merges decoded and streamed calls while preserving provider call order.
/// Calls whose arguments are not JSON objects are returned separately so the
/// coordinator can retry them without sending a malformed invocation to tools.
pub(super) fn collect_tool_calls(
    decoded: HashMap<usize, ToolCall>,
    partial: HashMap<usize, PartialToolCall>,
) -> (Vec<ToolCall>, Vec<ToolCall>) {
    let mut all = decoded;
    for (index, partial) in partial {
        all.entry(index).or_insert_with(|| partial.finish());
    }
    let mut indexes: Vec<_> = all.keys().copied().collect();
    indexes.sort_unstable();
    let mut valid = Vec::new();
    let mut malformed = Vec::new();
    for index in indexes {
        let Some(call) = all.remove(&index) else {
            continue;
        };
        if call.arguments.is_object() {
            valid.push(call);
        } else {
            malformed.push(call);
        }
    }
    (valid, malformed)
}

/// Recovers structured calls from providers that emitted textual tool-call markup.
pub(super) fn recover_text_tool_calls(text: &str) -> Vec<ToolCall> {
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    while let Some(relative_start) = text[cursor..].find("<tool_call>") {
        let start = cursor + relative_start + "<tool_call>".len();
        let Some(relative_end) = text[start..].find("</tool_call>") else {
            break;
        };
        let end = start + relative_end;
        blocks.push(&text[start..end]);
        cursor = end + "</tool_call>".len();
    }
    if blocks.is_empty() && text.contains("<function=") {
        blocks.push(text);
    }
    blocks
        .into_iter()
        .filter_map(parse_text_tool_call_block)
        .collect()
}

/// Parses one JSON or XML-like textual tool-call block.
fn parse_text_tool_call_block(block: &str) -> Option<ToolCall> {
    let trimmed = block.trim().trim_matches('`').trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed)
        && let Some(call) = tool_call_from_json(&value)
    {
        return Some(call);
    }

    let function_marker = "<function=";
    let function_start = trimmed.find(function_marker)? + function_marker.len();
    let function_name_end = function_start + trimmed[function_start..].find('>')?;
    let name = trimmed[function_start..function_name_end].trim();
    if name.is_empty() {
        return None;
    }
    let body_start = function_name_end + 1;
    let body_end = trimmed[body_start..]
        .find("</function>")
        .map(|index| body_start + index)
        .unwrap_or(trimmed.len());
    let body = &trimmed[body_start..body_end];
    let mut arguments = serde_json::Map::new();
    let mut cursor = 0usize;
    while let Some(relative_start) = body[cursor..].find("<parameter=") {
        let parameter_start = cursor + relative_start + "<parameter=".len();
        let Some(relative_name_end) = body[parameter_start..].find('>') else {
            break;
        };
        let parameter_name_end = parameter_start + relative_name_end;
        let parameter_name = body[parameter_start..parameter_name_end].trim();
        if parameter_name.is_empty() {
            break;
        }
        let value_start = parameter_name_end + 1;
        let Some(relative_value_end) = body[value_start..].find("</parameter>") else {
            break;
        };
        let value_end = value_start + relative_value_end;
        let raw = body[value_start..value_end].trim();
        let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()));
        arguments.insert(parameter_name.to_owned(), value);
        cursor = value_end + "</parameter>".len();
    }
    if arguments.is_empty()
        && let Ok(Value::Object(object)) = serde_json::from_str::<Value>(body.trim())
    {
        arguments = object;
    }
    Some(ToolCall {
        id: format!("recovered-{}", Uuid::new_v4().simple()),
        name: name.to_owned(),
        arguments: Value::Object(arguments),
    })
}

/// Converts common provider JSON call shapes into Yeet's canonical call type.
fn tool_call_from_json(value: &Value) -> Option<ToolCall> {
    let object = value.as_object()?;
    let function = object.get("function").and_then(Value::as_object);
    let name = object.get("name").and_then(Value::as_str).or_else(|| {
        function
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
    })?;
    let raw_arguments = object
        .get("arguments")
        .or_else(|| function.and_then(|value| value.get("arguments")))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let arguments = match raw_arguments {
        Value::String(raw) => {
            let parsed = serde_json::from_str::<Value>(&raw).ok()?;
            parsed.is_object().then_some(parsed)?
        }
        other if other.is_object() => other,
        _ => return None,
    };
    Some(ToolCall {
        id: object
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("recovered-{}", Uuid::new_v4().simple())),
        name: name.to_owned(),
        arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_stream_call_ids_are_unique_across_rounds() {
        let first = PartialToolCall::new(0, None, Some("lookup".into())).finish();
        let second = PartialToolCall::new(0, None, Some("lookup".into())).finish();
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn mcp_content_is_not_repeated_from_structured_content() {
        let payload = json!({
            "content": [{
                "type": "text",
                "text": "{\"value\":42,\"durationMilliseconds\":8}"
            }],
            "structuredContent": {
                "workspace": "/tmp/project",
                "result": {"value":42,"durationMilliseconds":8}
            },
            "isError": false
        })
        .to_string();

        let (text, images) = normalize_tool_output_for_model(&payload, false);

        assert!(images.is_empty());
        assert_eq!(text, "{\"value\":42}");
        assert!(!text.contains("Structured output"));
        assert!(!text.contains("/tmp/project"));
    }

    #[test]
    fn builtin_results_render_as_compact_text() {
        let render = |value: Value| normalize_tool_output_for_model(&value.to_string(), false).0;
        assert_eq!(
            render(json!({
                "route": "actor", "command": "printf ok", "workingDirectory": "/tmp",
                "exitCode": 0, "succeeded": true, "durationMilliseconds": 12,
                "stdoutBytes": 3, "stdoutTruncated": false, "stdout": "ok\n", "stderr": null
            })),
            "ok"
        );
        assert_eq!(render(json!({"exitCode": 2, "stdout": null, "stderr": "boom\n"})), "boom\n[exit code 2]");
        assert_eq!(
            render(json!({
                "content": [],
                "structuredContent": {"workspace": "/tmp", "result": {
                    "path": "src/main.rs", "snapshot": "opaque", "requestedStartLine": 1,
                    "startLine": 1, "endLine": 1, "totalLines": 9, "nextStartLine": 2,
                    "lines": "1:abcd|fn main() {}"
                }},
                "isError": false
            })),
            "src/main.rs (lines 1-1 of 9)\n1|fn main() {}\n[more from line 2]"
        );
        assert_eq!(
            render(json!({"filesScanned": 3, "truncated": false, "matches": [{"path": "a.rs", "line": 2, "text": "  hit"}]})),
            "a.rs:2: hit"
        );
        assert_eq!(render(json!({"error": "nope", "reason": "io"})), "Error: nope");
    }
}
