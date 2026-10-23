//! Recoverable session context. Window files retain exact model messages; the
//! active window is committed atomically in one manifest.
use crate::{
    core::{Message, MessageRole, ToolCall, ToolDefinition},
    platform::{replace_file, set_private_directory, set_private_file, sync_directory},
};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs, io::Write, path::PathBuf};
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
        use super::goal::GoalStatus;
        match self.state.goal.as_mut() {
            // A message sent to a live goal corrects it rather than discarding
            // the original request; a cancelled or finished goal starts fresh.
            Some(goal) if goal.resumable() && goal.status != GoalStatus::Cancelled => {
                if !continuation {
                    goal.amend(objective);
                }
            }
            Some(goal) if continuation && goal.resumable() => {}
            _ => self.state.goal = Some(super::goal::GoalJob::new(objective)),
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
        goal.brief()
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
        // Skills invoked for the current request follow its user message; a
        // manual compaction has no objective, so anchor on the last user turn.
        let anchor = match objective {
            Some(objective) => history.iter().rposition(|message| message == objective),
            None => history
                .iter()
                .rposition(|message| message.role == MessageRole::User),
        };
        let carryover = conversation_carryover(history, objective);
        let progress = objective
            .and(anchor)
            .and_then(|index| current_run_progress(&history[index + 1..]));
        let active_skill_instructions = anchor
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
        let archive_path = self
            .root
            .as_ref()
            .unwrap()
            .join(format!("{}.json", self.id()));
        ensure!(
            !archive_path.exists(),
            "Archived context window is immutable and already exists: {}",
            archive_path.display()
        );
        write_json_new(archive_path, history)?;
        let old = self.state.clone();
        self.state.windows.push(Window {
            id: Uuid::new_v4().to_string(),
            number: self.number() + 1,
        });
        let mut next = vec![Message::system(super::SYSTEM_INSTRUCTION)];
        next.extend(carryover);
        if let Some(objective) = objective {
            next.push(objective.clone());
        }
        next.extend(active_skill_instructions);
        next.extend(progress);
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
        format!(
            "Context window: previous={}; current={}; window={}; budget={budget}. Earlier windows can be read with context_history if a specific detail is missing.",
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
                json!({"scheduled":true,"after":"current tool batch","hint":"Execution state is carried into the next window automatically."})
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

                    let include_tool_messages = a
                        .get("includeToolMessages")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        || a.get("role").and_then(Value::as_str) == Some("tool")
                        || a.get("toolName").is_some();

                    let include_assistant_messages = a
                        .get("includeAssistantMessages")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        || a.get("role").and_then(Value::as_str) == Some("assistant");

                    let include_active_window = a
                        .get("includeActiveWindow")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let offset = a.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let mut found = Vec::new();
                    let mut skipped = 0;
                    let window_filter = a
                        .get("windowId")
                        .and_then(Value::as_str)
                        .map(|id| {
                            resolve_window_id(self.state.windows.iter().map(|w| w.id.as_str()), id)
                        })
                        .transpose()?;
                    'windows: for window in &self.state.windows {
                        if window_filter.as_ref().is_some_and(|id| *id != window.id) {
                            continue;
                        }

                        if op != "read" && window.id == self.id() && !include_active_window {
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
                            if op != "read"
                                && item.role == MessageRole::Tool
                                && !include_tool_messages
                            {
                                continue;
                            }

                            if op != "read"
                                && item.role == MessageRole::Assistant
                                && !include_assistant_messages
                            {
                                continue;
                            }
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
    let temp_path = temp.into_temp_path();
    replace_file(temp_path.as_ref(), &path)?;

    set_private_file(&path)?;
    sync_directory(parent)?;
    Ok(())
}

fn write_json_new(path: PathBuf, value: &(impl Serialize + ?Sized)) -> Result<()> {
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent)?;
    set_private_directory(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temp, value)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(&path)?;

    set_private_file(&path)?;
    sync_directory(parent)?;
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
    matches!(name, "context_status" | "new_context" | "context_history")
}
pub(super) fn tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new(
            "context_status",
            "Side context diagnostic: show the current window and estimated remaining budget.",
            json!({"type":"object","properties":{}}),
        ),
        ToolDefinition::new(
            "new_context",
            "Side context control: roll after this tool batch. The archived window becomes immutable; the coordinator carries execution state into the new window.",
            json!({"type":"object","properties":{}}),
        ),
        ToolDefinition::new(
            "context_history",
            "Side recovery read of archived immutable session windows. windows lists windows; list/search skip the active model-visible window and recover archived user/system conversational state by default. Assistant history, archived tool output, and active-window lookup are explicit opt-in surfaces via role/includeAssistantMessages/includeToolMessages/includeActiveWindow. read uses a known windowId/item and 8192-char paging. Never use archived assistant/tool output as current workspace source.",
            json!({"type":"object","properties":{"operation":{"type":"string","enum":["windows","list","search","read"]},"windowId":{"type":"string"},"item":{"type":"integer","minimum":0},"query":{"type":"string"},"role":{"type":"string"},"toolName":{"type":"string"},"includeActiveWindow":{"type":"boolean"},"includeAssistantMessages":{"type":"boolean"},"includeToolMessages":{"type":"boolean"},"offset":{"type":"integer","minimum":0}},"required":["operation"]}),
        ),
    ]
}

