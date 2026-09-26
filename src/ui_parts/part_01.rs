fn prefixed_wrapped_line(
    prefix: Span<'static>,
    line: Line<'static>,
    width: u16,
) -> Vec<Line<'static>> {
    let line_style = line.style;
    let prefix_width = prefix.width();
    let width = width as usize;
    if prefix_width >= width {
        let mut visible = String::new();
        let mut used = 0usize;
        for ch in prefix.content.chars() {
            let cells = char_cell_width(ch);
            if used + cells > width {
                break;
            }
            visible.push(ch);
            used += cells;
        }
        return vec![Line::from(Span::styled(visible, prefix.style))];
    }
    let content_width = width - prefix_width;
    let continuation_indent = transcript_continuation_indent(&line, content_width);
    let mut rows = vec![vec![prefix.clone()]];
    let mut used = 0usize;
    let mut has_content = false;

    let new_row = |rows: &mut Vec<Vec<Span<'static>>>| {
        let mut row = vec![prefix.clone()];
        if continuation_indent > 0 {
            row.push(Span::raw(" ".repeat(continuation_indent)));
        }
        rows.push(row);
    };

    for span in line.spans {
        let style = line_style.patch(span.style);
        let text = span.content.into_owned();
        for run in text_runs(&text) {
            let whitespace = run.chars().all(char::is_whitespace);
            let run_width = Span::raw(run).width();
            if whitespace {
                if !has_content && rows.len() > 1 {
                    continue;
                }
                if used + run_width <= content_width {
                    push_styled_text(rows.last_mut().unwrap(), run, style);
                    used += run_width;
                    has_content = true;
                } else {
                    new_row(&mut rows);
                    used = continuation_indent;
                    has_content = false;
                }
                continue;
            }

            if has_content && used + run_width > content_width {
                new_row(&mut rows);
                used = continuation_indent;
                has_content = false;
            }
            if used + run_width <= content_width {
                push_styled_text(rows.last_mut().unwrap(), run, style);
                used += run_width;
                has_content = true;
                continue;
            }

            for ch in run.chars() {
                let value = ch.to_string();
                let cells = Span::raw(&value).width();
                if used + cells > content_width {
                    if has_content {
                        new_row(&mut rows);
                        used = continuation_indent;
                        has_content = false;
                    }
                    if used + cells > content_width {
                        push_styled_text(rows.last_mut().unwrap(), "…", style);
                        break;
                    }
                }
                push_styled_text(rows.last_mut().unwrap(), &value, style);
                used += cells;
                has_content = true;
            }
        }
    }

    rows.into_iter().map(Line::from).collect()
}

