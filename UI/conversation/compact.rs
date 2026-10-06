//! Compact work labels for constrained conversation presentations.
//! Formatting semantics are shared; native glyphs and cell layout stay adapters.
use crate::model::ToolCallStatus;
use serde_json::Value;

fn tool_call_status_bucket(call: &crate::model::ConversationToolCall) -> u8 {
    match call.status {
        ToolCallStatus::Preparing
        | ToolCallStatus::AwaitingPermission
        | ToolCallStatus::Running => 1,
        ToolCallStatus::Failed
        | ToolCallStatus::Cancelled
        | ToolCallStatus::Interrupted
        | ToolCallStatus::TimedOut => 2,
        ToolCallStatus::Completed => 0,
        ToolCallStatus::Suppressed => 3,
    }
}

fn humanize_tool_name(name: &str) -> String {
    let human = name.replace(['_', '-'], " ");
    let mut chars = human.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => "Tool".into(),
    }
}

pub fn tool_activity_title(call: &crate::model::ConversationToolCall) -> String {
    tool_activity_title_with_style(call, false)
}

fn tool_activity_title_with_style(
    call: &crate::model::ConversationToolCall,
    legacy: bool,
) -> String {
    let active = matches!(
        call.status,
        ToolCallStatus::Preparing | ToolCallStatus::AwaitingPermission | ToolCallStatus::Running
    );
    let fallback_verb = humanize_tool_name(&call.name);
    let verb = match call.name.as_str() {
        "apply_file_edits" => {
            if active {
                "Editing"
            } else {
                "Edited"
            }
        }
        "search_workspace" | "search_artifact" | "web_search" => {
            if active {
                "Searching"
            } else {
                "Searched"
            }
        }
        "search_tools" => {
            if active {
                "Finding tools"
            } else {
                "Found tools"
            }
        }
        "task_notes" => {
            if active {
                "Updating notes"
            } else {
                "Updated notes"
            }
        }
        "context_history" => {
            if active {
                "Reading context"
            } else {
                "Read context"
            }
        }
        "read_file" | "read_files" => {
            if active {
                "Reading File"
            } else {
                "Read File"
            }
        }
        "read_artifact" => {
            if active {
                "Reading output"
            } else {
                "Read output"
            }
        }
        "read_document" => {
            if active {
                "Reading document"
            } else {
                "Read document"
            }
        }
        "web_read" => {
            if active {
                "Reading Website"
            } else {
                "Read Website"
            }
        }
        "run_shell" => {
            if active {
                "Running Shell"
            } else {
                "Ran Shell"
            }
        }
        "shell_job" => {
            if active {
                "Checking command"
            } else {
                "Checked command"
            }
        }
        "list_files" => {
            if active {
                "Listing files"
            } else {
                "Listed files"
            }
        }
        "analyze_data" => {
            if active {
                "Analyzing data"
            } else {
                "Analyzed data"
            }
        }
        "computer_use" | "desktop_control" => {
            if active {
                "Using computer"
            } else {
                "Used computer"
            }
        }
        _ => fallback_verb.as_str(),
    };

    let verb = if legacy {
        verb
    } else {
        call.label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or(verb)
    };
    let outcome = match call.status {
        ToolCallStatus::Failed => Some("Failed"),
        ToolCallStatus::TimedOut => Some("Timed out"),
        ToolCallStatus::Cancelled => Some("Cancelled"),
        ToolCallStatus::Interrupted => Some("Interrupted"),
        ToolCallStatus::AwaitingPermission if !legacy => Some("Approval needed"),
        ToolCallStatus::Preparing if !legacy => Some("Preparing"),
        _ => None,
    };
    outcome
        .map(|outcome| format!("{outcome} · {verb}"))
        .unwrap_or_else(|| verb.to_owned())
}

pub fn tool_activity_summary(call: &crate::model::ConversationToolCall) -> Option<String> {
    tool_activity_summary_with_style(call, false)
}

fn tool_activity_summary_with_style(
    call: &crate::model::ConversationToolCall,
    legacy: bool,
) -> Option<String> {
    let mut summary = tool_target_with_style(call, legacy);
    if matches!(
        call.status,
        ToolCallStatus::Failed
            | ToolCallStatus::Cancelled
            | ToolCallStatus::Interrupted
            | ToolCallStatus::TimedOut
    ) && let Some(error) = call
        .error
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let error = compact_tool_text(error, 72);
        summary = Some(match summary {
            Some(summary) => format!("{summary} · {error}"),
            None => error,
        });
    }
    summary
}