const CARRYOVER_MAX_MESSAGES: usize = 8;
const CARRYOVER_MAX_MESSAGE_CHARS: usize = 2_000;
const CARRYOVER_MAX_TOTAL_CHARS: usize = 12_000;
const CARRYOVER_HEADER: &str = "Earlier conversation (carried over from the archived context window; tool output omitted, use context_history for exact details):";

/// Recent user requests and final assistant replies from the window being
/// archived, so a rollover keeps the thread of the conversation. Messages from
/// the current request onward are excluded because they are carried verbatim.
fn conversation_carryover(history: &[Message], objective: Option<&Message>) -> Option<Message> {
    let end = objective
        .and_then(|objective| history.iter().rposition(|message| message == objective))
        .unwrap_or(history.len());
    let mut entries = Vec::new();
    let mut total = 0usize;
    for message in history[..end].iter().rev() {
        let budget = CARRYOVER_MAX_TOTAL_CHARS.saturating_sub(total);
        if entries.len() >= CARRYOVER_MAX_MESSAGES || budget == 0 {
            break;
        }
        if message.request_only == Some(true) {
            continue;
        }
        let Some(content) = message.content.as_deref().map(str::trim) else {
            continue;
        };
        let label = match message.role {
            MessageRole::User => "User",
            MessageRole::Assistant
                if message
                    .tool_calls
                    .as_ref()
                    .is_none_or(|calls| calls.is_empty()) =>
            {
                "Assistant"
            }
            // Chain the previous window's carryover so repeated rollovers keep
            // older turns. It sits at the window start, so nothing older remains.
            MessageRole::System if content.starts_with(CARRYOVER_HEADER) => {
                let body = content[CARRYOVER_HEADER.len()..].trim();
                if !body.is_empty() {
                    entries.push(truncate_chars(body, budget));
                }
                break;
            }
            _ => continue,
        };
        if content.is_empty() {
            continue;
        }
        let entry = format!(
            "[{label}] {}",
            truncate_chars(content, CARRYOVER_MAX_MESSAGE_CHARS.min(budget))
        );
        total += entry.len();
        entries.push(entry);
    }
    if entries.is_empty() {
        return None;
    }
    entries.reverse();
    Some(Message::system(format!(
        "{CARRYOVER_HEADER}\n\n{}",
        entries.join("\n\n")
    )))
}

/// Assistant prose from the current request, so the new window knows what
/// this task already found instead of only what earlier turns concluded.
fn current_run_progress(messages: &[Message]) -> Option<Message> {
    let mut entries = Vec::new();
    let mut total = 0usize;
    for message in messages.iter().rev() {
        let budget = CARRYOVER_MAX_TOTAL_CHARS.saturating_sub(total);
        if entries.len() >= CARRYOVER_MAX_MESSAGES || budget == 0 {
            break;
        }
        if message.role != MessageRole::Assistant || message.request_only == Some(true) {
            continue;
        }
        let Some(content) = message.content.as_deref().map(str::trim) else {
            continue;
        };
        if content.is_empty() {
            continue;
        }
        let entry = truncate_chars(content, CARRYOVER_MAX_MESSAGE_CHARS.min(budget));
        total += entry.len();
        entries.push(format!("- {entry}"));
    }
    if entries.is_empty() {
        return None;
    }
    entries.reverse();
    Some(Message::system(format!(
        "Progress on the current request before compaction (your own notes):\n{}",
        entries.join("\n")
    )))
}