fn entry_lines(app: &App, entry: &ConversationEntry, width: u16) -> Vec<Line<'static>> {
    match &entry.kind {
        ConversationKind::User { content } => {
            let mut lines = vec![
                Line::from(Span::styled(
                    "▌ You",
                    Style::default()
                        .fg(theme::user())
                        .add_modifier(Modifier::BOLD),
                ))
                .style(Style::default().bg(theme::user_surface())),
            ];
            for line in content.lines() {
                lines.extend(prefixed_wrapped_line(
                    Span::styled("▌ ", Style::default().fg(theme::user())),
                    Line::from(Span::styled(
                        line.to_owned(),
                        Style::default().fg(theme::text()),
                    )),
                    width,
                ));
            }
            for line in &mut lines {
                line.style = line.style.bg(theme::user_surface());
                let padding = usize::from(width).saturating_sub(line.width());
                line.spans.push(Span::raw(" ".repeat(padding)));
            }
            lines
        }
        ConversationKind::Assistant {
            content,
            tool_calls,
        } => {
            let content =
                if app.state.active_assistant_entry_id.as_deref() == Some(entry.id.as_str()) {
                    &app.state.active_assistant_text
                } else {
                    content
                };
            let mut lines = if content.trim().is_empty() {
                Vec::new()
            } else {
                vec![Line::styled("◆ Yeet", theme::brand())]
            };
            for line in markdown_lines(content) {
                lines.extend(prefixed_wrapped_line(
                    Span::styled("  │ ", Style::default().fg(theme::border_dim())),
                    line,
                    width,
                ));
            }
            let tool_events = tool_calls.iter().map(WorkEvent::Tool).collect::<Vec<_>>();
            lines.extend(tool_group_lines(
                app,
                &tool_events,
                width,
                app.state.is_streaming,
            ));
            lines
        }
        ConversationKind::Reasoning { content, summary } => {
            let (content, summary) =
                if app.state.active_reasoning_entry_id.as_deref() == Some(entry.id.as_str()) {
                    (
                        app.state.active_reasoning_text.as_str(),
                        (!app.state.active_reasoning_summary.is_empty())
                            .then_some(app.state.active_reasoning_summary.as_str()),
                    )
                } else {
                    (content.as_str(), summary.as_deref())
                };
            let mut lines = vec![Line::from(vec![
                Span::styled("  ◇ ", Style::default().fg(theme::accent_warm())),
                Span::styled(
                    "Thinking",
                    Style::default()
                        .fg(theme::text_dim())
                        .add_modifier(Modifier::BOLD),
                ),
            ])];
            if let Some(summary) = summary {
                lines.extend(reasoning_summary_render_lines(
                    summary,
                    width,
                    "  │ • ",
                    "  │   ",
                ));
            }
            for line in markdown_lines(content) {
                lines.extend(prefixed_wrapped_line(
                    Span::styled("  │ ", Style::default().fg(theme::border_dim())),
                    line.style(Style::default().fg(theme::muted())),
                    width,
                ));
            }
            lines
        }
        ConversationKind::Activity { activity } => {
            let latest = app.state.active_activity_entry_id.as_deref() == Some(entry.id.as_str());
            vec![activity_line(app, activity, false, latest, width)]
        }
        ConversationKind::ToolCall { tool_call } => tool_group_lines(
            app,
            &[WorkEvent::Tool(tool_call)],
            width,
            app.state.is_streaming,
        ),
        ConversationKind::Skill {
            name,
            content,
            status,
        } => {
            let mut lines = vec![Line::from(vec![
                Span::styled("skill ", Style::default().fg(theme::accent())),
                Span::styled(name.clone(), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(
                    status
                        .as_deref()
                        .map(|value| format!(" · {value}"))
                        .unwrap_or_default(),
                    Style::default().fg(theme::muted()),
                ),
            ])];
            lines.extend(markdown_lines(content));
            lines
        }
        ConversationKind::Mcp {
            server,
            name,
            content,
            is_error,
        } => {
            let color = if *is_error {
                theme::error()
            } else {
                theme::accent()
            };
            let mut lines = vec![Line::from(Span::styled(
                format!("MCP {server}/{name}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ))];
            lines.extend(content.lines().map(|line| Line::from(line.to_owned())));
            lines
        }
        ConversationKind::System { content } => content
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    line.to_owned(),
                    Style::default().fg(theme::text_dim()),
                ))
            })
            .collect(),
    }
}

fn activity_line(
    app: &App,
    activity: &crate::model::ModelActivity,
    active: bool,
    show_elapsed: bool,
    width: u16,
) -> Line<'static> {
    let phase = activity.phase.as_str().unwrap_or_default();
    let (marker, marker_style, title_style) = if active {
        (
            "·",
            Style::default()
                .fg(theme::accent_hot())
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(theme::accent_hot())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        match phase {
            "done" => (
                "·",
                Style::default().fg(theme::muted()),
                Style::default().fg(theme::text_dim()),
            ),
            "failed" => (
                "!",
                Style::default()
                    .fg(theme::error())
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(theme::error()),
            ),
            "interrupted" => (
                "—",
                Style::default().fg(theme::accent()),
                Style::default().fg(theme::text_dim()),
            ),
            _ => (
                "·",
                Style::default().fg(theme::muted()),
                Style::default().fg(theme::muted()),
            ),
        }
    };

    let mut spans = vec![
        Span::styled(format!("  {marker} "), marker_style),
        Span::styled(activity.title.clone(), title_style),
    ];

    let width = width as usize;
    if width >= 42
        && let Some(detail) = activity.detail.as_deref().filter(|value| !value.is_empty())
    {
        let budget = if width >= 100 {
            52
        } else if width >= 72 {
            34
        } else {
            22
        };
        spans.push(Span::styled("  ", Style::default()));
        spans.push(Span::styled(
            truncate_middle(detail, budget),
            Style::default().fg(theme::muted()),
        ));
    }

    if width >= 64 && show_elapsed {
        let elapsed = if active {
            app.stream_elapsed()
        } else {
            app.latest_turn_duration()
        };
        if let Some(elapsed) = elapsed {
            spans.push(Span::styled(
                format!("  {}", format_elapsed(elapsed.as_millis())),
                Style::default().fg(theme::muted()),
            ));
        }
    }

    Line::from(spans)
}

