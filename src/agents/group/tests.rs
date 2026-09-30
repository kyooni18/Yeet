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

use super::{AgentGroupRuntime, AgentLimits};
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
        _on_progress: &mut dyn FnMut(MemberProgress<'_>),
    ) -> Result<RunReport> {
        self.runs += 1;
        while input.contains("block") {
            if cancel.load(Ordering::Acquire) {
                bail!("cancelled");
            }
            thread::sleep(Duration::from_millis(5));
        }
        Ok(RunReport {
            outcome: RunOutcome::Completed,
            summary: format!("{input} #{}", self.runs),
        })
    }
}

fn runtime() -> AgentGroupRuntime {
    AgentGroupRuntime::new(Arc::new(ScriptedLauncher), AgentLimits::default())
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
fn foreground_waits_and_background_completion_notifies() {
    let group = runtime();
    let (_, foreground) = group.spawn(request("scan", false), "m", None).unwrap();
    let task = group.wait(foreground, &never()).unwrap();
    assert_eq!(task.status, AgentTaskStatus::Reported);
    assert_eq!(task.summary.as_deref(), Some("scan #1"));
    assert!(group.take_notifications().is_empty());

    let (agent, background) = group.spawn(request("audit", true), "m", None).unwrap();
    group.wait(background, &never()).unwrap();
    let notices = group.take_notifications();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].message.contains(&agent.to_string()));
    assert!(notices[0].message.contains("audit #1"));
}

#[test]
fn follow_up_messages_reuse_the_member() {
    let group = runtime();
    let (agent, first) = group.spawn(request("scan", false), "m", None).unwrap();
    group.wait(first, &never()).unwrap();

    let follow_up = group.steer(agent, "dig deeper".into()).unwrap();
    let task = group.wait(follow_up, &never()).unwrap();
    assert_eq!(task.summary.as_deref(), Some("dig deeper #2"));
    assert_eq!(group.take_notifications().len(), 1);

    // The frontend projection names who did what, in order.
    use crate::model::AgentActivityKind::*;
    let view = group.group_item();
    assert_eq!(view.members.len(), 1);
    assert_eq!(view.members[0].status, "idle");
    assert_eq!(view.members[0].summary.as_deref(), Some("dig deeper #2"));
    let trail = view
        .activity
        .iter()
        .map(|entry| (entry.kind, entry.text.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        trail,
        [
            (Message, "inspect"),
            (Finished, "scan #1"),
            (Steer, "dig deeper"),
            (Finished, "dig deeper #2"),
        ]
    );
}

#[test]
fn stop_cancel_and_replace_settle_work_without_notifications() {
    let group = runtime();
    let (agent, stopped) = group.spawn(request("block", true), "m", None).unwrap();
    group.stop(agent).unwrap();
    let task = group.wait(stopped, &never()).unwrap();
    assert_eq!(task.status, AgentTaskStatus::Cancelled);
    assert!(group.send(agent, "again".into()).is_err());

    let (_, interrupted) = group.spawn(request("block", false), "m", None).unwrap();
    assert!(group.wait(interrupted, &AtomicBool::new(true)).is_err());

    let (_, replaced) = group.spawn(request("block", true), "m", None).unwrap();
    group.replace_group();
    assert!(group.wait(replaced, &never()).is_err());

    thread::sleep(Duration::from_millis(50));
    assert!(group.take_notifications().is_empty());
}

#[test]
fn batch_prestart_launches_background_agent_before_foreground_finishes() {
    let group = runtime();
    let handle = AgentGroupHandle::new(group.clone());
    let calls =
        [("fg", "block", false), ("bg", "audit", true)].map(|(id, prompt, background)| ToolCall {
            id: id.into(),
            name: "agent".into(),
            arguments: serde_json::json!({
                "description": prompt,
                "prompt": prompt,
                "role": "researcher",
                "run_in_background": background,
            }),
        });
    handle.prestart(&calls, "m", None);
    // The foreground agent blocks, yet the background one already runs.
    let items = group.task_items();
    assert_eq!(items.len(), 2);
    group.replace_group();
}
