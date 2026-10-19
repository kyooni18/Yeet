//! Compact tool traces and backend operation summaries.
use super::*;
pub(in crate::platforms::tui::ui) use crate::shared_ui::conversation::compact::{
    reasoning_summary_items, tool_activity_summary, tool_activity_title,
};
use crate::tui::ui::support::icons;

pub(in crate::platforms::tui::ui) fn tool_icon(name: &str) -> &'static str {
    // Font Awesome's Nerd Font codepoints cover the mockup's pixel icons in a
    // terminal cell. The active terminal font controls their final shape.
    match name {
        "apply_file_edits" => "\u{f040}",
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

    fn render(entries: Vec<ConversationEntry>, width: u16, expanded: bool) -> Vec<Line<'static>> {
        let mut app = App::default();
        app.conversation = entries;
        if expanded {
            app.apply_conversation_action(
                crate::shared_ui::conversation::ConversationAction::SetExpandAll(true),
            );
        }
        let view = app
            .application
            .conversation_state()
            .view(&app.conversation, &app.state);
        super::super::shared::content(&app, &view, width).0.lines
    }
    fn tool_entry(tool: &crate::model::ConversationToolCall) -> ConversationEntry {
        ConversationEntry {
            id: tool.id.clone(),
            kind: ConversationKind::ToolCall {
                tool_call: tool.clone(),
            },
        }
    }
    #[test]
    fn groups_collapse_unless_active_or_failed_and_fit_any_width() {
        let done = call("read_file", ToolCallStatus::Completed);
        let collapsed = render(vec![tool_entry(&done)], 80, false);
        assert_eq!(collapsed.len(), 1);
        let expanded = render(vec![tool_entry(&done)], 80, true);
        assert!(expanded.len() > 1);
        let running = call("apply_file_edits", ToolCallStatus::Running);
        let failed = call("search_workspace", ToolCallStatus::Failed);
        for tool in [&running, &failed] {
            let lines = render(vec![tool_entry(tool)], 80, false);
            if matches!(tool.status, ToolCallStatus::Failed) {
                assert!(lines.len() > 1, "failed tools open their details");
            } else {
                assert_eq!(lines.len(), 1, "running tools stay compact");
            }
            for width in [0, 1, 2, 8, 24, 40, 60] {
                assert!(
                    render(vec![tool_entry(tool)], width, false)
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
        let reasoning = |summary: &str| ConversationEntry {
            id: "reasoning".into(),
            kind: ConversationKind::Reasoning {
                content: String::new(),
                summary: Some(summary.into()),
            },
        };
        let text = render(
            vec![
                reasoning("**Analyzed recent logs**"),
                tool_entry(&read),
                tool_entry(&hidden),
            ],
            80,
            false,
        )
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
        assert!(text.contains("Analyzed recent logs"), "{text}");
        let summary = "Checking inputs\nComparing the actual provider formats";
        let expanded = render(vec![reasoning(summary)], 80, true)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            expanded.contains("Comparing the actual provider formats"),
            "{expanded}"
        );
        assert!(render(vec![tool_entry(&hidden)], 80, false).is_empty());
    }
    #[test]
    fn real_shell_call_replaces_redundant_live_activity_row() {
        let activity = ConversationEntry {
            id: "progress".into(),
            kind: ConversationKind::Activity {
                activity: crate::model::ModelActivity {
                    phase: serde_json::json!("tool"),
                    title: "Running command".into(),
                    detail: Some("cargo check".into()),
                    run_id: None,
                },
            },
        };
        let command = call("run_shell", ToolCallStatus::Running);
        let transcript = render(vec![activity, tool_entry(&command)], 80, false)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!transcript.contains("Running command"), "{transcript}");
        assert!(transcript.contains("cargo check"), "{transcript}");
        assert!(transcript.contains("running"), "{transcript}");
    }

    #[test]
    fn adjacent_work_is_one_row_each_without_duplicate_labels_or_completion_badges() {
        let reasoning = ConversationEntry {
            id: "reasoning".into(),
            kind: ConversationKind::Reasoning {
                content: "Full reasoning details".into(),
                summary: Some("Checking sources".into()),
            },
        };
        let mut done = call("run_shell", ToolCallStatus::Completed);
        done.duration_ms = Some(1200);
        let lines = render(vec![reasoning, tool_entry(&done)], 80, false);
        assert_eq!(lines.len(), 3);
        let text = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Checking sources"), "{text}");
        assert!(text.contains("cargo check"), "{text}");
        for noise in ["Thinking", "Reasoning", "Activity", "✓", "1.2 s"] {
            assert!(!text.contains(noise), "{text}");
        }
        let mut waiting = call("run_shell", ToolCallStatus::AwaitingPermission);
        waiting.error = Some("Approval required".into());
        let text = render(vec![tool_entry(&waiting)], 80, false)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.lines().next().unwrap().contains("Awaiting approval"),
            "{text}"
        );
    }
}

pub(in crate::platforms::tui::ui) fn spinner() -> &'static str {
    let tick = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() / 180)
        .unwrap_or(0);
    ["|", "/", "-", "\\"][(tick % 4) as usize]
}

