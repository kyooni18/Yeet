//! End-to-end prompt-cache continuity invariants.
//!
//! These drive a real `AgentCoordinator` against a scripted stand-in bridge
//! (`testdata/fake_bridge.mjs`) and re-check the recorded provider requests
//! independently of `ContinuityTracker`: within one context window, every
//! request's provider-visible messages must extend the previous request's
//! messages unchanged. Only an explicit window change (rollover, rewind) may
//! rewrite that prefix. Cache continuity is a protocol invariant here, not an
//! optimization.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use serde_json::{Value, json};

use super::*;
use crate::{
    core::{BridgeClient, CallRequest},
    permission::PermissionBroker,
    tools::{BridgeHandle, ToolRegistry},
    workers::WorkerRegistry,
};

struct Harness {
    _dir: tempfile::TempDir,
    workspace: PathBuf,
    log: PathBuf,
    queue: PathBuf,
    mcp: PathBuf,
    disabled: Vec<String>,
    coordinator: AgentCoordinator,
}

impl Harness {
    fn start() -> Option<Self> {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let log = dir.path().join("requests.jsonl");
        let queue = dir.path().join("queue.json");
        let mcp = dir.path().join("mcp-tools.json");
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/agent/testdata/fake_bridge.mjs");
        let bridge = match BridgeClient::start_script(
            &script,
            &[
                ("YEET_FAKE_BRIDGE_LOG", log.as_path()),
                ("YEET_FAKE_BRIDGE_QUEUE", queue.as_path()),
                ("YEET_FAKE_BRIDGE_MCP", mcp.as_path()),
            ],
        ) {
            Ok(bridge) => bridge,
            Err(error) => {
                // Node is a declared runtime dependency; report rather than
                // silently pass when it is missing.
                eprintln!("skipping continuity e2e: node bridge unavailable: {error:#}");
                return None;
            }
        };
        let registry = ToolRegistry::new(
            bridge.clone(),
            workspace.clone(),
            WorkerRegistry::new(Vec::new()).unwrap(),
            PermissionBroker::default(),
        )
        .unwrap();
        let mut coordinator = AgentCoordinator::new(BridgeHandle::eager(bridge), registry);
        // Context rollover persists the retired window, so bind a session.
        let store = crate::session_store::SessionStore::new(&dir.path().join("config"));
        store.prepare().unwrap();
        coordinator.set_session_runtime(store, Some(crate::session_store::SessionStore::new_id()));
        Some(Self {
            _dir: dir,
            workspace,
            log,
            queue,
            mcp,
            disabled: Vec::new(),
            coordinator,
        })
    }

    fn enable_agent_group(&mut self) {
        let config = self._dir.path().join("config");
        let settings = crate::project_settings::ProjectSettingsStore::new(&self.workspace).unwrap();
        let factory = crate::agents::runtime::AgentRuntimeFactory::new(
            self.coordinator.bridge.clone(),
            self.workspace.clone(),
            WorkerRegistry::new(Vec::new()).unwrap(),
            PermissionBroker::default(),
            settings,
            "test-project".into(),
            crate::session_store::SessionStore::new(&config),
        );
        let supervisor = crate::agents::AgentGroupSupervisor::new(factory, Default::default());
        self.coordinator.set_agent_group(Some(supervisor.handle()));
    }

    fn script(&self, responses: Value) {
        fs::write(&self.queue, responses.to_string()).unwrap();
    }

    fn turn(&mut self, input: &str) -> AgentRunOutcome {
        self.run(input, false, false)
    }

    fn run(&mut self, input: &str, goal: bool, continuation: bool) -> AgentRunOutcome {
        self.coordinator
            .run(
                AgentRunRequest {
                    input,
                    images: Vec::new(),
                    model: "openai/test-model",
                    reasoning_level: "auto",
                    attached_capabilities: None,
                    disabled_capabilities: self.disabled.clone(),
                    goal_mode: Arc::new(AtomicBool::new(goal)),
                    cancel: Arc::new(AtomicBool::new(false)),
                    continuation,
                },
                |_| {},
            )
            .unwrap_or_else(|error| panic!("turn {input:?} failed: {error:#}"))
    }

    fn requests(&self) -> Vec<CallRequest> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

fn window(request: &CallRequest) -> String {
    request
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("contextWindowId"))
        .cloned()
        .or_else(|| request.context_key.clone())
        .unwrap_or_default()
}

/// Provider-visible message content with adapter-only flags removed.
fn wire(request: &CallRequest) -> Vec<Value> {
    request
        .messages
        .iter()
        .cloned()
        .map(|mut message| {
            message.cache_breakpoint = None;
            message.request_only = None;
            serde_json::to_value(message).unwrap()
        })
        .collect()
}

/// Asserts the append-only invariant between consecutive same-window
/// requests and returns how many window changes occurred.
fn assert_append_only(requests: &[CallRequest]) -> usize {
    let mut window_changes = 0;
    for pair in requests.windows(2) {
        let (previous, current) = (&pair[0], &pair[1]);
        if window(previous) != window(current) {
            window_changes += 1;
            continue;
        }
        let (previous, current) = (wire(previous), wire(current));
        let common = previous
            .iter()
            .zip(&current)
            .take_while(|(left, right)| left == right)
            .count();
        assert_eq!(
            common,
            previous.len(),
            "provider-visible prefix rewritten at message {common}: before={} after={}",
            previous[common],
            current.get(common).cloned().unwrap_or(Value::Null),
        );
    }
    window_changes
}

