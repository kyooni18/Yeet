//! Compact tool traces and backend operation summaries.
use super::*;
use serde_json::Value;

#[cfg(test)]
pub(in crate::ui) fn tool_activity_line(
    call: &crate::model::ConversationToolCall,
    width: u16,
) -> Line<'static> {
    tool_activity_line_count(call, width, 1, "    ")
}

fn tool_activity_line_count(
    call: &crate::model::ConversationToolCall,
    width: u16,
    count: usize,
    rail: &str,
) -> Line<'static> {
    tool_activity_line_count_with_style(call, width, count, rail, false)
}

fn tool_activity_line_count_with_style(
    call: &crate::model::ConversationToolCall,
    width: u16,
    count: usize,
    rail: &str,
    legacy: bool,
) -> Line<'static> {
    let active = matches!(
        call.status,
        ToolCallStatus::Preparing | ToolCallStatus::AwaitingPermission | ToolCallStatus::Running
    );
    let failed = matches!(
        call.status,
        ToolCallStatus::Failed
            | ToolCallStatus::Cancelled
            | ToolCallStatus::Interrupted
            | ToolCallStatus::TimedOut
    );
    let icon_name = if legacy {
        tool_icon_legacy(&call.name)
    } else {
        tool_icon(&call.name)
    };
    let icon = format!("{} {} ", tool_status_glyph(call), icon_name);
    let icon_style = if failed {
        Style::default().fg(theme::error())
    } else if active {
        Style::default().fg(theme::accent())
    } else {
        Style::default().fg(theme::muted())
    };
    let text_style = if failed {
        Style::default().fg(theme::error())
    } else if active {
        Style::default().fg(theme::text())
    } else {
        Style::default().fg(theme::text_dim())
    };

    let width = usize::from(width);
    let rail = truncate_end(rail, width);
    let rail_width = Span::raw(&rail).width();
    let icon_budget = width.saturating_sub(rail_width);
    let icon = truncate_end(&icon, icon_budget);
    let duration = (!legacy && count == 1)
        .then_some(call.duration_ms)
        .flatten()
        .filter(|_| width >= 48)
        .map(|milliseconds| format!("  {}", format_elapsed(milliseconds as u128)))
        .unwrap_or_default();
    let remaining = icon_budget
        .saturating_sub(Span::raw(&icon).width())
        .saturating_sub(Span::raw(&duration).width());
    let detail = if count > 1 {
        format!("{} ×{count}", compact_tool_pattern_detail(call))
    } else if legacy {
        tool_activity_detail_legacy(call)
    } else {
        tool_activity_detail(call)
    };
    let text = task::fit(&detail, remaining);

    Line::from(vec![
        Span::styled(
            rail,
            Style::default().fg(if legacy {
                theme::muted()
            } else {
                theme::surface_color()
            }),
        ),
        Span::styled(icon, icon_style),
        Span::styled(text, text_style),
        Span::styled(duration, Style::default().fg(theme::muted())),
    ])
}

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

