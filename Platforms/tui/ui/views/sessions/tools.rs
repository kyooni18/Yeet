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
            assert!(lines.len() > 2, "active or failed groups stay expanded");
            for width in [24, 40, 60] {
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
}

pub(in crate::platforms::tui::ui) fn spinner() -> &'static str {
    let tick = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() / 180)
        .unwrap_or(0);
    ["|", "/", "-", "\\"][(tick % 4) as usize]
}

/// Paint shared work descriptions using terminal glyphs, wrapping and styles.
pub(super) fn activity_group_lines(
    group: &crate::shared_ui::conversation::ActivityGroup,
    width: u16,
    selected: bool,
) -> Vec<Line<'static>> {
    use crate::shared_ui::conversation::ActivityEvent;
    let width = usize::from(width);
    let secondary = Style::default().fg(theme::secondary());
    let muted = Style::default().fg(theme::muted());
    let rail = Style::default().fg(theme::hairline());
    let lead = if group.active {
        format!(" {} ", spinner())
    } else {
        " ".into()
    };
    let mut glyphs = Vec::new();
    for event in &group.events {
        let glyph = conversation_icon(event.trace().icon);
        if !glyphs.contains(&glyph) {
            glyphs.push(glyph);
        }
    }
    let glyphs = glyphs.into_iter().take(3).collect::<Vec<_>>().join(" ");
    let chevron = format!(" {}", icons::chevron(group.expanded));
    let budget = width.saturating_sub(
        Span::raw(&lead).width() + Span::raw(&glyphs).width() + 1 + Span::raw(&chevron).width(),
    );
    let mut lines = vec![Line::from(vec![
        Span::styled(lead, Style::default().fg(theme::text())),
        Span::styled(glyphs, secondary),
        Span::raw(" "),
        Span::styled(
            task::fit(&group.summary, budget),
            if group.active {
                Style::default()
                    .fg(theme::text())
                    .add_modifier(Modifier::BOLD)
            } else {
                secondary
            },
        ),
        Span::styled(chevron, muted),
    ])];
    if group.expanded {
        for event in &group.events {
            let trace = event.trace();
            let target = trace.summary.as_deref().unwrap_or_default();
            let action = format!(" {}", trace.title);
            let tail = if trace.active {
                format!(" {}", spinner())
            } else {
                String::new()
            };
            let used = 5 + Span::raw(&action).width() + Span::raw(&tail).width() + 1;
            lines.push(Line::from(vec![
                Span::styled(" │   ", rail),
                Span::styled(
                    conversation_icon(trace.icon),
                    if event.failed() {
                        Style::default().fg(theme::error_subtle())
                    } else {
                        secondary
                    },
                ),
                Span::styled(action, secondary),
                Span::styled(task::fit(target, width.saturating_sub(used)), secondary),
                Span::styled(tail, Style::default().fg(theme::text())),
            ]));
            // Skills and MCP responses already have native visible bodies. Tool
            // details keep the compact trace presentation used by the terminal.
            if !matches!(event, ActivityEvent::Tool { .. }) {
                for detail in &trace.details {
                    for line in session_markdown_lines(&detail.content, width.saturating_sub(5)) {
                        lines.extend(prefixed_wrapped_line(
                            Span::styled(" │   ", rail),
                            line.style(muted),
                            width as u16,
                        ));
                    }
                }
            }
        }
        lines.push(Line::default());
    }
    if selected {
        lines[0] = lines[0].clone().style(
            Style::default()
                .bg(theme::code_background())
                .add_modifier(Modifier::REVERSED),
        );
    }
    lines
}

pub(super) fn reasoning_lines(
    content: &str,
    summary: Option<&str>,
    trace: &crate::shared_ui::conversation::TraceView,
    width: u16,
    selected: bool,
) -> Vec<Line<'static>> {
    let width = usize::from(width);
    let text = summary
        .filter(|text| !text.trim().is_empty())
        .unwrap_or(content);
    let lead = if trace.active {
        format!(" {} ", spinner())
    } else {
        " ".into()
    };
    let chevron = format!(" {}", icons::chevron(trace.expanded));
    let title = format!(
        "{} · {}",
        if summary.is_some() {
            "Thinking"
        } else {
            "Reasoning"
        },
        trace.title
    );
    let budget = width.saturating_sub(Span::raw(&lead).width() + Span::raw(&chevron).width());
    let mut lines = vec![Line::from(vec![
        Span::styled(lead, Style::default().fg(theme::text())),
        Span::styled(
            task::fit(&title, budget),
            Style::default().fg(theme::secondary()),
        ),
        Span::styled(chevron, Style::default().fg(theme::muted())),
    ])];
    if trace.expanded {
        for line in
            session_markdown_lines(&text.replace("****", "**\n\n**"), width.saturating_sub(5))
        {
            lines.extend(prefixed_wrapped_line(
                Span::styled(" │   ", Style::default().fg(theme::hairline())),
                line.style(Style::default().fg(theme::muted())),
                width as u16,
            ));
        }
        lines.push(Line::default());
    }
    if selected {
        lines[0] = lines[0].clone().style(
            Style::default()
                .bg(theme::code_background())
                .add_modifier(Modifier::REVERSED),
        );
    }
    lines
}

fn conversation_icon(icon: crate::shared_ui::conversation::ConversationIcon) -> &'static str {
    use crate::shared_ui::conversation::ConversationIcon::*;
    match icon {
        Terminal => "\u{f120}",
        Edit => "\u{f040}",
        File => "\u{f15c}",
        Search => "\u{f002}",
        Folder => "\u{f07b}",
        Screen => "\u{f108}",
        Web => "\u{f02d}",
        Mcp | Tool => "\u{f013}",
        Skill => "\u{f0e7}",
        Reasoning => "\u{f0eb}",
        Info | Activity => "·",
        Error => "!",
        Copy => "\u{f0c5}",
        Refresh => "\u{f021}",
    }
}