#[cfg(test)]
fn tool_activity_line(call: &crate::model::ConversationToolCall, width: u16) -> Line<'static> {
    tool_activity_line_count(call, width, 1, "    ")
}

fn tool_activity_line_count(
    call: &crate::model::ConversationToolCall,
    width: u16,
    count: usize,
    rail: &str,
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
    let completed = matches!(call.status, ToolCallStatus::Completed);
    let icon = format!("{} {} ", tool_status_glyph(call), tool_icon(&call.name));
    let icon_style = if failed {
        Style::default().fg(theme::error())
    } else if active {
        Style::default().fg(theme::accent())
    } else if completed {
        Style::default().fg(theme::success())
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
    let remaining = icon_budget.saturating_sub(Span::raw(&icon).width());
    let detail = if count > 1 {
        format!("{} ×{count}", compact_tool_pattern_detail(call))
    } else {
        tool_activity_detail(call)
    };
    let text = task::fit(&detail, remaining);

    Line::from(vec![
        Span::styled(rail, Style::default().fg(theme::muted())),
        Span::styled(icon, icon_style),
        Span::styled(text, text_style),
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

const COLLAPSED_ACTIVITY_EVENT_LIMIT: usize = 6;

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
    match name {
        "apply_file_edits" => "✎",
        "search_workspace" | "search_artifact" | "search_tools" | "web_search" => "⌕",
        "read_file" | "read_files" | "read_artifact" | "read_document" | "web_read" => "▤",
        "run_shell" | "shell_job" => "⌘",
        "list_files" => "≡",
        "analyze_data" => "▦",
        "computer_use" | "desktop_control" => "◇",
        "activate_capability" => "◇",
        "task_notes" => "✎",
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

fn tool_activity_title(call: &crate::model::ConversationToolCall) -> String {
    let active = matches!(
        call.status,
        ToolCallStatus::Preparing | ToolCallStatus::AwaitingPermission | ToolCallStatus::Running
    );
    let fallback_verb = humanize_tool_name(&call.name);
    let verb = match call.name.as_str() {
        "apply_file_edits" => {
            if active { "Editing" } else { "Edited" }
        }
        "search_workspace" | "search_artifact" | "web_search" => {
            if active { "Searching" } else { "Searched" }
        }
        "search_tools" => {
            if active { "Finding tools" } else { "Found tools" }
        }
        "task_notes" => {
            if active { "Updating notes" } else { "Updated notes" }
        }
        "context_history" => {
            if active { "Reading context" } else { "Read context" }
        }
        "read_file" | "read_files" => {
            if active { "Reading" } else { "Read" }
        }
        "read_artifact" => {
            if active { "Reading output" } else { "Read output" }
        }
        "read_document" => {
            if active { "Reading document" } else { "Read document" }
        }
        "web_read" => {
            if active { "Reading web source" } else { "Read web source" }
        }
        "run_shell" => {
            if active { "Running command" } else { "Ran command" }
        }
        "shell_job" => {
            if active { "Checking command" } else { "Checked command" }
        }
        "list_files" => {
            if active { "Listing files" } else { "Listed files" }
        }
        "analyze_data" => {
            if active { "Analyzing data" } else { "Analyzed data" }
        }
        "computer_use" | "desktop_control" => {
            if active { "Using computer" } else { "Used computer" }
        }
        _ => fallback_verb.as_str(),
    };

    let outcome = match call.status {
        ToolCallStatus::Failed => Some("Failed"),
        ToolCallStatus::TimedOut => Some("Timed out"),
        ToolCallStatus::Cancelled => Some("Cancelled"),
        ToolCallStatus::Interrupted => Some("Interrupted"),
        _ => None,
    };
    outcome
        .map(|outcome| format!("{outcome} · {verb}"))
        .unwrap_or_else(|| verb.to_owned())
}

fn tool_activity_summary(call: &crate::model::ConversationToolCall) -> Option<String> {
    let mut summary = match call.name.as_str() {
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
    }
    .filter(|value| !value.trim().is_empty());

    if matches!(
        call.status,
        ToolCallStatus::Failed
            | ToolCallStatus::Cancelled
            | ToolCallStatus::Interrupted
            | ToolCallStatus::TimedOut
    ) && let Some(error) = call.error.as_deref().filter(|value| !value.trim().is_empty())
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

fn compact_number(value: u64) -> String {
    if value >= 1_000_000 {
        compact_scaled(value, 1_000_000, "M")
    } else if value >= 1_000 {
        compact_scaled(value, 1_000, "k")
    } else {
        value.to_string()
    }
}

fn compact_scaled(value: u64, divisor: u64, suffix: &str) -> String {
    let scaled = value as f64 / divisor as f64;
    if scaled >= 100.0 || (scaled.fract() * 10.0).round() == 0.0 {
        format!("{}{suffix}", scaled.round() as u64)
    } else {
        format!("{scaled:.1}{suffix}")
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    responsive::modal_rect(area, percent_x, percent_y)
}

#[cfg(test)]
mod suggestion_tests {
    use super::*;
    use ratatui::{
        buffer::Buffer,
        widgets::{ListState, StatefulWidget},
    };

    #[test]
    fn selection_highlights_the_whole_row_and_keeps_a_visible_marker() {
        let suggestions = vec![
            ("/help".into(), "Show help".into()),
            ("/models".into(), "Choose model".into()),
        ];
        let area = Rect::new(0, 0, 64, 4);
        let mut buffer = Buffer::empty(area);
        let mut state = ListState::default().with_selected(Some(1));
        StatefulWidget::render(suggestion_list(&suggestions), area, &mut buffer, &mut state);
        assert_eq!(buffer[(2, 2)].symbol(), "›");
        assert_eq!(buffer[(2, 1)].symbol(), " ");
        for x in 2..62 {
            assert_eq!(buffer[(x, 2)].bg, theme::selected_color(), "column {x}");
            assert_ne!(buffer[(x, 1)].bg, theme::selected_color());
        }
    }

    #[test]
    fn long_commands_have_a_gap_before_the_description_and_selection_scrolls() {
        let suggestions = vec![
            ("/help".into(), "Help".into()),
            ("/long-command-name".into(), "Description".into()),
        ];
        let area = Rect::new(0, 0, 64, 3);
        let mut buffer = Buffer::empty(area);
        let mut state = ListState::default().with_selected(Some(1));
        StatefulWidget::render(suggestion_list(&suggestions), area, &mut buffer, &mut state);
        let row: String = (0..area.width).map(|x| buffer[(x, 1)].symbol()).collect();
        assert!(row.contains("› /long-command-name  Description"), "{row}");
        assert_eq!(state.offset(), 1);
    }
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
            "    ✓ ✎ Edited · src/ui.rs +46 −48"
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
            "    ✓ ⌕ Searched · src/ui.rs for elapsed_ms"
        );

        let mut context = call(ToolCallStatus::Completed);
        context.name = "context_history".into();
        assert_eq!(
            tool_activity_line(&context, 100).to_string(),
            "    ✓ ↺ Read context"
        );
        context.status = ToolCallStatus::Failed;
        assert_eq!(
            tool_activity_line(&context, 100).to_string(),
            "    × ↺ Failed · Read context"
        );

        let mut tools = call(ToolCallStatus::Completed);
        tools.name = "search_tools".into();
        assert_eq!(
            tool_activity_line(&tools, 100).to_string(),
            "    ✓ ⌕ Found tools"
        );

        let mut unknown = call(ToolCallStatus::Completed);
        unknown.name = "mystery_plugin".into();
        assert_eq!(
            tool_activity_line(&unknown, 100).to_string(),
            "    ✓ · Mystery plugin · Verify Rust changes"
        );
    }
}

#[derive(Clone, Copy)]
enum WorkEvent<'a> {
    Reasoning(&'a str),
    Tool(&'a crate::model::ConversationToolCall),
}

enum ActivityRow<'a> {
    Reasoning(String),
    Tool {
        call: &'a crate::model::ConversationToolCall,
        count: usize,
    },
}

fn chronological_activity_rows<'a>(events: &[WorkEvent<'a>]) -> Vec<ActivityRow<'a>> {
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

fn activity_reasoning_lines(item: &str, width: u16, rail: &str) -> Vec<Line<'static>> {
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
            Span::styled(prefix, Style::default().fg(theme::accent())),
            line.style(Style::default().fg(theme::text())),
            width,
        ));
    }
    lines
}

