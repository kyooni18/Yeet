//! Agent Group state: serializable membership, tasks, and usage, kept apart
//! from the live runtime resources (inboxes, cancellation flags) that drive
//! member threads.
//!
//! Tasks and members are stored in creation order so frontends render a
//! stable list.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64},
    },
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    agents::{
        AgentId,
        member::{AgentMember, MemberStatus},
        task::{AgentTask, AgentTaskId},
    },
    core::Usage,
    model::{
        AgentActivityItem, AgentActivityKind, AgentGroupBudgetItem, AgentGroupEventItem,
        AgentGroupFindingItem,
    },
};

use super::budget_ledger::BudgetLedger;

pub(crate) type AgentGroupId = Uuid;

/// Frontends only need recent history; older entries are dropped.
const ACTIVITY_LIMIT: usize = 200;
const SHARED_FINDING_LIMIT: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentGroupState {
    pub id: AgentGroupId,
    /// Bumped when the group is replaced; late updates from member threads
    /// of an older generation are ignored.
    pub generation: u64,
    pub primary_agent: Option<AgentId>,
    /// Shared objective owned by the group coordinator.
    pub objective: Option<String>,
    /// Lifecycle of the group coordinator and its integrated result.
    pub status: String,
    pub final_result: Option<String>,
    pub checkpoint_summary: Option<String>,
    pub synthesis_started: bool,
    pub budget: AgentGroupBudgetItem,
    pub members: Vec<AgentMember>,
    pub tasks: Vec<AgentTask>,
    /// Usage since the group was created.
    pub usage: Usage,
    /// Recent member activity, oldest first.
    pub activity: VecDeque<AgentActivityItem>,
    pub event_sequence: u64,
    pub events: VecDeque<AgentGroupEventItem>,
    pub shared_findings: VecDeque<AgentGroupFindingItem>,
}

impl AgentGroupState {
    pub(crate) fn new(generation: u64, primary_agent: Option<AgentId>) -> Self {
        Self {
            id: AgentGroupId::new_v4(),
            generation,
            primary_agent,
            objective: None,
            status: "idle".into(),
            final_result: None,
            checkpoint_summary: None,
            synthesis_started: false,
            budget: AgentGroupBudgetItem::default(),
            members: Vec::new(),
            tasks: Vec::new(),
            usage: Usage::default(),
            activity: VecDeque::new(),
            event_sequence: 0,
            events: VecDeque::new(),
            shared_findings: VecDeque::new(),
        }
    }

    pub(crate) fn set_objective(&mut self, objective: String) {
        self.objective = Some(objective);
        self.status = "created".into();
        self.synthesis_started = false;
        self.final_result = None;
        self.checkpoint_summary = None;
        self.record_event(None, None, "group_created", "Group objective accepted");
    }

    pub(crate) fn record_event(
        &mut self,
        member_id: Option<AgentId>,
        task_id: Option<AgentTaskId>,
        kind: &str,
        detail: &str,
    ) {
        self.event_sequence = self.event_sequence.saturating_add(1);
        if self.events.len() == ACTIVITY_LIMIT {
            self.events.pop_front();
        }
        self.events.push_back(AgentGroupEventItem {
            group_id: self.id.to_string(),
            sequence: self.event_sequence,
            member_id: member_id.map(|id| id.to_string()),
            task_id: task_id.map(|id| id.to_string()),
            at: chrono::Utc::now().to_rfc3339(),
            kind: kind.to_owned(),
            detail: first_line(detail),
        });
    }

    pub(crate) fn promote_finding(
        &mut self,
        member_id: AgentId,
        task_id: AgentTaskId,
        summary: &str,
    ) {
        if self.shared_findings.len() == SHARED_FINDING_LIMIT {
            self.shared_findings.pop_front();
        }
        self.shared_findings.push_back(AgentGroupFindingItem {
            member_id: member_id.to_string(),
            task_id: task_id.to_string(),
            at: chrono::Utc::now().to_rfc3339(),
            summary: truncate_chars(summary, 4_000),
        });
        self.record_event(Some(member_id), Some(task_id), "finding_promoted", summary);
    }

    /// Build a focused context envelope for a new task: the shared objective,
    /// the task itself, and only findings that overlap its subject.
    pub(crate) fn task_context(&self, task: &str) -> String {
        let mut output = format!(
            "GROUP OBJECTIVE:\n{}\n\nTASK-SPECIFIC ASSIGNMENT:\n{}",
            self.objective
                .as_deref()
                .unwrap_or("Coordinate the assigned work."),
            task
        );
        let tokens = keywords(task);
        let relevant = self
            .shared_findings
            .iter()
            .filter_map(|finding| {
                let score = keywords(&finding.summary).intersection(&tokens).count();
                (score > 0).then_some((score, finding))
            })
            .collect::<Vec<_>>();
        let mut relevant = relevant;
        relevant.sort_by(|left, right| right.0.cmp(&left.0));
        if !relevant.is_empty() {
            output.push_str("\n\nRELEVANT SHARED FINDINGS:\n");
            for (_, finding) in relevant.into_iter().take(3) {
                output.push_str("- ");
                output.push_str(&finding.summary);
                output.push('\n');
            }
        }
        output
    }

