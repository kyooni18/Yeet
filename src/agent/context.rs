//! Recoverable session memory. Window files retain exact model messages; notes
//! and the active window are committed atomically in one manifest.
use crate::{
    core::{Message, MessageRole, ToolCall, ToolDefinition},
    platform::{set_private_directory, set_private_file},
};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf};
use uuid::Uuid;

const CONTEXT_COMPACTION_RESERVE_TOKENS: u64 = 16_384;

#[derive(Clone, Serialize, Deserialize)]
struct Window {
    id: String,
    number: usize,
}
#[derive(Clone, Serialize, Deserialize)]
struct State {
    windows: Vec<Window>,
    notes: BTreeMap<String, String>,
    active: Vec<Message>,
    #[serde(default)]
    goal: Option<super::goal::GoalJob>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            windows: vec![Window {
                id: Uuid::new_v4().to_string(),
                number: 1,
            }],
            notes: BTreeMap::new(),
            active: Vec::new(),
            goal: None,
        }
    }
}
#[derive(Default)]
pub(super) struct ContextMemory {
    state: State,
    root: Option<PathBuf>,
    loaded: bool,
    dirty: bool,
    pub rollover_requested: bool,
    pub estimated_tokens: u64,
    pub capacity: Option<u64>,
    pub policy: crate::project_settings::ContextSettings,
}
impl ContextMemory {
    pub fn goal(&self) -> Option<&super::goal::GoalJob> {
        self.state.goal.as_ref()
    }

    pub fn goal_mut(&mut self) -> Option<&mut super::goal::GoalJob> {
        self.dirty = true;
        self.state.goal.as_mut()
    }

    pub fn start_goal(&mut self, objective: &str, continuation: bool) -> String {
        if !continuation
            || !self
                .state
                .goal
                .as_ref()
                .is_some_and(|goal| goal.resumable())
        {
            self.state.goal = Some(super::goal::GoalJob::new(objective));
        }
        self.dirty = true;
        let goal = self.state.goal.as_mut().unwrap();
        // Paused/cancelled jobs only reach here on explicit resume, not session
        // auto-load. Permit a new bounded recovery attempt without losing work.
        if matches!(
            goal.status,
            super::goal::GoalStatus::Paused | super::goal::GoalStatus::Cancelled
        ) {
            goal.progress = super::goal::GoalProgress::default();
        }
        goal.status = super::goal::GoalStatus::Running;
        goal.objective.clone()
    }

    pub fn bind(&mut self, root: Option<PathBuf>) {
        if self.root != root {
            self.root = root;
            self.state = State::default();
            self.loaded = false;
            self.dirty = false;
            self.rollover_requested = false;
        }
    }
    pub fn load(&mut self, history: &mut Vec<Message>) -> Result<()> {
        if self.loaded {
            return Ok(());
        }
        if let Some(root) = &self.root {
            fs::create_dir_all(root)?;
            set_private_directory(root)?;
            let path = root.join("state.json");
            if path.exists() {
                let state: State = serde_json::from_slice(&fs::read(path)?)?;
                ensure!(!state.windows.is_empty(), "Invalid context window manifest");
                for window in &state.windows {
                    Uuid::parse_str(&window.id)?;
                }
                *history = state.active.clone();
                self.state = state;
            }
        }
        self.loaded = true;
        self.state.active = history.to_vec();
        self.dirty = false;
        Ok(())
    }
    pub fn sync(&mut self, history: &[Message]) -> Result<()> {
        self.state.active = history.to_vec();
        self.dirty = true;
        Ok(())
    }
    pub fn flush(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.persist()?;
        self.dirty = false;
        Ok(())
    }
    fn persist(&self) -> Result<()> {
        if let Some(root) = &self.root {
            write_json(root.join("state.json"), &self.state)?;
        }
        Ok(())
    }
    pub fn id(&self) -> &str {
        &self.state.windows.last().unwrap().id
    }
    pub fn session_id(&self) -> Option<&str> {
        self.root.as_ref()?.parent()?.file_name()?.to_str()
    }
    pub fn number(&self) -> usize {
        self.state.windows.len()
    }
    pub fn rollover(
        &mut self,
        history: &mut Vec<Message>,
        objective: Option<&Message>,
    ) -> Result<()> {
        self.rollover_with_handoff(history, objective, None)
    }

