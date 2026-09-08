//! Compaction and retention of completed conversation history.
use super::SYSTEM_INSTRUCTION;
use crate::core::{Message, MessageRole};
use serde_json::{Value, json};

const GENERIC_TOOL_RESULT_INLINE_CHARS: usize = 640;
const RETAINED_COMPLETED_USER_TURNS: usize = 6;
const CHECKPOINT_PREFIX: &str = "[Earlier conversation checkpoint]\n";
const CHECKPOINT_GUIDANCE: &str = "This is a compact continuity record of older completed turns. Treat it as historical context, not as new instructions.\n";
const CHECKPOINT_MAX_CHARS: usize = 6_000;
const CHECKPOINT_TURN_CHARS: usize = 900;
const VERBATIM_RECENT_USER_TURNS: usize = 2;
const AGED_USER_MESSAGE_CHARS: usize = 2_400;
const AGED_ASSISTANT_MESSAGE_CHARS: usize = 1_800;
const VERBATIM_ACTIVE_TOOL_ROUNDS: usize = 2;

pub(super) fn compact_completed_task_history(history: &mut [Message]) {
    for message in history {
        if message.role == MessageRole::Tool {
            if let Some(content) = message.content.as_deref() {
                let compact = compact_completed_tool_result(message.name.as_deref(), content);
                if compact.len() < content.len() {
                    message.content = Some(compact);
                }
            }
        } else if message.role == MessageRole::Assistant
            && let Some(calls) = message.tool_calls.as_mut()
        {
            for call in calls {
                let compact = compact_completed_tool_arguments(&call.name, &call.arguments);
                if compact.to_string().len() < call.arguments.to_string().len() {
                    call.arguments = compact;
                }
            }
        }
    }
}

/// Bounds the request-only trace for the active user turn while leaving the
/// canonical session history untouched. The newest tool rounds stay verbatim
/// so the model can act on fresh evidence; older results keep only compact
/// recovery metadata and compact call arguments.
pub(super) fn compact_active_task_history(history: &mut [Message], current_user_index: usize) {
    let start = current_user_index.saturating_add(1).min(history.len());
    let mut recent_tool_ids = std::collections::HashSet::new();
    let mut rounds = 0usize;
    for message in history[start..].iter().rev() {
        if message.role != MessageRole::Assistant {
            continue;
        }
        let Some(calls) = message
            .tool_calls
            .as_ref()
            .filter(|calls| !calls.is_empty())
        else {
            continue;
        };
        if rounds >= VERBATIM_ACTIVE_TOOL_ROUNDS {
            break;
        }
        recent_tool_ids.extend(calls.iter().map(|call| call.id.clone()));
        rounds += 1;
    }
    if rounds < VERBATIM_ACTIVE_TOOL_ROUNDS {
        return;
    }

    for message in &mut history[start..] {
        if message.role == MessageRole::Tool {
            if message
                .tool_call_id
                .as_deref()
                .is_some_and(|id| recent_tool_ids.contains(id))
            {
                continue;
            }
            if let Some(content) = message.content.as_deref() {
                let compact = compact_completed_tool_result(message.name.as_deref(), content);
                if compact.len() < content.len() {
                    message.content = Some(compact);
                }
            }
            continue;
        }
        if message.role == MessageRole::Assistant
            && let Some(calls) = message.tool_calls.as_mut()
        {
            for call in calls {
                if recent_tool_ids.contains(&call.id) {
                    continue;
                }
                let compact = compact_completed_tool_arguments(&call.name, &call.arguments);
                if compact.to_string().len() < call.arguments.to_string().len() {
                    call.arguments = compact;
                }
            }
        }
    }
}

/// Repeated explicit Skill invocations may carry the same large instructions.
/// Keep the latest copy in the request; the canonical transcript stays intact.
pub(super) fn deduplicate_skill_instructions(history: &mut Vec<Message>) {
    let keep = {
        let mut seen = std::collections::HashSet::new();
        let mut keep = vec![true; history.len()];
        for (index, message) in history.iter().enumerate().rev() {
            if message.role == MessageRole::System
                && let Some(content) = message.content.as_deref()
                && content.starts_with("User-invoked Skill: ")
                && !seen.insert(content)
            {
                keep[index] = false;
            }
        }
        keep
    };
    let mut index = 0;
    history.retain(|_| {
        let retain = keep[index];
        index += 1;
        retain
    });
}