/// What a trace row points at. Failures are shown by the row's icon colour.
pub fn tool_target(call: &crate::model::ConversationToolCall) -> Option<String> {
    tool_target_with_style(call, false)
}

fn tool_target_with_style(
    call: &crate::model::ConversationToolCall,
    legacy: bool,
) -> Option<String> {
    let explicit_detail = (!legacy)
        .then_some(call.detail.as_deref())
        .flatten()
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
        .map(str::to_owned);
    explicit_detail
        .or_else(|| match call.name.as_str() {
            "apply_file_edits" => Some(edit_activity_detail(call)),
            "search_workspace" | "search_artifact" | "web_search" => {
                Some(search_activity_detail(call))
            }
            "read_file" | "read_files" => Some(read_activity_detail(call)),
            "run_shell" => tool_json_string(call, &["command"])
                .map(|value| compact_tool_text(&value, 88))
                .or_else(|| tool_argument_summary(&call.arguments, 88)),
            "shell_job" => tool_json_string(call, &["command", "job", "action", "id"])
                .map(|value| compact_tool_text(&value, 88))
                .or_else(|| tool_argument_summary(&call.arguments, 88)),
            "web_read" => tool_json_string(call, &["url", "href", "ref_id", "refId"])
                .map(|value| compact_tool_text(&value, 88))
                .or_else(|| tool_argument_summary(&call.arguments, 88)),
            "list_files" => tool_json_string(call, &["path"])
                .map(|value| short_tool_path(&value))
                .or_else(|| tool_argument_summary(&call.arguments, 88)),
            "search_tools" | "task_notes" | "context_history" => None,
            _ => tool_argument_summary(&call.arguments, 88),
        })
        .filter(|value| !value.trim().is_empty())
}

fn short_tool_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let parts = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [] => path.to_owned(),
        [only] => (*only).to_owned(),
        _ => parts[parts.len().saturating_sub(2)..].join("/"),
    }
}