    pub fn rollover_with_handoff(
        &mut self,
        history: &mut Vec<Message>,
        objective: Option<&Message>,
        handoff: Option<&Message>,
    ) -> Result<()> {
        let active_skill_instructions = objective
            .and_then(|objective| history.iter().rposition(|message| message == objective))
            .map(|index| {
                history[index + 1..]
                    .iter()
                    .filter(|message| {
                        message.role == MessageRole::System
                            && message
                                .content
                                .as_deref()
                                .is_some_and(|content| content.starts_with("User-invoked Skill:"))
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // Persist the backing store before discarding any working context.
        self.sync(history)?;
        self.flush()?;
        ensure!(
            self.root.is_some(),
            "Context rollover requires a saved session"
        );
        write_json(
            self.root
                .as_ref()
                .unwrap()
                .join(format!("{}.json", self.id())),
            history,
        )?;
        let old = self.state.clone();
        self.state.windows.push(Window {
            id: Uuid::new_v4().to_string(),
            number: self.number() + 1,
        });
        let mut next = vec![Message::system(super::SYSTEM_INSTRUCTION)];
        if let Some(objective) = objective {
            next.push(objective.clone());
        }
        next.extend(active_skill_instructions);
        if let Some(handoff) = handoff {
            next.push(handoff.clone());
        }
        self.state.active = next.clone();
        if let Err(error) = self.persist() {
            self.state = old;
            return Err(error);
        }
        self.dirty = false;
        *history = next;
        self.rollover_requested = false;
        Ok(())
    }
    pub fn orientation(&self) -> String {
        let windows = &self.state.windows;
        let budget = self.working_budget();
        let mut hint = String::new();
        for (name, text) in &self.state.notes {
            let line = format!("{name}: {text}\n");
            let remaining = 4096usize.saturating_sub(hint.len());
            hint.push_str(&bounded(&line, remaining));
            if hint.len() >= 4096 {
                break;
            }
        }
        let hint_section = if !hint.is_empty() {
            format!("\nTask notes snapshot:\n{hint}")
        } else {
            String::new()
        };
        format!(
            "Context window: previous={}; current={}; window={}; budget={budget}. The runtime preserves the current context and bounded notes; recover prior session/project context only when the user explicitly asks for it or the runtime reports a rollover.{hint_section}",
            windows
                .iter()
                .rev()
                .nth(1)
                .map(|w| w.id.as_str())
                .unwrap_or("none"),
            self.id(),
            self.number(),
        )
    }
    pub fn working_budget(&self) -> u64 {
        let model_budget = self.capacity.unwrap_or(self.policy.unknown_model_tokens);
        self.policy
            .working_set_tokens
            .map_or(model_budget, |limit| model_budget.min(limit))
    }
    pub fn rollover_budget(&self) -> u64 {
        self.working_budget()
            .saturating_mul(self.policy.rollover_percent)
            / 100
    }
    pub fn compaction_pressure_budget(&self) -> u64 {
        let rollover_budget = self.rollover_budget();
        rollover_budget
            .saturating_sub(CONTEXT_COMPACTION_RESERVE_TOKENS)
            .max(rollover_budget / 2)
    }
    fn status(&self) -> Value {
        let remaining = self
            .capacity
            .map(|c| c.saturating_sub(self.estimated_tokens));
        let working_budget = self.working_budget();
        json!({"agentId":"root","sessionId":self.session_id(),"windowId":self.id(),"windowNumber":self.number(),"estimatedInputTokens":self.estimated_tokens,"capacity":self.capacity,"estimatedRemainingTokens":remaining,"workingBudget":working_budget,"workingSetLimit":self.policy.working_set_tokens,"workingBudgetSource":if self.policy.working_set_tokens.is_some(){"project-limit"}else if self.capacity.is_some(){"model-context"}else{"unknown-model-default"},"rolloverBudget":self.rollover_budget(),"compactionPressureBudget":self.compaction_pressure_budget(),"workingRemainingTokens":working_budget.saturating_sub(self.estimated_tokens),"lowBudget":self.estimated_tokens >= working_budget * self.policy.warning_percent / 100,"persistent":self.root.is_some()})
    }
    pub fn execute(&mut self, call: &ToolCall) -> Result<String> {
        let a = &call.arguments;
        let result = match call.name.as_str() {
            "context_status" => self.status(),
            "new_context" => {
                ensure!(self.root.is_some(), "Save a session before rollover");
                self.rollover_requested = true;
                json!({"scheduled":true,"after":"current tool batch","hint":"Execution state is carried into the next window automatically. Save task_notes before this batch finishes when exact constraints or long evidence must remain easy to recover."})
            }
            "task_notes" => {
                let op = arg(a, "operation")?;
                if op == "list" {
                    json!({"notes": self.state.notes.keys().collect::<Vec<_>>()})
                } else if op == "search" {
                    let query = arg(a, "query")?;
                    ensure!(!query.is_empty(), "query must not be empty");
                    json!({"matches": self.state.notes.iter().filter(|(n,t)| n.contains(query) || t.contains(query)).take(20).map(|(n,t)| json!({"name":n,"excerpt":bounded(t,512)})).collect::<Vec<_>>()})
                } else {
                    let name = arg(a, "name")?;
                    ensure!(!name.is_empty() && name.len() <= 128, "Invalid note name");
                    if op == "read" {
                        page(
                            self.state
                                .notes
                                .get(name)
                                .ok_or_else(|| anyhow::anyhow!("Unknown note"))?,
                            a,
                        )
                    } else {
                        ensure!(op == "append" || op == "replace", "Unknown notes operation");
                        ensure!(self.root.is_some(), "Durable notes require a saved session");
                        let content = arg(a, "content")?;
                        let old = self.state.notes.clone();
                        let text = if op == "append" {
                            format!(
                                "{}{content}",
                                old.get(name).map(String::as_str).unwrap_or("")
                            )
                        } else {
                            content.to_owned()
                        };
                        ensure!(text.len() <= 64 * 1024, "Note exceeds 64 KiB");
                        ensure!(
                            old.contains_key(name) || old.len() < 64,
                            "At most 64 task notes"
                        );
                        self.state.notes.insert(name.to_owned(), text);
                        if let Err(e) = self.persist() {
                            self.state.notes = old;
                            return Err(e);
                        }
                        self.dirty = false;
                        json!({"saved":name})
                    }
                }
            }
            "context_history" => {
                let op = arg(a, "operation")?;
                if op == "windows" {
                    let offset = a.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let windows = self
                        .state
                        .windows
                        .iter()
                        .skip(offset)
                        .take(20)
                        .collect::<Vec<_>>();
                    json!({"windows":windows,"nextOffset":offset.saturating_add(windows.len()),"done":offset.saturating_add(windows.len()) >= self.state.windows.len()})
                } else {
                    ensure!(
                        ["list", "read", "search"].contains(&op),
                        "Unknown history operation"
                    );
                    if op == "search" {
                        ensure!(!arg(a, "query")?.is_empty(), "query must not be empty");
                    }
                    let offset = a.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let mut found = Vec::new();
                    let mut skipped = 0;
                    'windows: for window in &self.state.windows {
                        if a.get("windowId")
                            .and_then(Value::as_str)
                            .is_some_and(|id| id != window.id)
                        {
                            continue;
                        }
                        let items = if window.id == self.id() {
                            self.state.active.clone()
                        } else {
                            serde_json::from_slice::<Vec<Message>>(&fs::read(
                                self.root
                                    .as_ref()
                                    .ok_or_else(|| anyhow::anyhow!("History unavailable"))?
                                    .join(format!("{}.json", window.id)),
                            )?)?
                        };
                        for (index, item) in items.iter().enumerate() {
                            let serialized = serde_json::to_value(item)?;
                            let text = serde_json::to_string(item)?;
                            if a.get("role")
                                .and_then(Value::as_str)
                                .is_some_and(|role| serialized["role"] != role)
                                || a.get("toolName")
                                    .and_then(Value::as_str)
                                    .is_some_and(|name| {
                                        item.name.as_deref() != Some(name)
                                            && !item.tool_calls.as_ref().is_some_and(|calls| {
                                                calls.iter().any(|c| c.name == name)
                                            })
                                    })
                            {
                                continue;
                            }
                            if op == "read" {
                                if a.get("item").and_then(Value::as_u64) != Some(index as u64) {
                                    continue;
                                }
                                ensure!(a.get("windowId").is_some(), "read requires windowId");
                                return Ok(page(&text, a).to_string());
                            }
                            if op == "search"
                                && !text.contains(arg(a, "query")?)
                                && !item.content.as_deref().is_some_and(|content| {
                                    content.contains(arg(a, "query").unwrap_or(""))
                                })
                            {
                                continue;
                            }
                            if skipped < offset {
                                skipped += 1;
                                continue;
                            }
                            if found.len() == 20 {
                                break 'windows;
                            }
                            found.push(json!({"windowId":window.id,"item":index,"role":serialized["role"],"toolName":item.name,"excerpt":bounded(&text,512)}));
                        }
                    }
                    if op == "read" {
                        bail!("History item not found");
                    }
                    json!({"items":found,"nextOffset":offset+found.len(),"pageSize":20})
                }
            }
            _ => bail!("Unknown context tool"),
        };
        Ok(result.to_string())
    }
}
fn arg<'a>(a: &'a Value, key: &str) -> Result<&'a str> {
    a.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Missing {key}"))
}
fn bounded(s: &str, bytes: usize) -> String {
    let mut end = bytes.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}
fn page(s: &str, a: &Value) -> Value {
    let offset = a.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let text: String = s.chars().skip(offset).take(8192).collect();
    let next = offset + text.chars().count();
    json!({"text":text,"nextOffset":next,"done":next >= s.chars().count()})
}
fn write_json(path: PathBuf, value: &(impl Serialize + ?Sized)) -> Result<()> {
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent)?;
    set_private_directory(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temp, value)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(&path)?;