fn truncate_chars(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut cut = max_bytes;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}

#[cfg(test)]
mod goal_persistence_tests {
    use super::*;
    use crate::agent::goal::{
        GoalCheckpointAction, GoalObservation, GoalStatus, parse_goal_verdict,
    };

    #[test]
    fn restart_and_rollover_preserve_unbounded_goal_recovery() -> Result<()> {
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
            workspace_mutated: false,
            validation_call: true,
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
        for _ in 0..12 {
            assert_eq!(
                goal.progress.checkpoint(true),
                GoalCheckpointAction::Recover
            );
        }
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
    fn rollover_never_overwrites_an_existing_archived_window() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().join("context");
        let mut memory = ContextMemory::default();
        memory.bind(Some(root.clone()));
        let mut history = vec![Message::system("system"), Message::user("task")];
        memory.load(&mut history)?;

        let window = memory.id().to_owned();
        let archive = root.join(format!("{window}.json"));
        fs::create_dir_all(&root)?;
        fs::write(&archive, b"immutable sentinel")?;

        let error = memory.rollover(&mut history, None).unwrap_err();
        assert!(error.to_string().contains("immutable"));
        assert_eq!(fs::read(&archive)?, b"immutable sentinel");
        assert_eq!(memory.number(), 1);
        Ok(())
    }

    #[test]
    fn context_history_hides_archived_tool_output_by_default() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().join("context");
        let mut memory = ContextMemory::default();
        memory.bind(Some(root));
        let mut history = vec![
            Message::system("system"),
            Message::user("task"),
            Message::tool(
                "secret current-source payload",
                "read-1",
                Some("read_file".into()),
            ),
            Message::assistant("assistant source echo RemoteOptions", None),
        ];
        memory.load(&mut history)?;
        let archived = memory.id().to_owned();
        memory.rollover(&mut history, Some(&Message::user("continue")))?;

        let default_search = ToolCall {
            id: "history-1".into(),
            name: "context_history".into(),
            arguments: json!({"operation":"search","query":"secret current-source payload"}),
        };
        let result: Value = serde_json::from_str(&memory.execute(&default_search)?)?;
        assert_eq!(result["items"].as_array().map(Vec::len), Some(0));

        let assistant_default = ToolCall {
            id: "history-assistant-default".into(),
            name: "context_history".into(),
            arguments: json!({"operation":"search","query":"RemoteOptions"}),
        };
        let result: Value = serde_json::from_str(&memory.execute(&assistant_default)?)?;
        assert_eq!(result["items"].as_array().map(Vec::len), Some(0));

        let assistant_explicit = ToolCall {
            id: "history-assistant-explicit".into(),
            name: "context_history".into(),
            arguments: json!({
                "operation":"search",
                "windowId": archived,
                "query":"RemoteOptions",
                "includeAssistantMessages": true
            }),
        };
        let result: Value = serde_json::from_str(&memory.execute(&assistant_explicit)?)?;
        assert_eq!(result["items"].as_array().map(Vec::len), Some(1));

        let user_search = ToolCall {
            id: "history-user".into(),
            name: "context_history".into(),
            arguments: json!({"operation":"search","query":"task"}),
        };
        let result: Value = serde_json::from_str(&memory.execute(&user_search)?)?;
        let items = result["items"].as_array().expect("history items");
        assert!(items.iter().any(|item| item["role"] == json!("user")));
        assert!(
            items
                .iter()
                .all(|item| matches!(item["role"].as_str(), Some("user" | "system")))
        );

        let explicit_search = ToolCall {
            id: "history-2".into(),
            name: "context_history".into(),
            arguments: json!({
                "operation":"search",
                "windowId": archived,
                "query":"secret current-source payload",
                "includeToolMessages": true
            }),
        };
        let result: Value = serde_json::from_str(&memory.execute(&explicit_search)?)?;
        assert_eq!(result["items"].as_array().map(Vec::len), Some(1));
        Ok(())
    }
    #[test]
    fn legacy_context_with_notes_and_checkpoint_remains_loadable() -> Result<()> {
        let mut value = serde_json::to_value(State::default())?;
        let object = value.as_object_mut().unwrap();
        object.remove("goal");
        object.insert("notes".into(), json!({"state": "old"}));
        object.insert("agent_checkpoint".into(), json!({"objective": "old"}));
        let state: State = serde_json::from_value(value)?;
        assert!(state.goal.is_none());
        Ok(())
    }

    #[test]
    fn nested_group_history_stays_volatile_and_isolated() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let owner_context = directory.path().join("context");
        let mut memory = ContextMemory::default();
        memory.bind(Some(owner_context.clone()));
        // Group-scoped coordinators retain session tools but do not share the
        // Main Agent's durable provider-history file.
        memory.bind(None);
        let mut history = vec![Message::system("group"), Message::user("bounded task")];
        memory.load(&mut history)?;
        memory.sync(&history)?;
        memory.flush()?;
        assert!(!owner_context.exists());
        assert_eq!(history[1].content.as_deref(), Some("bounded task"));
        Ok(())
    }
}