#[test]
fn ordinary_turns_tool_results_and_tool_surface_changes_stay_append_only() {
    let Some(mut harness) = Harness::start() else {
        return;
    };
    harness.turn("Say hello.");
    harness.turn("Now say goodbye.");

    // Tool-result insertion: a tool round inside one turn.
    harness.script(json!([
        {"toolCalls": [{"id": "call-1", "name": "find_capabilities", "arguments": {"query": "pdf"}}]},
        {"text": "No PDF capability is installed."}
    ]));
    harness.turn("Is there a PDF capability?");

    // A tool-surface change between turns (lazy activation / toggles change
    // the envelope) must not rewrite already-submitted history.
    harness.disabled = vec!["builtin:sessions".to_owned()];
    harness.turn("Thanks.");

    // Adaptive agent tools becoming available mid-session.
    harness.enable_agent_group();
    harness.turn("Carry on.");

    let requests = harness.requests();
    assert!(
        requests.len() >= 5,
        "expected one request per model attempt, got {}",
        requests.len()
    );
    assert_eq!(
        assert_append_only(&requests),
        0,
        "no window change expected"
    );
    let surface = |request: &CallRequest| -> Vec<String> {
        request
            .tools
            .iter()
            .flatten()
            .chain(request.deferred_tools.iter().flatten())
            .map(|tool| tool.name.clone())
            .collect()
    };
    assert!(
        !surface(&requests[0]).contains(&"agent".to_owned())
            && surface(requests.last().unwrap()).contains(&"agent".to_owned()),
        "the agent tool surface must actually change mid-session"
    );
    let last = wire(requests.last().unwrap());
    assert!(
        last.iter()
            .any(|message| message["role"] == "tool" && message["toolCallId"] == "call-1"),
        "tool result must be part of later provider-visible history"
    );
    assert!(
        requests.iter().any(|request| request
            .messages
            .iter()
            .any(|message| message.request_only == Some(true))),
        "request-only overlays must be under test"
    );
    assert!(harness.workspace.exists());
}

#[test]
fn goal_execution_and_resumed_continuation_stay_append_only() {
    let Some(mut harness) = Harness::start() else {
        return;
    };
    harness.turn("Set up the plan.");
    let outcome = harness.run("Finish the plan.", true, false);
    assert!(
        !matches!(outcome, AgentRunOutcome::Completed),
        "an unavailable goal judge must not report verified completion: {outcome:?}"
    );
    harness.run(
        "Continue the current goal from the existing working state.",
        true,
        true,
    );
    let requests = harness.requests();
    assert!(requests.len() >= 3);
    assert_eq!(assert_append_only(&requests), 0);
}

#[test]
#[should_panic(expected = "provider-visible prefix rewritten")]
fn append_only_check_detects_a_rewritten_prefix() {
    let mut first = CallRequest::simple("m", vec![Message::user("a"), Message::user("b")]);
    first.context_key = Some("window".into());
    let mut second = first.clone();
    second.messages[0] = Message::user("changed");
    assert_append_only(&[first, second]);
}

#[test]
fn mcp_activation_and_mcp_tool_results_stay_append_only() {
    let Some(mut harness) = Harness::start() else {
        return;
    };
    harness.turn("Hello.");

    // An MCP server appears between turns; the next run attaches it.
    fs::write(
        &harness.mcp,
        json!([{
            "server": "docs", "name": "search", "qualifiedName": "docs.search",
            "description": "Search docs", "annotations": {"readOnlyHint": true},
            "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}}
        }])
        .to_string(),
    )
    .unwrap();
    harness.turn("Any docs available?");
    // MCP schemas join the registry catalog; the coordinator attaches them
    // to a request lazily, which is itself a tool-surface change.
    let mcp_tool = harness
        .coordinator
        .registry
        .tools(false)
        .into_iter()
        .map(|tool| tool.name)
        .find(|name| name.contains("search") && name.contains("docs"))
        .expect("MCP tool registered after activation");

    harness.script(json!([
        {"toolCalls": [{"id": "find-1", "name": "search_tools", "arguments": {"query": "docs search"}}]},
        {"toolCalls": [{"id": "mcp-1", "name": mcp_tool, "arguments": {"q": "cache"}}]},
        {"text": "Found it."}
    ]));
    harness.turn("Use the docs MCP tool to look up cache.");

    let requests = harness.requests();
    assert_eq!(assert_append_only(&requests), 0);
    let last = serde_json::to_string(&wire(requests.last().unwrap())).unwrap();
    assert!(
        last.contains("docs result"),
        "MCP result must enter history: {last}"
    );
}

#[test]
fn rollover_and_rewind_are_explicit_window_changes() {
    let Some(mut harness) = Harness::start() else {
        return;
    };
    harness.turn("First request.");
    harness.turn("Second request.");
    let before_rollover = harness.requests().len();

    harness.coordinator.compact_model_history().unwrap();
    harness.turn("After rollover.");
    let after_rollover = harness.requests();
    assert_ne!(
        window(&after_rollover[before_rollover - 1]),
        window(&after_rollover[before_rollover]),
        "a rollover must open a new context window"
    );

    // Regenerate/edit-last rewinds to the turn checkpoint; it may only
    // rewrite history by starting a fresh window.
    let checkpoint = harness.coordinator.prepare_turn_history_checkpoint();
    harness.turn("Draft question.");
    harness
        .coordinator
        .rewind_model_history(checkpoint)
        .unwrap();
    harness.turn("Edited question.");

    let requests = harness.requests();
    assert_eq!(assert_append_only(&requests), 2);
    let last = wire(requests.last().unwrap());
    let rendered = serde_json::to_string(&last).unwrap();
    assert!(!rendered.contains("Draft question."));
    assert!(rendered.contains("Edited question."));
}