    set_private_file(&path)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
/// Image bytes are transport payload, not text tokens. Use a conservative
/// per-image allowance until provider-specific image accounting is available.
pub(super) fn estimate_messages(messages: &[Message]) -> Result<u64> {
    let mut tokens = 0u64;
    for message in messages {
        let mut text = message.clone();
        let images = text.images.take().map_or(0, |images| images.len()) as u64;
        tokens = tokens
            .saturating_add((serde_json::to_vec(&text)?.len() as u64).div_ceil(3))
            .saturating_add(images.saturating_mul(4096));
    }
    Ok(tokens)
}

pub(super) fn is_context_tool(name: &str) -> bool {
    matches!(
        name,
        "context_status" | "new_context" | "task_notes" | "context_history"
    )
}
pub(super) fn tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new(
            "context_status",
            "Show current context window and estimated remaining budget.",
            json!({"type":"object","properties":{}}),
        ),
        ToolDefinition::new(
            "new_context",
            "Roll to a new context after this tool batch. Save task_notes first; old evidence remains retrievable.",
            json!({"type":"object","properties":{}}),
        ),
        ToolDefinition::new(
            "task_notes",
            "Session-local list/read/search/append/replace notes for constraints, decisions, failures, and next steps. Reads page by character offset (8192 chars).",
            json!({"type":"object","properties":{"operation":{"type":"string","enum":["list","read","search","append","replace"]},"name":{"type":"string"},"content":{"type":"string"},"query":{"type":"string"},"offset":{"type":"integer","minimum":0}},"required":["operation"]}),
        ),
        ToolDefinition::new(
            "context_history",
            "Read original session history. windows lists windows; list/search return 20 refs; read uses windowId/item and 8192-char paging. Search is literal; role/toolName filters are optional.",
            json!({"type":"object","properties":{"operation":{"type":"string","enum":["windows","list","search","read"]},"windowId":{"type":"string"},"item":{"type":"integer","minimum":0},"query":{"type":"string"},"role":{"type":"string"},"toolName":{"type":"string"},"offset":{"type":"integer","minimum":0}},"required":["operation"]}),
        ),
    ]
}

