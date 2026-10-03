//! Compact tool traces and backend operation summaries.
use super::*;
use crate::tui::ui::support::icons;
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

pub(in crate::tui::ui) fn tool_icon(name: &str) -> &'static str {
    // Font Awesome's Nerd Font codepoints cover the mockup's pixel icons in a
    // terminal cell. The active terminal font controls their final shape.
    match name {
        "apply_file_edits" | "task_notes" => "\u{f040}",
        "search_workspace" | "search_artifact" | "search_tools" | "web_search" => "\u{f002}",
        "read_file" | "read_files" | "read_artifact" | "read_document" => "\u{f15c}",
        "web_read" => "\u{f02d}",
        "run_shell" | "shell_job" => "\u{f120}",
        "list_files" => "\u{f07b}",
        "analyze_data" => "\u{f080}",
        "computer_use" | "desktop_control" => "\u{f108}",
        "activate_capability" => "\u{f0e7}",
        "context_history" => "\u{f1da}",
        _ => "\u{f013}",
    }
}

pub(in crate::tui::ui) fn tool_activity_title(call: &crate::model::ConversationToolCall) -> String {
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

pub(in crate::tui::ui) fn tool_activity_summary(
    call: &crate::model::ConversationToolCall,
) -> Option<String> {
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
fn tool_target(call: &crate::model::ConversationToolCall) -> Option<String> {
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

fn collapse_repeats<'a>(
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

pub(in crate::tui::ui) fn reasoning_summary_items(summary: &str) -> Vec<String> {
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

#[cfg(test)]
mod tool_summary_layout_tests {
    use super::*;

    pub(super) fn call(name: &str, status: ToolCallStatus) -> crate::model::ConversationToolCall {
        crate::model::ConversationToolCall {
            id: format!("{name}-test"),
            index: None,
            call_id: None,
            name: name.into(),
            arguments: r#"{"purpose":"Verify Rust changes","command":"cargo check"}"#.into(),
            status,
            label: None,
            detail: None,
            started_at: None,
            ended_at: None,
            duration_ms: None,
            attempt: None,
            parent_call_id: None,
            parallel_group_id: None,
            job_id: None,
            result: None,
            error: None,
        }
    }

    #[test]
    fn summaries_use_compact_file_language() {
        let mut search = call("search_workspace", ToolCallStatus::Completed);
        search.arguments =
            serde_json::json!({"query": "elapsed_ms", "path": "src/ui.rs"}).to_string();
        assert_eq!(
            tool_activity_summary(&search).as_deref(),
            Some("src/ui.rs for elapsed_ms")
        );
        let shell = call("run_shell", ToolCallStatus::Completed);
        assert_eq!(
            tool_activity_summary(&shell).as_deref(),
            Some("cargo check")
        );
    }

    #[test]
    fn groups_collapse_unless_active_or_failed_and_fit_any_width() {
        let done = call("read_file", ToolCallStatus::Completed);
        let items = [WorkItem::Tool(&done)];
        let collapsed = work_group_lines(&work_groups(&items), 80, false);
        assert_eq!(collapsed.len(), 1);
        let expanded = work_group_lines(&work_groups(&items), 80, true);
        assert!(expanded.len() > 1);

        let running = call("apply_file_edits", ToolCallStatus::Running);
        let failed = call("search_workspace", ToolCallStatus::Failed);
        for tool in [&running, &failed] {
            let items = [WorkItem::Tool(tool)];
            let lines = work_group_lines(&work_groups(&items), 80, false);
            assert!(lines.len() > 2, "active or failed groups stay expanded");
            for width in [24, 40, 60] {
                assert!(
                    work_group_lines(&work_groups(&items), width, false)
                        .iter()
                        .all(|line| line.width() <= usize::from(width))
                );
            }
        }
    }

    #[test]
    fn model_summaries_expand_independently_and_suppressed_calls_vanish() {
        let read = call("read_file", ToolCallStatus::Completed);
        let hidden = call("run_shell", ToolCallStatus::Suppressed);
        let items = [
            WorkItem::Summary {
                text: "**Analyzed recent logs**",
                live: false,
            },
            WorkItem::Tool(&read),
            WorkItem::Tool(&hidden),
        ];
        let text = work_group_lines(&work_groups(&items), 80, false)[0].to_string();
        assert!(text.contains("Analyzed recent logs"), "{text}");
        let summary = "Checking inputs\nComparing the actual provider formats";
        let groups = work_groups(&[WorkItem::Summary {
            text: summary,
            live: false,
        }]);
        let expanded = work_group_lines(&groups, 80, true)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            expanded.contains("Comparing the actual provider formats"),
            "{expanded}"
        );
        let items = [WorkItem::Tool(&hidden)];
        assert!(work_group_lines(&work_groups(&items), 80, false).is_empty());
    }
}

fn tool_action_label(name: &str, active: bool) -> String {
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

fn is_active(call: &crate::model::ConversationToolCall) -> bool {
    tool_call_status_bucket(call) == 1
}

fn is_failed(call: &crate::model::ConversationToolCall) -> bool {
    tool_call_status_bucket(call) == 2
}

/// One collapsible unit of agent work in the transcript.
pub(in crate::tui::ui) enum WorkGroup<'a> {
    Tools {
        calls: Vec<&'a crate::model::ConversationToolCall>,
        title: Option<String>,
    },
    Reasoning {
        text: String,
        live: bool,
        summary: bool,
        calls: Vec<&'a crate::model::ConversationToolCall>,
    },
}

pub(in crate::tui::ui) enum WorkItem<'a> {
    Tool(&'a crate::model::ConversationToolCall),
    Summary { text: &'a str, live: bool },
    Reasoning { text: &'a str, live: bool },
}

pub(in crate::tui::ui) fn work_groups<'a>(items: &[WorkItem<'a>]) -> Vec<WorkGroup<'a>> {
    let mut groups: Vec<WorkGroup<'a>> = Vec::new();
    let mut open = false;
    for item in items {
        match item {
            WorkItem::Tool(call) => {
                if matches!(call.status, ToolCallStatus::Suppressed) {
                    continue;
                }
                if !open {
                    groups.push(WorkGroup::Tools {
                        calls: Vec::new(),
                        title: None,
                    });
                    open = true;
                }
                match groups.last_mut() {
                    Some(WorkGroup::Tools { calls, .. })
                    | Some(WorkGroup::Reasoning {
                        calls,
                        summary: true,
                        ..
                    }) => calls.push(call),
                    _ => {}
                }
            }
            WorkItem::Summary { text, live } => {
                open = true;
                groups.push(WorkGroup::Reasoning {
                    text: (*text).to_owned(),
                    live: *live,
                    summary: true,
                    calls: Vec::new(),
                });
            }
            WorkItem::Reasoning { text, live } => {
                open = false;
                groups.push(WorkGroup::Reasoning {
                    text: (*text).to_owned(),
                    live: *live,
                    summary: false,
                    calls: Vec::new(),
                });
            }
        }
    }
    groups
}

pub(in crate::tui::ui) fn spinner() -> &'static str {
    let tick = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() / 180)
        .unwrap_or(0);
    ["|", "/", "-", "\\"][(tick % 4) as usize]
}