fn tool_group_lines(
    _app: &App,
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
    if rows.is_empty() {
        return Vec::new();
    }

    let needs_attention = |call: &crate::model::ConversationToolCall| {
        !matches!(call.status, ToolCallStatus::Completed)
    };
    let attention = calls.iter().filter(|call| needs_attention(call)).count();

    let visible_rows = if expanded || attention > 0 {
        rows.len()
    } else {
        rows.len().min(COLLAPSED_ACTIVITY_EVENT_LIMIT)
    };
    let hidden_rows = rows.len().saturating_sub(visible_rows);
    let first_visible = hidden_rows;

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

    for row in rows.iter().skip(first_visible) {
        let rail = "  │ ";
        match row {
            ActivityRow::Reasoning(item) => {
                lines.extend(activity_reasoning_lines(item, width, rail));
            }
            ActivityRow::Tool { call, count } => {
                lines.push(tool_activity_line_count(call, width, *count, rail));
            }
        }
    }

    lines
}

fn reasoning_summary_items(summary: &str) -> Vec<String> {
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

fn reasoning_summary_render_lines(
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
                Span::styled(prefix.to_owned(), Style::default().fg(theme::border_dim())),
                line.style(Style::default().fg(theme::text_dim())),
                width,
            ));
        }
    }
    lines
}

