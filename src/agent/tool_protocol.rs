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
    pub(super) fn new(index: usize, id: Option<String>, name: Option<String>) -> Self {
        Self {
            id: id.unwrap_or_else(|| format!("tool-{index}")),
            name: name.unwrap_or_else(|| "tool".into()),
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
        if let Some(id) = id.filter(|value| !value.is_empty()) {
            self.id = id;
        }
        if let Some(name) = name.filter(|value| !value.is_empty()) {
            self.name = name;
        }
        if let Some(delta) = delta {
            self.arguments.push_str(&delta);
        }
    }

    /// Finalizes streamed JSON arguments into a canonical tool call.
    fn finish(self) -> ToolCall {
        ToolCall {
            id: self.id,
            name: self.name,
            arguments: serde_json::from_str(&self.arguments).unwrap_or_else(|_| json!({})),
        }
    }
}

/// Converts structured MCP output into compact model text plus supported images.
pub(super) fn normalize_tool_output_for_model(
    content: &str,
    vision_enabled: bool,
) -> (String, Vec<ImageAttachment>) {
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return (content.to_owned(), Vec::new());
    };
    let Some(object) = value.as_object() else {
        return (content.to_owned(), Vec::new());
    };
    let Some(items) = object.get("content").and_then(Value::as_array) else {
        return (content.to_owned(), Vec::new());
    };

    let structured = object.get("structuredContent");
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
                    let parsed = serde_json::from_str::<Value>(text).ok();
                    if parsed
                        .as_ref()
                        .is_some_and(|value| Some(value) == structured)
                    {
                        continue;
                    }
                    lines.push(match parsed {
                        Some(value @ (Value::Object(_) | Value::Array(_))) => value.to_string(),
                        _ => text.to_owned(),
                    });
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

    if let Some(structured) = structured {
        lines.push(format!("Structured output: {structured}"));
    }
    if object.get("isError").and_then(Value::as_bool) == Some(true) {
        lines.push("Tool reported an error.".into());
    }
    if lines.is_empty() {
        (sanitize_tool_json_for_model(&value).to_string(), images)
    } else {
        (lines.join("\n"), images)
    }
}

/// Removes large binary payloads before returning generic structured output to a model.
fn sanitize_tool_json_for_model(value: &Value) -> Value {
    match value {
        Value::Array(values) => {
            Value::Array(values.iter().map(sanitize_tool_json_for_model).collect())
        }
        Value::Object(object) => {
            let mut sanitized = serde_json::Map::new();
            for (key, value) in object {
                if key == "data" && value.as_str().is_some_and(|text| text.len() > 512) {
                    sanitized.insert(key.clone(), Value::String("[binary data omitted]".into()));
                } else {
                    sanitized.insert(key.clone(), sanitize_tool_json_for_model(value));
                }
            }
            Value::Object(sanitized)
        }
        _ => value.clone(),
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
pub(super) fn collect_tool_calls(
    decoded: HashMap<usize, ToolCall>,
    partial: HashMap<usize, PartialToolCall>,
) -> Vec<ToolCall> {
    let mut all = decoded;
    for (index, partial) in partial {
        all.entry(index).or_insert_with(|| partial.finish());
    }
    let mut indexes: Vec<_> = all.keys().copied().collect();
    indexes.sort_unstable();
    indexes
        .into_iter()
        .filter_map(|index| all.remove(&index))
        .collect()
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
        Value::String(raw) => serde_json::from_str::<Value>(&raw).unwrap_or_else(|_| json!({})),
        other => other,
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

/// Detects provider text that looks like an unexecuted textual tool call.
pub(super) fn looks_like_malformed_tool_call(text: &str) -> bool {
    let value = text.trim().to_ascii_lowercase();
    value.starts_with("<tool_call")
        || value.starts_with("<function=")
        || value.starts_with("```tool_call")
        || value.starts_with("<|tool_call|>")
        || (value.contains("<tool_call>") && value.contains("<function="))
}
