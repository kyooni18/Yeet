//! Application-authored view content, status, controls and layout semantics.
use super::*;
use crate::harness::HarnessState;
use crate::model::{AgentActivityKind, AgentGroupItem, AgentMemberItem};
pub fn member_status(member: &AgentMemberItem) -> MemberStatus {
    let (key, label, tone) = match (
        member.status.as_str(),
        member.activity_state.as_str(),
        member.task_status.as_str(),
    ) {
        (_, _, "failed") => ("failed", "Failed", Tone::Error),
        (_, _, "cancelled") => ("cancelled", "Cancelled", Tone::Error),
        ("stopped", _, _) => ("stopped", "Stopped", Tone::Error),
        (_, "reasoning", _) => ("reasoning", "Reasoning", Tone::Active),
        (_, "tool_call", _) => ("tool call", "Using a tool", Tone::Active),
        (_, "provider_activity", _) => ("working", "Working", Tone::Active),
        (_, "waiting_for_input", _) => ("input", "Waiting for input", Tone::Waiting),
        (_, "queued", _) => ("queued", "Queued", Tone::Active),
        (_, _, "needs_verification") => ("review", "Needs review", Tone::Waiting),
        (_, _, "verified" | "reported" | "done") => ("done", "Completed", Tone::Complete),
        ("running", _, _) => ("running", "Running", Tone::Active),
        _ => ("idle", "Idle", Tone::Idle),
    };
    MemberStatus {
        key: key.into(),
        label: label.into(),
        tone,
    }
}
impl AgentState {
    pub fn view(&self, state: &HarnessState) -> AgentsView {
        let group = &state.agent_group;
        let mut normalized = self.clone();
        normalized.reconcile(group);
        let control = |label: &str, icon, enabled, action| AgentControl {
            label: label.into(),
            icon,
            enabled,
            visible: match &action {
                AgentAction::RunGroup
                | AgentAction::CancelGroup
                | AgentAction::StopGroup
                | AgentAction::InspectGroup => enabled,
                AgentAction::CreateGroup => enabled && !normalized.creating_group,
                _ => true,
            },
            action,
        };
        let running = group.status == "running";
        let paused = group.status == "paused";
        let group_controls = vec![
            control(
                if paused {
                    "Resume group"
                } else {
                    "Start group"
                },
                AgentIcon::Play,
                paused
                    || (group.objective.is_some()
                        && matches!(group.status.as_str(), "created" | "completed" | "failed")),
                AgentAction::RunGroup,
            ),
            control(
                "Cancel group",
                AgentIcon::Stop,
                running,
                AgentAction::CancelGroup,
            ),
            control(
                "Stop group",
                AgentIcon::Stop,
                running || paused,
                AgentAction::StopGroup,
            ),
            control(
                "Refresh state",
                AgentIcon::Refresh,
                !group.group_id.is_empty(),
                AgentAction::InspectGroup,
            ),
        ];
        let selected = normalized.selected_member(group);
        let selection_controls = vec![
            control(
                "Steer",
                AgentIcon::Message,
                selected.is_none_or(|member| member.status != "stopped"),
                AgentAction::Steer,
            ),
            control(
                "Stop",
                AgentIcon::Stop,
                selected.map_or(running, |member| member.status != "stopped"),
                AgentAction::Stop,
            ),
            control(
                "Remove",
                AgentIcon::Remove,
                selected.is_some()
                    || group
                        .members
                        .iter()
                        .any(|member| member.status == "stopped"),
                AgentAction::Remove,
            ),
        ];
        let members: Vec<_> = group
            .members
            .iter()
            .map(|member| MemberView {
                member: member.clone(),
                status: member_status(member),
                selected: normalized.selected.as_deref() == Some(member.id.as_str()),
                select_action: AgentAction::Select(
                    if normalized.selected.as_deref() == Some(member.id.as_str()) {
                        None
                    } else {
                        Some(member.id.clone())
                    },
                ),
            })
            .collect();
        let active_count = members
            .iter()
            .filter(|member| member.status.tone == Tone::Active)
            .count();
        let waiting_count = members
            .iter()
            .filter(|member| member.status.tone == Tone::Waiting)
            .count();
        let actor = |id: Option<&str>| {
            id.map_or_else(
                || "Group coordinator".to_owned(),
                |id| {
                    group
                        .members
                        .iter()
                        .find(|member| member.id == id)
                        .map_or_else(
                            || format!("Member {}", id.chars().take(8).collect::<String>()),
                            |member| member.description.clone(),
                        )
                },
            )
        };
        let mut feed: Vec<FeedEntry> = if !group.events.is_empty() {
            group
                .events
                .iter()
                .filter(|event| group.group_id.is_empty() || event.group_id == group.group_id)
                .map(|event| FeedEntry {
                    id: format!("{}:{}", event.group_id, event.sequence),
                    sequence: Some(event.sequence),
                    at: event.at.clone(),
                    kind: event.kind.clone(),
                    detail: event.detail.clone(),
                    member_id: event.member_id.clone(),
                    actor: actor(event.member_id.as_deref()),
                    icon: feed_icon(&event.kind),
                    running: false,
                    tool: None,
                })
                .collect()
        } else {
            group
                .activity
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    normalized.selected.as_deref().is_none_or(|id| {
                        item.from.as_deref() == Some(id) || item.to.as_deref() == Some(id)
                    })
                })
                .map(|(index, item)| {
                    let member_id = item.from.clone().or_else(|| item.to.clone());
                    FeedEntry {
                        id: format!("{}:{index}", item.at),
                        sequence: None,
                        at: item.at.clone(),
                        kind: serde_json::to_value(item.kind)
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .into(),
                        detail: item.text.clone(),
                        actor: {
                            let from = if item.from.is_none() {
                                if item.kind == AgentActivityKind::Steer {
                                    "You".into()
                                } else {
                                    "Yeet".into()
                                }
                            } else {
                                actor(item.from.as_deref())
                            };
                            if let Some(to) = item.to.as_deref() {
                                format!("{from} → {}", actor(Some(to)))
                            } else {
                                from
                            }
                        },
                        member_id,
                        icon: feed_icon(
                            &serde_json::to_value(item.kind).unwrap().as_str().unwrap(),
                        ),
                        running: item.kind == AgentActivityKind::Tool
                            && group.members.iter().any(|member| {
                                member.activity_state == "tool_call"
                                    && item.from.as_deref() == Some(member.id.as_str())
                                    && group
                                        .activity
                                        .iter()
                                        .rev()
                                        .find(|entry| {
                                            entry.from.as_deref() == Some(member.id.as_str())
                                        })
                                        .is_some_and(|entry| std::ptr::eq(entry, item))
                            }),
                        tool: item.tool.clone(),
                    }
                })
                .collect()
        };
        if !group.events.is_empty() {
            feed.sort_by_key(|entry| entry.sequence);
        }
        feed.reverse();
        feed.truncate(60);
        if let Some(id) = normalized
            .selected
            .as_ref()
            .filter(|_| !group.events.is_empty())
        {
            feed.retain(|entry| entry.member_id.as_ref() == Some(id));
        }
        AgentsView {
            state: normalized.clone(),
            group: group.clone(),
            group_status: group_status(&group.status),
            accessible_label: "Agent Group".into(),
            empty_title: "No Agent Group yet".into(),
            settings_control: control("Group Agent settings", AgentIcon::Settings, true, AgentAction::AgentGroup),
            create_control: control(if group.objective.is_some() { "New group" } else { "Create group" }, AgentIcon::Add, !running && !paused, AgentAction::CreateGroup),
            create_hint: if normalized.creating_group { "Creating group objective" } else if running { "Group is running" } else if paused { "Group is paused" } else { "New group" }.into(),
            composer_hint: if normalized.creating_group { "Describe the shared Group Agent objective".into() } else if let Some(member) = selected { format!("Steer {}", member.description) } else { "Steer the group coordinator".into() },
            headline: if let Some(member) = selected { format!("{} {} {}", member.description, member_verb(member), member_task(group, member).unwrap_or(member.description.as_str())) } else if group.objective.is_none() { "No Agent Group objective yet.".into() } else { format!("Group is {} · {active_count} active · {waiting_count} waiting · {} members", group.status, group.members.len()) },
            empty_message: if group.objective.is_some() { "This shared objective is ready. Start the group to let the coordinator plan and delegate member tasks." } else { "Create a Group Agent objective. The coordinator will plan and delegate member tasks around it." }.into(),
            findings: group.shared_findings.iter().map(|finding| FindingView { member_id: finding.member_id.clone(), task_id: finding.task_id.clone(), at: finding.at.clone(), summary: finding.summary.clone(), actor: actor(Some(&finding.member_id)) }).collect(),
            title: group
                .objective
                .clone()
                .unwrap_or_else(|| "Agent Group".into()),
            result: group
                .final_result
                .clone()
                .or_else(|| group.checkpoint_summary.clone()),
            members,
            feed,
            active_count,
            waiting_count,
            group_controls,
            selection_controls,
            sections: vec![
                SectionView { kind: AgentSection::Objective, layout: SectionLayout::FullWidth, label: "Shared objective".into(), visible: true },
                SectionView { kind: AgentSection::Result, layout: SectionLayout::FullWidth, label: if group.final_result.is_some() { "Group result" } else { "Latest checkpoint" }.into(), visible: group.final_result.is_some() || group.checkpoint_summary.is_some() },
                SectionView { kind: AgentSection::Members, layout: SectionLayout::Column, label: "Members".into(), visible: true },
                SectionView { kind: AgentSection::Activity, layout: SectionLayout::Column, label: "Activity".into(), visible: true },
                SectionView { kind: AgentSection::Findings, layout: SectionLayout::FullWidth, label: "Shared findings".into(), visible: !group.shared_findings.is_empty() },
            ],
        }
    }
}