#[cfg(test)]
mod compact_tool_group_tests {
    use super::*;

    #[test]
    fn activity_text_morphs_between_live_phases() {
        assert_eq!(
            morph_activity_text("Thinking", "Searching", 0.0),
            "Thinking"
        );
        assert_eq!(
            morph_activity_text("Thinking", "Searching", 1.0),
            "Searching"
        );
        let middle = morph_activity_text("Thinking", "Searching", 0.5);
        assert_ne!(middle, "Thinking");
        assert_ne!(middle, "Planning");
    }

    #[test]
    fn renders_readable_tool_summary_and_fits_narrow_screens() {
        let mut calls = (0..8)
            .map(|index| crate::model::ConversationToolCall {
                id: index.to_string(),
                index: None,
                call_id: None,
                name: "run_shell".into(),
                arguments: serde_json::json!({"purpose": format!("Check {index}")}).to_string(),
                status: ToolCallStatus::Completed,
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
            })
            .collect::<Vec<_>>();
        calls[0].status = ToolCallStatus::Failed;
        calls[1].status = ToolCallStatus::Running;
        let events = calls.iter().map(WorkEvent::Tool).collect::<Vec<_>>();
        let app = App::default();
        let lines = tool_group_lines(&app, &events, 100, true);
        assert_eq!(lines.len(), 3); // one row per adjacent status/action group
        let text = lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("Activity"));
        assert!(!text.contains("╭─"));
        assert!(!text.contains("├─"));
        assert!(!text.contains("╰─"));
        assert!(text.lines().all(|line| line.starts_with("  │ ")));
        assert!(!text.contains("pattern"));
        assert!(text.contains("Failed · Ran command"));
        assert!(text.contains("Running command"));
        assert!(text.contains("Shell ×6"));
        assert_eq!(tool_group_lines(&app, &events, 100, false).len(), 3);
        for width in [0, 1, 2, 12, 24, 80] {
            assert!(
                tool_group_lines(&app, &events, width, true)
                    .iter()
                    .all(|line| line.width() <= usize::from(width))
            );
        }
    }

    #[test]
    fn completed_tool_summary_preserves_non_adjacent_tool_order() {
        let call = |id: &str, name: &str| crate::model::ConversationToolCall {
            id: id.into(),
            index: None,
            call_id: None,
            name: name.into(),
            arguments: "{}".into(),
            status: ToolCallStatus::Completed,
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
        };
        let calls = [
            call("1", "web_search"),
            call("2", "web_read"),
            call("3", "web_search"),
            call("4", "web_read"),
        ];
        let events = calls.iter().map(WorkEvent::Tool).collect::<Vec<_>>();
        let app = App::default();
        let text = tool_group_lines(&app, &events, 100, false)
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!text.contains("Activity"));
        assert!(text.lines().all(|line| line.starts_with("  │ ")));
        assert!(!text.contains("Search web ×2"));
        assert!(!text.contains("Read web ×2"));
        let rows = chronological_activity_rows(&events);
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn reasoning_and_tools_render_in_transcript_time_order() {
        let call = |id: &str, name: &str| crate::model::ConversationToolCall {
            id: id.into(),
            index: None,
            call_id: None,
            name: name.into(),
            arguments: "{}".into(),
            status: ToolCallStatus::Completed,
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
        };
        let mut app = App::default();
        app.conversation = vec![
            ConversationEntry {
                id: "user".into(),
                kind: ConversationKind::User {
                    content: "GPT-6 Sol rumors".into(),
                },
            },
            ConversationEntry {
                id: "reasoning-1".into(),
                kind: ConversationKind::Reasoning {
                    content: String::new(),
                    summary: Some("**Checking sources**".into()),
                },
            },
            ConversationEntry {
                id: "tool-1".into(),
                kind: ConversationKind::ToolCall {
                    tool_call: call("search", "web_search"),
                },
            },
            ConversationEntry {
                id: "reasoning-2".into(),
                kind: ConversationKind::Reasoning {
                    content: String::new(),
                    summary: Some("**Cross-checking claims**".into()),
                },
            },
            ConversationEntry {
                id: "tool-2".into(),
                kind: ConversationKind::ToolCall {
                    tool_call: call("read", "web_read"),
                },
            },
        ];

        let transcript = transcript_text(&app, 100);
        let text = transcript
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        let reasoning_1 = text.find("Checking sources").expect("first reasoning");
        let search = text.find("⌕").expect("search tool");
        let reasoning_2 = text
            .find("Cross-checking claims")
            .expect("second reasoning");
        let read = text.find("▤").expect("read tool");
        assert!(reasoning_1 < search);
        assert!(search < reasoning_2);
        assert!(reasoning_2 < read);
        assert!(!text.contains("◇ Reasoning"));
        assert!(!text.contains("**"));

        for expected in ["Checking sources", "Cross-checking claims"] {
            let line = transcript
                .lines
                .iter()
                .find(|line| line.to_string().contains(expected))
                .expect("reasoning summary line");
            assert!(
                line.spans.iter().any(|span| span.content.contains(expected)
                    && span.style.add_modifier.contains(Modifier::BOLD)),
                "{line:?}"
            );
        }
    }

    #[test]
    fn streaming_reasoning_summary_repairs_incomplete_markdown() {
        let mut app = App::default();
        app.state.active_reasoning_entry_id = Some("reasoning".into());
        app.state.active_reasoning_summary = "**Checking live sources".into();
        app.conversation = vec![ConversationEntry {
            id: "reasoning".into(),
            kind: ConversationKind::Reasoning {
                content: String::new(),
                summary: None,
            },
        }];

        let lines = entry_lines(&app, &app.conversation[0], 100);
        let text = lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("• Checking live sources"));
        assert!(!text.contains("**"));
        let summary_line = lines
            .iter()
            .find(|line| line.to_string().contains("Checking live sources"))
            .expect("streaming reasoning summary line");
        assert!(
            summary_line.spans.iter().any(|span| {
                span.content.contains("Checking live sources")
                    && span.style.add_modifier.contains(Modifier::BOLD)
            }),
            "{summary_line:?}"
        );
    }
}

