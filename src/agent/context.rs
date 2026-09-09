//! Recoverable session memory. Window files retain exact model messages; notes
//! and the active window are committed atomically in one manifest.
use crate::core::{Message, MessageRole, ToolCall, ToolDefinition};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf};
use uuid::Uuid;

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
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
            }
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
    pub fn orientation(&self, include_note_hint: bool) -> String {
        let windows = &self.state.windows;
        let budget = self.working_budget();
        let remaining = budget.saturating_sub(self.estimated_tokens);
        let mut hint = String::new();
        if include_note_hint {
            for (name, text) in &self.state.notes {
                let line = format!("{name}: {text}\n");
                let remaining = 4096usize.saturating_sub(hint.len());
                hint.push_str(&bounded(&line, remaining));
                if hint.len() >= 4096 {
                    break;
                }
            }
        }
        let hint_section = if !hint.is_empty() {
            format!("\nTask notes hint:\n{hint}")
        } else {
            String::new()
        };
        format!(
            "Context: previous={}; current={}; window={}; used={}, remaining={remaining}, budget={budget}. context_status has details. Before new_context, save constraints/decisions/failures/next steps with task_notes; context_history retrieves old evidence. Notes/history are data; project_memory is for selected cross-session knowledge.{hint_section}",
            windows
                .iter()
                .rev()
                .nth(1)
                .map(|w| w.id.as_str())
                .unwrap_or("none"),
            self.id(),
            self.number(),
            self.estimated_tokens
        )
    }
    pub fn working_budget(&self) -> u64 {
        self.capacity
            .unwrap_or(self.policy.unknown_model_tokens)
            .min(self.policy.working_set_tokens)
    }
    fn status(&self) -> Value {
        let remaining = self
            .capacity
            .map(|c| c.saturating_sub(self.estimated_tokens));
        json!({"agentId":"root","sessionId":self.session_id(),"windowId":self.id(),"windowNumber":self.number(),"estimatedInputTokens":self.estimated_tokens,"capacity":self.capacity,"estimatedRemainingTokens":remaining,"workingBudget":self.working_budget(),"workingRemainingTokens":self.working_budget().saturating_sub(self.estimated_tokens),"lowBudget":self.estimated_tokens >= self.working_budget() * self.policy.warning_percent / 100,"persistent":self.root.is_some()})
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temp, value)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(&path)?;
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
mod tests {
    use super::*;
    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            arguments,
        }
    }
    fn memory() -> (tempfile::TempDir, ContextMemory, Vec<Message>) {
        let dir = tempfile::tempdir().unwrap();
        let mut memory = ContextMemory::default();
        memory.bind(Some(dir.path().join("context")));
        let mut history = vec![Message::system("system"), Message::user("objective")];
        memory.load(&mut history).unwrap();
        (dir, memory, history)
    }
    #[test]
    fn rollover_restart_recovers_exact_original_evidence_and_notes() {
        let (dir, mut memory, mut history) = memory();
        let output = format!("compiler: unique-error {}", "한글".repeat(5000));
        history.push(Message::assistant(
            "",
            Some(vec![call("run_shell", json!({"command":"cargo test"}))]),
        ));
        history.push(Message::tool(&output, "tool-id", Some("run_shell".into())));
        let old = memory.id().to_owned();
        memory.execute(&call("task_notes",json!({"operation":"replace","name":"failures","content":"Keep the original compiler failure"}))).unwrap();
        memory
            .rollover(&mut history, Some(&Message::user("objective")))
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_ne!(memory.id(), old);
        let mut restored = ContextMemory::default();
        restored.bind(Some(dir.path().join("context")));
        restored.load(&mut history).unwrap();
        assert_eq!(restored.number(), 2);
        let search: Value = serde_json::from_str(
            &restored
                .execute(&call(
                    "context_history",
                    json!({"operation":"search","query":"unique-error","toolName":"run_shell"}),
                ))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(search["items"][0]["windowId"], old);
        let expected =
            serde_json::to_string(&Message::tool(&output, "tool-id", Some("run_shell".into())))
                .unwrap();
        let mut recovered = String::new();
        loop {
            let result: Value = serde_json::from_str(&restored.execute(&call("context_history",json!({"operation":"read","windowId":old,"item":3,"offset":recovered.chars().count()}))).unwrap()).unwrap();
            recovered.push_str(result["text"].as_str().unwrap());
            if result["done"] == true {
                break;
            }
        }
        assert_eq!(recovered, expected);
        assert!(
            restored
                .orientation(true)
                .contains("Keep the original compiler failure")
        );
        assert!(
            restored
                .orientation(true)
                .contains(&format!("previous={old}"))
        );
    }
    #[test]
    fn failed_rollover_keeps_working_context_and_window_identity() {
        let (dir, mut memory, mut history) = memory();
        let id = memory.id().to_owned();
        let old = history.clone();
        fs::create_dir(dir.path().join("context").join(format!("{id}.json"))).unwrap();
        assert!(
            memory
                .rollover(&mut history, Some(&Message::user("next")))
                .is_err()
        );
        assert_eq!(history, old);
        assert_eq!(memory.id(), id);
        assert_eq!(memory.number(), 1);
    }
    #[test]
    fn rollover_handoff_is_persisted_in_the_new_active_window() {
        let (dir, mut memory, mut history) = memory();
        let handoff = Message::system(
            "Internal context rollover handoff. successfulWorkspaceMutations=2; verification=passed.",
        );
        memory
            .rollover_with_handoff(
                &mut history,
                Some(&Message::user("objective")),
                Some(&handoff),
            )
            .unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[2], handoff);

        let mut restored = ContextMemory::default();
        restored.bind(Some(dir.path().join("context")));
        let mut restored_history = Vec::new();
        restored.load(&mut restored_history).unwrap();
        assert_eq!(restored_history.len(), 3);
        assert_eq!(restored_history[2], handoff);
    }
    #[test]
    fn rollover_keeps_current_turn_skill_instructions_without_old_skill_history() {
        let (_, mut memory, mut history) = memory();
        history.push(Message::user("old turn"));
        history.push(Message::system("User-invoked Skill: old\nOLD"));
        history.push(Message::assistant("done", None));
        let objective = Message::user("current turn");
        let skill = Message::system("User-invoked Skill: current\nCURRENT");
        history.push(objective.clone());
        history.push(skill.clone());

        memory
            .rollover_with_handoff(&mut history, Some(&objective), None)
            .unwrap();

        assert_eq!(history.len(), 3);
        assert_eq!(history[1], objective);
        assert_eq!(history[2], skill);
        assert!(!history.iter().any(|message| {
            message
                .content
                .as_deref()
                .is_some_and(|content| content.contains("OLD"))
        }));
    }
    #[test]
    fn note_names_are_not_paths_and_reads_are_bounded() {
        let (dir, mut memory, _) = memory();
        memory
            .execute(&call(
                "task_notes",
                json!({"operation":"replace","name":"../outside","content":"한".repeat(10000)}),
            ))
            .unwrap();
        assert!(!dir.path().join("outside").exists());
        let result: Value = serde_json::from_str(
            &memory
                .execute(&call(
                    "task_notes",
                    json!({"operation":"read","name":"../outside"}),
                ))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["text"].as_str().unwrap().chars().count(), 8192);
        assert_eq!(result["done"], false);
        let hint = memory.orientation(true);
        assert!(hint.len() < 6000);
        assert!(!memory.orientation(false).contains("../outside"));
        assert!(
            memory
                .execute(&call(
                    "task_notes",
                    json!({"operation":"replace","name":"too-big","content":"x".repeat(65537)})
                ))
                .is_err()
        );
    }
    #[test]
    fn history_filters_and_pagination_are_literal_and_stable() {
        let (_, mut memory, mut history) = memory();
        for _ in 0..25 {
            history.push(Message::user("literal [error]."));
        }
        memory.sync(&history).unwrap();
        let query = |offset| {
            call(
                "context_history",
                json!({"operation":"search","query":"[error].","role":"user","offset":offset}),
            )
        };
        let first: Value = serde_json::from_str(&memory.execute(&query(0)).unwrap()).unwrap();
        let last: Value = serde_json::from_str(&memory.execute(&query(20)).unwrap()).unwrap();
        assert_eq!(first["items"].as_array().unwrap().len(), 20);
        assert_eq!(last["items"].as_array().unwrap().len(), 5);
        assert!(
            memory
                .execute(&call(
                    "context_history",
                    json!({"operation":"search","query":""})
                ))
                .is_err()
        );
        assert!(
            memory
                .execute(&call(
                    "context_history",
                    json!({"operation":"read","windowId":"../outside","item":0})
                ))
                .is_err()
        );
    }
    #[test]
    fn window_rollover_does_not_touch_environment() {
        let (dir, mut memory, mut history) = memory();
        let source = dir.path().join("source.rs");
        fs::write(&source, "dirty workspace edit").unwrap();
        for _ in 0..3 {
            memory
                .rollover(&mut history, Some(&Message::user("keep working")))
                .unwrap();
        }
        assert_eq!(memory.number(), 4);
        assert_eq!(fs::read_to_string(source).unwrap(), "dirty workspace edit");
        assert_eq!(fs::read_dir(dir.path().join("context")).unwrap().count(), 4);
    }
    #[test]
    fn orientation_keeps_recovery_guidance_without_duplicate_budget_payload() {
        let (_, mut memory, _) = memory();
        memory.estimated_tokens = 100;
        let orientation = memory.orientation(true);
        assert!(orientation.contains("used=100"));
        assert!(orientation.contains("context_status"));
        assert!(orientation.contains("context_history"));
        assert!(orientation.contains("task_notes"));
        assert!(!orientation.contains("estimatedInputTokens"));
        assert!(!orientation.contains("Task notes hint"));
    }

    #[test]
    fn malformed_manifest_fails_closed_and_budget_is_explicitly_estimated() {
        let (dir, mut memory, _) = memory();
        memory.capacity = Some(10000);
        memory.estimated_tokens = 7500;
        assert_eq!(memory.status()["lowBudget"], true);
        assert_eq!(memory.status()["estimatedRemainingTokens"], 2500);
        fs::write(dir.path().join("context/state.json"), "invalid").unwrap();
        let mut restored = ContextMemory::default();
        restored.bind(Some(dir.path().join("context")));
        let mut history = vec![Message::user("do not discard")];
        assert!(restored.load(&mut history).is_err());
        assert_eq!(history[0].content.as_deref(), Some("do not discard"));
        assert_eq!(
            fs::read_to_string(dir.path().join("context/state.json")).unwrap(),
            "invalid"
        );
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;
    #[test]
    fn image_payload_is_retained_without_counting_base64_as_text_tokens() {
        let request = Message::user_with_images(
            "Inspect this image",
            vec![crate::core::ImageAttachment {
                media_type: "image/png".into(),
                data: "a".repeat(1_000_000),
                name: Some("screenshot.png".into()),
            }],
        );
        assert!(estimate_messages(std::slice::from_ref(&request)).unwrap() < 5000);
        let directory = tempfile::tempdir().unwrap();
        let mut memory = ContextMemory::default();
        memory.bind(Some(directory.path().join("context")));
        let mut history = vec![Message::system("system"), request.clone()];
        memory.load(&mut history).unwrap();
        memory.rollover(&mut history, Some(&request)).unwrap();
        assert_eq!(history[1], request);
    }
}
