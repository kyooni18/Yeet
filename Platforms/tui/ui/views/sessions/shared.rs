//! Native typography and row geometry for the shared transcript projection.
use super::*;
use crate::shared_ui::conversation::{ConversationView, DisplayItem};

pub(super) fn content(
    app: &App,
    view: &ConversationView,
    width: u16,
) -> (Text<'static>, Vec<usize>, Vec<String>) {
    let mut lines = Vec::new();
    let mut rows = Vec::new();
    let mut ids = Vec::new();
    for item in &view.items {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        match item {
            DisplayItem::Entry { entry, .. } => {
                // Shared projection already resolves streaming content and embedded tools.
                match &entry.kind {
                    ConversationKind::User { content } => {
                        lines.extend(user_message_lines(content, width))
                    }
                    ConversationKind::Assistant { content, .. } => {
                        lines.extend(session_markdown_lines(content, width as usize))
                    }
                    _ => lines.extend(entry_lines(app, entry, width)),
                }
            }
            DisplayItem::Activity { id, group } => {
                rows.push(lines.len());
                ids.push(id.clone());
                let selected = view.selected.as_deref() == Some(id.as_str())
                    && !app.input_focused
                    && !app.sidebar_focus;
                lines.extend(tools::activity_group_lines(group, width, selected));
            }
            DisplayItem::Reasoning {
                id,
                content,
                summary,
                trace,
                ..
            } => {
                rows.push(lines.len());
                ids.push(id.clone());
                let selected = view.selected.as_deref() == Some(id.as_str())
                    && !app.input_focused
                    && !app.sidebar_focus;
                lines.extend(tools::reasoning_lines(
                    content,
                    summary.as_deref(),
                    trace,
                    width,
                    selected,
                ));
            }
        }
    }
    (Text::from(lines), rows, ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn work_geometry_uses_shared_stable_ids_and_disclosure() {
        let mut app = App::default();
        app.conversation = vec![ConversationEntry {
            id: "reasoning-1".into(),
            kind: ConversationKind::Reasoning {
                content: "Inspect the provider logs".into(),
                summary: None,
            },
        }];
        let view = app
            .application
            .conversation_state()
            .view(&app.conversation, &app.state);
        let (text, rows, ids) = content(&app, &view, 60);
        assert_eq!(ids, vec!["reasoning:reasoning-1"]);
        assert_eq!(rows, vec![0]);
        assert!(text.to_string().contains("Inspect the provider logs"));
        app.apply_conversation_action(crate::shared_ui::conversation::ConversationAction::Toggle(
            "reasoning:reasoning-1".into(),
        ));
        let view = app.application.conversation_projection().view;
        let (text, _, _) = content(&app, &view, 60);
        assert!(!text.to_string().contains(" │   Inspect the provider logs"));
    }
}