    /// Appends one activity entry. `from`/`to` of `None` is the primary agent.
    pub(crate) fn record(
        &mut self,
        from: Option<AgentId>,
        to: Option<AgentId>,
        kind: AgentActivityKind,
        tool: Option<String>,
        text: &str,
    ) {
        if self.activity.len() == ACTIVITY_LIMIT {
            self.activity.pop_front();
        }
        self.activity.push_back(AgentActivityItem {
            at: chrono::Utc::now().to_rfc3339(),
            from: from.map(|id| id.to_string()),
            to: to.map(|id| id.to_string()),
            kind,
            tool,
            text: first_line(text),
        });
    }

    pub(crate) fn member(&self, id: AgentId) -> Option<&AgentMember> {
        self.members.iter().find(|member| member.id == id)
    }

    pub(crate) fn member_mut(&mut self, id: AgentId) -> Option<&mut AgentMember> {
        self.members.iter_mut().find(|member| member.id == id)
    }

    pub(crate) fn task(&self, id: AgentTaskId) -> Option<&AgentTask> {
        self.tasks.iter().find(|task| task.id == id)
    }

    pub(crate) fn task_mut(&mut self, id: AgentTaskId) -> Option<&mut AgentTask> {
        self.tasks.iter_mut().find(|task| task.id == id)
    }

    /// Members running or holding queued work.
    pub(crate) fn busy_member_count(&self) -> usize {
        self.members
            .iter()
            .filter(|member| member.status == MemberStatus::Running)
            .count()
    }

    pub(crate) fn live_member_count(&self) -> usize {
        self.members
            .iter()
            .filter(|member| member.status != MemberStatus::Stopped)
            .count()
    }

    pub(crate) fn has_busy_writer(&self) -> bool {
        self.members
            .iter()
            .any(|member| member.status == MemberStatus::Running && member.role.writes_workspace())
    }
}

/// Runtime-only resources for one member thread.
#[derive(Default)]
pub(super) struct MemberSlot {
    pub inbox: VecDeque<(AgentTaskId, String)>,
    pub running: Option<(AgentTaskId, Arc<AtomicBool>)>,
    pub stop: bool,
}

/// Live group state plus the runtime-only resources of its member threads.
pub(super) struct GroupRuntimeState {
    pub group: AgentGroupState,
    /// Monotonic checkpoint ordering, incremented for each published change.
    pub checkpoint_revision: u64,
    pub slots: HashMap<AgentId, MemberSlot>,
    pub task_caps: HashMap<AgentTaskId, Arc<AtomicU64>>,
    pub budget: BudgetLedger,
}

/// Durable group state without threads, inboxes, locks, or cancellation flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentGroupCheckpoint {
    pub schema_version: u32,
    #[serde(default)]
    pub revision: u64,
    pub group: AgentGroupState,
    pub budget: BudgetLedger,
}

impl GroupRuntimeState {
    pub(super) fn new(
        generation: u64,
        primary_agent: Option<AgentId>,
        budget: BudgetLedger,
    ) -> Self {
        Self {
            group: AgentGroupState::new(generation, primary_agent),
            checkpoint_revision: 0,
            slots: HashMap::new(),
            task_caps: HashMap::new(),
            budget,
        }
    }

    pub(super) fn sync_budget_projection(&mut self) {
        let limits = self.budget.limits();
        let usage = self.budget.usage();
        let mut tasks = self
            .task_caps
            .iter()
            .filter_map(|(task_id, cap)| {
                let id = task_id.to_string();
                let allocation = self.budget.allocation(&id)?;
                let task_usage = self.budget.task_usage(&id);
                Some(crate::model::AgentTaskBudgetItem {
                    task_id: id,
                    allocated_output_tokens: allocation.output_tokens,
                    used_output_tokens: task_usage.map_or(0, |usage| usage.output_tokens),
                    remaining_output_tokens: cap.load(std::sync::atomic::Ordering::Acquire),
                    allocated_cost_usd: allocation.cost_usd,
                    estimated_cost_used_usd: task_usage.and_then(|usage| usage.estimated_cost_usd),
                    context_window_tokens: allocation.context_window_tokens,
                })
            })
            .collect::<Vec<_>>();
        tasks.sort_by(|left, right| left.task_id.cmp(&right.task_id));
        self.group.budget = crate::model::AgentGroupBudgetItem {
            output_limit_tokens: limits.output_tokens,
            output_used_tokens: usage.output_tokens,
            cost_limit_usd: limits.estimated_cost_usd,
            estimated_cost_used_usd: usage.estimated_cost_usd,
            coordination_reserve_tokens: limits.coordination_output_reserve,
            synthesis_reserve_tokens: limits.synthesis_output_reserve,
            coordination_reserve_cost_usd: limits.coordination_cost_reserve_usd,
            synthesis_reserve_cost_usd: limits.synthesis_cost_reserve_usd,
            tasks,
        };
        for (task_id, cap) in &self.task_caps {
            if let Some(remaining) = self.budget.remaining_output_cap(&task_id.to_string()) {
                cap.store(remaining, std::sync::atomic::Ordering::Release);
            }
        }
    }

    pub(super) fn checkpoint(&self) -> AgentGroupCheckpoint {
        AgentGroupCheckpoint {
            schema_version: 1,
            revision: self.checkpoint_revision,
            group: self.group.clone(),
            budget: self.budget.checkpoint_clone(),
        }
    }
}

/// The first non-empty line, bounded so one entry cannot flood the wire.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    match line.char_indices().nth(240) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_owned(),
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

fn keywords(text: &str) -> std::collections::HashSet<String> {
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| word.chars().count() >= 4)
        .map(str::to_ascii_lowercase)
        .collect()
}