#[cfg(test)]
mod carryover_tests {
    use super::*;

    #[test]
    fn rollover_carries_recent_conversation_across_repeated_windows() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut memory = ContextMemory::default();
        memory.bind(Some(directory.path().join("context")));
        let mut history = vec![
            Message::system("system"),
            Message::user("compare option A and option B"),
            Message::assistant(
                "",
                Some(vec![ToolCall {
                    id: "call-1".into(),
                    name: "read_file".into(),
                    arguments: json!({"path": "src/lib.rs"}),
                }]),
            ),
            Message::tool("large tool payload", "call-1", Some("read_file".into())),
            Message::assistant("Option B is better because it avoids locking.", None),
        ];
        memory.load(&mut history)?;

        // Manual compaction: no objective, the whole exchange carries over.
        memory.rollover(&mut history, None)?;
        let carried = history[1].content.as_deref().unwrap_or_default();
        assert!(carried.starts_with(CARRYOVER_HEADER));
        assert!(carried.contains("[User] compare option A and option B"));
        assert!(carried.contains("[Assistant] Option B is better"));
        assert!(!carried.contains("large tool payload"));

        // A later automatic rollover keeps the older turn and the objective.
        history.push(Message::user("implement option B"));
        let objective = history.last().unwrap().clone();
        history.push(Message::assistant("Starting with the lock removal.", None));
        memory.rollover(&mut history, Some(&objective))?;
        let carried = history[1].content.as_deref().unwrap_or_default();
        assert!(carried.contains("compare option A and option B"));
        assert!(!carried.contains("implement option B"));
        assert_eq!(history[2], objective);
        Ok(())
    }
}

#[cfg(test)]
mod goal_amendment_tests {
    use super::*;
    use crate::agent::goal::GoalStatus;

    #[test]
    fn user_messages_amend_a_live_goal_and_cancelled_goals_start_fresh() {
        let mut memory = ContextMemory::default();
        memory.start_goal("validate the model; do not fly the interceptor", false);
        memory.goal_mut().unwrap().remaining = vec!["stale judge item".into()];

        let brief = memory.start_goal("now fly the interceptor", false);
        assert!(brief.starts_with("validate the model"));
        assert!(brief.ends_with("- now fly the interceptor"));
        assert!(memory.goal().unwrap().remaining.is_empty());
        // Resuming keeps the amended goal instead of re-reading the prompt.
        assert_eq!(memory.start_goal("Resume the goal job", true), brief);

        memory.goal_mut().unwrap().status = GoalStatus::Cancelled;
        assert_eq!(memory.start_goal("new task", false), "new task");
    }
}

/// Models often copy long window UUIDs with dropped or altered characters.
/// Accept the window sharing the longest prefix when that match is unique
/// and covers at least the first UUID group.
fn resolve_window_id<'a>(
    windows: impl Iterator<Item = &'a str>,
    requested: &str,
) -> Result<String> {
    let windows: Vec<&str> = windows.collect();
    if windows.contains(&requested) {
        return Ok(requested.to_owned());
    }
    let shared = |id: &str| -> usize {
        id.chars()
            .zip(requested.chars())
            .take_while(|(left, right)| left == right)
            .count()
    };
    let best = windows.iter().map(|id| shared(id)).max().unwrap_or(0);
    let matches: Vec<&str> = windows
        .iter()
        .copied()
        .filter(|id| shared(id) == best)
        .collect();
    if best >= 8 && matches.len() == 1 {
        return Ok(matches[0].to_owned());
    }
    bail!(
        "Unknown windowId {requested}. Known windows: {}",
        windows.join(", ")
    )
}
