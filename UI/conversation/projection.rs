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
/// Routine tool progress mirrors a real tool call, not a separate action.
/// Failures and suppressed calls remain explicit exceptional state even when
/// the session also contains other tools.
fn routine_tool_phase(activity: &ModelActivity) -> bool {
    matches!(activity.phase.as_str(), Some("tool" | "tool-complete"))
}

/// A provider can update the first activity row throughout a turn, while
/// appending the actual tool calls later. Find those real calls once per turn
/// so the projection remains linear even for long-running sessions.
fn turns_with_visible_tools(entries: &[ConversationEntry]) -> Vec<bool> {
    let mut has_tool = vec![false; entries.len()];
    let mut turn_start = 0;
    let mut any_tool = false;
    for (index, entry) in entries.iter().enumerate() {
        if matches!(entry.kind, ConversationKind::User { .. }) {
            has_tool[turn_start..index].fill(any_tool);
            turn_start = index;
            any_tool = false;
        }
        any_tool |= match &entry.kind {
            ConversationKind::ToolCall { tool_call } => {
                !matches!(tool_call.status, ToolCallStatus::Suppressed)
            }
            ConversationKind::Assistant { tool_calls, .. } => tool_calls
                .iter()
                .any(|tool| !matches!(tool.status, ToolCallStatus::Suppressed)),
            _ => false,
        };
    }
    has_tool[turn_start..].fill(any_tool);
    has_tool
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
) -> TraceView {
    TraceView {
        id: id.into(),
        title: title.into(),
        summary,
        status_label: None,
        metadata: None,
        icon,
        active,
        expanded: state.expanded_for(id, false),
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
    if state.individual_work_rows {
        for event in pending.drain(..) {
            let id = event.key().to_owned();
            let trace = event.trace();
            let active = trace.active;
            let failed = event.failed();
            let awaits_permission = event.awaits_permission();
            let summary = match trace.summary.as_deref().filter(|value| !value.is_empty()) {
                Some(summary) if summary != trace.title => {
                    format!("{} · {summary}", trace.title)
                }
                _ => trace.title.clone(),
            };
            let status_label = trace.attention_label().map(str::to_owned);
            let expanded = state.expanded_for(
                &id,
                failed || awaits_permission || (state.inspect_work && active),
            );
            items.push(DisplayItem::Activity {
                id: id.clone(),
                group: ActivityGroup {
                    id,
                    events: vec![event],
                    summary,
                    status_label,
                    active,
                    failed,
                    awaits_permission,
                    expanded,
                },
            });
        }
        return;
    }
    let id = format!("activity-group:{}", first.key());
    let active = pending.iter().any(|event| event.trace().active);
    let failed = pending.iter().any(ActivityEvent::failed);
    let awaits_permission = pending.iter().any(ActivityEvent::awaits_permission);
    // Prefer work requiring attention, then live work, then the latest event.
    // Completed tool-only groups must not lose their action to an "Activity" label.
    let lead = pending
        .iter()
        .rev()
        .find(|event| event.awaits_permission())
        .or_else(|| pending.iter().rev().find(|event| event.failed()))
        .or_else(|| {
            pending
                .iter()
                .rev()
                .find(|event| event.trace().attention_label().is_some())
        })
        .or_else(|| pending.iter().rev().find(|event| event.trace().active))
        .unwrap_or_else(|| pending.last().expect("nonempty group"));
    let trace = lead.trace();
    let summary = match trace.summary.as_deref().filter(|value| !value.is_empty()) {
        Some(summary) if summary != trace.title => format!("{} · {summary}", trace.title),
        _ => trace.title.clone(),
    };
    let expanded = state.expanded_for(
        &id,
        failed || awaits_permission || (state.inspect_work && active),
    );
    let mut labels = Vec::new();
    // Keep approval first when it coexists with a failed/background tool.
    if awaits_permission {
        labels.push("Awaiting approval");
    }
    for event in pending.iter() {
        if let Some(label) = event.trace().attention_label() {
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
    }
    let status_label = (!labels.is_empty()).then(|| labels.join(" · "));
    let events = std::mem::take(pending);
    items.push(DisplayItem::Activity {
        id: id.clone(),
        group: ActivityGroup {
            id,
            events,
            summary,
            status_label,
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
    let tool_backed_turn = turns_with_visible_tools(entries);
    for (index, entry) in entries.iter().enumerate() {
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
            ConversationKind::Activity { activity }
                if should_display_activity(activity)
                    && !(routine_tool_phase(activity) && tool_backed_turn[index]) =>
            {
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
    if summary.is_none() {
        return;
    }
    let id = format!("reasoning:{entry_id}");
    let mut sections = Vec::new();
    if let Some(summary) = &summary {
        sections.extend(details("Model summary", summary));
    }
    sections.extend(details("Reasoning", &content));
    let mut trace = trace(
        state,
        &id,
        "",
        summary.clone(),
        ConversationIcon::Reasoning,
        active,
        sections,
    );
    trace.expanded = false;
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
    state: &ConversationState,
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
    let prompt = match &entry.kind {
        ConversationKind::User { content } => {
            prompt_preview(content).map(|preview| PromptDisclosure {
                preview,
                expanded: state.expanded_for(&format!("entry:{}", entry.id), false),
            })
        }
        _ => None,
    };
    items.push(DisplayItem::Entry {
        id: format!("entry:{}", entry.id),
        entry: entry.clone(),
        controls,
        prompt,
    });
}

/// Bound previews by both characters and lines, independent of client viewport.
fn prompt_preview(content: &str) -> Option<String> {
    let mut end = content.len();
    let mut lines = 1;
    for (count, (offset, ch)) in content.char_indices().enumerate() {
        if count == 400 || (ch == '\n' && lines == 6) {
            end = offset;
            break;
        }
        if ch == '\n' {
            lines += 1;
        }
    }
    (end < content.len()).then(|| format!("{}…", content[..end].trim_end()))
}