#[cfg(test)]
mod composer_viewport_tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn long_draft_shows_visible_range_and_follows_cursor() {
        let mut app = App::default();
        app.input = (1..=12)
            .map(|n| format!("draft {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.cursor = app.input.chars().count();
        let mut terminal = Terminal::new(TestBackend::new(60, 5)).unwrap();
        terminal
            .draw(|frame| draw_input(frame, &mut app, frame.area()))
            .unwrap();
        let contents = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(contents.contains("Lines 10–12 of 12"));
        assert!(contents.contains("draft 12"));
        app.cursor = 0;
        terminal
            .draw(|frame| draw_input(frame, &mut app, frame.area()))
            .unwrap();
        let contents = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(contents.contains("Lines 1–3 of 12"));
        assert!(contents.contains("draft 1"));
    }

    #[test]
    fn multiline_draft_shows_compact_metadata_when_not_scrolled() {
        let mut app = App::default();
        app.input = "alpha\nbeta\ngamma".into();
        app.cursor = app.input.chars().count();
        let mut terminal = Terminal::new(TestBackend::new(60, 6)).unwrap();
        terminal
            .draw(|frame| draw_input(frame, &mut app, frame.area()))
            .unwrap();
        let contents = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(contents.contains("3 lines"));
        assert!(contents.contains("16 chars"));
    }
}

#[cfg(test)]
mod overall_layout_tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn history_status_explains_return_shortcut_at_narrow_widths() {
        let mut app = App::default();
        app.follow_tail = false;
        app.max_scroll = 200;
        app.scroll_y = 40;
        for width in [20, 40, 80] {
            let line = status_line(&app, width);
            assert!(line.to_string().contains("Ctrl+End"));
            assert!(line.width() <= width);
        }
    }

