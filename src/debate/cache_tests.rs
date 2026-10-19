//! Deterministic prompt-cache replay for a representative debate.
//!
//! Live provider runs are slow and noisy, so this drives the real request
//! builders through a scripted debate and replays the requests through a model
//! of Anthropic's block-prefix cache (the strictest provider Yeet supports):
//! tools → system → messages, explicit breakpoints, 20-block lookback, a
//! minimum cacheable prefix, and message-cache invalidation on `tool_choice`.
//! Token counts are chars/4 estimates; ratios, not absolute values, matter.

use super::research::{ResearchTools, run_with};
use super::*;
use crate::core::{CallResult, ToolCall};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::AtomicBool;

const MIN_CACHE_TOKENS: u64 = 1_024;
const LOOKBACK_BLOCKS: usize = 20;
const MAX_BREAKPOINTS: usize = 4;

#[derive(Default, Clone, Copy, Debug)]
pub(super) struct CacheTally {
    pub calls: u64,
    pub read: u64,
    pub write: u64,
    pub uncached: u64,
}

impl CacheTally {
    fn add(&mut self, other: CacheTally) {
        self.calls += other.calls;
        self.read += other.read;
        self.write += other.write;
        self.uncached += other.uncached;
    }
    fn input(&self) -> u64 {
        self.read + self.write + self.uncached
    }
    pub fn hit_rate(&self) -> f64 {
        self.read as f64 / self.input().max(1) as f64
    }
    /// Input cost in uncached-token equivalents (write 1.25x, read 0.1x).
    pub fn cost(&self) -> f64 {
        self.uncached as f64 + self.write as f64 * 1.25 + self.read as f64 * 0.1
    }
}

#[derive(Default)]
pub(super) struct ProviderCache {
    entries: HashSet<String>,
}

fn tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4)
}

impl ProviderCache {
    pub fn replay(&mut self, request: &CallRequest) -> CacheTally {
        let cache = request.prompt_cache != Some(false);
        let choice = request
            .tool_choice
            .as_ref()
            .map(Value::to_string)
            .unwrap_or_default();
        // (tokens, participates in tool_choice-sensitive key, explicit breakpoint)
        let mut blocks: Vec<(u64, String, bool)> = Vec::new();
        let tools = request.tools.clone().unwrap_or_default();
        for (index, tool) in tools.iter().enumerate() {
            let text = serde_json::to_string(tool).unwrap();
            blocks.push((tokens(&text), text, cache && index + 1 == tools.len()));
        }
        let mut messages = request.messages.iter().peekable();
        let mut system = String::new();
        while let Some(message) = messages.next_if(|m| m.role == crate::core::MessageRole::System) {
            system.push_str(message.content.as_deref().unwrap_or_default());
        }
        let prefix_blocks = blocks.len() + usize::from(!system.is_empty());
        if !system.is_empty() {
            blocks.push((tokens(&system), system, cache));
        }
        let mut explicit = Vec::new();
        for message in messages {
            let text = format!(
                "{:?}{}{}",
                message.role,
                message.content.as_deref().unwrap_or_default(),
                serde_json::to_string(&message.tool_calls).unwrap()
            );
            if cache && message.cache_breakpoint == Some(true) {
                explicit.push(blocks.len());
            }
            blocks.push((tokens(&text), format!("{choice}\0{text}"), false));
        }
        let reserved = usize::from(!tools.is_empty()) + usize::from(prefix_blocks > tools.len());
        let budget = MAX_BREAKPOINTS.saturating_sub(reserved);
        if explicit.len() > budget && budget > 0 {
            // Mirror selectStableAndRecentIndexes: keep the oldest plus newest.
            let mut kept = vec![explicit[0]];
            kept.extend(explicit[explicit.len() + 1 - budget..].iter().copied());
            explicit = kept;
        }
        for index in explicit {
            blocks[index].2 = true;
        }
        if cache && !blocks[prefix_blocks..].iter().any(|block| block.2) {
            if let Some(last) = blocks.last_mut() {
                last.2 = true;
            }
        }

        let mut hasher = Sha256::new();
        hasher.update(request.model.as_bytes());
        let mut keys = Vec::with_capacity(blocks.len());
        let mut cumulative = Vec::with_capacity(blocks.len());
        let mut total = 0;
        for (block_tokens, text, _) in &blocks {
            hasher.update(text.as_bytes());
            keys.push(format!("{:x}", hasher.clone().finalize()));
            total += block_tokens;
            cumulative.push(total);
        }
        let breakpoints: Vec<usize> = (0..blocks.len()).filter(|i| blocks[*i].2).collect();
        let mut read = 0;
        for &bp in &breakpoints {
            for position in (bp.saturating_sub(LOOKBACK_BLOCKS)..=bp).rev() {
                if self.entries.contains(&keys[position]) {
                    read = read.max(cumulative[position]);
                    break;
                }
            }
        }
        let mut written = read;
        for &bp in &breakpoints {
            if cumulative[bp] >= MIN_CACHE_TOKENS {
                self.entries.insert(keys[bp].clone());
                written = written.max(cumulative[bp]);
            }
        }
        CacheTally {
            calls: 1,
            read,
            write: written - read,
            uncached: total - written,
        }
    }
}