fn tool_pattern_label(name: &str) -> String {
    match name {
        "apply_file_edits" => "Edit files".into(),
        "search_workspace" => "Search files".into(),
        "search_artifact" => "Search output".into(),
        "web_search" => "Search web".into(),
        "search_tools" => "Find tools".into(),
        "task_notes" => "Update notes".into(),
        "context_history" => "Read context".into(),
        "read_file" | "read_files" => "Read files".into(),
        "read_artifact" => "Read output".into(),
        "read_document" => "Read document".into(),
        "web_read" => "Read web".into(),
        "run_shell" | "shell_job" => "Shell".into(),
        "list_files" => "List files".into(),
        "analyze_data" => "Analyze data".into(),
        "computer_use" | "desktop_control" => "Computer".into(),
        "activate_capability" => "Activate capability".into(),
        _ => humanize_tool_name(name),
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

fn compact_tool_pattern_detail(call: &crate::model::ConversationToolCall) -> String {
    tool_pattern_label(&call.name)
}

fn tool_icon(name: &str) -> &'static str {
    // Font Awesome's Nerd Font codepoints cover the mockup's pixel icons in a
    // terminal cell. The active terminal font controls their final shape.
    match name {
        "apply_file_edits" | "task_notes" => "\u{f040}",
        "search_workspace" | "search_artifact" | "search_tools" | "web_search" => "\u{f002}",
        "read_file" | "read_files" | "read_artifact" | "read_document" | "web_read" => "\u{f02d}",
        "run_shell" | "shell_job" => "\u{f120}",
        "list_files" => "\u{f07b}",
        "analyze_data" => "\u{f080}",
        "computer_use" | "desktop_control" => "\u{f108}",
        "activate_capability" => "\u{f0e7}",
        "context_history" => "\u{f1da}",
        _ => "\u{f013}",
    }
}

fn tool_icon_legacy(name: &str) -> &'static str {
    match name {
        "apply_file_edits" | "task_notes" => "✎",
        "search_workspace" | "search_artifact" | "search_tools" | "web_search" => "⌕",
        "read_file" | "read_files" | "read_artifact" | "read_document" | "web_read" => "▤",
        "run_shell" | "shell_job" => "⌘",
        "list_files" => "≡",
        "analyze_data" => "▦",
        "computer_use" | "desktop_control" => "◇",
        "activate_capability" => "◇",
        "context_history" => "↺",
        _ => "·",
    }
}

fn tool_activity_detail(call: &crate::model::ConversationToolCall) -> String {
    let title = tool_activity_title(call);
    match tool_activity_summary(call) {
        Some(summary) if !summary.is_empty() => format!("{title} · {summary}"),
        _ => title,
    }
}

fn tool_activity_detail_legacy(call: &crate::model::ConversationToolCall) -> String {
    let title = tool_activity_title_legacy(call);
    match tool_activity_summary_legacy(call) {
        Some(summary) if !summary.is_empty() => format!("{title} · {summary}"),
        _ => title,
    }
}

pub(in crate::ui) fn tool_activity_title(call: &crate::model::ConversationToolCall) -> String {
    tool_activity_title_with_style(call, false)
}

fn tool_activity_title_legacy(call: &crate::model::ConversationToolCall) -> String {
    tool_activity_title_with_style(call, true)
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
                "Reading"
            } else {
                "Read"
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
                "Reading web source"
            } else {
                "Read web source"
            }
        }
        "run_shell" => {
            if active {
                "Running command"
            } else {
                "Ran command"
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

pub(in crate::ui) fn tool_activity_summary(
    call: &crate::model::ConversationToolCall,
) -> Option<String> {
    tool_activity_summary_with_style(call, false)
}

fn tool_activity_summary_legacy(call: &crate::model::ConversationToolCall) -> Option<String> {
    tool_activity_summary_with_style(call, true)
}

fn tool_activity_summary_with_style(
    call: &crate::model::ConversationToolCall,
    legacy: bool,
) -> Option<String> {
    let explicit_detail = (!legacy)
        .then_some(call.detail.as_deref())
        .flatten()
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
        .map(str::to_owned);
    let mut summary = explicit_detail
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
        .filter(|value| !value.trim().is_empty());

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

fn tool_status_glyph(call: &crate::model::ConversationToolCall) -> &'static str {
    match call.status {
        ToolCallStatus::Preparing => "◌",
        ToolCallStatus::AwaitingPermission => "?",
        ToolCallStatus::Running => "●",
        ToolCallStatus::Completed => "✓",
        ToolCallStatus::Failed => "×",
        ToolCallStatus::Cancelled | ToolCallStatus::Interrupted => "—",
        ToolCallStatus::TimedOut => "!",
        ToolCallStatus::Suppressed => "·",
    }
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
        (None, false) => compact_tool_text(query, 88),
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

#[derive(Clone, Copy)]
pub(in crate::ui) enum WorkEvent<'a> {
    Reasoning(&'a str),
    Tool(&'a crate::model::ConversationToolCall),
}

pub(in crate::ui) enum ActivityRow<'a> {
    Reasoning(String),
    Tool {
        call: &'a crate::model::ConversationToolCall,
        count: usize,
    },
}

pub(super) fn chronological_activity_rows<'a>(events: &[WorkEvent<'a>]) -> Vec<ActivityRow<'a>> {
    let mut rows = Vec::new();
    for event in events {
        match *event {
            WorkEvent::Reasoning(summary) => {
                rows.extend(
                    reasoning_summary_items(summary)
                        .into_iter()
                        .map(ActivityRow::Reasoning),
                );
            }
            WorkEvent::Tool(call) => {
                if matches!(call.status, ToolCallStatus::Suppressed) {
                    continue;
                }
                if let Some(ActivityRow::Tool {
                    call: previous,
                    count,
                }) = rows.last_mut()
                    && previous.name == call.name
                    && tool_call_status_bucket(previous) == 0
                    && tool_call_status_bucket(call) == 0
                    && previous.label == call.label
                    && previous.detail == call.detail
                    && previous.arguments == call.arguments
                {
                    *count += 1;
                } else {
                    rows.push(ActivityRow::Tool { call, count: 1 });
                }
            }
        }
    }
    rows
}