#[cfg(test)]
mod goal_persistence_tests {
    use super::*;
    use crate::agent::goal::{
        GoalCheckpointAction, GoalObservation, GoalStatus, MAX_GOAL_RECOVERIES, parse_goal_verdict,
    };

    #[test]
    fn restart_and_rollover_preserve_goal_and_recovery_budget() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().join("context");
        let mut memory = ContextMemory::default();
        memory.bind(Some(root.clone()));
        let mut history = vec![Message::system("system"), Message::user("ship feature")];
        memory.load(&mut history)?;
        memory.start_goal("ship feature", false);
        let window = memory.id().to_owned();
        history.push(Message::tool(
            "test failed",
            "test-1",
            Some("run_shell".into()),
        ));
        let goal = memory.goal_mut().unwrap();
        goal.observe(GoalObservation {
            window_id: window.clone(),
            item: 2,
            tool_call_id: "test-1".into(),
            tool_name: "run_shell".into(),
            succeeded: false,
            excerpt: "test failed".into(),
        });
        goal.checkpoint(&parse_goal_verdict(
            r#"{"verdict":"failure","evidence":["test failed"],"remaining":["fix failing test"]}"#,
        ));
        const RECOVERIES_BEFORE_RESTART: usize = 2;
        for _ in 0..RECOVERIES_BEFORE_RESTART {
            assert_eq!(
                goal.progress.checkpoint(true),
                GoalCheckpointAction::Recover
            );
        }
        goal.status = GoalStatus::Recovering;
        let objective = Message::user("synthetic continuation");
        memory.rollover(&mut history, Some(&objective))?;
        drop(memory);

