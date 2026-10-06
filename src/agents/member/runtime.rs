//! Live execution resources for one member: its coordinator and run loop.
//!
//! `MemberLauncher` creates a member and its `MemberRunner`; the group runtime
//! owns the runner on a dedicated thread and feeds it one input per run, so
//! the member's model history stays in memory across tasks. The traits let group
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
    pub spawned_by: Option<AgentId>,
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
    Usage {
        usage: &'a Usage,
        event_key: &'a str,
    },
    /// Bounded, attribution-ready summary of a harness event.
    Event { kind: &'a str, detail: String },
    /// A tool started; `detail` is its short argument summary.
    Tool { name: &'a str, detail: String },
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

    fn set_output_cap(&mut self, _cap: Arc<std::sync::atomic::AtomicU64>) {}
}

pub(crate) trait MemberLauncher: Send + Sync {
    /// Registers the member identity and prepares its runner. Called on the
    /// requesting thread so the caller receives the member id immediately.
    fn launch(&self, spec: &MemberSpec) -> Result<(AgentId, Box<dyn MemberRunner>)>;

    fn context_window_tokens(&self, _model: &str) -> Option<u64> {
        None
    }

    fn model_for_budget(&self, model: &str, _role: AgentRole, _cost_budget_usd: f64) -> String {
        model.to_owned()
    }
}

/// Production launcher: each member owns a scoped coordinator and volatile
/// history, while the group remains the lifecycle and persistence owner.
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
        let mut coordinator = self
            .factory
            .build_group_scoped(spec.active_session_id.clone())?;
        let id =
            coordinator.register_runtime_agent(spec.role.as_str(), &spec.model, spec.spawned_by)?;
        if let Some(originator) = spec.spawned_by {
            crate::agents::global().connect(originator, id)?;
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
                usage_sequence: 0,
                output_cap: None,
            }),
        ))
    }

    fn context_window_tokens(&self, model: &str) -> Option<u64> {
        self.factory.context_window_tokens(model)
    }

    fn model_for_budget(&self, model: &str, role: AgentRole, cost_budget_usd: f64) -> String {
        self.factory.model_for_budget(model, role, cost_budget_usd)
    }
}

struct CoordinatorRunner {
    coordinator: AgentCoordinator,
    role: AgentRole,
    model: String,
    started: bool,
    usage_sequence: u64,
    output_cap: Option<Arc<std::sync::atomic::AtomicU64>>,
}

impl MemberRunner for CoordinatorRunner {
    fn set_output_cap(&mut self, cap: Arc<std::sync::atomic::AtomicU64>) {
        self.output_cap = Some(cap);
    }

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
        let mut usage_sequence = self.usage_sequence;
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
                max_output_tokens: self.output_cap.clone(),
            },
            {
                move |event| match event {
                    AgentEvent::ModelAttemptFinished(_, Some(usage)) => {
                        usage_sequence = usage_sequence.saturating_add(1);
                        let event_key = format!("usage:{usage_sequence}");
                        on_progress(MemberProgress::Usage {
                            usage: &usage,
                            event_key: &event_key,
                        });
                    }
                    AgentEvent::AuxiliaryUsage {
                        usage,
                        already_counted_calls: 0,
                    } => {
                        usage_sequence = usage_sequence.saturating_add(1);
                        let event_key = format!("aux:{usage_sequence}");
                        on_progress(MemberProgress::Usage {
                            usage: &usage,
                            event_key: &event_key,
                        });
                    }
                    AgentEvent::ToolExecutionStarted(call) => {
                        let detail = crate::harness::tool_detail(&call).unwrap_or_default();
                        on_progress(MemberProgress::Event {
                            kind: "tool_started",
                            detail: format!("{} {detail}", call.name),
                        });
                        on_progress(MemberProgress::Tool {
                            name: &call.name,
                            detail,
                        });
                    }
                    AgentEvent::ReasoningStart => on_progress(MemberProgress::Event {
                        kind: "reasoning_started",
                        detail: String::new(),
                    }),
                    AgentEvent::ProviderActivity { title, detail } => {
                        let detail =
                            detail.map_or(title.clone(), |detail| format!("{title}: {detail}"));
                        on_progress(MemberProgress::Event {
                            kind: "provider_activity",
                            detail,
                        });
                    }
                    AgentEvent::ToolExecutionFinished {
                        call,
                        succeeded,
                        result,
                    } => {
                        on_progress(MemberProgress::Event {
                            kind: if succeeded {
                                "tool_finished"
                            } else {
                                "tool_failed"
                            },
                            detail: format!("{}: {result}", call.name),
                        });
                    }
                    AgentEvent::ToolExecutionSuppressed { call, reason } => {
                        on_progress(MemberProgress::Event {
                            kind: "tool_suppressed",
                            detail: format!("{}: {reason}", call.name),
                        });
                    }
                    _ => {}
                }
            },
        )?;
        self.usage_sequence = usage_sequence;
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