fn feed_icon(kind: &str) -> AgentIcon {
    match kind {
        "tool" => AgentIcon::Tool,
        "finished" => AgentIcon::Complete,
        "failed" => AgentIcon::Error,
        "stopped" => AgentIcon::Stop,
        _ => AgentIcon::Message,
    }
}
pub fn member_verb(member: &AgentMemberItem) -> &'static str {
    match member_status(member).key.as_str() {
        "running" | "reasoning" | "working" | "queued" => "is working on:",
        "tool call" => "is calling a tool for:",
        "input" => "is waiting for input on:",
        "review" => "is waiting for review of:",
        "failed" => "failed:",
        "stopped" | "cancelled" => "stopped working on:",
        _ => "finished:",
    }
}

/// Initial coordinator assignment retained independently of display labels.
pub fn member_task<'a>(group: &'a AgentGroupItem, member: &AgentMemberItem) -> Option<&'a str> {
    group
        .activity
        .iter()
        .find(|entry| {
            entry.from.is_none()
                && entry.to.as_deref() == Some(member.id.as_str())
                && entry.kind == AgentActivityKind::Message
        })
        .map(|entry| entry.text.as_str())
}
pub fn group_status(status: &str) -> MemberStatus {
    let tone = match status {
        "running" | "queued" => Tone::Active,
        "paused" => Tone::Waiting,
        "failed" | "cancelled" | "stopped" => Tone::Error,
        "completed" => Tone::Complete,
        _ => Tone::Idle,
    };
    let label = status
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    MemberStatus {
        key: status.into(),
        label: if label.is_empty() {
            "Idle".into()
        } else {
            label
        },
        tone,
    }
}
