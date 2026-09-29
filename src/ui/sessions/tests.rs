use super::tools::*;
use super::*;

#[cfg(test)]
mod compact_tool_group_tests {
    use super::*;

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
        let lines = tool_group_lines(&events, 100);
        assert_eq!(lines.len(), 8); // distinct operations retain their transcript order
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
        for index in 0..8 {
            assert!(text.contains(&format!("Check {index}")), "{text}");
        }
        assert_eq!(tool_group_lines(&events, 100).len(), 8);
        for width in [0, 1, 2, 12, 24, 80] {
            assert!(
                tool_group_lines(&events, width)
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
        let mut calls = [
            call("1", "web_search"),
            call("2", "web_read"),
            call("3", "web_search"),
            call("4", "web_read"),
        ];
        for (call, (label, detail)) in calls.iter_mut().zip([
            ("Search", "searched source alpha"),
            ("Read", "read source alpha"),
            ("Search", "searched source beta"),
            ("Read", "read source beta"),
        ]) {
            call.label = Some(label.into());
            call.detail = Some(detail.into());
        }
        let events = calls.iter().map(WorkEvent::Tool).collect::<Vec<_>>();
        let text = tool_group_lines(&events, 100)
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!text.contains("Activity"));
        assert!(text.lines().all(|line| line.starts_with("  │ ")));
        let first_search = text.find("searched source alpha").expect("first search");
        let first_read = text.find("read source alpha").expect("first read");
        let second_search = text.find("searched source beta").expect("second search");
        let second_read = text.find("read source beta").expect("second read");
        assert!(first_search < first_read);
        assert!(first_read < second_search);
        assert!(second_search < second_read);
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
        let app = App {
            conversation: vec![
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
            ],
            ..App::default()
        };

        let transcript = transcript_text(&app, 100);
        let text = transcript
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        let reasoning_1 = text.find("Checking sources").expect("first reasoning");
        let search = text.find('\u{f002}').expect("search tool Nerd Font icon");
        let reasoning_2 = text
            .find("Cross-checking claims")
            .expect("second reasoning");
        let read = text.find('\u{f02d}').expect("read tool Nerd Font icon");
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

    #[test]
    fn streaming_reasoning_summary_uses_live_state_in_tool_trace() {
        let call = crate::model::ConversationToolCall {
            id: "live-search".into(),
            index: None,
            call_id: None,
            name: "web_search".into(),
            arguments: "{}".into(),
            status: ToolCallStatus::Running,
            label: Some("Search".into()),
            detail: Some("current query".into()),
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
        app.state.is_streaming = true;
        app.state.active_reasoning_entry_id = Some("reasoning".into());
        app.state.active_reasoning_summary = "**Live qualification check**".into();
        app.conversation = vec![
            ConversationEntry {
                id: "reasoning".into(),
                kind: ConversationKind::Reasoning {
                    content: String::new(),
                    summary: Some("Stale persisted summary".into()),
                },
            },
            ConversationEntry {
                id: "search".into(),
                kind: ConversationKind::ToolCall {
                    tool_call: call.clone(),
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
        assert!(text.contains("Live qualification check"), "{text}");
        assert!(!text.contains("Stale persisted summary"), "{text}");
        assert!(text.contains("current query"), "{text}");
        let reasoning = text.find("Live qualification check").unwrap();
        let search = text.find("current query").unwrap();
        assert!(reasoning < search);
    }
}