fn activity_reasoning_lines(
    item: &str,
    width: u16,
    rail: &str,
    legacy: bool,
) -> Vec<Line<'static>> {
    let rendered = markdown_lines(item);
    if rendered.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    for (index, line) in rendered.into_iter().enumerate() {
        let prefix = if index == 0 {
            format!("{rail}● ")
        } else {
            "  │   ".to_owned()
        };
        lines.extend(prefixed_wrapped_line(
            Span::styled(
                prefix,
                Style::default().fg(if legacy {
                    theme::accent()
                } else {
                    theme::surface_color()
                }),
            ),
            line.style(Style::default().fg(theme::text())),
            width,
        ));
    }
    lines
}

pub(in crate::ui) fn tool_group_lines(events: &[WorkEvent<'_>], width: u16) -> Vec<Line<'static>> {
    let calls = events
        .iter()
        .filter_map(|event| match *event {
            WorkEvent::Tool(call) if !matches!(call.status, ToolCallStatus::Suppressed) => {
                Some(call)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if calls.is_empty() {
        return Vec::new();
    }

    let rows = chronological_activity_rows(events);
    if rows.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    for row in &rows {
        let rail = "  │ ";
        match row {
            ActivityRow::Reasoning(item) => {
                lines.extend(activity_reasoning_lines(item, width, rail, false));
            }
            ActivityRow::Tool { call, count } => {
                lines.push(tool_activity_line_count(call, width, *count, rail));
            }
        }
    }

    lines
}

const LEGACY_COLLAPSED_ACTIVITY_EVENT_LIMIT: usize = 6;

pub(super) fn tool_group_lines_legacy(
    events: &[WorkEvent<'_>],
    width: u16,
    expanded: bool,
) -> Vec<Line<'static>> {
    let calls = events
        .iter()
        .filter_map(|event| match *event {
            WorkEvent::Tool(call) if !matches!(call.status, ToolCallStatus::Suppressed) => {
                Some(call)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if calls.is_empty() {
        return Vec::new();
    }

    let rows = chronological_activity_rows(events);
    let needs_attention = calls
        .iter()
        .filter(|call| !matches!(call.status, ToolCallStatus::Completed))
        .count();
    let visible_rows = if expanded || needs_attention > 0 {
        rows.len()
    } else {
        rows.len().min(LEGACY_COLLAPSED_ACTIVITY_EVENT_LIMIT)
    };
    let hidden_rows = rows.len().saturating_sub(visible_rows);
    let mut lines = Vec::new();
    if hidden_rows > 0 {
        let overflow = format!(
            "  │ … {hidden_rows} earlier event{}",
            if hidden_rows == 1 { "" } else { "s" }
        );
        lines.push(Line::styled(
            truncate_end(&overflow, usize::from(width)),
            Style::default().fg(theme::muted()),
        ));
    }

    for row in rows.iter().skip(hidden_rows) {
        let rail = "  │ ";
        match row {
            ActivityRow::Reasoning(item) => {
                lines.extend(activity_reasoning_lines(item, width, rail, true));
            }
            ActivityRow::Tool { call, count } => {
                lines.push(tool_activity_line_count_with_style(
                    call, width, *count, rail, true,
                ));
            }
        }
    }
    lines
}

pub(in crate::ui) fn reasoning_summary_items(summary: &str) -> Vec<String> {
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

pub(in crate::ui) fn reasoning_summary_render_lines(
    summary: &str,
    width: u16,
    first_prefix: &str,
    continuation_prefix: &str,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for item in reasoning_summary_items(summary) {
        for (index, line) in markdown_lines(&item).into_iter().enumerate() {
            let prefix = if index == 0 {
                first_prefix
            } else {
                continuation_prefix
            };
            lines.extend(prefixed_wrapped_line(
                Span::styled(
                    prefix.to_owned(),
                    Style::default().fg(theme::surface_color()),
                ),
                line.style(Style::default().fg(theme::text_dim())),
                width,
            ));
        }
    }
    lines
}

#[cfg(test)]
mod tool_summary_layout_tests {
    use super::*;

    fn call(status: ToolCallStatus) -> crate::model::ConversationToolCall {
        crate::model::ConversationToolCall {
            id: "summary-test".into(),
            index: None,
            call_id: None,
            name: "run_shell".into(),
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
    fn status_rows_show_semantic_tool_detail() {
        for status in [
            ToolCallStatus::Running,
            ToolCallStatus::Completed,
            ToolCallStatus::Failed,
            ToolCallStatus::Suppressed,
        ] {
            let line = tool_activity_line(&call(status), 100);
            let text = line.to_string();
            assert!(text.contains("command"), "{text}");
            assert!(!text.contains("run_shell"));
            assert!(text.contains("cargo check"), "{text}");
        }
    }

    #[test]
    fn narrow_unicode_summaries_fit_terminal_cells() {
        let mut call = call(ToolCallStatus::Completed);
        call.arguments = serde_json::json!({"purpose": "检查界面 🦀 ".repeat(50)}).to_string();
        for width in [0, 1, 4, 12, 24, 40, 80] {
            let line = tool_activity_line(&call, width);
            assert!(line.width() <= usize::from(width));
        }
    }

    #[test]
    fn activity_rows_use_compact_file_language() {
        let mut edit = call(ToolCallStatus::Completed);
        edit.name = "apply_file_edits".into();
        edit.arguments = serde_json::json!({
            "changes": [{
                "path": "src/ui.rs",
                "edits": [{
                    "kind": "replace",
                    "range": {"start": 1, "end": 48},
                    "text": (0..46).map(|_| "line").collect::<Vec<_>>().join("\n")
                }]
            }]
        })
        .to_string();
        assert_eq!(
            tool_activity_line(&edit, 100).to_string(),
            "    ✓ \u{f040} Edited · src/ui.rs +46 −48"
        );

        let mut search = call(ToolCallStatus::Completed);
        search.name = "search_workspace".into();
        search.arguments = serde_json::json!({
            "query": "elapsed_ms",
            "path": "src/ui.rs"
        })
        .to_string();
        assert_eq!(
            tool_activity_line(&search, 100).to_string(),
            "    ✓ \u{f002} Searched · src/ui.rs for elapsed_ms"
        );

        let mut context = call(ToolCallStatus::Completed);
        context.name = "context_history".into();
        assert_eq!(
            tool_activity_line(&context, 100).to_string(),
            "    ✓ \u{f1da} Read context"
        );
        context.status = ToolCallStatus::Failed;
        assert_eq!(
            tool_activity_line(&context, 100).to_string(),
            "    × \u{f1da} Failed · Read context"
        );

        let mut tools = call(ToolCallStatus::Completed);
        tools.name = "search_tools".into();
        assert_eq!(
            tool_activity_line(&tools, 100).to_string(),
            "    ✓ \u{f002} Found tools"
        );

        let mut unknown = call(ToolCallStatus::Completed);
        unknown.name = "mystery_plugin".into();
        assert_eq!(
            tool_activity_line(&unknown, 100).to_string(),
            "    ✓ \u{f013} Mystery plugin · Verify Rust changes"
        );
    }
}