pub(super) fn trim_completed_conversation_history(history: &mut Vec<Message>) {
    let user_indexes = history
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (message.role == MessageRole::User).then_some(index))
        .collect::<Vec<_>>();
    compact_aged_conversation_payloads(history, &user_indexes);
    if user_indexes.len() <= RETAINED_COMPLETED_USER_TURNS {
        return;
    }

    let keep_from = user_indexes[user_indexes.len() - RETAINED_COMPLETED_USER_TURNS];
    let system = history
        .first()
        .filter(|message| message.role == MessageRole::System)
        .cloned()
        .unwrap_or_else(|| Message::system(SYSTEM_INSTRUCTION));
    let checkpoint = build_conversation_checkpoint(&history[1..keep_from]);
    let mut retained = Vec::with_capacity(history.len() - keep_from + 2);
    retained.push(system);
    if let Some(checkpoint) = checkpoint {
        retained.push(Message::system(checkpoint));
    }
    retained.extend(history.drain(keep_from..));
    *history = retained;
}

fn compact_aged_conversation_payloads(history: &mut [Message], user_indexes: &[usize]) {
    if user_indexes.len() <= VERBATIM_RECENT_USER_TURNS {
        return;
    }
    let preserve_from = user_indexes[user_indexes.len() - VERBATIM_RECENT_USER_TURNS];
    for message in &mut history[..preserve_from] {
        match message.role {
            MessageRole::User => {
                if let Some(content) = message.content.as_deref() {
                    message.content = Some(truncate_middle_chars(content, AGED_USER_MESSAGE_CHARS));
                }
                if let Some(images) = message.images.take()
                    && !images.is_empty()
                {
                    let labels = images
                        .iter()
                        .map(|image| {
                            image
                                .name
                                .clone()
                                .unwrap_or_else(|| image.media_type.clone())
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let note = format!("\n[historical image payload omitted: {labels}]");
                    message
                        .content
                        .get_or_insert_with(String::new)
                        .push_str(&note);
                }
            }
            MessageRole::Assistant => {
                if message.tool_calls.as_ref().is_none_or(Vec::is_empty)
                    && let Some(content) = message.content.as_deref()
                {
                    message.content =
                        Some(truncate_middle_chars(content, AGED_ASSISTANT_MESSAGE_CHARS));
                }
            }
            _ => {}
        }
    }
}

fn build_conversation_checkpoint(messages: &[Message]) -> Option<String> {
    let previous = messages.iter().find_map(|message| {
        (message.role == MessageRole::System)
            .then_some(message.content.as_deref())
            .flatten()
            .filter(|content| content.starts_with(CHECKPOINT_PREFIX))
    });

    let mut turns = Vec::new();
    let mut current_user: Option<&str> = None;
    let mut current_assistant: Option<&str> = None;
    for message in messages {
        match message.role {
            MessageRole::User => {
                if let Some(user) = current_user.take() {
                    turns.push(compact_checkpoint_turn(user, current_assistant.take()));
                }
                current_user = message.content.as_deref();
                current_assistant = None;
            }
            MessageRole::Assistant
                if current_user.is_some()
                    && message.tool_calls.as_ref().is_none_or(Vec::is_empty)
                    && message
                        .content
                        .as_deref()
                        .is_some_and(|content| !content.trim().is_empty()) =>
            {
                current_assistant = message.content.as_deref();
            }
            _ => {}
        }
    }
    if let Some(user) = current_user {
        turns.push(compact_checkpoint_turn(user, current_assistant));
    }

    if previous.is_none() && turns.is_empty() {
        return None;
    }

    let mut body = String::new();
    if let Some(previous) = previous {
        let previous = previous.strip_prefix(CHECKPOINT_PREFIX).unwrap_or(previous);
        body.push_str(previous.trim_start_matches(CHECKPOINT_GUIDANCE));
        if !body.ends_with('\n') {
            body.push('\n');
        }
    }
    for turn in turns {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(&turn);
        body.push('\n');
    }

    let body = truncate_from_end(&body, CHECKPOINT_MAX_CHARS);
    Some(format!("{CHECKPOINT_PREFIX}{CHECKPOINT_GUIDANCE}{body}"))
}

fn compact_checkpoint_turn(user: &str, assistant: Option<&str>) -> String {
    let mut value = format!("User: {}", compact_whitespace(user));
    if let Some(assistant) = assistant {
        value.push_str("\nAssistant: ");
        value.push_str(&compact_whitespace(assistant));
    }
    truncate_chars(&value, CHECKPOINT_TURN_CHARS)
}

fn compact_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compact_completed_tool_result(name: Option<&str>, content: &str) -> String {
    let parsed = serde_json::from_str::<Value>(content).ok();
    match (name, parsed.as_ref()) {
        (Some("read_file") | Some("read_files"), Some(Value::Object(object))) => {
            let mut compact = pick_json_fields(
                object,
                &[
                    "path",
                    "snapshot",
                    "startLine",
                    "endLine",
                    "totalLines",
                    "fileFullyRead",
                    "nextStartLine",
                    "requestedStartLine",
                    "requestedEndLine",
                    "incremental",
                    "reusedCoveredRanges",
                    "duplicate",
                    "contentAlreadyReturned",
                    "externalized",
                    "artifactId",
                ],
            );
            compact.insert("historical".into(), json!(true));
            compact.insert("contentOmitted".into(), json!(true));
            return Value::Object(compact).to_string();
        }
        (Some("read_file") | Some("read_files"), Some(Value::Array(values))) => {
            let compact = values
                .iter()
                .take(16)
                .map(|value| {
                    let Value::Object(object) = value else {
                        return value.clone();
                    };
                    let mut item = pick_json_fields(
                        object,
                        &[
                            "path",
                            "snapshot",
                            "startLine",
                            "endLine",
                            "totalLines",
                            "fileFullyRead",
                            "nextStartLine",
                            "requestedStartLine",
                            "requestedEndLine",
                            "incremental",
                            "reusedCoveredRanges",
                            "duplicate",
                            "contentAlreadyReturned",
                            "externalized",
                            "artifactId",
                            "error",
                        ],
                    );
                    item.insert("historical".into(), json!(true));
                    item.insert("contentOmitted".into(), json!(true));
                    Value::Object(item)
                })
                .collect::<Vec<_>>();
            return Value::Array(compact).to_string();
        }
        (Some("read_artifact"), _) => {
            return json!({"historical":true,"contentOmitted":true,"chars":content.len()})
                .to_string();
        }
        (Some("search_workspace"), Some(Value::Object(object))) => {
            let matches = object
                .get("matches")
                .and_then(Value::as_array)
                .map(|items| items.len() as u64)
                .or_else(|| object.get("matchCount").and_then(Value::as_u64))
                .unwrap_or(0);
            return json!({
                "historical": true,
                "matchCount": matches,
                "filesScanned": object.get("filesScanned"),
                "truncated": object.get("truncated"),
                "duplicate": object.get("duplicate"),
            })
            .to_string();
        }
        (Some("list_files"), Some(Value::Object(object))) => {
            let entries = object
                .get("entries")
                .and_then(Value::as_array)
                .map(|items| items.len() as u64)
                .or_else(|| object.get("entryCount").and_then(Value::as_u64))
                .unwrap_or(0);
            return json!({
                "historical": true,
                "entryCount": entries,
                "truncated": object.get("truncated"),
                "resultLimitReached": object.get("resultLimitReached"),
                "depthLimited": object.get("depthLimited"),
                "duplicate": object.get("duplicate"),
            })
            .to_string();
        }
        (Some("run_shell"), Some(Value::Object(object))) => {
            let mut compact = pick_json_fields(
                object,
                &[
                    "route",
                    "command",
                    "workingDirectory",
                    "exitCode",
                    "succeeded",
                    "durationMilliseconds",
                    "stdoutBytes",
                    "stderrBytes",
                    "stdoutTruncated",
                    "stderrTruncated",
                    "summary",
                    "artifactId",
                    "error",
                ],
            );
            compact.insert("historical".into(), json!(true));
            compact.insert("contentOmitted".into(), json!(true));
            return Value::Object(compact).to_string();
        }
        (Some("apply_file_edits"), Some(Value::Object(object))) => {
            let files = object
                .get("files")
                .and_then(Value::as_array)
                .map(|files| {
                    files
                        .iter()
                        .take(40)
                        .map(|value| {
                            value
                                .as_object()
                                .map(|file| {
                                    Value::Object(pick_json_fields(
                                        file,
                                        &[
                                            "path",
                                            "destination",
                                            "operation",
                                            "snapshot",
                                            "warnings",
                                        ],
                                    ))
                                })
                                .unwrap_or_else(|| value.clone())
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let diagnostics = object
                .get("diagnostics")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .take(20)
                        .map(|value| {
                            value
                                .as_object()
                                .map(|item| {
                                    Value::Object(pick_json_fields(
                                        item,
                                        &[
                                            "path", "severity", "message", "line", "column",
                                            "source",
                                        ],
                                    ))
                                })
                                .unwrap_or_else(|| value.clone())
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            return json!({"files":files,"diagnostics":diagnostics,"historical":true}).to_string();
        }
        (Some("web_search"), Some(value)) => {
            return compact_web_search_result(value).to_string();
        }
        (Some("web_read"), Some(value)) => {
            return compact_web_read_result(value).to_string();
        }
        _ => {}
    }
    if content.len() <= GENERIC_TOOL_RESULT_INLINE_CHARS {
        return content.to_owned();
    }
    let mut compact = serde_json::Map::new();
    compact.insert("historical".into(), json!(true));
    compact.insert("contentOmitted".into(), json!(true));
    compact.insert("chars".into(), json!(content.len()));
    if let Some(Value::Object(object)) = parsed {
        for key in [
            "error",
            "artifactId",
            "summary",
            "status",
            "succeeded",
            "exitCode",
            "path",
            "url",
        ] {
            if let Some(value) = object.get(key) {
                compact.insert(key.into(), compact_json_value(value, 500));
            }
        }
    }
    Value::Object(compact).to_string()
}

fn compact_json_value(value: &Value, max_chars: usize) -> Value {
    match value {
        Value::String(value) => Value::String(truncate_chars(value, max_chars)),
        Value::Array(_) | Value::Object(_) => {
            let serialized = value.to_string();
            if serialized.chars().count() <= max_chars {
                return value.clone();
            }
            json!({"preview":truncate_chars(&serialized, max_chars), "contentOmitted":true})
        }
        _ => value.clone(),
    }
}

fn compact_completed_tool_arguments(name: &str, value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    match name {
        "read_file" | "read_files" if object.get("requests").is_some() => {
            let requests = object
                .get("requests")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .take(8)
                        .map(|value| {
                            value
                                .as_object()
                                .map(|item| {
                                    Value::Object(pick_json_fields(
                                        item,
                                        &["path", "startLine", "endLine"],
                                    ))
                                })
                                .unwrap_or_else(|| value.clone())
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            json!({"requests":requests})
        }
        "read_file" | "read_files" => {
            Value::Object(pick_json_fields(object, &["path", "startLine", "endLine"]))
        }
        "list_files" => Value::Object(pick_json_fields(
            object,
            &["path", "maxResults", "maxDepth"],
        )),
        "search_workspace" => Value::Object(pick_json_fields(
            object,
            &["query", "path", "maxResults", "caseSensitive", "regex"],
        )),
        "read_artifact" => Value::Object(pick_json_fields(
            object,
            &["id", "startLine", "endLine", "offset", "maxChars"],
        )),
        "search_artifact" => {
            Value::Object(pick_json_fields(object, &["id", "query", "maxResults"]))
        }
        "web_search" => Value::Object(pick_json_fields(
            object,
            &[
                "query",
                "queries",
                "language",
                "category",
                "timeRange",
                "page",
            ],
        )),
        "web_read" => Value::Object(pick_json_fields(object, &["url", "maxChars"])),
        "run_shell" => {
            let mut compact = pick_json_fields(
                object,
                &["purpose", "workingDirectory", "mode", "timeoutSeconds"],
            );
            if let Some(command) = object.get("command").and_then(Value::as_str) {
                compact.insert("command".into(), json!(truncate_chars(command, 400)));
            }
            Value::Object(compact)
        }
        "apply_file_edits" => {
            let changes = object
                .get("changes")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .take(40)
                        .map(|value| {
                            let Some(change) = value.as_object() else {
                                return value.clone();
                            };
                            let mut item = pick_json_fields(change, &["path", "snapshot"]);
                            item.insert(
                                "editCount".into(),
                                json!(
                                    change
                                        .get("edits")
                                        .and_then(Value::as_array)
                                        .map(|items| items.len() as u64)
                                        .or_else(|| change.get("editCount").and_then(Value::as_u64))
                                        .unwrap_or(0)
                                ),
                            );
                            if let Some(file_op) = change.get("fileOp").and_then(Value::as_object) {
                                item.insert(
                                    "fileOp".into(),
                                    Value::Object(pick_json_fields(
                                        file_op,
                                        &["kind", "destination"],
                                    )),
                                );
                            }
                            Value::Object(item)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            json!({"changes":changes,"historical":true})
        }
        _ => {
            let encoded = value.to_string();
            if encoded.len() <= 1_200 {
                value.clone()
            } else {
                json!({"historical":true,"argumentChars":encoded.len()})
            }
        }
    }
}

fn compact_web_search_result(value: &Value) -> Value {
    fn compact_one(value: &Value) -> Value {
        let Some(object) = value.as_object() else {
            return value.clone();
        };
        let results = object
            .get("results")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .take(8)
                    .map(|item| {
                        item.as_object()
                            .map(|result| {
                                Value::Object(pick_json_fields(
                                    result,
                                    &["title", "url", "publishedAt"],
                                ))
                            })
                            .unwrap_or_else(|| item.clone())
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut compact = json!({
            "query": object.get("query"),
            "numberOfResults": object.get("numberOfResults"),
            "results": results,
            "historical": true,
            "snippetsOmitted": true,
        });
        if let Some(target) = compact.as_object_mut() {
            for key in [
                "source",
                "failed",
                "error",
                "fallbackFrom",
                "fallbackReason",
            ] {
                if let Some(value) = object.get(key) {
                    target.insert(key.into(), value.clone());
                }
            }
        }
        compact
    }

    if let Some(searches) = value.get("searches").and_then(Value::as_array) {
        return json!({"searches": searches.iter().map(compact_one).collect::<Vec<_>>(), "historical": true, "snippetsOmitted": true});
    }
    compact_one(value)
}

fn compact_web_read_result(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    let mut compact = pick_json_fields(
        object,
        &[
            "url",
            "source",
            "status",
            "contentType",
            "truncated",
            "artifactId",
            "externalized",
            "fallbackFrom",
            "fallbackReason",
            "error",
        ],
    );
    compact.insert("historical".into(), json!(true));
    if object.get("content").is_some() {
        compact.insert("contentOmitted".into(), json!(true));
    }
    Value::Object(compact)
}

fn pick_json_fields(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> serde_json::Map<String, Value> {
    keys.iter()
        .filter_map(|key| {
            object.get(*key).map(|value| {
                let bounded = match *key {
                    "summary" | "error" | "message" | "warnings" | "fallbackReason" => {
                        compact_json_value(value, 500)
                    }
                    _ => value.clone(),
                };
                ((*key).to_owned(), bounded)
            })
        })
        .collect()
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    value
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>()
        + "…"
}

fn truncate_from_end(value: &str, limit: usize) -> String {
    let count = value.chars().count();
    if count <= limit {
        return value.to_owned();
    }
    let keep = limit.saturating_sub(1);
    let start = count.saturating_sub(keep);
    "…".to_owned() + &value.chars().skip(start).collect::<String>()
}

fn truncate_middle_chars(value: &str, limit: usize) -> String {
    let count = value.chars().count();
    if count <= limit {
        return value.to_owned();
    }
    if limit < 5 {
        return value.chars().take(limit).collect();
    }
    let head = (limit - 1) * 2 / 3;
    let tail = limit - 1 - head;
    let start = value.chars().take(head).collect::<String>();
    let end = value.chars().skip(count - tail).collect::<String>();
    format!("{start}…{end}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ToolCall;

    #[test]
    fn completed_task_history_discards_raw_tool_traces() {
        let mut history = vec![
            Message::system("system"),
            Message::user("inspect"),
            Message::assistant("", Some(vec![ToolCall {
                id: "read-1".into(),
                name: "read_file".into(),
                arguments: json!({"path":"src/lib.rs","startLine":1,"endLine":160,"refresh":true}),
            }])),
            Message::tool(
                json!({"path":"src/lib.rs","snapshot":"s1","startLine":1,"endLine":160,"totalLines":500,"lines":"x".repeat(20_000)}).to_string(),
                "read-1",
                Some("read_file".into()),
            ),
            Message::assistant("done", None),
        ];

        compact_completed_task_history(&mut history);

        let tool = history[3].content.as_deref().unwrap();
        assert!(!tool.contains("\"lines\""));
        assert!(tool.contains("contentOmitted"));
        assert_eq!(
            history[2].tool_calls.as_ref().unwrap()[0].arguments,
            json!({"path":"src/lib.rs","startLine":1,"endLine":160})
        );
        assert_eq!(history[4].content.as_deref(), Some("done"));
    }

    #[test]
    fn active_turn_keeps_only_two_latest_tool_rounds_verbatim() {
        let call = |id: &str, path: &str| ToolCall {
            id: id.into(),
            name: "read_file".into(),
            arguments: json!({"path":path,"startLine":1,"endLine":200,"refresh":true}),
        };
        let result = |id: &str, path: &str, marker: &str| {
            Message::tool(
                json!({
                    "path":path,
                    "snapshot":format!("snapshot-{id}"),
                    "startLine":1,
                    "endLine":200,
                    "totalLines":500,
                    "lines":format!("{marker}{}", "x".repeat(12_000)),
                })
                .to_string(),
                id,
                Some("read_file".into()),
            )
        };
        let mut history = vec![
            Message::system(SYSTEM_INSTRUCTION),
            Message::user("fix it"),
            Message::assistant("", Some(vec![call("r1", "src/a.rs")])),
            result("r1", "src/a.rs", "old-round"),
            Message::assistant("", Some(vec![call("r2", "src/b.rs")])),
            result("r2", "src/b.rs", "recent-round-1"),
            Message::assistant("", Some(vec![call("r3", "src/c.rs")])),
            result("r3", "src/c.rs", "recent-round-2"),
        ];

        compact_active_task_history(&mut history, 1);

        let old = history[3].content.as_deref().unwrap();
        assert!(!old.contains("old-round"));
        assert!(old.contains("contentOmitted"));
        assert_eq!(
            history[2].tool_calls.as_ref().unwrap()[0].arguments,
            json!({"path":"src/a.rs","startLine":1,"endLine":200})
        );
        assert!(
            history[5]
                .content
                .as_deref()
                .unwrap()
                .contains("recent-round-1")
        );
        assert!(
            history[7]
                .content
                .as_deref()
                .unwrap()
                .contains("recent-round-2")
        );
    }

    #[test]
    fn completed_conversation_history_is_bounded_by_user_turns() {
        let mut history = vec![Message::system(SYSTEM_INSTRUCTION)];
        for index in 0..12 {
            history.push(Message::user(format!("request {index}")));
            history.push(Message::assistant(format!("answer {index}"), None));
        }

        trim_completed_conversation_history(&mut history);

        assert_eq!(
            history
                .iter()
                .filter(|message| message.role == MessageRole::User)
                .count(),
            RETAINED_COMPLETED_USER_TURNS
        );
        assert_eq!(history[0].role, MessageRole::System);
        let checkpoint = history[1].content.as_deref().unwrap();
        assert!(checkpoint.starts_with(CHECKPOINT_PREFIX));
        assert!(checkpoint.contains("User: request 0"));
        assert!(checkpoint.contains("Assistant: answer 5"));
        assert_eq!(history[2].content.as_deref(), Some("request 6"));
        assert_eq!(
            history
                .last()
                .and_then(|message| message.content.as_deref()),
            Some("answer 11")
        );
    }

    #[test]
    fn conversation_checkpoint_rolls_forward_without_nesting() {
        let mut history = vec![Message::system(SYSTEM_INSTRUCTION)];
        for index in 0..8 {
            history.push(Message::user(format!("request {index}")));
            history.push(Message::assistant(format!("answer {index}"), None));
        }
        trim_completed_conversation_history(&mut history);

        for index in 8..12 {
            history.push(Message::user(format!("request {index}")));
            history.push(Message::assistant(format!("answer {index}"), None));
        }
        trim_completed_conversation_history(&mut history);

        let checkpoints = history
            .iter()
            .filter_map(|message| message.content.as_deref())
            .filter(|content| content.starts_with(CHECKPOINT_PREFIX))
            .collect::<Vec<_>>();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].matches(CHECKPOINT_PREFIX).count(), 1);
        assert_eq!(checkpoints[0].matches(CHECKPOINT_GUIDANCE).count(), 1);
        assert!(checkpoints[0].contains("User: request 0"));
        assert!(checkpoints[0].contains("User: request 5"));
        assert_eq!(
            history
                .iter()
                .filter(|message| message.role == MessageRole::User)
                .count(),
            RETAINED_COMPLETED_USER_TURNS
        );
    }

    #[test]
    fn repeated_compaction_preserves_search_list_and_edit_counts() {
        for (name, input, key, expected) in [
            (
                "search_workspace",
                json!({"matches": [1, 2, 3]}),
                "matchCount",
                3,
            ),
            ("list_files", json!({"entries": [1, 2]}), "entryCount", 2),
        ] {
            let first = compact_completed_tool_result(Some(name), &input.to_string());
            let second = compact_completed_tool_result(Some(name), &first);
            let value: Value = serde_json::from_str(&second).unwrap();
            assert_eq!(value[key], json!(expected));
            assert_eq!(first, second);
        }
        let input = json!({"changes": [{"path": "a", "edits": [1, 2, 3]}]});
        let first = compact_completed_tool_arguments("apply_file_edits", &input);
        let second = compact_completed_tool_arguments("apply_file_edits", &first);
        assert_eq!(second["changes"][0]["editCount"], json!(3));
        assert_eq!(first, second);
    }

    #[test]
    fn generic_large_tool_result_keeps_only_recovery_metadata() {
        let mut history = vec![Message::tool(
            json!({
                "status":"ok",
                "summary":"important result",
                "artifactId":"artifact-1",
                "payload":"x".repeat(4_000),
            })
            .to_string(),
            "generic-1",
            Some("custom_tool".into()),
        )];

        compact_completed_task_history(&mut history);

        let compact: Value = serde_json::from_str(history[0].content.as_deref().unwrap()).unwrap();
        assert_eq!(compact.get("status"), Some(&json!("ok")));
        assert_eq!(compact.get("summary"), Some(&json!("important result")));
        assert_eq!(compact.get("artifactId"), Some(&json!("artifact-1")));
        assert!(compact.get("payload").is_none());
        assert_eq!(compact.get("historical"), Some(&json!(true)));
    }

    #[test]
    fn aged_turns_drop_image_bytes_and_bound_large_text() {
        let mut history = vec![Message::system(SYSTEM_INSTRUCTION)];
        let mut first = Message::user_with_images(
            format!("prefix {} suffix", "x".repeat(8_000)),
            vec![crate::core::ImageAttachment {
                media_type: "image/png".into(),
                data: "a".repeat(20_000),
                name: Some("screen.png".into()),
            }],
        );
        first.content.as_mut().unwrap().push_str(" durable-tail");
        history.push(first);
        history.push(Message::assistant("y".repeat(8_000), None));
        history.push(Message::user("middle"));
        history.push(Message::assistant("middle answer", None));
        history.push(Message::user("recent one"));
        history.push(Message::assistant("recent answer", None));
        history.push(Message::user("recent two"));
        history.push(Message::assistant("recent answer two", None));

        trim_completed_conversation_history(&mut history);

        assert!(history[1].images.is_none());
        let old_user = history[1].content.as_deref().unwrap();
        assert!(old_user.contains("historical image payload omitted: screen.png"));
        assert!(old_user.contains("durable-tail"));
        assert!(old_user.chars().count() < 2_500);
        assert!(
            history[2].content.as_deref().unwrap().chars().count() <= AGED_ASSISTANT_MESSAGE_CHARS
        );
        assert_eq!(history[5].content.as_deref(), Some("recent one"));
        assert_eq!(history[7].content.as_deref(), Some("recent two"));
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;
    use crate::core::ToolCall;

    #[test]
    fn repeated_skills_keep_latest_copy_and_distinct_versions() {
        let skill = format!(
            "User-invoked Skill: review.\n{}",
            "instruction ".repeat(1000)
        );
        let updated = format!("{skill}\nUpdated requirement");
        let original = vec![
            Message::system(SYSTEM_INSTRUCTION),
            Message::user("first task"),
            Message::system(&skill),
            Message::system("Other guidance"),
            Message::system(&updated),
            Message::user("second task"),
            Message::system(&skill),
        ];
        let mut working = original.clone();
        deduplicate_skill_instructions(&mut working);
        assert_eq!(working.len(), original.len() - 1);
        assert_eq!(
            working.last().unwrap().content.as_deref(),
            Some(skill.as_str())
        );
        assert!(
            working
                .iter()
                .any(|m| m.content.as_deref() == Some(updated.as_str()))
        );
        assert_eq!(original[2].content.as_deref(), Some(skill.as_str()));
        assert_eq!(working[2].content.as_deref(), Some("Other guidance"));
    }

    #[test]
    fn nested_historical_metadata_is_bounded_without_losing_retrieval_ids() {
        let original = json!({
            "error":{"details": "한".repeat(20000)},
            "summary":["x".repeat(30000)],
            "artifactId":"exact-artifact-reference", "succeeded":false
        })
        .to_string();
        let compact = compact_completed_tool_result(Some("custom"), &original);
        assert!(compact.len() < 3000);
        let value: Value = serde_json::from_str(&compact).unwrap();
        assert_eq!(value["artifactId"], "exact-artifact-reference");
        assert_eq!(value["succeeded"], false);
        assert_eq!(value["error"]["contentOmitted"], true);
        let edited = json!({"files":[{"path":"src/a.rs","warnings":["w".repeat(20000)]}],"diagnostics":[{"severity":"error","message":"e".repeat(20000)}]}).to_string();
        let compact = compact_completed_tool_result(Some("apply_file_edits"), &edited);
        assert!(compact.len() < 2000);
        assert!(compact.contains("src/a.rs"));
    }

    #[test]
    fn historical_artifact_reads_keep_character_page_coordinates() {
        let args =
            json!({"id":"artifact", "startLine":1,"endLine":1,"offset":8192,"maxChars":1000});
        assert_eq!(
            compact_completed_tool_arguments("read_artifact", &args),
            args
        );
    }

    #[test]
    fn thinning_does_not_expand_small_tool_results_or_touch_original_evidence() {
        let original = vec![Message::tool("{}", "call", Some("read_file".into()))];
        let mut working = original.clone();
        compact_completed_task_history(&mut working);
        assert_eq!(working[0].content, original[0].content);
        let content = json!({"path":"a.rs", "lines":"x".repeat(10000)}).to_string();
        working[0].content = Some(content.clone());
        compact_completed_task_history(&mut working);
        assert!(working[0].content.as_ref().unwrap().len() < content.len());
        assert_eq!(original[0].content.as_deref(), Some("{}"));
    }

    #[test]
    fn active_tool_trace_size_stays_bounded_as_rounds_accumulate() {
        let mut history = vec![Message::system(SYSTEM_INSTRUCTION), Message::user("fix it")];
        for index in 0..10 {
            let id = format!("read-{index}");
            history.push(Message::assistant(
                "",
                Some(vec![ToolCall {
                    id: id.clone(),
                    name: "read_file".into(),
                    arguments: json!({"path":format!("src/{index}.rs"),"startLine":1,"endLine":200}),
                }]),
            ));
            history.push(Message::tool(
                json!({
                    "path":format!("src/{index}.rs"),
                    "snapshot":format!("snapshot-{index}"),
                    "startLine":1,
                    "endLine":200,
                    "totalLines":200,
                    "lines":"x".repeat(20_000),
                })
                .to_string(),
                id,
                Some("read_file".into()),
            ));
        }
        let raw_bytes = serde_json::to_vec(&history).unwrap().len();
        let mut working = history.clone();

        compact_active_task_history(&mut working, 1);

        let compact_bytes = serde_json::to_vec(&working).unwrap().len();
        assert!(compact_bytes * 3 < raw_bytes);
        assert_eq!(history.last().unwrap(), working.last().unwrap());
    }
}