fn edit_activity_detail(call: &crate::model::ConversationToolCall) -> String {
    let value = serde_json::from_str::<Value>(&call.arguments).ok();
    let paths = value
        .as_ref()
        .and_then(|value| value.get("changes"))
        .and_then(Value::as_array)
        .map(|changes| {
            changes
                .iter()
                .filter_map(|change| change.get("path").and_then(Value::as_str))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let path = paths
        .first()
        .map(|path| short_tool_path(path))
        .unwrap_or_else(|| "file".into());
    let (added, removed) = edit_line_delta(call, value.as_ref());
    let mut detail = match (added, removed) {
        (0, 0) => path,
        (added, 0) => format!("{path} +{added}"),
        (0, removed) => format!("{path} −{removed}"),
        (added, removed) => format!("{path} +{added} −{removed}"),
    };
    if paths.len() > 1 {
        detail.push_str(&format!(" · +{} files", paths.len() - 1));
    }
    detail
}

fn edit_line_delta(
    call: &crate::model::ConversationToolCall,
    arguments: Option<&Value>,
) -> (usize, usize) {
    if let Ok(result) = serde_json::from_str::<Value>(call.result.as_deref().unwrap_or("")) {
        let mut added = 0;
        let mut removed = 0;
        if let Some(files) = result.get("files").and_then(Value::as_array) {
            for file in files {
                if let Some(hunks) = file
                    .get("diff")
                    .and_then(|diff| diff.get("hunks"))
                    .and_then(Value::as_array)
                {
                    for hunk in hunks {
                        if let Some(lines) = hunk.get("lines").and_then(Value::as_array) {
                            for line in lines.iter().filter_map(Value::as_str) {
                                if line.starts_with('+') {
                                    added += 1;
                                }
                                if line.starts_with('-') {
                                    removed += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        if added > 0 || removed > 0 {
            return (added, removed);
        }
    }
    let mut added = 0;
    let mut removed = 0;
    let Some(changes) = arguments
        .and_then(|value| value.get("changes"))
        .and_then(Value::as_array)
    else {
        return (0, 0);
    };
    for change in changes {
        if let Some(file_op) = change.get("fileOp").and_then(Value::as_object) {
            match file_op.get("kind").and_then(Value::as_str) {
                Some("create") => added += line_count(file_op.get("text")),
                Some("delete") => removed += 1,
                _ => {}
            }
        }
        for edit in change
            .get("edits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match edit.get("kind").and_then(Value::as_str) {
                Some("insert") | Some("insertAfterBlock") => added += line_count(edit.get("text")),
                Some("delete") | Some("deleteBlock") => {
                    removed += range_line_count(edit.get("range"))
                }
                Some("replace") | Some("replaceBlock") => {
                    added += line_count(edit.get("text"));
                    removed += range_line_count(edit.get("range"));
                }
                _ => {}
            }
        }
    }
    (added, removed)
}

fn line_count(value: Option<&Value>) -> usize {
    value
        .and_then(Value::as_str)
        .map(|text| text.lines().count().max(1))
        .unwrap_or(0)
}

fn range_line_count(value: Option<&Value>) -> usize {
    let Some(range) = value.and_then(Value::as_object) else {
        return 0;
    };
    let start = range.get("start").and_then(Value::as_u64).unwrap_or(0);
    let end = range.get("end").and_then(Value::as_u64).unwrap_or(start);
    end.saturating_sub(start).saturating_add(1) as usize
}

fn search_activity_detail(call: &crate::model::ConversationToolCall) -> String {
    let Ok(value) = serde_json::from_str::<Value>(&call.arguments) else {
        return "workspace".into();
    };
    let query = ["query", "q", "search", "text", "pattern"]
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .unwrap_or("");
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .map(short_tool_path);
    match (path, query.is_empty()) {
        (Some(path), false) => format!("{path} for {query}"),
        (Some(path), true) => path,
        (None, false) => format!("\"{}\"", compact_tool_text(query, 86)),
        (None, true) => "workspace".into(),
    }
}

fn read_activity_detail(call: &crate::model::ConversationToolCall) -> String {
    let Ok(value) = serde_json::from_str::<Value>(&call.arguments) else {
        return "file".into();
    };
    let mut paths = Vec::new();
    if let Some(path) = value.get("path").and_then(Value::as_str) {
        paths.push(path);
    }
    if let Some(requests) = value.get("requests").and_then(Value::as_array) {
        paths.extend(
            requests
                .iter()
                .filter_map(|request| request.get("path").and_then(Value::as_str)),
        );
    }
    let Some(first) = paths.first() else {
        return "file".into();
    };
    let first = short_tool_path(first);
    if paths.len() > 1 {
        format!("{first} · +{} files", paths.len() - 1)
    } else {
        first
    }
}

fn tool_json_string(call: &crate::model::ConversationToolCall, keys: &[&str]) -> Option<String> {
    let value = serde_json::from_str::<Value>(&call.arguments).ok()?;
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

fn compact_tool_text(text: &str, max_chars: usize) -> String {
    let clean = text
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_middle(&clean, max_chars)
}

fn tool_argument_summary(arguments: &str, max_chars: usize) -> Option<String> {
    if arguments.trim().is_empty() || max_chars == 0 {
        return None;
    }
    let value: Value = serde_json::from_str(arguments).ok()?;
    let object = value.as_object()?;

    if let Some(purpose) = object.get("purpose").and_then(Value::as_str) {
        let clean: String = purpose
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect();
        let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
        if !clean.is_empty() {
            return Some(truncate_middle(&clean, max_chars));
        }
    }

    for key in ["path", "query", "capability", "url"] {
        if let Some(text) = object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let prefix = if key == "path" {
                String::new()
            } else {
                format!("{key} ")
            };
            return Some(truncate_middle(&format!("{prefix}{text}"), max_chars));
        }
    }

    for collection in ["requests", "changes"] {
        if let Some(values) = object.get(collection).and_then(Value::as_array) {
            let paths = values
                .iter()
                .filter_map(|value| value.get("path").and_then(Value::as_str))
                .collect::<Vec<_>>();
            if !paths.is_empty() {
                return Some(truncate_middle(&summarize_values(&paths), max_chars));
            }
        }
    }

    None
}

fn summarize_values(values: &[&str]) -> String {
    let shown = values
        .iter()
        .take(2)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if values.len() > 2 {
        format!("{shown}  +{}", values.len() - 2)
    } else {
        shown
    }
}

pub fn collapse_repeats<'a>(
    calls: &[&'a crate::model::ConversationToolCall],
) -> Vec<(&'a crate::model::ConversationToolCall, usize)> {
    let mut rows: Vec<(&crate::model::ConversationToolCall, usize)> = Vec::new();
    for &call in calls {
        if matches!(call.status, ToolCallStatus::Suppressed) {
            continue;
        }
        match rows.last_mut() {
            Some((previous, count))
                if previous.name == call.name
                    && tool_call_status_bucket(previous) == 0
                    && tool_call_status_bucket(call) == 0
                    && previous.label == call.label
                    && previous.detail == call.detail
                    && previous.arguments == call.arguments =>
            {
                *count += 1;
            }
            _ => rows.push((call, 1)),
        }
    }
    rows
}

pub fn reasoning_summary_items(summary: &str) -> Vec<String> {
    let summary = summary.trim();
    if summary.is_empty()
        || summary
            .chars()
            .all(|ch| matches!(ch, '*' | '_' | '~') || ch == '\x60')
    {
        return Vec::new();
    }

    if summary.starts_with('*') {
        let mut items = Vec::new();
        let mut rest = summary;
        while let Some(after_open) = rest.strip_prefix("**") {
            if let Some(end) = after_open.find("**") {
                let body = after_open[..end].trim();
                if !body.is_empty() {
                    items.push(format!("**{body}**"));
                }
                rest = &after_open[end + 2..];
                continue;
            }

            let body = after_open.trim().trim_end_matches('*').trim();
            if !body.is_empty() {
                items.push(format!("**{body}**"));
            }
            rest = "";
            break;
        }

        if items.is_empty() {
            let body = summary.trim_matches('*').trim();
            if !body.is_empty() {
                items.push(format!("**{body}**"));
            }
        } else {
            for line in rest.lines().map(str::trim).filter(|line| !line.is_empty()) {
                items.push(line.to_owned());
            }
        }
        return items;
    }

    summary
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn tool_action_label(name: &str, active: bool) -> String {
    let (past, present) = match name {
        "apply_file_edits" => ("Edited Files", "Editing Files"),
        "task_notes" => ("Updated Notes", "Updating Notes"),
        "search_workspace" => ("Searched Files", "Searching Files"),
        "search_artifact" => ("Searched Output", "Searching Output"),
        "search_tools" => ("Searched Tools", "Searching Tools"),
        "web_search" => ("Searched Web", "Searching Web"),
        "read_file" | "read_files" => ("Read File", "Reading File"),
        "read_artifact" => ("Read Output", "Reading Output"),
        "read_document" => ("Read Document", "Reading Document"),
        "web_read" => ("Read Website", "Reading Website"),
        "run_shell" => ("Ran Shell", "Running Shell"),
        "shell_job" => ("Checked Shell Job", "Checking Shell Job"),
        "context_history" => ("Read Context", "Reading Context"),
        "list_files" => ("Listed Files", "Listing Files"),
        "analyze_data" => ("Analyzed Data", "Analyzing Data"),
        "computer_use" | "desktop_control" => ("Used Computer", "Using Computer"),
        "activate_capability" => ("Activated Capability", "Activating Capability"),
        _ => return humanize_tool_name(name),
    };
    if active { present } else { past }.to_owned()
}

pub fn is_active(call: &crate::model::ConversationToolCall) -> bool {
    tool_call_status_bucket(call) == 1
}

pub fn is_failed(call: &crate::model::ConversationToolCall) -> bool {
    tool_call_status_bucket(call) == 2
}

// Shared copy truncates Unicode scalar values; terminal cell fitting belongs to adapters.
fn truncate_middle(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_chars {
        return value.into();
    }
    if max_chars < 2 {
        return chars.into_iter().take(max_chars).collect();
    }
    let left = (max_chars - 1).div_ceil(2);
    let right = max_chars - 1 - left;
    chars[..left]
        .iter()
        .chain(std::iter::once(&'…'))
        .chain(chars[chars.len() - right..].iter())
        .collect()
}