struct FakeTools {
    definitions: Vec<ToolDefinition>,
    calls: usize,
}

impl FakeTools {
    fn new() -> Self {
        let names = [
            "find_capabilities",
            "activate_capability",
            "read_file",
            "list_files",
            "search_workspace",
            "read_document",
            "analyze_data",
            "artifact_info",
            "read_artifact",
            "search_artifact",
            "web_search",
            "web_read",
            "git_log",
            "git_show",
            "mcp_docs_lookup",
            "skill_reference",
        ];
        let definitions = names
            .iter()
            .map(|name| {
                ToolDefinition::new(
                    *name,
                    format!("{name}: {}", "Read-only research capability. ".repeat(20)),
                    json!({"type":"object","properties":{
                        "path":{"type":"string","description":"x".repeat(600)},
                        "query":{"type":"string","description":"y".repeat(600)}
                    }}),
                )
            })
            .collect();
        Self {
            definitions,
            calls: 0,
        }
    }
}

impl ResearchTools for FakeTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        self.definitions.clone()
    }
    fn execute(&mut self, call: &ToolCall, _model: &str, _cancel: &AtomicBool) -> Result<String> {
        self.calls += 1;
        Ok(format!(
            "{} output for {}:\n{}",
            call.name,
            call.arguments,
            "source line with concrete evidence\n".repeat(90)
        ))
    }
}

fn text_result(text: impl Into<String>) -> CallResult {
    CallResult {
        provider: "sim".into(),
        model: "sim".into(),
        id: None,
        text: text.into(),
        reasoning: None,
        reasoning_summary: None,
        tool_calls: Vec::new(),
        provider_state: None,
        finish_reason: "stop".into(),
        usage: None,
        raw: None,
    }
}

fn tool_result(name: &str, arguments: Value) -> CallResult {
    let mut result = text_result("");
    result.finish_reason = "tool_use".into();
    result.tool_calls = vec![ToolCall {
        id: format!("call-{}", arguments),
        name: name.into(),
        arguments,
    }];
    result
}

