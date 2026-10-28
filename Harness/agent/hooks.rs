//! User-configured command hooks.
//!
//! `<config dir>/hooks.json` maps harness events to shell commands:
//!
//! ```json
//! {"hooks": {"PostToolRound": [{"command": "my-check", "timeoutSecs": 5, "minIntervalSecs": 3}]}}
//! ```
//!
//! A hook receives a JSON object on stdin (`event`, `cwd`, `sessionId`). Its
//! stdout, when non-empty, is added to the conversation as a system note.
//! Failing or slow hooks are ignored. Only the global file is read, so a
//! repository can never make Yeet run its commands.

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::json;

use super::AgentCoordinator;

const DEFAULT_TIMEOUT_SECS: u64 = 5;
const MAX_OUTPUT_BYTES: u64 = 8 * 1024;
/// Times in one turn a `Stop` hook may keep the model working after it answered.
pub(super) const STOP_CONTINUATION_LIMIT: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum HookEvent {
    /// A user turn begins.
    UserPromptSubmit,
    /// Tool results were appended and the model is about to continue.
    PostToolRound,
    /// The model answered without tool calls. Output keeps the turn going.
    Stop,
}

impl HookEvent {
    fn name(self) -> &'static str {
        match self {
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::PostToolRound => "PostToolRound",
            Self::Stop => "Stop",
        }
    }
}

#[derive(Deserialize)]
struct HookFile {
    #[serde(default)]
    hooks: HashMap<String, Vec<HookDef>>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct HookDef {
    command: String,
    timeout_secs: Option<u64>,
    /// Skip the hook when it ran more recently than this.
    min_interval_secs: Option<u64>,
}

struct Entry {
    def: HookDef,
    last_run: Option<Instant>,
    /// Output last injected, so an unchanged result is not repeated.
    last_output: Option<String>,
}

#[derive(Default)]
pub(super) struct HookEngine {
    entries: HashMap<&'static str, Vec<Entry>>,
}

impl HookEngine {
    /// Loads `hooks.json` from `config_dir`. A missing or invalid file means no hooks.
    pub(super) fn load(config_dir: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(config_dir.join("hooks.json")) else {
            return Self::default();
        };
        match serde_json::from_str::<HookFile>(&text) {
            Ok(file) => Self::from_file(file),
            Err(error) => {
                eprintln!("yeet: ignoring invalid hooks.json: {error}");
                Self::default()
            }
        }
    }

    fn from_file(file: HookFile) -> Self {
        let mut entries = HashMap::new();
        for event in [
            HookEvent::UserPromptSubmit,
            HookEvent::PostToolRound,
            HookEvent::Stop,
        ] {
            if let Some(defs) = file.hooks.get(event.name()) {
                let list = defs
                    .iter()
                    .filter(|def| !def.command.trim().is_empty())
                    .map(|def| Entry {
                        def: def.clone(),
                        last_run: None,
                        last_output: None,
                    })
                    .collect();
                entries.insert(event.name(), list);
            }
        }
        Self { entries }
    }

    /// Runs the event's hooks and returns their combined new output, if any.
    pub(super) fn run(
        &mut self,
        event: HookEvent,
        cwd: &str,
        session_id: Option<&str>,
    ) -> Option<String> {
        let entries = self.entries.get_mut(event.name())?;
        let input =
            json!({ "event": event.name(), "cwd": cwd, "sessionId": session_id }).to_string();
        let mut outputs: Vec<String> = Vec::new();
        for entry in entries {
            let now = Instant::now();
            if let (Some(last), Some(min)) = (entry.last_run, entry.def.min_interval_secs) {
                if now.duration_since(last) < Duration::from_secs(min) {
                    continue;
                }
            }
            entry.last_run = Some(now);
            let timeout =
                Duration::from_secs(entry.def.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
            let Some(output) = run_command(&entry.def.command, cwd, &input, timeout) else {
                continue;
            };
            if output.is_empty() {
                entry.last_output = None;
            } else if entry.last_output.as_deref() != Some(output.as_str()) {
                entry.last_output = Some(output.clone());
                outputs.push(output);
            }
        }
        (!outputs.is_empty()).then(|| outputs.join("\n"))
    }
}

/// Runs `sh -c command`. Returns trimmed stdout on exit 0, `None` on failure or timeout.
fn run_command(command: &str, cwd: &str, input: &str, timeout: Duration) -> Option<String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = (&mut stdout)
            .take(MAX_OUTPUT_BYTES)
            .read_to_end(&mut buffer);
        buffer
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let bytes = reader.join().ok()?;
    status
        .success()
        .then(|| String::from_utf8_lossy(&bytes).trim().to_owned())
}

impl AgentCoordinator {
    /// Runs the hooks for `event` and returns text to add to the conversation.
    pub(super) fn run_hooks(&mut self, event: HookEvent) -> Option<String> {
        let cwd = self
            .registry
            .workspace_root()
            .to_string_lossy()
            .into_owned();
        let session_id = self.context_memory.session_id().map(str::to_owned);
        self.hooks.run(event, &cwd, session_id.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(json: &str) -> HookEngine {
        HookEngine::from_file(serde_json::from_str(json).unwrap())
    }

    #[test]
    fn output_is_injected_once_until_it_changes_and_failures_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let flag = dir.path().join("flag");
        let script = format!(
            "if [ -f {0} ]; then echo second; else echo first; fi",
            flag.display()
        );
        let mut engine = engine(&format!(
            r#"{{"hooks":{{"Stop":[{{"command":{script:?}}},{{"command":"exit 1"}}]}}}}"#
        ));

        assert_eq!(
            engine.run(HookEvent::Stop, cwd, None).as_deref(),
            Some("first")
        );
        assert_eq!(engine.run(HookEvent::Stop, cwd, None), None);
        std::fs::write(&flag, "").unwrap();
        assert_eq!(
            engine.run(HookEvent::Stop, cwd, None).as_deref(),
            Some("second")
        );
        // Events without hooks do nothing.
        assert_eq!(engine.run(HookEvent::PostToolRound, cwd, None), None);
    }

    #[test]
    fn min_interval_skips_and_timeout_kills() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let mut throttled = engine(
            r#"{"hooks":{"PostToolRound":[{"command":"date +%s%N","minIntervalSecs":60}]}}"#,
        );
        assert!(throttled.run(HookEvent::PostToolRound, cwd, None).is_some());
        assert_eq!(throttled.run(HookEvent::PostToolRound, cwd, None), None);

        let started = Instant::now();
        let mut slow =
            engine(r#"{"hooks":{"Stop":[{"command":"sleep 5; echo late","timeoutSecs":1}]}}"#);
        assert_eq!(slow.run(HookEvent::Stop, cwd, None), None);
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}
