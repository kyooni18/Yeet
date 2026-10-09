use super::*;
use crate::harness::{HarnessCommand, HarnessState};
use crate::model::{AgentActivityItem, AgentGroupEventItem, AgentGroupItem, AgentMemberItem};
fn state(status: &str) -> HarnessState {
    HarnessState {
        agent_group: AgentGroupItem {
            group_id: "group".into(),
            status: status.into(),
            objective: Some("Shared objective".into()),
            members: vec![
                AgentMemberItem {
                    id: "planner".into(),
                    description: "Planner".into(),
                    status: "running".into(),
                    ..Default::default()
                },
                AgentMemberItem {
                    id: "stopped".into(),
                    status: "stopped".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        ..Default::default()
    }
}
#[test]
fn actions_keep_member_and_group_lifecycles_scoped_and_drafts_recoverable() {
    let mut runtime = state("running");
    let mut ui = AgentState::default();
    ui.apply(AgentAction::CreateGroup, &runtime);
    assert!(!ui.creating_group);
    ui.apply(AgentAction::Select(Some("planner".into())), &runtime);
    let effect = ui.apply(AgentAction::SubmitDraft("  steer here  ".into()), &runtime);
    assert!(
        matches!(effect.command, Some(HarnessCommand::MessageAgent { ref agent_id, ref message }) if agent_id == "planner" && message == "steer here")
    );
    assert_eq!(effect.submitted_text.as_deref(), Some("steer here"));
    ui.apply(AgentAction::Select(Some("stopped".into())), &runtime);
    let rejected = ui.apply(AgentAction::SubmitDraft("keep draft".into()), &runtime);
    assert!(rejected.command.is_none() && rejected.submitted_text.is_none());
    assert!(ui.apply(AgentAction::Stop, &runtime).command.is_none());
    assert!(
        matches!(ui.apply(AgentAction::Remove, &runtime).command, Some(HarnessCommand::RemoveAgent { agent_id: Some(ref id) }) if id == "stopped")
    );
    assert!(matches!(
        ui.apply(AgentAction::Stop, &runtime).command,
        Some(HarnessCommand::CancelAgentGroup { .. })
    ));
    runtime.agent_group.status = "paused".into();
    assert!(matches!(
        ui.apply(AgentAction::RunGroup, &runtime).command,
        Some(HarnessCommand::ResumeAgentGroup { .. })
    ));
    runtime.agent_group.status = "completed".into();
    ui.apply(AgentAction::CreateGroup, &runtime);
    let created = ui.apply(AgentAction::SubmitDraft("next objective".into()), &runtime);
    assert!(matches!(
        created.command,
        Some(HarnessCommand::CreateAgentGroup { .. })
    ));
    assert!(!ui.creating_group);
    runtime.is_streaming = true;
    assert!(
        ui.apply(AgentAction::SubmitDraft("wait".into()), &runtime)
            .submitted_text
            .is_none()
    );
}
#[test]
fn shared_controls_and_event_projection_keep_identity_and_status_precedence() {
    let mut runtime = state("running");
    runtime.agent_group.members[0].task_status = "failed".into();
    runtime.agent_group.members[0].activity_state = "reasoning".into();
    runtime.agent_group.events = vec![
        AgentGroupEventItem {
            group_id: "old".into(),
            sequence: 99,
            ..Default::default()
        },
        AgentGroupEventItem {
            group_id: "group".into(),
            sequence: 2,
            member_id: Some("planner".into()),
            detail: "second".into(),
            ..Default::default()
        },
        AgentGroupEventItem {
            group_id: "group".into(),
            sequence: 1,
            detail: "first".into(),
            ..Default::default()
        },
    ];
    let ui = AgentState {
        selected: Some("planner".into()),
        ..Default::default()
    };
    let view = ui.view(&runtime);
    assert_eq!(view.members[0].status.key, "failed");
    assert_eq!(view.members[0].select_action, AgentAction::Select(None));
    assert_eq!(view.feed.len(), 1);
    assert_eq!(view.feed[0].id, "group:2");
    assert!(!view.create_control.visible);
    assert_eq!(
        view.group_controls
            .iter()
            .filter(|control| control.visible)
            .map(|control| control.label.as_str())
            .collect::<Vec<_>>(),
        vec!["Cancel group", "Stop group", "Refresh state"]
    );
    assert_eq!(view.settings_control.action, AgentAction::AgentGroup);
    assert_eq!(view.accessible_label, "Agent Group");
    let empty = ui.view(&HarnessState::default());
    assert_eq!(empty.accessible_label, "Agent Group");
    assert_eq!(empty.empty_title, "No Agent Group yet");
    assert!(
        empty
            .sections
            .iter()
            .find(|section| section.kind == AgentSection::Objective)
            .unwrap()
            .visible
    );
    let action: AgentAction =
        serde_json::from_value(serde_json::json!({ "type": "select", "value": "planner" }))
            .unwrap();
    assert_eq!(action, AgentAction::Select(Some("planner".into())));
}
#[test]
fn legacy_incoming_activity_and_assignment_survive_shared_projection() {
    let mut runtime = state("running");
    runtime.agent_group.activity = vec![AgentActivityItem {
        to: Some("planner".into()),
        text: "Sweep feasible radii".into(),
        ..Default::default()
    }];
    let ui = AgentState {
        selected: Some("planner".into()),
        ..Default::default()
    };
    let view = ui.view(&runtime);
    assert_eq!(view.feed.len(), 1);
    assert_eq!(view.feed[0].actor, "Yeet → Planner");
    assert_eq!(view.headline, "Planner is working on: Sweep feasible radii");
    runtime.agent_group.members.clear();
    let refreshed = ui.view(&runtime);
    assert!(refreshed.state.selected.is_none());
    assert_eq!(refreshed.feed.len(), 1);
}
