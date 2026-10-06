//! One transcript ordering and visibility policy; adapters only paint its items.
use super::*;
use crate::harness::HarnessState;
use crate::model::{
    ConversationEntry, ConversationKind, ConversationToolCall, ModelActivity, ToolCallStatus,
};
use std::collections::HashSet;

fn terminal_label(text: &str) -> bool {
    matches!(
        text.trim().to_lowercase().as_str(),
        "done" | "completed" | "complete"
    )
}
pub fn should_display_reasoning(content: &str, summary: Option<&str>) -> bool {
    !(terminal_label(content) && summary.is_none_or(terminal_label)
        || content.trim().is_empty() && summary.is_some_and(terminal_label))
}
pub fn should_display_activity(activity: &ModelActivity) -> bool {
    if !activity
        .phase
        .as_str()
        .is_some_and(|phase| phase.trim().eq_ignore_ascii_case("done"))
    {
        return true;
    }
    let detail = activity
        .detail
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if terminal_label(&activity.title) && (detail.is_empty() || terminal_label(detail)) {
        return false;
    }
    !(matches!(
        activity.title.trim().to_lowercase().as_str(),
        "reasoning" | "추론" | "reasoning status"
    ) && !detail.is_empty()
        && terminal_label(detail))
}
fn readable(text: &str) -> String {
    text.trim().replace("****", "\n\n").replace("**", "")
}
fn useful_line(text: &str, last: bool) -> String {
    let lines: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if last {
        lines.last().copied().unwrap_or_default().to_owned()
    } else {
        lines.first().copied().unwrap_or_default().to_owned()
    }
}
fn details(title: &str, content: &str) -> Vec<DetailSection> {
    if content.is_empty() {
        Vec::new()
    } else {
        vec![DetailSection {
            title: title.into(),
            content: content.into(),
            monospaced: false,
            is_error: false,
            has_background: false,
        }]
    }
}
fn trace(
    state: &ConversationState,
    id: &str,
    title: &str,
    summary: Option<String>,
    icon: ConversationIcon,
    active: bool,
    sections: Vec<DetailSection>,
    reasoning: bool,
) -> TraceView {
    TraceView {
        id: id.into(),
        title: title.into(),
        summary,
        status_label: None,
        metadata: None,
        icon,
        active,
        expanded: state.expanded_for(id, reasoning),
        details: sections,
    }
}
fn append_tool(
    pending: &mut Vec<ActivityEvent>,
    seen: &mut HashSet<String>,
    tool: &ConversationToolCall,
    state: &ConversationState,
) {
    if matches!(tool.status, ToolCallStatus::Suppressed) || !seen.insert(tool.id.clone()) {
        return;
    }
    let view = project_tool(tool);
    let key = format!("tool:{}", tool.id);
    let trace = TraceView {
        id: key.clone(),
        title: view.title.clone(),
        summary: view.summary.clone(),
        status_label: view.status_label.clone(),
        metadata: view.metadata.clone(),
        icon: view.icon,
        active: view.active,
        expanded: state.expanded_for(&key, false),
        details: view.details.clone(),
    };
    pending.push(ActivityEvent::Tool {
        key,
        tool: tool.clone(),
        view,
        trace,
    });
}
fn flush(
    pending: &mut Vec<ActivityEvent>,
    items: &mut Vec<DisplayItem>,
    state: &ConversationState,
) {
    let Some(first) = pending.first() else {
        return;
    };
    let id = format!("activity-group:{}", first.key());
    let active = pending.iter().any(|event| event.trace().active);
    let failed = pending.iter().any(ActivityEvent::failed);
    let awaits_permission = pending.iter().any(ActivityEvent::awaits_permission);
    let summary = pending
        .iter()
        .rev()
        .find(|event| {
            event.trace().active
                && matches!(
                    event,
                    ActivityEvent::Tool { .. } | ActivityEvent::Activity { .. }
                )
        })
        .or_else(|| {
            pending
                .iter()
                .rev()
                .find(|event| matches!(event, ActivityEvent::Activity { .. }))
        })
        .map(|event| {
            let trace = event.trace();
            if let Some(summary) = trace.summary.as_deref().filter(|value| !value.is_empty()) {
                format!("{} · {summary}", trace.title)
            } else {
                trace.title.clone()
            }
        })
        .unwrap_or_else(|| "Activity".into());
    let expanded = state.expanded_for(
        &id,
        failed || awaits_permission || (state.inspect_work && active),
    );
    let events = std::mem::take(pending);
    items.push(DisplayItem::Activity {
        id: id.clone(),
        group: ActivityGroup {
            id,
            events,
            summary,
            active,
            failed,
            awaits_permission,
            expanded,
        },
    });
}
pub fn project(
    entries: &[ConversationEntry],
    harness: &HarnessState,
    state: &ConversationState,
) -> ConversationView {
    let last_user_id = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.kind, ConversationKind::User { .. }))
        .map(|entry| entry.id.clone());
    let last_assistant_id = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.kind, ConversationKind::Assistant { .. }))
        .map(|entry| entry.id.clone());
    let mut items = Vec::new();
    let mut pending = Vec::new();
    let mut seen = HashSet::new();
    for entry in entries {
        if harness.is_streaming
            && (harness.active_assistant_entry_id.as_deref() == Some(entry.id.as_str())
                || harness.active_reasoning_entry_id.as_deref() == Some(entry.id.as_str()))
        {
            continue;
        }
        match &entry.kind {
            ConversationKind::Reasoning { content, summary } => {
                flush(&mut pending, &mut items, state);
                if should_display_reasoning(content, summary.as_deref()) {
                    append_reasoning(
                        &mut items,
                        &entry.id,
                        content,
                        summary.as_deref(),
                        false,
                        state,
                    );
                }
            }
            ConversationKind::ToolCall { tool_call } => {
                append_tool(&mut pending, &mut seen, tool_call, state)
            }
            ConversationKind::Activity { activity } if should_display_activity(activity) => {
                let key = format!("activity:{}", entry.id);
                let active = harness.is_streaming
                    && harness.active_activity_entry_id.as_deref() == Some(entry.id.as_str());
                let trace = trace(
                    state,
                    &key,
                    &activity.title,
                    activity
                        .detail
                        .as_deref()
                        .map(|detail| useful_line(detail, true)),
                    ConversationIcon::Activity,
                    active,
                    details("Activity", activity.detail.as_deref().unwrap_or_default()),
                    false,
                );
                pending.push(ActivityEvent::Activity {
                    id: entry.id.clone(),
                    key,
                    activity: activity.clone(),
                    is_active: active,
                    trace,
                });
            }
            ConversationKind::Skill {
                name,
                content,
                status,
            } => {
                let key = format!("skill:{}", entry.id);
                let active = matches!(status.as_deref(), Some("running" | "started"));
                let mut trace = trace(
                    state,
                    &key,
                    name,
                    Some(useful_line(content, false)),
                    ConversationIcon::Skill,
                    active,
                    details("Skill", content),
                    false,
                );
                trace.status_label = status.clone();
                pending.push(ActivityEvent::Skill {
                    id: entry.id.clone(),
                    key,
                    name: name.clone(),
                    content: content.clone(),
                    status: status.clone(),
                    trace,
                });
            }
            ConversationKind::Mcp {
                server,
                name,
                content,
                is_error,
            } => {
                let key = format!("mcp:{}", entry.id);
                let mut trace = trace(
                    state,
                    &key,
                    &format!("{server} · {name}"),
                    Some(useful_line(content, false)),
                    if *is_error {
                        ConversationIcon::Error
                    } else {
                        ConversationIcon::Mcp
                    },
                    false,
                    details("MCP", content),
                    false,
                );
                trace.status_label = is_error.then(|| "Failed".into());
                for detail in &mut trace.details {
                    detail.is_error = *is_error;
                }
                pending.push(ActivityEvent::Mcp {
                    id: entry.id.clone(),
                    key,
                    server: server.clone(),
                    name: name.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                    trace,
                });
            }
            ConversationKind::Assistant {
                content,
                tool_calls,
            } => {
                for tool in tool_calls {
                    append_tool(&mut pending, &mut seen, tool, state);
                }
                if !content.is_empty() {
                    flush(&mut pending, &mut items, state);
                    append_entry(
                        &mut items,
                        entry,
                        state,
                        harness,
                        last_user_id.as_deref(),
                        last_assistant_id.as_deref(),
                    );
                } else if tool_calls.is_empty() {
                    flush(&mut pending, &mut items, state);
                }
            }
            ConversationKind::User { .. } | ConversationKind::System { .. } => {
                flush(&mut pending, &mut items, state);
                append_entry(
                    &mut items,
                    entry,
                    state,
                    harness,
                    last_user_id.as_deref(),
                    last_assistant_id.as_deref(),
                );
            }
            _ => {}
        }
    }
    if harness.is_streaming
        && (!harness.active_reasoning_text.trim().is_empty()
            || !harness.active_reasoning_summary.trim().is_empty())
    {
        flush(&mut pending, &mut items, state);
        append_reasoning(
            &mut items,
            harness
                .active_reasoning_entry_id
                .as_deref()
                .unwrap_or("live"),
            &harness.active_reasoning_text,
            Some(&harness.active_reasoning_summary),
            true,
            state,
        );
    }
    flush(&mut pending, &mut items, state);
    let has_streaming_activity = items.last().is_some_and(|item| {
        matches!(item,DisplayItem::Activity{group,..} if !group.events.is_empty())
            || matches!(
                item,
                DisplayItem::Reasoning {
                    is_active: true,
                    ..
                }
            )
    });
    let streaming_text = if harness.is_streaming && harness.active_assistant_entry_id.is_some() {
        harness.active_assistant_text.clone()
    } else {
        String::new()
    };
    ConversationView {
        items,
        last_user_id,
        last_assistant_id,
        preparing: harness.is_streaming && streaming_text.is_empty() && !has_streaming_activity,
        preparing_label: "Preparing response".into(),
        streaming_text,
        streaming: harness.is_streaming,
        error: harness.error_message.clone(),
        selected: state.selected.clone(),
    }
}
fn append_reasoning(
    items: &mut Vec<DisplayItem>,
    entry_id: &str,
    content: &str,
    summary: Option<&str>,
    active: bool,
    state: &ConversationState,
) {
    let content = readable(content);
    let summary = summary.map(readable).filter(|value| !value.is_empty());
    if content.is_empty() && summary.is_none() {
        return;
    }
    let id = format!("reasoning:{entry_id}");
    let mut sections = Vec::new();
    if let Some(summary) = &summary {
        sections.extend(details("Model summary", summary));
    }
    sections.extend(details("Reasoning", &content));
    let trace = trace(
        state,
        &id,
        if active { "Thinking" } else { "Reasoning" },
        Some(useful_line(summary.as_deref().unwrap_or(&content), true)),
        ConversationIcon::Reasoning,
        active,
        sections,
        true,
    );
    items.push(DisplayItem::Reasoning {
        id,
        content,
        summary,
        is_active: active,
        trace,
    });
}
fn append_entry(
    items: &mut Vec<DisplayItem>,
    entry: &ConversationEntry,
    _state: &ConversationState,
    harness: &HarnessState,
    last_user: Option<&str>,
    last_assistant: Option<&str>,
) {
    let mut controls = Vec::new();
    if matches!(
        entry.kind,
        ConversationKind::User { .. } | ConversationKind::Assistant { .. }
    ) {
        controls.push(MessageControl {
            label: "Copy message".into(),
            icon: ConversationIcon::Copy,
            action: ConversationAction::Copy(entry.id.clone()),
            enabled: true,
        });
        if !harness.is_streaming
            && last_user == Some(entry.id.as_str())
            && matches!(entry.kind, ConversationKind::User { .. })
        {
            controls.push(MessageControl {
                label: "Edit message".into(),
                icon: ConversationIcon::Edit,
                action: ConversationAction::Edit(entry.id.clone()),
                enabled: true,
            });
        }
        if !harness.is_streaming
            && last_assistant == Some(entry.id.as_str())
            && matches!(entry.kind, ConversationKind::Assistant { .. })
        {
            controls.push(MessageControl {
                label: "Regenerate response".into(),
                icon: ConversationIcon::Refresh,
                action: ConversationAction::Regenerate(entry.id.clone()),
                enabled: true,
            });
        }
    }
    items.push(DisplayItem::Entry {
        id: format!("entry:{}", entry.id),
        entry: entry.clone(),
        controls,
    });
}