/// Records every request a scripted debate sends, grouped by phase.
pub(super) fn scripted_debate_requests() -> Vec<(&'static str, CallRequest)> {
    let mut log = Vec::new();
    let mut debate = DebateState {
        topic: "The current implementation's retry policy prevents stalled runs.".into(),
        models: DebateModels {
            pro: "claude-sonnet-5".into(),
            con: "claude-sonnet-5".into(),
            jury: "claude-sonnet-5".into(),
        },
        subject: DebateSubject {
            workspace_bound: true,
            workspace_root: "/repo".into(),
            revision: Some("abc123".into()),
            anchor_files: vec!["Cargo.toml".into(), "README.md".into()],
        },
        ..DebateState::default()
    };
    log.push(("framing", debate.contract_request()));
    debate.contract = Some(DebateContract::minimal(&debate.topic));
    let cancel = AtomicBool::new(false);
    let speech = |stage: usize, pro: bool| {
        format!(
            "{} stage {stage} argument. {}",
            if pro { "PRO" } else { "CON" },
            "A detailed evidence-grounded point citing src/retry.rs and its limits. ".repeat(110)
        )
    };

    let research = |debate: &mut DebateState, stage: usize, log: &mut Vec<(&str, CallRequest)>| {
        for pro in [true, false] {
            let mut tools = FakeTools::new();
            let mut round = 0usize;
            let record = run_with(
                debate,
                stage,
                pro,
                &mut tools,
                &cancel,
                |_, _| Ok(()),
                |request| {
                    log.push(("research", request.clone()));
                    round += 1;
                    let synthesis = request.tools.as_ref().is_none_or(Vec::is_empty)
                        || request.tool_choice == Some(json!("none"))
                        || request
                            .messages
                            .last()
                            .and_then(|m| m.content.as_deref())
                            .is_some_and(|c| c.contains("synthesize the final compact dossier"));
                    Ok(if synthesis {
                        text_result(format!(
                            "Directly observed: {}\nInferences: ...\nUnresolved: ...",
                            "src/retry.rs bounds attempts. ".repeat(60)
                        ))
                    } else if round <= 4 {
                        tool_result(
                            "read_file",
                            json!({"path": format!("src/{}{stage}_{round}.rs", if pro {"p"} else {"c"})}),
                        )
                    } else {
                        text_result("Evidence collection looks sufficient.")
                    })
                },
            )
            .unwrap();
            debate.upsert_research_record(record);
        }
    };

    research(&mut debate, 0, &mut log);
    for stage in 0..4 {
        if stage == 2 {
            research(&mut debate, 2, &mut log);
        }
        if stage == 3 {
            debate.closing_stage = Some(3);
        }
        for pro in [true, false] {
            log.push(("advocate", debate.advocate_request(stage, pro)));
            debate
                .record_speech(Speech::from_response(stage, pro, speech(stage, pro)))
                .unwrap();
        }
        if stage == 2 {
            log.push(("checkpoint", debate.checkpoint_request(stage)));
        }
    }
    let ballots = [(9u8, 6u8), (5, 8), (8, 6), (7, 6)];
    for (a, b) in ballots {
        let reversed = debate.next_jury_reversed();
        log.push(("jury", debate.jury_request(reversed)));
        let ballot = tool_result(
            "submit_debate_ballot",
            json!({"a":[a,a,a,a],"b":[b,b,b,b],"reason":"Scores follow the evidence packet."}),
        );
        assert!(debate.record_jury_response(reversed, &ballot));
    }
    assert!(debate.jury_done());
    log.push(("knowledge", debate.knowledge_summary_request()));
    log
}

pub(super) fn replay(requests: &[(&'static str, CallRequest)]) -> BTreeMap<&'static str, CacheTally> {
    let mut cache = ProviderCache::default();
    let mut phases: BTreeMap<&str, CacheTally> = BTreeMap::new();
    for (phase, request) in requests {
        let tally = cache.replay(request);
        phases.entry(phase).or_default().add(tally);
        phases.entry("TOTAL").or_default().add(tally);
    }
    phases
}

fn report(phases: &BTreeMap<&str, CacheTally>) -> String {
    let mut out = String::from("phase       calls   input    read   write uncached   hit%   cost\n");
    for (phase, t) in phases {
        out.push_str(&format!(
            "{phase:<10} {:>5} {:>7} {:>7} {:>7} {:>7} {:>6.1} {:>7.0}\n",
            t.calls,
            t.input(),
            t.read,
            t.write,
            t.uncached,
            t.hit_rate() * 100.0,
            t.cost()
        ));
    }
    out
}

#[test]
fn simulated_debate_prompt_cache_report() {
    let phases = replay(&scripted_debate_requests());
    println!("{}", report(&phases));
}
