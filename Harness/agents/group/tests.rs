//! Group lifecycle tests with scripted runners in place of coordinators.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Result, bail};

use crate::agents::{
    AgentId,
    member::{
        AgentRole, MemberLauncher, MemberProgress, MemberRunner, MemberSpec, RunOutcome, RunReport,
    },
    task::{AgentTaskStatus, SpawnRequest},
};

use super::{AgentGroupPolicy, AgentGroupRuntime};
use crate::{agents::AgentGroupHandle, core::ToolCall};

struct ScriptedLauncher;

/// Echoes its input with a per-member run counter. Inputs containing
/// "block" run until cancelled.
struct ScriptedRunner {
    runs: usize,
}

impl MemberLauncher for ScriptedLauncher {
    fn launch(&self, _spec: &MemberSpec) -> Result<(AgentId, Box<dyn MemberRunner>)> {
        Ok((AgentId::new_v4(), Box::new(ScriptedRunner { runs: 0 })))
    }
}

impl MemberRunner for ScriptedRunner {
    fn run(
        &mut self,
        input: &str,
        cancel: Arc<AtomicBool>,
        on_progress: &mut dyn FnMut(MemberProgress<'_>),
    ) -> Result<RunReport> {
        self.runs += 1;
        on_progress(MemberProgress::Event {
            kind: "reasoning_started",
            detail: "scripted reasoning".into(),
        });
        if input.contains("fail") {
            bail!("scripted member failure");
        }
        if input.contains("tool call") {
            on_progress(MemberProgress::Event {
                kind: "tool_started",
                detail: "scripted tool".into(),
            });
            thread::sleep(Duration::from_millis(120));
            on_progress(MemberProgress::Event {
                kind: "tool_finished",
                detail: "scripted tool returned".into(),
            });
        }
        while input.contains("block") {
            if cancel.load(Ordering::Acquire) {
                bail!("cancelled");
            }
            thread::sleep(Duration::from_millis(5));
        }
        let usage = crate::core::Usage {
            input_tokens: Some(12),
            output_tokens: Some(5),
            estimated_cost_usd: Some(0.01),
            ..Default::default()
        };
        on_progress(MemberProgress::Usage {
            usage: &usage,
            event_key: "scripted:1",
        });
        let task = input
            .split("TASK-SPECIFIC ASSIGNMENT:\n")
            .nth(1)
            .unwrap_or(input)
            .split("\n\nRELEVANT SHARED FINDINGS:")
            .next()
            .unwrap_or(input)
            .trim();
        let context_note = if input.contains("RELEVANT SHARED FINDINGS") {
            " — RELEVANT SHARED FINDINGS"
        } else {
            ""
        };
        Ok(RunReport {
            outcome: if input.contains("wait for input") {
                RunOutcome::Paused
            } else {
                RunOutcome::Completed
            },
            summary: format!("{task}{context_note} #{}", self.runs),
        })
    }
}

fn runtime() -> AgentGroupRuntime {
    AgentGroupRuntime::new(Arc::new(ScriptedLauncher), AgentGroupPolicy::default())
}

fn request(prompt: &str, background: bool) -> SpawnRequest {
    SpawnRequest {
        role: AgentRole::Researcher,
        description: "inspect".into(),
        prompt: prompt.into(),
        background,
    }
}

fn never() -> AtomicBool {
    AtomicBool::new(false)
}

#[test]
fn member_results_are_integrated_into_group_findings() {
    let group = runtime();
    let (_, foreground) = group.spawn(request("scan", false), "m", None).unwrap();
    let task = group.wait(foreground, &never()).unwrap();
    assert_eq!(task.status, AgentTaskStatus::Reported);
    assert!(task.summary.as_deref().unwrap().ends_with("scan #1"));

    let (agent, background) = group.spawn(request("audit", true), "m", None).unwrap();
    let task = group.wait(background, &never()).unwrap();
    assert!(task.summary.as_deref().unwrap().ends_with("audit #1"));
    assert!(group.group_item().shared_findings.iter().any(|finding| {
        finding.member_id == agent.to_string() && finding.summary == "audit #1"
    }));
}

#[test]
fn follow_up_messages_reuse_the_member() {
    let group = runtime();
    let (agent, first) = group.spawn(request("scan", false), "m", None).unwrap();
    group.wait(first, &never()).unwrap();

    let follow_up = group.steer(agent, "dig deeper".into()).unwrap();
    let task = group.wait(follow_up, &never()).unwrap();
    assert!(task.summary.as_deref().unwrap().ends_with("dig deeper #2"));

    // The frontend projection names who did what, in order.
    use crate::model::AgentActivityKind::*;
    let view = group.group_item();
    assert_eq!(view.members.len(), 1);
    assert_eq!(view.members[0].status, "idle");
    assert!(
        view.members[0]
            .summary
            .as_deref()
            .unwrap()
            .ends_with("dig deeper #2")
    );
    let trail = view
        .activity
        .iter()
        .map(|entry| (entry.kind, entry.text.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        trail,
        [
            (Message, "scan"),
            (Finished, "scan #1"),
            (Steer, "dig deeper"),
            (Finished, "dig deeper #2"),
        ]
    );
}

#[test]
fn member_events_are_attributed_and_results_feed_relevant_shared_context() {
    let group = runtime();
    let group_id = group
        .create_group("Compare parser recovery behavior across providers".into())
        .unwrap();
    group.begin_group(&group_id).unwrap();
    let (member, first) = group
        .spawn(
            request("inspect parser recovery behavior", false),
            "m",
            None,
        )
        .unwrap();
    let first_result = group.wait(first, &never()).unwrap();
    assert_eq!(first_result.status, AgentTaskStatus::Reported);

    let (_, second) = group
        .spawn(request("verify parser recovery behavior", false), "m", None)
        .unwrap();
    let second_result = group.wait(second, &never()).unwrap();
    assert!(
        second_result
            .summary
            .as_deref()
            .unwrap_or_default()
            .contains("RELEVANT SHARED FINDINGS")
    );

    let view = group.group_item();
    assert_eq!(view.group_id, group_id);
    assert_eq!(view.status, "running");
    assert_eq!(view.shared_findings.len(), 2);
    assert!(view.events.iter().all(|event| event.group_id == group_id));
    assert!(
        view.events
            .windows(2)
            .all(|events| events[0].sequence < events[1].sequence)
    );
    let member_id = member.to_string();
    let first_id = first.to_string();
    assert!(view.events.iter().any(|event| {
        event.member_id.as_deref() == Some(member_id.as_str())
            && event.task_id.as_deref() == Some(first_id.as_str())
            && event.kind == "reasoning_started"
    }));
}

#[test]
fn group_usage_counts_member_and_coordinator_reports_once() {
    let group = runtime();
    let group_id = group
        .create_group("Integrate member and coordinator usage".into())
        .unwrap();
    group.begin_group(&group_id).unwrap();
    let (_, task_id) = group
        .spawn(request("collect usage", false), "m", None)
        .unwrap();
    group.wait(task_id, &never()).unwrap();

    let coordinator_usage = crate::core::Usage {
        input_tokens: Some(11),
        output_tokens: Some(7),
        estimated_cost_usd: Some(0.02),
        ..Default::default()
    };
    group.record_coordinator_usage("run:1", &coordinator_usage);
    group.record_coordinator_usage("run:1", &coordinator_usage);
    let usage = group.checkpoint().group.usage.clone();
    assert_eq!(usage.input_tokens, Some(23));
    assert_eq!(usage.output_tokens, Some(12));

    let view = group.group_item();
    assert!(view.events.iter().any(|event| {
        event.kind == "usage" && event.member_id.is_none() && event.task_id.is_none()
    }));
}

#[test]
fn concurrent_members_complete_out_of_order_and_failure_stays_in_group() {
    let group = runtime();
    let (_, slow) = group
        .spawn(request("block slow task", false), "m", None)
        .unwrap();
    let (_, fast) = group
        .spawn(request("quick task", false), "m", None)
        .unwrap();
    assert_eq!(
        group.wait(fast, &never()).unwrap().status,
        AgentTaskStatus::Reported
    );
    let fast_completion = group
        .group_item()
        .events
        .into_iter()
        .find(|event| event.kind == "task_completed")
        .unwrap();
    assert_eq!(
        fast_completion.task_id.as_deref(),
        Some(fast.to_string().as_str())
    );

    let (_, failed) = group
        .spawn(request("fail this task", false), "m", None)
        .unwrap();
    assert_eq!(
        group.wait(failed, &never()).unwrap().status,
        AgentTaskStatus::Failed
    );
    assert!(
        group
            .group_item()
            .events
            .iter()
            .any(|event| event.kind == "task_failed")
    );

    group.cancel_group();
    assert_eq!(group.group_item().status, "cancelled");
    assert_eq!(
        group.wait(slow, &never()).unwrap().status,
        AgentTaskStatus::Cancelled
    );
}

#[test]
fn members_can_be_reasoning_calling_a_tool_waiting_and_completed_together() {
    let group = runtime();
    let group_id = group
        .create_group("Coordinate independent checks".into())
        .unwrap();
    group.begin_group(&group_id).unwrap();
    let (_, reasoning) = group
        .spawn(request("block while reasoning", false), "m", None)
        .unwrap();
    let (_, tool_call) = group
        .spawn(request("tool call while assigned", false), "m", None)
        .unwrap();
    let (_, waiting) = group
        .spawn(request("wait for input", false), "m", None)
        .unwrap();
    let (_, completed) = group
        .spawn(request("quick completed check", false), "m", None)
        .unwrap();
    group.wait(waiting, &never()).unwrap();
    group.wait(completed, &never()).unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        let states = group
            .group_item()
            .members
            .into_iter()
            .map(|member| member.activity_state)
            .collect::<Vec<_>>();
        if ["reasoning", "tool_call", "waiting_for_input", "completed"]
            .iter()
            .all(|wanted| states.iter().any(|state| state == wanted))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "member states did not overlap: {states:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(
        group.wait(tool_call, &never()).unwrap().status,
        AgentTaskStatus::Reported
    );
    group.cancel_group();
    assert_eq!(
        group.wait(reasoning, &never()).unwrap().status,
        AgentTaskStatus::Cancelled
    );
}

#[test]
fn stop_cancel_and_replace_settle_group_work() {
    let group = runtime();
    let (agent, stopped) = group.spawn(request("block", true), "m", None).unwrap();
    group.stop(agent).unwrap();
    let task = group.wait(stopped, &never()).unwrap();
    assert_eq!(task.status, AgentTaskStatus::Cancelled);
    assert!(group.steer(agent, "again".into()).is_err());
    group.remove(agent).unwrap();
    let view = group.group_item();
    assert!(view.members.is_empty() && view.activity.is_empty());

    let (_, interrupted) = group.spawn(request("block", false), "m", None).unwrap();
    assert!(group.wait(interrupted, &AtomicBool::new(true)).is_err());

    let (_, replaced) = group.spawn(request("block", true), "m", None).unwrap();
    group.replace_group();
    assert!(group.wait(replaced, &never()).is_err());

    thread::sleep(Duration::from_millis(50));
}

#[test]
fn batch_prestart_launches_parallel_group_members() {
    let group = runtime();
    let handle = AgentGroupHandle::members(group.clone());
    let calls = [("fg", "block"), ("bg", "audit")].map(|(id, prompt)| ToolCall {
        id: id.into(),
        name: super::commands::AGENT_TOOL.into(),
        arguments: serde_json::json!({
            "description": prompt,
            "prompt": prompt,
            "role": "researcher"
        }),
    });
    handle.prestart(&calls, "m", None);
    // The foreground agent blocks, yet the background one already runs.
    let items = group.task_items();
    assert_eq!(items.len(), 2);
    group.replace_group();
}

#[test]
fn persisted_group_checkpoint_restores_paused_work_and_event_order() {
    let original = runtime();
    let group_id = original
        .create_group("Review persisted group restoration".into())
        .unwrap();
    original.begin_group(&group_id).unwrap();
    let (_, task_id) = original
        .spawn(request("block interrupted review", false), "m", None)
        .unwrap();
    let checkpoint = original.checkpoint();
    let checkpoint_revision = checkpoint.revision;
    let bytes = serde_json::to_vec(&checkpoint).unwrap();
    let checkpoint: super::AgentGroupCheckpoint = serde_json::from_slice(&bytes).unwrap();

    let restored = runtime();
    restored.restore_checkpoint(checkpoint).unwrap();
    original.replace_group();

    let view = restored.group_item();
    assert!(restored.checkpoint().revision > checkpoint_revision);
    assert_eq!(view.group_id, group_id);
    assert_eq!(view.status, "paused");
    assert!(
        view.events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert_eq!(
        restored
            .task_items()
            .iter()
            .find(|task| task.id == task_id.to_string())
            .unwrap()
            .status,
        "cancelled"
    );
    let resumed_context = restored.resume_context();
    assert!(resumed_context.contains("Review persisted group restoration"));
    assert!(resumed_context.contains("INTERRUPTED MEMBER TASKS"));
    restored.begin_group(&group_id).unwrap();
    restored.complete_group("Integrated group result".into());
    assert_eq!(
        restored.group_item().final_result.as_deref(),
        Some("Integrated group result")
    );
}

#[test]
fn group_members_are_not_limited_by_a_capacity_setting() {
    let group = runtime();
    for index in 0..16 {
        group
            .spawn(request(&format!("research task {index}"), true), "m", None)
            .unwrap();
    }
    assert_eq!(group.group_item().members.len(), 16);
}