/// Paint a single disclosure per event; child IDs follow the shared work order.
pub(super) fn activity_group_lines(
    group: &crate::shared_ui::conversation::ActivityGroup,
    width: u16,
    selected: Option<&str>,
) -> (Vec<Line<'static>>, Vec<(usize, String)>) {
    let mut rows = vec![(0, group.id.clone())];
    if let Some(trace) = group.single_trace() {
        return (
            trace_lines(&trace, width, selected == Some(&group.id)),
            rows,
        );
    }
    // Only collapsed groups repeat the lead action; the open header is a count.
    let count = group.events.len().to_string();
    let title = if group.expanded {
        format!("{count} actions")
    } else {
        format!(
            "{} {}",
            icons::conversation(group.events.last().unwrap().trace().icon),
            group.summary
        )
    };
    let status = if group.expanded {
        None
    } else {
        group.status_label.as_deref()
    };
    let mut lines = vec![work_row(
        &title,
        if group.expanded { None } else { Some(&count) },
        group.active && !group.expanded,
        status,
        Some(group.expanded),
        width,
        selected == Some(&group.id),
    )];
    if group.expanded {
        for event in &group.events {
            let trace = event.trace();
            if !trace.details.is_empty() || trace.metadata.is_some() {
                rows.push((lines.len(), trace.id.clone()));
            }
            let indent = " ".repeat(usize::from(width.min(2)));
            for line in trace_lines(trace, width.saturating_sub(2), selected == Some(&trace.id)) {
                let style = line.style;
                lines.push(
                    Line::from(
                        std::iter::once(Span::raw(indent.clone()))
                            .chain(line.spans)
                            .collect::<Vec<_>>(),
                    )
                    .style(style),
                );
            }
        }
    }
    (lines, rows)
}

/// One line of meaningful copy, with space reserved for exceptional status first.
fn work_row(
    title: &str,
    count: Option<&str>,
    active: bool,
    status: Option<&str>,
    expanded: Option<bool>,
    width: u16,
    selected: bool,
) -> Line<'static> {
    let width = usize::from(width);
    let status_style = Style::default().fg(if status.is_some_and(|s| s.contains("approval")) {
        theme::text()
    } else {
        theme::error_subtle()
    });
    let mut suffix = String::new();
    if let Some(status) = status {
        suffix.push_str(&format!(" · {status}"));
    }
    if let Some(count) = count {
        suffix.push_str(&format!(" · {count}"));
    }
    if active {
        suffix.push_str(" · running");
    }
    if let Some(expanded) = expanded {
        suffix.push_str(&format!(" {}", icons::chevron(expanded)));
    }
    let suffix = if suffix.is_empty() || width == 0 {
        String::new()
    } else {
        format!(" {}", task::fit(&suffix, width.saturating_sub(1)))
    };
    let budget = width.saturating_sub(Span::raw(&suffix).width());
    let (action, target) = title.split_once(" · ").unwrap_or((title, ""));
    let action = task::fit(action, budget);
    let remaining = budget.saturating_sub(Span::raw(&action).width());
    let target = if target.is_empty() {
        String::new()
    } else {
        task::fit(&format!(" · {target}"), remaining)
    };
    let line = Line::from(vec![
        Span::styled(action, Style::default().fg(theme::secondary())),
        Span::styled(target, Style::default().fg(theme::muted())),
        Span::styled(
            suffix,
            if status.is_some() {
                status_style
            } else {
                Style::default().fg(theme::muted())
            },
        ),
    ]);
    if selected {
        line.style(
            Style::default()
                .bg(theme::code_background())
                .add_modifier(Modifier::REVERSED),
        )
    } else {
        line
    }
}

pub(super) fn trace_lines(
    trace: &crate::shared_ui::conversation::TraceView,
    width: u16,
    selected: bool,
) -> Vec<Line<'static>> {
    use crate::shared_ui::conversation::ConversationIcon;
    let summary = trace
        .summary
        .as_deref()
        .filter(|text| !text.trim().is_empty());
    let title = match summary {
        Some(summary) if trace.icon == ConversationIcon::Reasoning || summary == trace.title => {
            summary.to_owned()
        }
        Some(summary) => format!("{} · {summary}", trace.title),
        None => trace.title.clone(),
    };
    // Completed status is intentionally empty; only waiting/error states add copy.
    let title = format!("{} {title}", icons::conversation(trace.icon));
    let status = trace.attention_label();
    let expandable = !trace.details.is_empty() || trace.metadata.is_some();
    let mut lines = vec![work_row(
        &title,
        None,
        trace.active,
        status,
        expandable.then_some(trace.expanded),
        width,
        selected,
    )];
    if trace.expanded && expandable {
        if let Some(metadata) = &trace.metadata {
            lines.extend(prefixed_wrapped_line(
                Span::raw("  "),
                Line::styled(
                    format!("Duration: {metadata}"),
                    Style::default().fg(theme::muted()),
                ),
                width,
            ));
        }
        for detail in &trace.details {
            let style = Style::default().fg(if detail.is_error {
                theme::error_subtle()
            } else {
                theme::secondary()
            });
            let body = if detail.monospaced {
                // Tool output is literal text, not Markdown (preserve code and diffs).
                detail
                    .content
                    .lines()
                    .map(|line| Line::raw(line.to_owned()))
                    .collect()
            } else {
                session_markdown_lines(&detail.content, usize::from(width.saturating_sub(2)))
            };
            for line in body {
                lines.extend(prefixed_wrapped_line(
                    Span::raw("  "),
                    line.style(style),
                    width,
                ));
            }
        }
    }
    lines
}