fn group_icons(calls: &[&crate::model::ConversationToolCall]) -> String {
    let mut icons: Vec<&str> = Vec::new();
    for call in calls.iter().rev() {
        let icon = tool_icon(&call.name);
        if !icons.contains(&icon) {
            icons.push(icon);
        }
    }
    icons.truncate(3);
    icons.reverse();
    icons.join(" ")
}

fn fallback_group_title(calls: &[&crate::model::ConversationToolCall]) -> String {
    let mut labels: Vec<String> = Vec::new();
    for call in calls {
        let label = tool_action_label(&call.name, is_active(call));
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels.truncate(3);
    labels.join(" · ")
}

/// Collapsed one-line summaries with a tree of calls when expanded. Groups
/// with running or failed calls are always expanded.
pub(in crate::tui::ui) fn work_group_lines(
    groups: &[WorkGroup<'_>],
    width: u16,
    expand_all: bool,
) -> Vec<Line<'static>> {
    work_group_lines_selected(groups, width, expand_all, None, &Default::default(), 0).0
}

pub(super) fn work_group_lines_selected(
    groups: &[WorkGroup<'_>],
    width: u16,
    expand_all: bool,
    selected: Option<usize>,
    expanded_work: &std::collections::BTreeSet<usize>,
    base: usize,
) -> (Vec<Line<'static>>, Vec<usize>) {
    let width = usize::from(width);
    let muted = Style::default().fg(theme::muted());
    let secondary = Style::default().fg(theme::secondary());
    let rail = Style::default().fg(theme::hairline());
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut headers = Vec::new();
    for (local, group) in groups.iter().enumerate() {
        let index = base + local;
        let expand_all = expand_all || expanded_work.contains(&index);
        match group {
            WorkGroup::Tools { calls, title } => {
                if calls.is_empty() {
                    continue;
                }
                let active = calls.iter().any(|call| is_active(call));
                let failed = calls.iter().any(|call| is_failed(call));
                let expanded = expand_all || active || failed;
                // Expanded groups breathe; collapsed ones stack tightly.
                if expanded && lines.last().is_some_and(|line| line.width() > 0) {
                    lines.push(Line::default());
                }
                headers.push(lines.len());
                let icons = group_icons(calls);
                let title = title.clone().unwrap_or_else(|| fallback_group_title(calls));
                let lead = if active {
                    format!(" {} ", spinner())
                } else {
                    " ".to_owned()
                };
                let chevron = format!(" {}", icons::chevron(expanded));
                let budget = width.saturating_sub(
                    Span::raw(&lead).width()
                        + Span::raw(&icons).width()
                        + 1
                        + Span::raw(&chevron).width(),
                );
                let title_style = if active {
                    Style::default()
                        .fg(theme::text())
                        .add_modifier(Modifier::BOLD)
                } else {
                    secondary
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        lead,
                        Style::default()
                            .fg(theme::text())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(icons, secondary),
                    Span::raw(" "),
                    Span::styled(task::fit(&title, budget), title_style),
                    Span::styled(chevron, muted),
                ]));
                if expanded {
                    for (call, count) in collapse_repeats(calls) {
                        let failed = is_failed(call);
                        let running = is_active(call);
                        let mut target = tool_target(call)
                            .or_else(|| call.detail.clone())
                            .unwrap_or_default();
                        if count > 1 {
                            target = format!("{target} ×{count}");
                        }
                        let action = format!(" {}", tool_action_label(&call.name, running));
                        let tail = if running {
                            format!(" {}", spinner())
                        } else {
                            String::new()
                        };
                        let used = 2 + 3 + Span::raw(&action).width() + tail.len();
                        let target = task::fit(&target, width.saturating_sub(used + 1));
                        let icon_style = if failed {
                            Style::default().fg(theme::error_subtle())
                        } else {
                            secondary
                        };
                        lines.push(Line::from(vec![
                            Span::styled(" │", rail),
                            Span::raw("   "),
                            Span::styled(tool_icon(&call.name), icon_style),
                            Span::styled(action, secondary),
                            Span::styled(target, secondary),
                            Span::styled(
                                tail,
                                Style::default()
                                    .fg(theme::text())
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]));
                    }
                    lines.push(Line::default());
                }
            }
            WorkGroup::Reasoning {
                text,
                live,
                summary,
                calls,
            } => {
                headers.push(lines.len());
                let summary_items = reasoning_summary_items(text);
                let current = if *live {
                    summary_items.last()
                } else {
                    summary_items.first()
                };
                let first = current
                    .map(String::as_str)
                    .unwrap_or(text)
                    .lines()
                    .map(|line| line.trim().trim_start_matches(['#', '*', '-', ' ']))
                    .find(|line| !line.is_empty())
                    .unwrap_or_default()
                    .replace('*', "");
                let expanded = expand_all;
                let lead = if *live {
                    format!(" {} ", spinner())
                } else {
                    " ".to_owned()
                };
                let title = if *summary {
                    first
                } else {
                    format!("Reasoning · {first}")
                };
                let recent_tools = group_icons(calls);
                let chevron = format!(" {}", icons::chevron(expanded));
                let mut spans = vec![Span::styled(lead, Style::default().fg(theme::text()))];
                let trailing = if recent_tools.is_empty() {
                    chevron.clone()
                } else {
                    format!(" {recent_tools}{chevron}")
                };
                let prefix_width = spans.iter().map(Span::width).sum::<usize>();
                let budget = width.saturating_sub(prefix_width + Span::raw(&trailing).width());
                spans.push(Span::styled(task::fit(&title, budget), secondary));
                if !recent_tools.is_empty() {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(recent_tools, secondary));
                }
                spans.push(Span::styled(chevron, muted));
                lines.push(Line::from(spans));
                if expanded {
                    for line in session_markdown_lines(
                        &text.replace("****", "**\n\n**"),
                        width.saturating_sub(5),
                    ) {
                        lines.extend(prefixed_wrapped_line(
                            Span::styled(" │   ", rail),
                            line.style(muted),
                            width as u16,
                        ));
                    }
                    if !calls.is_empty() {
                        // Reuse the tool dropdown's presentation, but the summary
                        // owns this list and its keyboard expansion state.
                        let nested = WorkGroup::Tools {
                            calls: calls.clone(),
                            title: None,
                        };
                        let (tool_lines, _) = work_group_lines_selected(
                            &[nested],
                            width as u16,
                            true,
                            None,
                            &std::collections::BTreeSet::new(),
                            0,
                        );
                        lines.extend(tool_lines);
                    }
                    lines.push(Line::default());
                }
            }
        }
        if selected == Some(index) {
            if let Some(&header) = headers.last() {
                lines[header] = lines[header].clone().style(
                    Style::default()
                        .bg(theme::code_background())
                        .add_modifier(Modifier::REVERSED),
                );
            }
        }
    }
    (lines, headers)
}