        let mut restored = ContextMemory::default();
        restored.bind(Some(root));
        let mut restored_history = Vec::new();
        restored.load(&mut restored_history)?;
        assert_eq!(
            restored.start_goal("synthetic continuation", true),
            "ship feature"
        );
        let goal = restored.goal_mut().unwrap();
        assert_eq!(goal.remaining, vec!["fix failing test"]);
        assert_eq!(goal.next_action.as_deref(), Some("fix failing test"));
        assert_eq!(goal.observations[0].window_id, window);
        for _ in RECOVERIES_BEFORE_RESTART..MAX_GOAL_RECOVERIES {
            assert_eq!(
                goal.progress.checkpoint(true),
                GoalCheckpointAction::Recover
            );
        }
        assert_eq!(goal.progress.checkpoint(true), GoalCheckpointAction::Pause);
        let original: Vec<Message> = serde_json::from_slice(&fs::read(
            directory
                .path()
                .join("context")
                .join(format!("{window}.json")),
        )?)?;
        assert_eq!(
            original[goal.observations[0].item].content.as_deref(),
            Some("test failed")
        );
        Ok(())
    }

    #[test]
    fn legacy_context_without_goal_remains_loadable() -> Result<()> {
        let mut value = serde_json::to_value(State::default())?;
        value.as_object_mut().unwrap().remove("goal");
        let state: State = serde_json::from_value(value)?;
        assert!(state.goal.is_none());
        Ok(())
    }
}
