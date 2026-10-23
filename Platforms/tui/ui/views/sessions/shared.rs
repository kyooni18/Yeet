//! Native typography and row geometry for the shared transcript projection.
use super::*;
use crate::shared_ui::conversation::{ConversationView, DisplayItem};

#[cfg(test)]
pub(super) fn content(
    app: &App,
    view: &ConversationView,
    width: u16,
) -> (Text<'static>, Vec<usize>, Vec<String>) {
    let (text, rows, ids, _) = content_with_entries(app, view, width);
    (text, rows, ids)
}

pub(super) fn content_with_entries(
    app: &App,
    view: &ConversationView,
    width: u16,
) -> (
    Text<'static>,
    Vec<usize>,
    Vec<String>,
    Vec<(std::ops::Range<usize>, String)>,
) {
    let mut lines = Vec::new();
    let mut rows = Vec::new();
    let mut ids = Vec::new();
    let mut entry_ranges = Vec::new();
    let mut previous_was_entry = false;
    let mut previous_was_reasoning = None;
    for item in &view.items {
        let is_entry = matches!(item, DisplayItem::Entry { .. });
        let is_reasoning = matches!(item, DisplayItem::Reasoning { .. });
        if !lines.is_empty()
            && (is_entry
                || previous_was_entry
                || previous_was_reasoning.is_some_and(|previous| previous != is_reasoning))
        {
            lines.push(Line::default());
        }
        previous_was_entry = is_entry;
        previous_was_reasoning = Some(is_reasoning);
        match item {
            DisplayItem::Entry {
                id, entry, prompt, ..
            } => {
                let start = lines.len();
                // Shared projection already resolves streaming content and embedded tools.
                match &entry.kind {
                    ConversationKind::User { content } => {
                        let visible = prompt
                            .as_ref()
                            .filter(|p| !p.expanded)
                            .map_or(content.as_str(), |p| p.preview.as_str());
                        if let Some(prompt) = prompt {
                            rows.push(lines.len());
                            ids.push(id.clone());
                            let label = if prompt.expanded {
                                "▾ Show less"
                            } else {
                                "▸ Show full prompt"
                            };
                            let style = Style::default().fg(theme::muted());
                            let style = if !app.input_focused && view.selected.as_ref() == Some(id)
                            {
                                style.add_modifier(Modifier::REVERSED)
                            } else {
                                style
                            };
                            lines.push(Line::styled(truncate_end(label, width as usize), style));
                        }
                        lines.extend(user_message_lines(visible, width))
                    }
                    ConversationKind::Assistant { content, .. } => {
                        lines.extend(session_markdown_lines(content, width as usize))
                    }
                    _ => lines.extend(entry_lines(app, entry, width)),
                }
                entry_ranges.push((start..lines.len(), entry.id.clone()));
            }
            DisplayItem::Activity { group, .. } => {
                let selected = if !app.input_focused && !app.sidebar_focus {
                    view.selected.as_deref()
                } else {
                    None
                };
                let (work_lines, work_rows) = tools::activity_group_lines(group, width, selected);
                for (row, id) in work_rows {
                    rows.push(lines.len() + row);
                    ids.push(id);
                }
                lines.extend(work_lines);
            }
            DisplayItem::Reasoning {
                summary, is_active, ..
            } => {
                if let Some(summary) = summary {
                    let style = Style::default().fg(if *is_active {
                        theme::secondary()
                    } else {
                        theme::muted()
                    });
                    for line in summary.lines() {
                        lines.extend(prefixed_wrapped_line(
                            Span::raw("  "),
                            Line::styled(line.to_owned(), style),
                            width,
                        ));
                    }
                }
            }
        }
    }
    (Text::from(lines), rows, ids, entry_ranges)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_prompt_disclosure_has_clickable_and_keyboard_geometry() {
        let mut app = App::default();
        app.conversation = vec![ConversationEntry {
            id: "long-prompt".into(),
            kind: ConversationKind::User {
                content: format!("{}HIDDEN_END", "x".repeat(500)),
            },
        }];
        let view = app
            .application
            .conversation_state()
            .view(&app.conversation, &app.state);
        let (text, rows, ids) = content(&app, &view, 80);
        assert_eq!(ids, view.work_ids());
        assert_eq!(rows.len(), 1);
        assert!(text.lines[rows[0]].to_string().contains("Show full prompt"));
        assert!(!text.to_string().contains("HIDDEN_END"));
        app.apply_conversation_action(crate::shared_ui::conversation::ConversationAction::Toggle(
            ids[0].clone(),
        ));
        let view = app.application.conversation_projection().view;
        let (text, _, _) = content(&app, &view, 80);
        assert!(text.to_string().contains("Show less"));
        assert!(text.to_string().contains("HIDDEN_END"));
    }

    #[test]
    fn work_geometry_uses_shared_stable_ids_and_disclosure() {
        let mut app = App::default();
        app.conversation = vec![ConversationEntry {
            id: "reasoning-1".into(),
            kind: ConversationKind::Reasoning {
                content: "Private reasoning".into(),
                summary: Some("Inspect the provider logs".into()),
            },
        }];
        let view = app
            .application
            .conversation_state()
            .view(&app.conversation, &app.state);
        let (text, rows, ids) = content(&app, &view, 60);
        assert!(ids.is_empty());
        assert!(rows.is_empty());
        assert!(text.to_string().contains("Inspect the provider logs"));
        for width in [0, 1, 2, 8, 24, 80] {
            let (text, rows, ids) = content(&app, &view, width);
            assert!(rows.is_empty() && ids.is_empty());
            assert!(
                text.lines
                    .iter()
                    .all(|line| line.width() <= usize::from(width))
            );
        }

        app.apply_conversation_action(crate::shared_ui::conversation::ConversationAction::Toggle(
            "reasoning:reasoning-1".into(),
        ));
        let view = app.application.conversation_projection().view;
        let (text, _, _) = content(&app, &view, 60);
        assert_eq!(text.lines.len(), 1);
        assert!(!text.to_string().contains("Private reasoning"));
        assert!(text.to_string().contains("Inspect the provider logs"));
    }

    #[test]
    fn expanded_group_children_keep_selectable_rows_and_literal_tool_output() {
        use crate::shared_ui::conversation::ConversationAction;
        let mut app = App::default();
        app.input_focused = false;
        app.conversation = ["first", "second"]
            .into_iter()
            .map(|id| ConversationEntry {
                id: id.into(),
                kind: ConversationKind::ToolCall {
                    tool_call: serde_json::from_value(serde_json::json!({
                        "id": id, "name": "run_shell", "arguments": "{\"command\":\"cargo check\"}",
                        "status": "completed", "result": "literal **output**"
                    }))
                    .unwrap(),
                },
            })
            .collect();
        app.apply_conversation_action(ConversationAction::Select(Some("tool:second".into())));
        app.apply_conversation_action(ConversationAction::Toggle("tool:second".into()));
        let view = app.application.conversation_projection().view;
        let (text, rows, ids) = content(&app, &view, 40);
        assert_eq!(ids, view.work_ids());
        assert_eq!(rows.len(), 2);
        assert!(
            text.lines[rows[1]]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert!(text.to_string().contains("literal **output**"));
        assert!(text.lines.iter().all(|line| line.width() <= 40));
    }

    #[test]
    fn transcript_geometry_keeps_message_source_ids_for_native_controls() {
        let mut app = App::default();
        app.conversation = vec![ConversationEntry {
            id: "user-1".into(),
            kind: ConversationKind::User {
                content: "First line\nSecond line".into(),
            },
        }];
        let view = app
            .application
            .conversation_state()
            .view(&app.conversation, &app.state);
        let (_, _, _, ranges) = content_with_entries(&app, &view, 60);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].1, "user-1");
        assert!(!ranges[0].0.is_empty());
    }
}
