//! Live execution resources for one member: its coordinator and run loop.
//!
//! `MemberLauncher` creates a member and its `MemberRunner`; the group runtime
//! owns the runner on a dedicated thread and feeds it one input per run, so
//! the member's model history persists across messages. The traits let group
//! lifecycle tests substitute scripted runners for real coordinators.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Result;

use crate::{
    agent::{AgentCoordinator, AgentEvent, AgentRunOutcome, AgentRunRequest},
    agents::{AgentId, runtime::AgentRuntimeFactory},
    core::{MessageRole, Usage},
};

use super::{AgentRole, worker_prompt, worker_reasoning_level};

/// Everything needed to start a member.
pub(crate) struct MemberSpec {
    pub role: AgentRole,
    pub description: String,
    pub model: String,
    pub parent: Option<AgentId>,
    pub active_session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunOutcome {
    Completed,
    CompletedUnverified,
    Paused,
}

impl RunOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::CompletedUnverified => "completed_unverified",
            Self::Paused => "paused",
        }
    }
}

/// What a runner reports while it works.
pub(crate) enum MemberProgress<'a> {
    Usage(&'a Usage),
    /// A tool started; `detail` is its short argument summary.
    Tool {
        name: &'a str,
        detail: String,
    },
}

pub(crate) struct RunReport {
    pub outcome: RunOutcome,
    pub summary: String,
}

pub(crate) trait MemberRunner: Send {
    /// Runs one input to completion. `on_progress` observes usage and tool
    /// starts as they happen.
    fn run(
        &mut self,
        input: &str,
        cancel: Arc<AtomicBool>,
        on_progress: &mut dyn FnMut(MemberProgress<'_>),
    ) -> Result<RunReport>;
}

pub(crate) trait MemberLauncher: Send + Sync {
    /// Registers the member identity and prepares its runner. Called on the
    /// requesting thread so the caller receives the member id immediately.
    fn launch(&self, spec: &MemberSpec) -> Result<(AgentId, Box<dyn MemberRunner>)>;
}

/// Production launcher: each member owns an independent `AgentCoordinator`.
pub(crate) struct CoordinatorLauncher {
    factory: AgentRuntimeFactory,
}

impl CoordinatorLauncher {
    pub(crate) fn new(factory: AgentRuntimeFactory) -> Self {
        Self { factory }
    }
}

impl MemberLauncher for CoordinatorLauncher {
    fn launch(&self, spec: &MemberSpec) -> Result<(AgentId, Box<dyn MemberRunner>)> {
        let mut coordinator = self.factory.build(spec.active_session_id.clone())?;
        let id =
            coordinator.register_runtime_agent(spec.role.as_str(), &spec.model, spec.parent)?;
        if let Some(parent) = spec.parent {
            crate::agents::global().connect(parent, id)?;
        }
        coordinator.set_permission_label(format!(
            "{} agent · {}",
            spec.role.as_str(),
            spec.description
        ));
        Ok((
            id,
            Box::new(CoordinatorRunner {
                coordinator,
                role: spec.role,
                model: spec.model.clone(),
                started: false,
            }),
        ))
    }
}

struct CoordinatorRunner {
    coordinator: AgentCoordinator,
    role: AgentRole,
    model: String,
    started: bool,
}

impl MemberRunner for CoordinatorRunner {
    fn run(
        &mut self,
        input: &str,
        cancel: Arc<AtomicBool>,
        on_progress: &mut dyn FnMut(MemberProgress<'_>),
    ) -> Result<RunReport> {
        let prompt = if self.started {
            input.to_owned()
        } else {
            worker_prompt(self.role, input)
        };
        self.started = true;
        let outcome = self.coordinator.run(
            AgentRunRequest {
                input: &prompt,
                images: Vec::new(),
                model: &self.model,
                reasoning_level: worker_reasoning_level(&self.model),
                attached_capabilities: None,
                disabled_capabilities: self.role.disabled_capabilities(),
                goal_mode: Arc::new(AtomicBool::new(true)),
                cancel: cancel.clone(),
                continuation: false,
            },
            |event| match event {
                AgentEvent::ModelAttemptFinished(_, Some(usage))
                | AgentEvent::AuxiliaryUsage { usage, .. } => {
                    on_progress(MemberProgress::Usage(&usage))
                }
                AgentEvent::ToolExecutionStarted(call) => on_progress(MemberProgress::Tool {
                    name: &call.name,
                    detail: crate::backend::tool_detail(&call).unwrap_or_default(),
                }),
                _ => {}
            },
        )?;
        if cancel.load(Ordering::Acquire) {
            anyhow::bail!("cancelled");
        }
        let outcome = match outcome {
            AgentRunOutcome::Completed => RunOutcome::Completed,
            AgentRunOutcome::CompletedUnverified { .. } => RunOutcome::CompletedUnverified,
            AgentRunOutcome::GoalPaused { .. } | AgentRunOutcome::AutonomousIdle { .. } => {
                RunOutcome::Paused
            }
        };
        Ok(RunReport {
            outcome,
            summary: last_assistant_text(&self.coordinator),
        })
    }
}

fn last_assistant_text(coordinator: &AgentCoordinator) -> String {
    coordinator
        .model_history()
        .iter()
        .rev()
        .find(|message| {
            message.role == MessageRole::Assistant
                && message
                    .content
                    .as_deref()
                    .is_some_and(|text| !text.trim().is_empty())
        })
        .and_then(|message| message.content.clone())
        .unwrap_or_default()
}