    #[test]
    fn missing_usage_is_not_shown_as_zero_percent() {
        let mut app = App::default();
        app.state.active_model_context_length = Some(100_000);
        app.state.current_context_tokens = None;
        assert!(status_line(&app, 120).to_string().contains("unavailable"));
    }

    #[test]
    fn runtime_status_anchor_uses_raised_surface() {
        let app = App::default();
        let line = status_line(&app, 80);
        assert_eq!(
            line.spans.first().and_then(|span| span.style.bg),
            Some(theme::surface_raised())
        );
    }

    #[test]
    fn wide_runtime_status_reads_like_an_instrument_panel() {
        let mut app = App::default();
        app.state.active_model = "openai/gpt-5.6-sol".into();
        app.state.active_model_context_length = Some(262_144);
        app.state.current_context_tokens = Some(58_300);
        app.state.active_reasoning_level = "high".into();

        let line = status_line(&app, 120);
        let text = line.to_string();
        assert!(text.contains("gpt-5.6-sol"));
        assert!(text.contains("CTX 22%"));
        assert!(text.contains("THINK high"));
        assert!(text.contains("MODE ask"));
        assert!(line.width() <= 120);
        assert_eq!(line.spans[1].style.bg, Some(theme::surface_raised()));
    }

    #[test]
    fn auxiliary_status_reports_usage_and_selection_shortcuts() {
        let mut app = App::default();
        app.state.token_usage.input_tokens = Some(12_500);
        app.state.token_usage.output_tokens = Some(2_300);
        app.state.token_usage.cached_input_tokens = Some(10_000);
        app.state.token_usage.cache_measured_input_tokens = Some(12_500);
        app.state.token_usage.model_calls = Some(4);

        let rich = status_aux_line(&app, 120).to_string();
        assert!(rich.contains("in 12.5k"));
        assert!(rich.contains("out 2.3k"));
        assert!(rich.contains("cache 80%"));
        assert!(rich.contains("calls 4"));
        for width in [12, 20, 40, 80] {
            assert!(status_aux_line(&app, width).width() <= width);
        }

        app.selection_start = Some((1, 1));
        app.selection_end = Some((2, 1));
        let selected = status_aux_line(&app, 80).to_string();
        assert!(selected.contains("Ctrl+C"));
        assert!(selected.contains("Esc"));
    }

    #[test]
    fn responsive_status_uses_second_row_only_when_space_allows() {
        assert_eq!(
            responsive::metrics(Rect::new(0, 0, 80, 24)).status_height,
            2
        );
        assert_eq!(
            responsive::metrics(Rect::new(0, 0, 60, 14)).status_height,
            1
        );
        assert_eq!(
            responsive::metrics(Rect::new(0, 0, 180, 40)).status_height,
            2
        );
    }

    #[test]
    fn shell_renders_from_small_terminal_to_ultrawide() {
        for (width, height) in [(32, 8), (60, 14), (80, 24), (120, 40), (180, 40)] {
            let mut app = App::default();
            app.conversation.push(ConversationEntry {
                id: "message".into(),
                kind: ConversationKind::User {
                    content: "Review these changes".into(),
                },
            });
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            assert!(app.transcript_area.3 > 0);
        }
    }
}
