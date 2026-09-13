use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{Read, Write},
};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::{
    config::{ConfigStore, parse_context_length, validate_model_id},
    core::{
        BridgeClient, CallRequest, ImageAttachment, McpServerConfiguration, Message,
        OpenAiCompatibleProvider, StreamEvent, node_executable, runtime_directory,
    },
    project_settings::ProjectSettingsStore,
    sandbox_cli,
    session_store::SessionStore,
    web_search,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const HELP: &str = r#"Usage:
  yeet remote [WORKSPACE] [--workspace PATH] [--bind ADDRESS] [--size COLSxROWS] [--origin URL]
  yeet remote status [WORKSPACE|--workspace PATH]
  yeet remote stop [WORKSPACE|--workspace PATH]
  yeet remote auth status [WORKSPACE|--workspace PATH]
  yeet remote auth key generate|set|clear [WORKSPACE|--workspace PATH]
  yeet remote auth passkey add|clear [WORKSPACE|--workspace PATH]
  yeet doctor
  yeet update [check]
  yeet model get
  yeet model set provider/model
  yeet model context [get [provider/model]|set [provider/model] length|auto [provider/model]]
  yeet cache [latest|SESSION_ID]
  yeet run [--model provider/model] [--image path] [prompt]
  yeet auth status [provider]
  yeet usage [codex|claude|gemini|provider]
  printf key | yeet auth set-key provider
  yeet auth login provider
  yeet auth logout provider
  yeet provider list
  yeet provider add id base-url [--require-api-key] [--header Name=Value]
  yeet provider remove id
  yeet skill list
  yeet skill show name
  yeet skill read name path
  yeet skill validate path
  yeet skill install path|git-url|zip
  yeet skill remove name
  yeet mcp list
  yeet mcp add-stdio name command [args...]
  yeet mcp add-http name url [--header Name=Value]...
  yeet mcp remove name
  yeet mcp tools [server]
  yeet mcp call server/tool [json-arguments]
  yeet mcp resources [server]
  yeet mcp resource server uri
  yeet mcp prompts [server]
  yeet mcp prompt server name [json-string-arguments]
  yeet mcpserver start [--port PORT] [--bind HOST] [--workspace PATH] [--public-url URL] [--auth key|oauth|none]
  yeet mcpserver status [--port PORT]
  yeet mcpserver stop [--port PORT]
  yeet mcpserver restart [--port PORT] [...]
  yeet mcpserver auth [status|mode|key] ...
  yeet mcpserver stdio [WORKSPACE|--workspace PATH]
  yeet memory status
  yeet memory recall <query>
  yeet memory on|off
  yeet memory models
  yeet memory model [provider/model]
  yeet memory reindex [provider/model]
  yeet memory import-foundation <export.jsonl>
  yeet memory export-foundation <export.jsonl>
  yeet sandbox [show|path|reset|scratch|workspace|network|env|secret|limits] ...
  yeet search [install [all|searxng|agent-reach]|status|path|remove [all|searxng|agent-reach]]

State:
  YEET_CONFIG_DIR overrides the per-user state directory.
  Existing ~/.yeet installs are preserved on every OS.
  Clean Linux installs use XDG config paths; clean Windows installs use AppData.
  Project state remains under ./.yeet/.

Runtime:
  Yeet is Rust-native. The TypeScript provider/MCP runtime is shipped separately
  and requires Node.js 20+. Set YEET_RUNTIME_DIR and YEET_NODE to override paths.

Remote:
  Remote mode is opt-in and runs as a detached background daemon serving the
  existing TUI over HTTP. It binds to 0.0.0.0:7331 by default and can target
  a workspace without changing the terminal's current directory. Access keys
  and WebAuthn passkeys are optional and scoped per workspace. Port forwarding
  and tunneling are user-managed."#;

pub fn run(arguments: &[String]) -> Result<i32> {
    let command = arguments.first().map(String::as_str).unwrap_or("help");
    let rest = if arguments.is_empty() {
        &[][..]
    } else {
        &arguments[1..]
    };
    let result = match command {
        "-h" | "--help" | "help" => {
            println!("yeet {VERSION}\n\n{HELP}");
            Ok(())
        }
        "-v" | "--version" | "version" => {
            println!("{VERSION}");
            Ok(())
        }
        "doctor" => doctor(),
        "update" => crate::update::run(rest),
        "model" => model(rest),
        "cache" => cache(rest),
        "run" => run_model(rest),
        "auth" => auth(rest),
        "usage" => usage_command(rest),
        "provider" | "providers" | "endpoint" | "endpoints" => provider(rest),
        "skill" | "skills" => skill(rest),
        "mcp" => mcp(rest),
        "mcpserver" => {
            let output = crate::mcp_server::run_cli(rest)?;
            if !output.is_empty() {
                println!("{output}");
            }
            Ok(())
        }
        "foundation" | "memory" => foundation(rest),
        "search" => {
            let output = web_search::run_cli(rest)?;
            if !output.is_empty() {
                println!("{output}");
            }
            Ok(())
        }
        "sandbox" => {
            let root = std::env::current_dir()?
                .canonicalize()
                .unwrap_or(std::env::current_dir()?);
            let output = sandbox_cli::run(&root, rest)?;
            if !output.is_empty() {
                println!("{output}");
            }
            Ok(())
        }
        other => bail!("Unknown command: {other}"),
    };
    match result {
        Ok(()) => Ok(0),
        Err(error) => {
            eprintln!("yeet: {error}");
            Ok(1)
        }
    }
}

fn cache(args: &[String]) -> Result<()> {
    if args.len() > 1 {
        bail!("Usage: yeet cache [latest|SESSION_ID]");
    }
    let config = ConfigStore::default();
    let store = SessionStore::new(&config.directory);
    let workspace = std::env::current_dir()?
        .canonicalize()
        .unwrap_or(std::env::current_dir()?);
    let sessions = store.list(&workspace)?;
    let requested = args.first().map(String::as_str).unwrap_or("latest");
    let session = if requested == "latest" {
        sessions.first()
    } else {
        sessions.iter().find(|session| session.id == requested)
    }
    .ok_or_else(|| anyhow!("No matching Yeet session for this workspace"))?;

    let path = store
        .directory
        .join(&session.id)
        .join("tasks")
        .join("events.jsonl");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut report = summarize_cache_events(&content);
    if report["attempts"].as_u64().unwrap_or(0) == 0 {
        let debate_path = store
            .directory
            .join(&session.id)
            .join("debates")
            .join("events.jsonl");
        let debate_content = match fs::read_to_string(&debate_path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };
        let debate_report = summarize_debate_cache_events(&debate_content);
        if debate_report["attempts"].as_u64().unwrap_or(0) > 0 {
            report = debate_report;
        }
    }
    if let Some(object) = report.as_object_mut() {
        object.insert("sessionId".into(), json!(session.id));
        object.insert("title".into(), json!(session.title));
        object.insert("model".into(), json!(session.model));
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn summarize_cache_events(content: &str) -> Value {
    let mut attempts = 0u64;
    let mut measured_attempts = 0u64;
    let mut unreported_attempts = 0u64;
    let mut disabled_attempts = 0u64;
    let mut unknown_attempts = 0u64;
    let mut hits = 0u64;
    let mut misses = 0u64;
    let mut input_tokens = 0u64;
    let mut measured_input_tokens = 0u64;
    let mut unreported_input_tokens = 0u64;
    let mut disabled_input_tokens = 0u64;
    let mut unknown_input_tokens = 0u64;
    let mut cached_input_tokens = 0u64;
    let mut cache_write_input_tokens = 0u64;
    let mut cache_write_measured_input_tokens = 0u64;
    let mut ordinary_input_tokens = 0u64;
    let mut cost_equivalent_input_tokens = 0u64;
    let mut provider_cache_diagnostic_attempts = 0u64;
    let mut provider_cache_miss_attempts = 0u64;
    let mut provider_cache_missed_tokens = 0u64;
    let mut provider_comparison_reusable_tokens = 0u64;
    let mut stable_provider_reported_miss_attempts = 0u64;
    let mut stable_provider_unattributed_miss_attempts = 0u64;

    let mut root_model_attempts = 0u64;
    let mut repeated_tool_round_attempts = 0u64;
    let mut root_measured_attempts = 0u64;
    let mut root_hits = 0u64;
    let mut root_misses = 0u64;
    let mut root_measured_input_tokens = 0u64;
    let mut root_cached_input_tokens = 0u64;
    let mut followup_measured_attempts = 0u64;
    let mut followup_hits = 0u64;
    let mut followup_misses = 0u64;
    let mut followup_measured_input_tokens = 0u64;
    let mut followup_cached_input_tokens = 0u64;

    let mut stable_surface_measured_attempts = 0u64;
    let mut stable_surface_hits = 0u64;
    let mut stable_surface_misses = 0u64;
    let mut stable_surface_measured_input_tokens = 0u64;
    let mut stable_surface_cached_input_tokens = 0u64;
    let mut cache_reset_miss_attempts = 0u64;
    let mut cache_reuse_expected_attempts = 0u64;
    let mut cache_reuse_expected_miss_attempts = 0u64;

    let mut cache_surface_tokens = 0u64;
    let mut cache_surface_measured_attempts = 0u64;
    let mut min_cache_surface_tokens: Option<u64> = None;
    let mut max_cache_surface_tokens = 0u64;
    let mut stable_prefix_changes = 0u64;
    let mut tool_schema_changes = 0u64;
    let mut cache_surface_changes = 0u64;
    let mut breakpoint_plan_changes = 0u64;
    let mut prompt_cache_key_changes = 0u64;
    let mut context_key_changes = 0u64;
    let mut context_window_changes = 0u64;
    let mut cache_epoch_changes = 0u64;
    let mut history_prefix_rewrites = 0u64;
    let mut wire_history_prefix_rewrites = 0u64;
    let mut cache_relevant_history_rewrites = 0u64;
    let mut continuity_measured_attempts = 0u64;
    let mut wire_continuity_measured_attempts = 0u64;
    let mut stable_previous_request_input_tokens = 0u64;
    let mut stable_previous_request_cached_input_tokens = 0u64;
    let mut stable_previous_request_current_input_tokens = 0u64;
    let mut stable_novel_input_tokens = 0u64;
    let mut max_model_visible_tool_result_chars = 0u64;
    let mut max_system_instruction_chars = 0u64;
    let mut max_context_estimated_tokens = 0u64;
    let mut total_tool_calls = 0u64;
    let mut exact_repeated_tool_calls = 0u64;
    let mut duplicate_tool_results = 0u64;
    let mut duplicate_read_results = 0u64;
    let mut duplicate_read_bytes_avoided = 0u64;
    let mut tool_call_counts = BTreeMap::<String, u64>::new();
    let mut seen_exact_tool_calls = HashSet::<(String, String, String)>::new();
    let mut previous_visible_tool_chars = HashMap::<String, u64>::new();
    let mut measured_visible_tool_rounds = HashSet::<(String, u64)>::new();
    let mut new_visible_tool_result_chars = 0u64;
    let mut max_new_visible_tool_result_chars = 0u64;

    let mut previous_run_id: Option<String> = None;
    let mut previous_stable_prefix: Option<String> = None;
    let mut previous_tool_schema: Option<String> = None;
    let mut previous_cache_surface: Option<String> = None;
    let mut previous_breakpoint_plan: Option<String> = None;
    let mut previous_prompt_cache_key: Option<String> = None;
    let mut previous_context_key: Option<String> = None;
    let mut previous_context_window: Option<String> = None;
    let mut previous_cache_epoch: Option<u64> = None;
    let mut previous_input_tokens: Option<u64> = None;
    let mut seen_runs = HashSet::<String>::new();
    let mut seen_tool_rounds = HashSet::<(String, u64)>::new();
    let mut observed_cache_epochs = HashSet::<(String, u64)>::new();
    let mut cache_epoch_reason_counts = BTreeMap::<String, u64>::new();
    let mut miss_by_cache_epoch_reason = BTreeMap::<String, u64>::new();
    let mut provider_cache_miss_reasons = BTreeMap::<String, u64>::new();

    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let event_type = record.get("type").and_then(Value::as_str).unwrap_or("");
        if event_type == "agent-tool-call" {
            let run_id = record
                .get("runId")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned();
            if let Some(call) = record
                .get("payload")
                .and_then(|payload| payload.get("call"))
                && let Some(name) = call.get("name").and_then(Value::as_str)
            {
                total_tool_calls += 1;
                *tool_call_counts.entry(name.to_owned()).or_default() += 1;
                let arguments = call
                    .get("arguments")
                    .and_then(|value| serde_json::to_string(value).ok())
                    .unwrap_or_default();
                if !seen_exact_tool_calls.insert((run_id, name.to_owned(), arguments)) {
                    exact_repeated_tool_calls += 1;
                }
            }
            continue;
        }
        if event_type == "agent-tool-finished" {
            let payload = record.get("payload");
            let tool_name = payload
                .and_then(|payload| payload.get("call"))
                .and_then(|call| call.get("name"))
                .and_then(Value::as_str);
            if let Some(result) = payload
                .and_then(|payload| payload.get("result"))
                .and_then(Value::as_str)
                .and_then(|result| serde_json::from_str::<Value>(result).ok())
            {
                let (duplicates, avoided_bytes) = tool_result_efficiency(&result);
                duplicate_tool_results = duplicate_tool_results.saturating_add(duplicates);
                duplicate_read_bytes_avoided =
                    duplicate_read_bytes_avoided.saturating_add(avoided_bytes);
                if matches!(tool_name, Some("read_file" | "read_files")) {
                    duplicate_read_results = duplicate_read_results.saturating_add(duplicates);
                }
            }
            continue;
        }
        if event_type != "agent-model-attempt-finished" {
            continue;
        }
        let Some(diagnostics) = record
            .get("payload")
            .and_then(|payload| payload.get("cacheDiagnostics"))
        else {
            continue;
        };
        let run_id = record
            .get("runId")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        seen_runs.insert(run_id.clone());
        if previous_run_id.as_deref() != Some(run_id.as_str()) {
            previous_run_id = Some(run_id.clone());
            previous_stable_prefix = None;
            previous_tool_schema = None;
            previous_cache_surface = None;
            previous_breakpoint_plan = None;
            previous_prompt_cache_key = None;
            previous_context_key = None;
            previous_context_window = None;
            previous_cache_epoch = None;
            previous_input_tokens = None;
        }

        attempts += 1;
        let input = u64_value(diagnostics.get("inputTokens")).unwrap_or(0);
        let cached = u64_value(diagnostics.get("cachedInputTokens"));
        let cache_write = u64_value(diagnostics.get("cacheWriteInputTokens"));
        let ordinary = u64_value(diagnostics.get("ordinaryInputTokens"));
        let cost_equivalent = u64_value(diagnostics.get("costEquivalentInputTokens"));
        let provider_cache_diagnostic_type = diagnostics
            .get("providerCacheDiagnosticType")
            .and_then(Value::as_str);
        if provider_cache_diagnostic_type.is_some() {
            provider_cache_diagnostic_attempts += 1;
        }
        if provider_cache_diagnostic_type == Some("cache_miss") {
            provider_cache_miss_attempts += 1;
            provider_cache_missed_tokens = provider_cache_missed_tokens.saturating_add(
                u64_value(diagnostics.get("providerCacheMissedTokens")).unwrap_or(0),
            );
            provider_comparison_reusable_tokens = provider_comparison_reusable_tokens
                .saturating_add(
                    u64_value(diagnostics.get("providerComparisonReusableTokens")).unwrap_or(0),
                );
            if let Some(reason) = diagnostics
                .get("providerCacheMissReason")
                .and_then(Value::as_str)
            {
                *provider_cache_miss_reasons
                    .entry(reason.to_owned())
                    .or_default() += 1;
            }
        }
        let tool_round = u64_value(diagnostics.get("toolRound"));
        let root_attempt = tool_round == Some(0);
        if root_attempt {
            root_model_attempts += 1;
        }
        if let Some(tool_round) = tool_round
            && !seen_tool_rounds.insert((run_id.clone(), tool_round))
        {
            repeated_tool_round_attempts += 1;
        }
        let visible_tool_chars =
            u64_value(diagnostics.get("modelVisibleToolResultChars")).unwrap_or(0);
        if let Some(tool_round) = tool_round
            && tool_round > 0
            && measured_visible_tool_rounds.insert((run_id.clone(), tool_round))
        {
            let previous = previous_visible_tool_chars
                .get(&run_id)
                .copied()
                .unwrap_or(0);
            let added = visible_tool_chars.saturating_sub(previous);
            new_visible_tool_result_chars = new_visible_tool_result_chars.saturating_add(added);
            max_new_visible_tool_result_chars = max_new_visible_tool_result_chars.max(added);
        }
        previous_visible_tool_chars.insert(run_id.clone(), visible_tool_chars);

        let stable_prefix_changed = hash_changed(
            &mut previous_stable_prefix,
            diagnostics.get("stablePrefixHash").and_then(Value::as_str),
        );
        let tool_schema_changed = hash_changed(
            &mut previous_tool_schema,
            diagnostics.get("toolSchemaHash").and_then(Value::as_str),
        );
        let cache_surface_changed = hash_changed(
            &mut previous_cache_surface,
            diagnostics.get("cacheSurfaceHash").and_then(Value::as_str),
        );
        let breakpoint_plan_changed = hash_changed(
            &mut previous_breakpoint_plan,
            diagnostics
                .get("breakpointPlanHash")
                .and_then(Value::as_str),
        );
        let prompt_cache_key_changed = hash_changed(
            &mut previous_prompt_cache_key,
            diagnostics
                .get("promptCacheKeyHash")
                .and_then(Value::as_str),
        );
        let context_key_changed = hash_changed(
            &mut previous_context_key,
            diagnostics.get("contextKey").and_then(Value::as_str),
        );
        let context_window_changed = hash_changed(
            &mut previous_context_window,
            diagnostics.get("contextWindowId").and_then(Value::as_str),
        );
        let cache_epoch = u64_value(diagnostics.get("cacheEpoch"));
        let cache_epoch_changed = numeric_changed(&mut previous_cache_epoch, cache_epoch);
        if let Some(cache_epoch) = cache_epoch {
            observed_cache_epochs.insert((run_id.clone(), cache_epoch));
        }
        stable_prefix_changes += stable_prefix_changed as u64;
        tool_schema_changes += tool_schema_changed as u64;
        cache_surface_changes += cache_surface_changed as u64;
        breakpoint_plan_changes += breakpoint_plan_changed as u64;
        prompt_cache_key_changes += prompt_cache_key_changed as u64;
        context_key_changes += context_key_changed as u64;
        context_window_changes += context_window_changed as u64;
        cache_epoch_changes += cache_epoch_changed as u64;

        let cache_epoch_reason = diagnostics
            .get("cacheEpochReason")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        *cache_epoch_reason_counts
            .entry(cache_epoch_reason.to_owned())
            .or_default() += 1;
        let history_prefix_continues = diagnostics
            .get("historyPrefixContinues")
            .and_then(Value::as_bool);
        if let Some(continues) = history_prefix_continues {
            continuity_measured_attempts += 1;
            history_prefix_rewrites += (!continues) as u64;
        }
        let wire_history_prefix_continues = diagnostics
            .get("wireHistoryPrefixContinues")
            .and_then(Value::as_bool);
        if let Some(continues) = wire_history_prefix_continues {
            wire_continuity_measured_attempts += 1;
            wire_history_prefix_rewrites += (!continues) as u64;
        }
        let cache_relevant_history_rewrite = diagnostics
            .get("cacheRelevantHistoryRewriteDetected")
            .and_then(Value::as_bool)
            .unwrap_or(history_prefix_continues == Some(false));
        cache_relevant_history_rewrites += cache_relevant_history_rewrite as u64;
        let stable_surface = cache_epoch_reason == "stable"
            && !cache_relevant_history_rewrite
            && wire_history_prefix_continues != Some(false)
            && !stable_prefix_changed
            && !tool_schema_changed
            && !cache_surface_changed
            && !prompt_cache_key_changed
            && !context_key_changed
            && !context_window_changed
            && !cache_epoch_changed;
        let cache_reuse_expected =
            u64_value(diagnostics.get("expectedCacheReuses")).is_some_and(|value| value > 0);
        if cache_reuse_expected {
            cache_reuse_expected_attempts += 1;
        }

        let status = diagnostics
            .get("cacheStatus")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if stable_surface && status == "miss" {
            match diagnostics
                .get("cacheMissAttribution")
                .and_then(Value::as_str)
            {
                Some("provider-reported") => stable_provider_reported_miss_attempts += 1,
                Some("stable-provider-miss-unattributed") | None => {
                    stable_provider_unattributed_miss_attempts += 1
                }
                _ => {}
            }
        }
        let measured = matches!(status, "hit" | "miss");
        match status {
            "hit" => {
                hits += 1;
                measured_attempts += 1;
                measured_input_tokens = measured_input_tokens.saturating_add(input);
                cached_input_tokens =
                    cached_input_tokens.saturating_add(cached.unwrap_or(0).min(input));
            }
            "miss" => {
                misses += 1;
                measured_attempts += 1;
                measured_input_tokens = measured_input_tokens.saturating_add(input);
                cached_input_tokens =
                    cached_input_tokens.saturating_add(cached.unwrap_or(0).min(input));
                *miss_by_cache_epoch_reason
                    .entry(cache_epoch_reason.to_owned())
                    .or_default() += 1;
                if cache_epoch_reason != "stable" {
                    cache_reset_miss_attempts += 1;
                }
                if cache_reuse_expected {
                    cache_reuse_expected_miss_attempts += 1;
                }
            }
            "unreported" => {
                unreported_attempts += 1;
                unreported_input_tokens = unreported_input_tokens.saturating_add(input);
            }
            "disabled" => {
                disabled_attempts += 1;
                disabled_input_tokens = disabled_input_tokens.saturating_add(input);
            }
            _ => {
                unknown_attempts += 1;
                unknown_input_tokens = unknown_input_tokens.saturating_add(input);
            }
        }
        if measured {
            let measured_cached = cached.unwrap_or(0).min(input);
            if root_attempt {
                root_measured_attempts += 1;
                root_measured_input_tokens = root_measured_input_tokens.saturating_add(input);
                root_cached_input_tokens = root_cached_input_tokens.saturating_add(measured_cached);
                root_hits += (status == "hit") as u64;
                root_misses += (status == "miss") as u64;
            } else {
                followup_measured_attempts += 1;
                followup_measured_input_tokens =
                    followup_measured_input_tokens.saturating_add(input);
                followup_cached_input_tokens =
                    followup_cached_input_tokens.saturating_add(measured_cached);
                followup_hits += (status == "hit") as u64;
                followup_misses += (status == "miss") as u64;
            }
            if stable_surface {
                stable_surface_measured_attempts += 1;
                stable_surface_measured_input_tokens =
                    stable_surface_measured_input_tokens.saturating_add(input);
                stable_surface_cached_input_tokens =
                    stable_surface_cached_input_tokens.saturating_add(measured_cached);
                stable_surface_hits += (status == "hit") as u64;
                stable_surface_misses += (status == "miss") as u64;
            }
            if stable_surface && let Some(previous_input) = previous_input_tokens {
                let reusable = previous_input.min(input);
                stable_previous_request_current_input_tokens =
                    stable_previous_request_current_input_tokens.saturating_add(input);
                stable_novel_input_tokens =
                    stable_novel_input_tokens.saturating_add(input.saturating_sub(reusable));
                stable_previous_request_input_tokens =
                    stable_previous_request_input_tokens.saturating_add(reusable);
                stable_previous_request_cached_input_tokens =
                    stable_previous_request_cached_input_tokens
                        .saturating_add(measured_cached.min(reusable));
            }
        }

        input_tokens = input_tokens.saturating_add(input);
        if let Some(cache_write) = cache_write {
            cache_write_input_tokens =
                cache_write_input_tokens.saturating_add(cache_write.min(input));
            cache_write_measured_input_tokens =
                cache_write_measured_input_tokens.saturating_add(input);
        }
        ordinary_input_tokens = ordinary_input_tokens.saturating_add(ordinary.unwrap_or(0));
        cost_equivalent_input_tokens =
            cost_equivalent_input_tokens.saturating_add(cost_equivalent.unwrap_or(0));
        if let Some(surface_tokens) = u64_value(diagnostics.get("estimatedCacheSurfaceTokens")) {
            cache_surface_measured_attempts += 1;
            cache_surface_tokens = cache_surface_tokens.saturating_add(surface_tokens);
            min_cache_surface_tokens = Some(
                min_cache_surface_tokens
                    .map_or(surface_tokens, |current| current.min(surface_tokens)),
            );
            max_cache_surface_tokens = max_cache_surface_tokens.max(surface_tokens);
        }
        max_model_visible_tool_result_chars = max_model_visible_tool_result_chars
            .max(u64_value(diagnostics.get("modelVisibleToolResultChars")).unwrap_or(0));
        max_system_instruction_chars = max_system_instruction_chars
            .max(u64_value(diagnostics.get("systemInstructionChars")).unwrap_or(0));
        max_context_estimated_tokens = max_context_estimated_tokens
            .max(u64_value(diagnostics.get("contextEstimatedTokens")).unwrap_or(0));
        previous_input_tokens = Some(input);
    }

    let rate = |numerator: u64, denominator: u64| {
        (denominator > 0).then(|| numerator as f64 / denominator as f64)
    };
    let mut report = Map::new();
    macro_rules! put {
        ($key:literal, $value:expr) => {
            report.insert($key.into(), json!($value));
        };
    }
    put!("telemetryAvailable", attempts > 0);
    put!("turns", seen_runs.len() as u64);
    put!("attempts", attempts);
    put!("toolRounds", seen_tool_rounds.len() as u64);
    let executed_tool_rounds = seen_tool_rounds
        .iter()
        .filter(|(_, round)| *round > 0)
        .count() as u64;
    put!("executedToolRounds", executed_tool_rounds);
    put!(
        "modelAttemptsPerTurn",
        rate(attempts, seen_runs.len() as u64)
    );
    put!(
        "toolRoundsPerTurn",
        rate(executed_tool_rounds, seen_runs.len() as u64)
    );
    put!("totalToolCalls", total_tool_calls);
    put!(
        "toolCallsPerRound",
        rate(total_tool_calls, executed_tool_rounds)
    );
    put!("toolCallCounts", tool_call_counts);
    put!("exactRepeatedToolCalls", exact_repeated_tool_calls);
    put!("duplicateToolResults", duplicate_tool_results);
    put!("duplicateReadResults", duplicate_read_results);
    put!("duplicateReadBytesAvoided", duplicate_read_bytes_avoided);
    put!(
        "averageNewModelVisibleToolResultCharsPerRound",
        rate(
            new_visible_tool_result_chars,
            measured_visible_tool_rounds.len() as u64
        )
    );
    put!(
        "maximumNewModelVisibleToolResultCharsPerRound",
        (!measured_visible_tool_rounds.is_empty()).then_some(max_new_visible_tool_result_chars)
    );
    put!("schemaPromotionCount", tool_schema_changes);
    put!("wireRewriteCount", wire_history_prefix_rewrites);
    put!(
        "costEquivalentInputTokensPerTurn",
        rate(cost_equivalent_input_tokens, seen_runs.len() as u64)
    );
    put!("rootModelAttempts", root_model_attempts);
    put!("repeatedToolRoundAttempts", repeated_tool_round_attempts);
    put!("measuredAttempts", measured_attempts);
    put!("unreportedAttempts", unreported_attempts);
    put!("disabledAttempts", disabled_attempts);
    put!("unknownAttempts", unknown_attempts);
    put!("hitAttempts", hits);
    put!("missAttempts", misses);
    put!("cacheAttemptHitRate", rate(hits, measured_attempts));
    put!("inputTokens", input_tokens);
    put!("cacheMeasuredInputTokens", measured_input_tokens);
    put!("cacheUnreportedInputTokens", unreported_input_tokens);
    put!("cacheDisabledInputTokens", disabled_input_tokens);
    put!("cacheUnknownInputTokens", unknown_input_tokens);
    put!("cachedInputTokens", cached_input_tokens);
    put!("cacheWriteInputTokens", cache_write_input_tokens);
    put!(
        "cacheWriteMeasuredInputTokens",
        cache_write_measured_input_tokens
    );
    put!("ordinaryInputTokens", ordinary_input_tokens);
    put!("costEquivalentInputTokens", cost_equivalent_input_tokens);
    put!(
        "costEquivalentInputRate",
        rate(cost_equivalent_input_tokens, input_tokens)
    );
    put!(
        "providerCacheDiagnosticAttempts",
        provider_cache_diagnostic_attempts
    );
    put!("providerCacheMissAttempts", provider_cache_miss_attempts);
    put!("providerCacheMissedTokens", provider_cache_missed_tokens);
    put!(
        "providerComparisonReusableTokens",
        provider_comparison_reusable_tokens
    );
    put!("providerCacheMissReasons", provider_cache_miss_reasons);
    put!(
        "stableProviderReportedMissAttempts",
        stable_provider_reported_miss_attempts
    );
    put!(
        "stableProviderUnattributedMissAttempts",
        stable_provider_unattributed_miss_attempts
    );
    put!(
        "cacheHitRate",
        rate(cached_input_tokens, measured_input_tokens)
    );
    put!(
        "cacheWriteRate",
        rate(cache_write_input_tokens, cache_write_measured_input_tokens)
    );
    put!(
        "ordinaryInputRate",
        rate(ordinary_input_tokens, measured_input_tokens)
    );
    put!(
        "cacheMeasurementCoverageRate",
        rate(measured_input_tokens, input_tokens)
    );
    put!(
        "cacheWriteMeasurementCoverageRate",
        rate(cache_write_measured_input_tokens, input_tokens)
    );
    put!(
        "attemptMeasurementCoverageRate",
        rate(measured_attempts, attempts)
    );
    put!("rootMeasuredAttempts", root_measured_attempts);
    put!("rootHitAttempts", root_hits);
    put!("rootMissAttempts", root_misses);
    put!("rootCacheMeasuredInputTokens", root_measured_input_tokens);
    put!("rootCachedInputTokens", root_cached_input_tokens);
    put!(
        "rootCacheHitRate",
        rate(root_cached_input_tokens, root_measured_input_tokens)
    );
    put!("followupMeasuredAttempts", followup_measured_attempts);
    put!("followupHitAttempts", followup_hits);
    put!("followupMissAttempts", followup_misses);
    put!(
        "followupCacheMeasuredInputTokens",
        followup_measured_input_tokens
    );
    put!("followupCachedInputTokens", followup_cached_input_tokens);
    put!(
        "followupCacheHitRate",
        rate(followup_cached_input_tokens, followup_measured_input_tokens)
    );
    put!(
        "stableSurfaceMeasuredAttempts",
        stable_surface_measured_attempts
    );
    put!("stableSurfaceHitAttempts", stable_surface_hits);
    put!("stableSurfaceMissAttempts", stable_surface_misses);
    put!(
        "stableSurfaceAttemptHitRate",
        rate(stable_surface_hits, stable_surface_measured_attempts)
    );
    put!(
        "stablePreviousRequestInputTokens",
        stable_previous_request_input_tokens
    );
    put!(
        "stablePreviousRequestCachedInputTokens",
        stable_previous_request_cached_input_tokens
    );
    put!(
        "stablePreviousRequestReuseRate",
        rate(
            stable_previous_request_cached_input_tokens,
            stable_previous_request_input_tokens
        )
    );
    put!(
        "stablePreviousRequestCurrentInputTokens",
        stable_previous_request_current_input_tokens
    );
    put!("stableNovelInputTokens", stable_novel_input_tokens);
    put!(
        "stablePreviousRequestShareCeilingRate",
        rate(
            stable_previous_request_input_tokens,
            stable_previous_request_current_input_tokens
        )
    );
    put!(
        "stableNovelInputRate",
        rate(
            stable_novel_input_tokens,
            stable_previous_request_current_input_tokens
        )
    );
    put!(
        "stableSurfaceCacheMeasuredInputTokens",
        stable_surface_measured_input_tokens
    );
    put!(
        "stableSurfaceCachedInputTokens",
        stable_surface_cached_input_tokens
    );
    put!(
        "stableSurfaceCacheHitRate",
        rate(
            stable_surface_cached_input_tokens,
            stable_surface_measured_input_tokens
        )
    );
    put!("cacheResetMissAttempts", cache_reset_miss_attempts);
    put!("cacheReuseExpectedAttempts", cache_reuse_expected_attempts);
    put!(
        "cacheReuseExpectedMissAttempts",
        cache_reuse_expected_miss_attempts
    );
    put!(
        "averageEstimatedCacheSurfaceTokens",
        rate(cache_surface_tokens, cache_surface_measured_attempts)
    );
    put!(
        "minimumEstimatedCacheSurfaceTokens",
        min_cache_surface_tokens
    );
    put!(
        "maximumEstimatedCacheSurfaceTokens",
        (cache_surface_measured_attempts > 0).then_some(max_cache_surface_tokens)
    );
    put!("intraTurnStablePrefixChanges", stable_prefix_changes);
    put!("intraTurnToolSchemaChanges", tool_schema_changes);
    put!("intraTurnCacheSurfaceChanges", cache_surface_changes);
    put!("intraTurnBreakpointPlanChanges", breakpoint_plan_changes);
    put!("intraTurnPromptCacheKeyChanges", prompt_cache_key_changes);
    put!("intraTurnContextKeyChanges", context_key_changes);
    put!("intraTurnContextWindowChanges", context_window_changes);
    put!("intraTurnCacheEpochChanges", cache_epoch_changes);
    put!("cacheEpochsObserved", observed_cache_epochs.len() as u64);
    put!("cacheEpochReasonCounts", cache_epoch_reason_counts);
    put!("missAttemptsByCacheEpochReason", miss_by_cache_epoch_reason);
    put!(
        "historyPrefixContinuityMeasuredAttempts",
        continuity_measured_attempts
    );
    put!("historyPrefixRewrites", history_prefix_rewrites);
    put!(
        "wireHistoryPrefixContinuityMeasuredAttempts",
        wire_continuity_measured_attempts
    );
    put!("wireHistoryPrefixRewrites", wire_history_prefix_rewrites);
    put!(
        "cacheRelevantHistoryRewrites",
        cache_relevant_history_rewrites
    );
    put!(
        "maximumModelVisibleToolResultChars",
        (attempts > 0).then_some(max_model_visible_tool_result_chars)
    );
    put!(
        "maximumSystemInstructionChars",
        (attempts > 0).then_some(max_system_instruction_chars)
    );
    put!(
        "maximumContextEstimatedTokens",
        (attempts > 0).then_some(max_context_estimated_tokens)
    );
    put!(
        "cacheHitRateSemantics",
        "cached input tokens divided by measured input tokens; this is a token-cost reuse share, not the fraction of model attempts that hit the cache"
    );
    put!(
        "cacheAttemptHitRateSemantics",
        "model attempts reporting a nonzero cache read divided by attempts with explicit cache-read telemetry"
    );
    put!(
        "stablePreviousRequestReuseSemantics",
        "on stable measured follow-up attempts, cached tokens divided by the immediately preceding request's input tokens, capped by the current request size; this estimates how much of the previously available prompt was reused"
    );
    put!(
        "stablePreviousRequestShareCeilingSemantics",
        "on stable measured follow-up attempts with a prior request, immediately preceding request input tokens divided by current request input tokens; this is the maximum current-request cache share attainable from immediate-prefix reuse before provider/cache misses"
    );
    put!(
        "stableNovelInputSemantics",
        "on the same stable follow-ups, current request input tokens not present in the immediately preceding request; this novel tail is expected to remain uncached on its first appearance and should not be mislabeled as a cache failure"
    );
    put!(
        "rootAttemptSemantics",
        "rootModelAttempts counts toolRound=0; repeatedToolRoundAttempts counts additional model attempts for an already-seen run/toolRound"
    );
    put!(
        "stableSurfaceSemantics",
        "stable-surface attempts exclude cache-relevant semantic or wire-history rewrites and changes to stable prefix, tool schema, cache surface, prompt cache key, context key/window, or cache epoch; completed-turn post-base pruning and request-local breakpoint rotation do not disqualify stability"
    );
    put!(
        "continuitySemantics",
        "historyPrefixRewrites tracks durable semantic divergence; wireHistoryPrefixRewrites also tracks request-only wire ordering/content; cacheRelevantHistoryRewrites excludes completed-turn divergence after an append-only reusable base prefix"
    );
    put!("note", (attempts == 0).then_some("No per-attempt cache telemetry exists in this session; run a new turn with the current Yeet build."));
    Value::Object(report)
}


fn summarize_debate_cache_events(content: &str) -> Value {
    let mut turns = HashSet::new();
    let mut attempts = 0u64;
    let mut measured_attempts = 0u64;
    let mut unreported_attempts = 0u64;
    let mut hits = 0u64;
    let mut misses = 0u64;
    let mut input_tokens = 0u64;
    let mut measured_input_tokens = 0u64;
    let mut unreported_input_tokens = 0u64;
    let mut cached_input_tokens = 0u64;
    let mut cache_write_input_tokens = 0u64;
    let mut cost_equivalent_input_tokens = 0u64;
    let mut root_measured_attempts = 0u64;
    let mut root_hits = 0u64;
    let mut root_misses = 0u64;
    let mut root_measured_input_tokens = 0u64;
    let mut root_cached_input_tokens = 0u64;
    let mut followup_measured_attempts = 0u64;
    let mut followup_hits = 0u64;
    let mut followup_misses = 0u64;
    let mut followup_measured_input_tokens = 0u64;
    let mut followup_cached_input_tokens = 0u64;

    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) != Some("debate-research")
            || event.pointer("/payload/event").and_then(Value::as_str) != Some("response")
        {
            continue;
        }
        let Some(usage) = event.pointer("/payload/detail/response/usage") else {
            continue;
        };
        attempts = attempts.saturating_add(1);
        let input = u64_value(usage.get("inputTokens")).unwrap_or(0);
        input_tokens = input_tokens.saturating_add(input);
        cache_write_input_tokens = cache_write_input_tokens.saturating_add(
            u64_value(usage.get("cacheWriteInputTokens")).unwrap_or(0).min(input),
        );
        cost_equivalent_input_tokens = cost_equivalent_input_tokens.saturating_add(
            u64_value(usage.get("costEquivalentInputTokens")).unwrap_or(input),
        );

        let run_id = event.get("runId").and_then(Value::as_str).unwrap_or("unknown");
        let stage = event.pointer("/payload/stage").and_then(Value::as_u64).unwrap_or(0);
        let pro = event.pointer("/payload/pro").and_then(Value::as_bool).unwrap_or(false);
        turns.insert(format!("{run_id}:{stage}:{pro}"));
        let root = event.pointer("/payload/detail/round").and_then(Value::as_u64) == Some(0);

        let Some(measured) = u64_value(usage.get("cacheMeasuredInputTokens")) else {
            unreported_attempts = unreported_attempts.saturating_add(1);
            unreported_input_tokens = unreported_input_tokens.saturating_add(input);
            continue;
        };
        let cached = u64_value(usage.get("cachedInputTokens"))
            .unwrap_or(0)
            .min(measured);
        measured_attempts = measured_attempts.saturating_add(1);
        measured_input_tokens = measured_input_tokens.saturating_add(measured);
        cached_input_tokens = cached_input_tokens.saturating_add(cached);
        if cached > 0 {
            hits = hits.saturating_add(1);
        } else {
            misses = misses.saturating_add(1);
        }
        if root {
            root_measured_attempts = root_measured_attempts.saturating_add(1);
            root_measured_input_tokens = root_measured_input_tokens.saturating_add(measured);
            root_cached_input_tokens = root_cached_input_tokens.saturating_add(cached);
            if cached > 0 {
                root_hits = root_hits.saturating_add(1);
            } else {
                root_misses = root_misses.saturating_add(1);
            }
        } else {
            followup_measured_attempts = followup_measured_attempts.saturating_add(1);
            followup_measured_input_tokens = followup_measured_input_tokens.saturating_add(measured);
            followup_cached_input_tokens = followup_cached_input_tokens.saturating_add(cached);
            if cached > 0 {
                followup_hits = followup_hits.saturating_add(1);
            } else {
                followup_misses = followup_misses.saturating_add(1);
            }
        }
    }

    let rate = |numerator: u64, denominator: u64| {
        (denominator > 0).then_some(numerator as f64 / denominator as f64)
    };
    json!({
        "turns": turns.len() as u64,
        "attempts": attempts,
        "measuredAttempts": measured_attempts,
        "unreportedAttempts": unreported_attempts,
        "hitAttempts": hits,
        "missAttempts": misses,
        "cacheAttemptHitRate": rate(hits, measured_attempts),
        "inputTokens": input_tokens,
        "cacheMeasuredInputTokens": measured_input_tokens,
        "cacheUnreportedInputTokens": unreported_input_tokens,
        "cachedInputTokens": cached_input_tokens,
        "cacheWriteInputTokens": cache_write_input_tokens,
        "costEquivalentInputTokens": cost_equivalent_input_tokens,
        "cacheHitRate": rate(cached_input_tokens, measured_input_tokens),
        "costEquivalentInputRate": rate(cost_equivalent_input_tokens, input_tokens),
        "cacheMeasurementCoverageRate": rate(measured_input_tokens, input_tokens),
        "attemptMeasurementCoverageRate": rate(measured_attempts, attempts),
        "rootMeasuredAttempts": root_measured_attempts,
        "rootHitAttempts": root_hits,
        "rootMissAttempts": root_misses,
        "rootCacheMeasuredInputTokens": root_measured_input_tokens,
        "rootCachedInputTokens": root_cached_input_tokens,
        "rootCacheHitRate": rate(root_cached_input_tokens, root_measured_input_tokens),
        "followupMeasuredAttempts": followup_measured_attempts,
        "followupHitAttempts": followup_hits,
        "followupMissAttempts": followup_misses,
        "followupCacheMeasuredInputTokens": followup_measured_input_tokens,
        "followupCachedInputTokens": followup_cached_input_tokens,
        "followupCacheHitRate": rate(followup_cached_input_tokens, followup_measured_input_tokens),
        "telemetryAvailable": attempts > 0,
        "telemetrySource": "debates/events.jsonl",
        "cacheHitRateSemantics": "cached input tokens divided by measured input tokens; this is a token-cost reuse share, not the fraction of model attempts that hit the cache",
        "cacheAttemptHitRateSemantics": "debate model responses reporting a nonzero cache read divided by responses with explicit cache-read telemetry",
        "note": if attempts > 0 { Some("Debate cache usage is available; detailed agent cache-surface/continuity diagnostics are not emitted by debate research yet.") } else { None::<&str> },
    })
}

fn u64_value(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|value| value.parse::<u64>().ok()))
    })
}

fn tool_result_efficiency(value: &Value) -> (u64, u64) {
    match value {
        Value::Array(values) => values.iter().fold((0u64, 0u64), |acc, value| {
            let current = tool_result_efficiency(value);
            (
                acc.0.saturating_add(current.0),
                acc.1.saturating_add(current.1),
            )
        }),
        Value::Object(object) => {
            let duplicates =
                u64::from(object.get("duplicate").and_then(Value::as_bool) == Some(true));
            let avoided = u64_value(object.get("duplicateReadBytesAvoided")).unwrap_or(0);
            (duplicates, avoided)
        }
        _ => (0, 0),
    }
}

fn hash_changed(previous: &mut Option<String>, current: Option<&str>) -> bool {
    let Some(current) = current else {
        return false;
    };
    let changed = previous
        .as_deref()
        .is_some_and(|previous| previous != current);
    *previous = Some(current.to_owned());
    changed
}

fn numeric_changed(previous: &mut Option<u64>, current: Option<u64>) -> bool {
    let Some(current) = current else {
        return false;
    };
    let changed = previous.is_some_and(|previous| previous != current);
    *previous = Some(current);
    changed
}

fn foundation(args: &[String]) -> Result<()> {
    let workspace = std::env::current_dir()?.canonicalize()?;
    let settings = ProjectSettingsStore::new(&workspace)?;
    let memory = crate::memory::MemoryStore::default();
    let sub = args.first().map(String::as_str).unwrap_or("status");
    match sub {
        "status" => {
            let mut status = memory.status()?;
            status["enabled"] = json!(settings.load()?.foundation_memory.enabled);
            status["project"] = json!(settings.project_identity()?);
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        "on" | "off" => {
            if args.len() > 1 {
                bail!(
                    "Native memory uses Yeet's API. Select a model with `yeet memory model provider/model` (full re-index)."
                );
            }
            settings.save_foundation_memory(sub == "on", None)?;
            println!("{}", if sub == "on" { "enabled" } else { "disabled" });
        }
        "model" if args.len() == 1 => {
            println!(
                "{}",
                memory.status()?["embeddingModel"]
                    .as_str()
                    .unwrap_or("automatic until first write")
            );
        }
        "export-foundation" => {
            let path = args
                .get(1)
                .ok_or_else(|| anyhow!("Usage: yeet memory export-foundation <export.jsonl>"))?;
            let result = memory
                .export_foundation(std::path::Path::new(path), &settings.project_identity()?)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        "model" | "reindex" | "models" | "import-foundation" | "recall" => {
            let bridge = BridgeClient::start()?;
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let result = (|| -> Result<Value> {
                match sub {
                    "models" => Ok(json!(bridge.embedding_models(&cancel)?)),
                    "recall" => {
                        let query = args
                            .get(1)
                            .ok_or_else(|| anyhow!("Usage: yeet memory recall <query>"))?;
                        memory.call(
                            "memory_recall",
                            &settings.project_identity()?,
                            &json!({"query":query}),
                            &bridge,
                            &cancel,
                        )
                    }
                    "import-foundation" => {
                        let path = args.get(1).ok_or_else(|| {
                            anyhow!("Usage: yeet memory import-foundation <export.jsonl>")
                        })?;
                        memory.import_foundation(
                            std::path::Path::new(path),
                            &settings.project_identity()?,
                            &bridge,
                            &cancel,
                        )
                    }
                    _ => memory.reindex(args.get(1).map(String::as_str), &bridge, &cancel),
                }
            })();
            bridge.shutdown();
            println!("{}", serde_json::to_string_pretty(&result?)?);
        }
        _ => bail!(
            "Usage: yeet memory [status|on|off|recall <query>|models|model [provider/model]|reindex [provider/model]|import-foundation <export.jsonl>|export-foundation <export.jsonl>]"
        ),
    }
    Ok(())
}

fn doctor() -> Result<()> {
    let config = ConfigStore::default();
    config.ensure()?;
    let runtime = runtime_directory()?;
    let node = node_executable()?;
    let bridge = BridgeClient::start()?;
    let providers = bridge.list_providers()?;
    bridge.shutdown();
    println!("yeet {VERSION}");
    println!("node: {}", node.display());
    println!("runtime: {}", runtime.display());
    println!("config: {}", config.directory.display());
    println!("providers: {}", providers.join(", "));
    Ok(())
}

fn model(args: &[String]) -> Result<()> {
    let config = ConfigStore::default();
    match args.first().map(String::as_str).unwrap_or("get") {
        "get" => {
            println!("{}", config.model()?.unwrap_or_else(|| "<not set>".into()));
            Ok(())
        }
        "set" => {
            let value = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("Usage: yeet model set provider/model"))?;
            println!("{}", config.set_model(value)?);
            Ok(())
        }
        "context" => model_context(&config, &args[1..]),
        _ => bail!("Usage: yeet model [get|set provider/model|context ...]"),
    }
}

fn model_context(config: &ConfigStore, args: &[String]) -> Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("get");
    match sub {
        "get" => {
            let model = if let Some(value) = args.get(1) {
                validate_model_id(value)?
            } else {
                config
                    .model()?
                    .ok_or_else(|| anyhow::anyhow!("No model is configured"))?
            };
            match config.context_length_override(&model)? {
                Some(length) => println!("{model}\t{length}\tmanual"),
                None => match config.context_length(&model)? {
                    Some(length) => println!("{model}\t{length}\tauto"),
                    None => println!("{model}\tunknown\tauto"),
                },
            }
            Ok(())
        }
        "set" => {
            let (model, value) = match (args.get(1), args.get(2)) {
                (Some(first), Some(second)) => (validate_model_id(first)?, second.as_str()),
                (Some(value), None) => (
                    config
                        .model()?
                        .ok_or_else(|| anyhow::anyhow!("No model is configured"))?,
                    value.as_str(),
                ),
                _ => bail!("Usage: yeet model context set [provider/model] length"),
            };
            let length = parse_context_length(value)?;
            config.set_context_length_override(&model, Some(length))?;
            println!("{model}\t{length}\tmanual");
            Ok(())
        }
        "auto" | "reset" => {
            let model = if let Some(value) = args.get(1) {
                validate_model_id(value)?
            } else {
                config
                    .model()?
                    .ok_or_else(|| anyhow::anyhow!("No model is configured"))?
            };
            config.set_context_length_override(&model, None)?;
            println!("{model}\tauto");
            Ok(())
        }
        _ => bail!(
            "Usage: yeet model context [get [provider/model]|set [provider/model] length|auto [provider/model]]"
        ),
    }
}

fn run_model(args: &[String]) -> Result<()> {
    let config = ConfigStore::default();
    let mut override_model = None;
    let mut prompt = Vec::new();
    let mut image_paths = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--model" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Usage: yeet run [--model provider/model] [--image path] [prompt]"
                    )
                })?;
                override_model = Some(validate_model_id(value)?);
                index += 2;
            }
            "--image" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| anyhow::anyhow!("--image requires a path"))?;
                image_paths.push(value.clone());
                index += 2;
            }
            _ => {
                prompt.push(args[index].clone());
                index += 1;
            }
        }
    }
    let model = override_model.or(config.model()?).ok_or_else(|| {
        anyhow::anyhow!("No model is configured. Run: yeet model set provider/model")
    })?;
    let prompt = if prompt.is_empty() {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        text.trim().to_owned()
    } else {
        prompt.join(" ")
    };
    if prompt.is_empty() {
        bail!("Usage: yeet run [--model provider/model] [--image path] prompt");
    }
    let images = image_paths
        .iter()
        .map(|path| ImageAttachment::from_file(std::path::Path::new(path)))
        .collect::<Result<Vec<_>>>()?;
    let bridge = BridgeClient::start()?;
    let request = CallRequest::simple(model, vec![Message::user_with_images(prompt, images)]);
    let mut stream = bridge.stream(&request)?;
    while let Some(event) = stream.recv()? {
        match event {
            StreamEvent::TextDelta(text) => {
                print!("{text}");
                std::io::stdout().flush()?;
            }
            StreamEvent::ToolCall { tool_call, .. } => println!("\n[tool] {}", tool_call.name),
            _ => {}
        }
    }
    println!();
    bridge.shutdown();
    Ok(())
}

fn usage_command(args: &[String]) -> Result<()> {
    if args.len() > 1 {
        bail!("Usage: yeet usage [codex|claude|gemini|provider]");
    }
    let providers = if let Some(provider) = args.first() {
        vec![match provider.as_str() {
            "codex" => "codex-cli".to_owned(),
            "claude" => "claude".to_owned(),
            "google" => "gemini-web".to_owned(),
            other => other.to_owned(),
        }]
    } else {
        vec![
            "openai".to_owned(),
            "codex-cli".to_owned(),
            "anthropic".to_owned(),
            "claude".to_owned(),
            "gemini".to_owned(),
            "gemini-web".to_owned(),
        ]
    };

    let bridge = BridgeClient::start()?;
    let result = (|| -> Result<()> {
        for provider in providers {
            let usage = bridge.provider_usage(&provider)?;
            let plan = usage
                .plan
                .as_deref()
                .map(|plan| format!(" · plan {plan}"))
                .unwrap_or_default();
            println!("{} · {}{plan}", usage.provider, usage.source);
            if usage.windows.is_empty() {
                println!(
                    "  {}",
                    usage.message.as_deref().unwrap_or("usage unavailable")
                );
                continue;
            }
            for window in &usage.windows {
                let reset = window
                    .resets_at
                    .as_deref()
                    .map(|reset| format!(" · resets {reset}"))
                    .unwrap_or_default();
                println!(
                    "  {}: {}% left · {}% used{reset}",
                    window.label, window.remaining_percent, window.used_percent
                );
            }
        }
        Ok(())
    })();
    bridge.shutdown();
    result
}

fn auth(args: &[String]) -> Result<()> {
    let sub = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("Usage: yeet auth [status|set-key|login|logout] ..."))?;
    let bridge = BridgeClient::start()?;
    let result = (|| -> Result<()> {
        match sub {
            "status" => {
                let providers = if let Some(provider) = args.get(1) {
                    vec![provider.clone()]
                } else {
                    bridge.list_providers()?
                };
                for provider in providers {
                    let status = bridge.auth_status(&provider)?;
                    println!(
                        "{}: {} via {}",
                        provider,
                        if status.authenticated {
                            "authenticated"
                        } else {
                            "not authenticated"
                        },
                        status.method
                    );
                }
                Ok(())
            }
            "set-key" => {
                let provider = args.get(1).ok_or_else(|| {
                    anyhow::anyhow!("Usage: printf key | yeet auth set-key provider")
                })?;
                let mut key = String::new();
                std::io::stdin().read_to_string(&mut key)?;
                let key = key.trim();
                if key.is_empty() {
                    bail!("Usage: printf key | yeet auth set-key provider");
                }
                let status = bridge.set_api_key(provider, key)?;
                println!("{}: authenticated", status.provider);
                Ok(())
            }
            "login" => {
                let provider = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet auth login provider"))?;
                let options = if provider == "gemini" || provider == "gemini-web" {
                    let env = std::env::vars().collect::<HashMap<_, _>>();
                    Some(
                        json!({"clientId":env.get("GEMINI_OAUTH_CLIENT_ID").or_else(||env.get("GOOGLE_CLIENT_ID")),"clientSecret":env.get("GEMINI_OAUTH_CLIENT_SECRET").or_else(||env.get("GOOGLE_CLIENT_SECRET")),"projectId":env.get("GEMINI_PROJECT_ID").or_else(||env.get("GOOGLE_CLOUD_PROJECT"))}),
                    )
                } else {
                    None
                };
                let status = bridge.login_browser(provider, options)?;
                println!("{}: authenticated via {}", status.provider, status.method);
                Ok(())
            }
            "logout" => {
                let provider = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet auth logout provider"))?;
                let status = bridge.logout(provider)?;
                println!("{}: logged out", status.provider);
                Ok(())
            }
            _ => bail!("Usage: yeet auth [status|set-key|login|logout] ..."),
        }
    })();
    bridge.shutdown();
    result
}

fn provider(args: &[String]) -> Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("list");
    let bridge = BridgeClient::start()?;
    let result = (|| -> Result<()> {
        match sub {
            "list" => {
                let values = bridge.list_provider_configurations()?;
                if values.is_empty() {
                    println!("No custom API endpoints are configured.");
                }
                for provider in values {
                    println!(
                        "{}\t{}{}",
                        provider.id,
                        provider.base_url,
                        if provider.require_api_key == Some(true) {
                            "\trequires-api-key"
                        } else {
                            ""
                        }
                    );
                }
                Ok(())
            }
            "add" | "set" => {
                if args.len() < 3 {
                    bail!(
                        "Usage: yeet provider add id base-url [--require-api-key] [--header Name=Value]"
                    );
                }
                let mut require = false;
                let mut headers = HashMap::new();
                let mut i = 3;
                while i < args.len() {
                    match args[i].as_str() {
                        "--require-api-key" => {
                            require = true;
                            i += 1
                        }
                        "--header" => {
                            let pair = args
                                .get(i + 1)
                                .ok_or_else(|| anyhow::anyhow!("--header requires Name=Value"))?;
                            let (k, v) = pair
                                .split_once('=')
                                .ok_or_else(|| anyhow::anyhow!("--header requires Name=Value"))?;
                            headers.insert(k.into(), v.into());
                            i += 2
                        }
                        other => bail!("Unknown provider option: {other}"),
                    }
                }
                let saved = bridge.save_provider_configuration(&OpenAiCompatibleProvider {
                    kind: "openai-compatible".into(),
                    id: args[1].clone(),
                    base_url: args[2].clone(),
                    api_key: None,
                    headers: (!headers.is_empty()).then_some(headers),
                    require_api_key: Some(require),
                })?;
                println!("{}\t{}", saved.id, saved.base_url);
                Ok(())
            }
            "remove" | "delete" => {
                if args.len() != 2 {
                    bail!("Usage: yeet provider remove id");
                }
                println!(
                    "{}",
                    if bridge.remove_provider_configuration(&args[1])? {
                        "removed"
                    } else {
                        "not found"
                    }
                );
                Ok(())
            }
            _ => bail!("Usage: yeet provider [list|add id base-url|remove id]"),
        }
    })();
    bridge.shutdown();
    result
}

fn skill(args: &[String]) -> Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("list");
    let bridge = BridgeClient::start()?;
    let result = (|| -> Result<()> {
        match sub {
            "list" => {
                for skill in bridge.list_skills()? {
                    println!("{}\t{}", skill.name, skill.description);
                }
                Ok(())
            }
            "show" => {
                let name = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet skill show name"))?;
                println!("{}", bridge.load_skill(name)?.instructions);
                Ok(())
            }
            "read" => {
                if args.len() < 3 {
                    bail!("Usage: yeet skill read name path");
                }
                println!("{}", bridge.read_skill_file(&args[1], &args[2])?);
                Ok(())
            }
            "validate" => {
                let source = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet skill validate path"))?;
                for skill in bridge.validate_skills(source)? {
                    println!("{}\t{}", skill.name, skill.description);
                }
                Ok(())
            }
            "install" => {
                let source = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet skill install path|git-url|zip"))?;
                for name in bridge.install_skills(source)? {
                    println!("installed\t{name}");
                }
                Ok(())
            }
            "remove" | "uninstall" => {
                let name = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet skill remove name"))?;
                println!(
                    "{}",
                    if bridge.remove_skill(name)? {
                        "removed"
                    } else {
                        "not found"
                    }
                );
                Ok(())
            }
            _ => bail!("Usage: yeet skill [list|show|read|validate|install|remove] ..."),
        }
    })();
    bridge.shutdown();
    result
}

fn mcp(args: &[String]) -> Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("list");
    let bridge = BridgeClient::start()?;
    let result = (|| -> Result<()> {
        match sub {
            "list" => {
                for server in bridge.list_mcp_servers()? {
                    println!(
                        "{}\t{}\t{}",
                        server.name,
                        server.transport,
                        if server.connected {
                            "connected"
                        } else {
                            "idle"
                        }
                    );
                }
                Ok(())
            }
            "add-stdio" => {
                if args.len() < 3 {
                    bail!("Usage: yeet mcp add-stdio name command [args...]");
                }
                let server = McpServerConfiguration {
                    name: args[1].clone(),
                    transport: "stdio".into(),
                    command: Some(args[2].clone()),
                    args: Some(args[3..].to_vec()),
                    env: None,
                    cwd: None,
                    url: None,
                    headers: None,
                };
                println!("{}", bridge.set_mcp_server(&server)?.name);
                Ok(())
            }
            "add-http" => {
                if args.len() < 3 {
                    bail!("Usage: yeet mcp add-http name url [--header Name=Value]...");
                }
                let mut headers = HashMap::new();
                let mut index = 3usize;
                while index < args.len() {
                    if args[index] != "--header" || index + 1 >= args.len() {
                        bail!("Usage: yeet mcp add-http name url [--header Name=Value]...");
                    }
                    let (name, value) = args[index + 1]
                        .split_once('=')
                        .ok_or_else(|| anyhow::anyhow!("MCP header must use Name=Value"))?;
                    let name = name.trim();
                    if name.is_empty() {
                        bail!("MCP header name cannot be empty");
                    }
                    headers.insert(name.to_owned(), value.to_owned());
                    index += 2;
                }
                let server = McpServerConfiguration {
                    name: args[1].clone(),
                    transport: "http".into(),
                    command: None,
                    args: None,
                    env: None,
                    cwd: None,
                    url: Some(args[2].clone()),
                    headers: (!headers.is_empty()).then_some(headers),
                };
                println!("{}", bridge.set_mcp_server(&server)?.name);
                Ok(())
            }
            "remove" => {
                let name = args
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: yeet mcp remove name"))?;
                println!(
                    "{}",
                    if bridge.remove_mcp_server(name)? {
                        "removed"
                    } else {
                        "not found"
                    }
                );
                Ok(())
            }
            "tools" => {
                for tool in bridge.list_mcp_tools(args.get(1).map(String::as_str))? {
                    println!(
                        "{}\t{}",
                        tool.qualified_name,
                        tool.description.unwrap_or_default()
                    );
                }
                Ok(())
            }
            "call" => {
                let qualified = args.get(1).ok_or_else(|| {
                    anyhow::anyhow!("Usage: yeet mcp call server/tool [json-arguments]")
                })?;
                let (server, tool) = qualified
                    .split_once('/')
                    .ok_or_else(|| anyhow::anyhow!("MCP tool must be server/tool"))?;
                let arguments: Map<String, Value> =
                    serde_json::from_str(args.get(2).map(String::as_str).unwrap_or("{}"))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&bridge.call_mcp_tool(server, tool, &arguments)?)?
                );
                Ok(())
            }
            "resources" => {
                for resource in bridge.list_mcp_resources(args.get(1).map(String::as_str))? {
                    println!("{}\t{}\t{}", resource.server, resource.uri, resource.name);
                }
                Ok(())
            }
            "resource" => {
                if args.len() < 3 {
                    bail!("Usage: yeet mcp resource server uri");
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&bridge.read_mcp_resource(&args[1], &args[2])?)?
                );
                Ok(())
            }
            "prompts" => {
                for prompt in bridge.list_mcp_prompts(args.get(1).map(String::as_str))? {
                    println!(
                        "{}\t{}",
                        prompt.qualified_name,
                        prompt.description.unwrap_or_default()
                    );
                }
                Ok(())
            }
            "prompt" => {
                if args.len() < 3 {
                    bail!("Usage: yeet mcp prompt server name [json-string-arguments]");
                }
                let arguments: Map<String, Value> =
                    serde_json::from_str(args.get(3).map(String::as_str).unwrap_or("{}"))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &bridge.get_mcp_prompt(&args[1], &args[2], &arguments)?
                    )?
                );
                Ok(())
            }
            _ => bail!(
                "Usage: yeet mcp [list|add-stdio|add-http|remove|tools|call|resources|resource|prompts|prompt] ..."
            ),
        }
    })();
    bridge.shutdown();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn cache_event(
        run_id: &str,
        tool_round: u64,
        status: &str,
        input: u64,
        cached: Option<u64>,
        epoch: u64,
        reason: &str,
        breakpoint: &str,
    ) -> String {
        let ordinary = cached.map(|cached| input.saturating_sub(cached));
        json!({
            "type": "agent-model-attempt-finished",
            "runId": run_id,
            "payload": {
                "cacheDiagnostics": {
                    "cacheStatus": status,
                    "inputTokens": input,
                    "cachedInputTokens": cached,
                    "cacheWriteInputTokens": Value::Null,
                    "ordinaryInputTokens": ordinary,
                    "toolRound": tool_round.to_string(),
                    "expectedCacheReuses": "1",
                    "cacheEpoch": epoch,
                    "cacheEpochReason": reason,
                    "historyPrefixContinues": if reason == "window-start" { Value::Null } else { json!(true) },
                    "stablePrefixHash": "stable-prefix",
                    "toolSchemaHash": "tool-schema",
                    "cacheSurfaceHash": "cache-surface",
                    "breakpointPlanHash": breakpoint,
                    "promptCacheKeyHash": "prompt-key",
                    "contextKey": "context-key",
                    "contextWindowId": "window-1",
                    "estimatedCacheSurfaceTokens": 2048,
                }
            }
        })
        .to_string()
    }

    fn with_provider_cache_telemetry(
        event: String,
        cost_equivalent_input_tokens: u64,
        provider_miss: Option<(&str, u64, u64)>,
    ) -> String {
        let mut record: Value = serde_json::from_str(&event).unwrap();
        let diagnostics = &mut record["payload"]["cacheDiagnostics"];
        diagnostics["costEquivalentInputTokens"] = json!(cost_equivalent_input_tokens);
        if let Some((reason, missed_tokens, reusable_tokens)) = provider_miss {
            diagnostics["providerCacheDiagnosticType"] = json!("cache_miss");
            diagnostics["providerCacheMissReason"] = json!(reason);
            diagnostics["providerCacheMissedTokens"] = json!(missed_tokens);
            diagnostics["providerComparisonReusableTokens"] = json!(reusable_tokens);
            diagnostics["cacheMissAttribution"] = json!("provider-reported");
        }
        record.to_string()
    }

    fn tool_call_event(run_id: &str, name: &str, arguments: Value) -> String {
        json!({
            "type": "agent-tool-call",
            "runId": run_id,
            "payload": {"call": {"name": name, "arguments": arguments}}
        })
        .to_string()
    }

    fn tool_finished_event(run_id: &str, name: &str, result: Value) -> String {
        json!({
            "type": "agent-tool-finished",
            "runId": run_id,
            "payload": {
                "call": {"name": name, "arguments": {}},
                "result": result.to_string()
            }
        })
        .to_string()
    }

    #[test]
    fn cache_summary_reports_round_and_duplicate_tool_efficiency() {
        let mut root: Value = serde_json::from_str(&cache_event(
            "run-a",
            0,
            "miss",
            100,
            Some(0),
            1,
            "window-start",
            "bp-a",
        ))
        .unwrap();
        root["payload"]["cacheDiagnostics"]["modelVisibleToolResultChars"] = json!(0);
        root["payload"]["cacheDiagnostics"]["costEquivalentInputTokens"] = json!(100);
        let mut round_one: Value = serde_json::from_str(&cache_event(
            "run-a",
            1,
            "hit",
            160,
            Some(96),
            1,
            "stable",
            "bp-b",
        ))
        .unwrap();
        round_one["payload"]["cacheDiagnostics"]["modelVisibleToolResultChars"] = json!(1200);
        round_one["payload"]["cacheDiagnostics"]["costEquivalentInputTokens"] = json!(74);

        let content = [
            tool_call_event("run-a", "read_file", json!({"path":"src/lib.rs"})),
            tool_call_event("run-a", "read_file", json!({"path":"src/lib.rs"})),
            tool_call_event("run-a", "run_shell", json!({"command":"cargo check"})),
            tool_finished_event(
                "run-a",
                "read_file",
                json!({
                    "duplicate": true,
                    "contentAlreadyReturned": true,
                    "duplicateReadBytesAvoided": 4096
                }),
            ),
            root.to_string(),
            round_one.to_string(),
        ]
        .join("\n");

        let report = summarize_cache_events(&content);
        assert_eq!(report["turns"], 1);
        assert_eq!(report["attempts"], 2);
        assert_eq!(report["executedToolRounds"], 1);
        assert_eq!(report["totalToolCalls"], 3);
        assert_eq!(report["exactRepeatedToolCalls"], 1);
        assert_eq!(report["duplicateToolResults"], 1);
        assert_eq!(report["duplicateReadResults"], 1);
        assert_eq!(report["duplicateReadBytesAvoided"], 4096);
        assert_eq!(report["toolCallCounts"]["read_file"], 2);
        assert_eq!(report["toolCallCounts"]["run_shell"], 1);
        assert_eq!(
            report["maximumNewModelVisibleToolResultCharsPerRound"],
            1200
        );
        assert!((report["modelAttemptsPerTurn"].as_f64().unwrap() - 2.0).abs() < 1e-12);
        assert!((report["toolRoundsPerTurn"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        assert!((report["toolCallsPerRound"].as_f64().unwrap() - 3.0).abs() < 1e-12);
        assert!(
            (report["costEquivalentInputTokensPerTurn"].as_f64().unwrap() - 174.0).abs() < 1e-12
        );
    }

    #[test]
    fn cache_summary_separates_unreported_disabled_and_retries() {
        let content = [
            cache_event("run-a", 0, "miss", 100, Some(0), 1, "window-start", "bp-a"),
            cache_event("run-a", 1, "unreported", 200, None, 1, "stable", "bp-b"),
            cache_event("run-a", 1, "hit", 300, Some(240), 1, "stable", "bp-c"),
            cache_event(
                "run-b",
                0,
                "disabled",
                400,
                Some(0),
                1,
                "window-start",
                "bp-d",
            ),
        ]
        .join("\n");

        let report = summarize_cache_events(&content);
        assert_eq!(report["attempts"], 4);
        assert_eq!(report["turns"], 2);
        assert_eq!(report["toolRounds"], 3);
        assert_eq!(report["rootModelAttempts"], 2);
        assert_eq!(report["repeatedToolRoundAttempts"], 1);
        assert_eq!(report["measuredAttempts"], 2);
        assert_eq!(report["unreportedAttempts"], 1);
        assert_eq!(report["disabledAttempts"], 1);
        assert_eq!(report["hitAttempts"], 1);
        assert_eq!(report["missAttempts"], 1);
        assert_eq!(report["inputTokens"], 1000);
        assert_eq!(report["cacheMeasuredInputTokens"], 400);
        assert_eq!(report["cacheUnreportedInputTokens"], 200);
        assert_eq!(report["cacheDisabledInputTokens"], 400);
        assert_eq!(report["cachedInputTokens"], 240);
        assert_eq!(report["intraTurnBreakpointPlanChanges"], 2);
        assert_eq!(report["intraTurnCacheSurfaceChanges"], 0);
        assert_eq!(report["stableSurfaceMeasuredAttempts"], 1);
        assert_eq!(report["stableSurfaceHitAttempts"], 1);
        assert!((report["cacheHitRate"].as_f64().unwrap() - 0.6).abs() < 1e-12);
        assert!((report["rootCacheHitRate"].as_f64().unwrap() - 0.0).abs() < 1e-12);
        assert!((report["followupCacheHitRate"].as_f64().unwrap() - 0.8).abs() < 1e-12);
        assert!((report["cacheMeasurementCoverageRate"].as_f64().unwrap() - 0.4).abs() < 1e-12);
        assert!((report["cacheAttemptHitRate"].as_f64().unwrap() - 0.5).abs() < 1e-12);
        assert!((report["stableSurfaceAttemptHitRate"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        assert_eq!(report["stablePreviousRequestInputTokens"], 200);
        assert_eq!(report["stablePreviousRequestCachedInputTokens"], 200);
        assert!((report["stablePreviousRequestReuseRate"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        assert_eq!(report["stablePreviousRequestCurrentInputTokens"], 300);
        assert_eq!(report["stableNovelInputTokens"], 100);
        assert!(
            (report["stablePreviousRequestShareCeilingRate"]
                .as_f64()
                .unwrap()
                - (2.0 / 3.0))
                .abs()
                < 1e-12
        );
        assert!((report["stableNovelInputRate"].as_f64().unwrap() - (1.0 / 3.0)).abs() < 1e-12);
    }

    #[test]
    fn cache_summary_keeps_breakpoint_rotation_separate_from_stable_surface() {
        let content = [
            cache_event("run-a", 0, "miss", 100, Some(0), 1, "window-start", "bp-a"),
            cache_event("run-a", 1, "miss", 200, Some(0), 1, "stable", "bp-b"),
            cache_event("run-a", 2, "hit", 300, Some(240), 1, "stable", "bp-c"),
        ]
        .join("\n");

        let report = summarize_cache_events(&content);
        assert_eq!(report["intraTurnBreakpointPlanChanges"], 2);
        assert_eq!(report["intraTurnStablePrefixChanges"], 0);
        assert_eq!(report["intraTurnToolSchemaChanges"], 0);
        assert_eq!(report["intraTurnCacheSurfaceChanges"], 0);
        assert_eq!(report["intraTurnPromptCacheKeyChanges"], 0);
        assert_eq!(report["intraTurnContextKeyChanges"], 0);
        assert_eq!(report["intraTurnContextWindowChanges"], 0);
        assert_eq!(report["intraTurnCacheEpochChanges"], 0);
        assert_eq!(report["historyPrefixRewrites"], 0);
        assert_eq!(report["stableSurfaceMeasuredAttempts"], 2);
        assert_eq!(report["stableSurfaceHitAttempts"], 1);
        assert_eq!(report["stableSurfaceMissAttempts"], 1);
        assert_eq!(report["cacheResetMissAttempts"], 1);
        assert_eq!(report["missAttemptsByCacheEpochReason"]["stable"], 1);
        assert_eq!(report["missAttemptsByCacheEpochReason"]["window-start"], 1);
        assert!((report["stableSurfaceCacheHitRate"].as_f64().unwrap() - 0.48).abs() < 1e-12);
        assert!((report["stableSurfaceAttemptHitRate"].as_f64().unwrap() - 0.5).abs() < 1e-12);
        assert_eq!(report["stablePreviousRequestInputTokens"], 300);
        assert_eq!(report["stablePreviousRequestCachedInputTokens"], 200);
        assert!(
            (report["stablePreviousRequestReuseRate"].as_f64().unwrap() - (2.0 / 3.0)).abs()
                < 1e-12
        );
        assert_eq!(report["stablePreviousRequestCurrentInputTokens"], 500);
        assert_eq!(report["stableNovelInputTokens"], 200);
        assert!(
            (report["stablePreviousRequestShareCeilingRate"]
                .as_f64()
                .unwrap()
                - 0.6)
                .abs()
                < 1e-12
        );
        assert!((report["stableNovelInputRate"].as_f64().unwrap() - 0.4).abs() < 1e-12);
    }

    #[test]
    fn previous_request_reuse_excludes_cache_reset_attempts() {
        let content = [
            cache_event("run-a", 0, "miss", 100, Some(0), 1, "window-start", "bp-a"),
            cache_event(
                "run-a",
                1,
                "miss",
                150,
                Some(0),
                2,
                "tool-envelope-change",
                "bp-b",
            ),
            cache_event("run-a", 2, "hit", 300, Some(128), 2, "stable", "bp-c"),
        ]
        .join("\n");

        let report = summarize_cache_events(&content);
        assert_eq!(report["stablePreviousRequestInputTokens"], 150);
        assert_eq!(report["stablePreviousRequestCachedInputTokens"], 128);
        assert!(
            (report["stablePreviousRequestReuseRate"].as_f64().unwrap() - (128.0 / 150.0)).abs()
                < 1e-12
        );
        assert_eq!(report["stablePreviousRequestCurrentInputTokens"], 300);
        assert_eq!(report["stableNovelInputTokens"], 150);
        assert!(
            (report["stablePreviousRequestShareCeilingRate"]
                .as_f64()
                .unwrap()
                - 0.5)
                .abs()
                < 1e-12
        );
        assert!((report["stableNovelInputRate"].as_f64().unwrap() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn cache_economics_fixture_distinguishes_novel_tail_from_cache_failures() {
        let content = [
            cache_event("run-a", 0, "miss", 100, Some(0), 1, "window-start", "bp-a"),
            cache_event("run-a", 1, "hit", 200, Some(100), 1, "stable", "bp-b"),
            cache_event("run-a", 2, "hit", 400, Some(200), 1, "stable", "bp-c"),
            cache_event(
                "run-a",
                3,
                "miss",
                300,
                Some(0),
                2,
                "tool-envelope-change",
                "bp-d",
            ),
            cache_event("run-a", 4, "hit", 600, Some(256), 2, "stable", "bp-e"),
        ]
        .join("\n");

        let report = summarize_cache_events(&content);
        assert_eq!(report["attempts"], 5);
        assert_eq!(report["hitAttempts"], 3);
        assert_eq!(report["missAttempts"], 2);
        assert_eq!(report["cacheResetMissAttempts"], 2);
        assert_eq!(report["stableSurfaceMeasuredAttempts"], 3);
        assert_eq!(report["stableSurfaceHitAttempts"], 3);
        assert_eq!(report["stablePreviousRequestCurrentInputTokens"], 1200);
        assert_eq!(report["stablePreviousRequestInputTokens"], 600);
        assert_eq!(report["stableNovelInputTokens"], 600);
        assert!((report["cacheHitRate"].as_f64().unwrap() - (556.0 / 1600.0)).abs() < 1e-12);
        assert!((report["cacheAttemptHitRate"].as_f64().unwrap() - 0.6).abs() < 1e-12);
        assert!((report["stableSurfaceAttemptHitRate"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        assert!(
            (report["stablePreviousRequestReuseRate"].as_f64().unwrap() - (556.0 / 600.0)).abs()
                < 1e-12
        );
        assert!(
            (report["stablePreviousRequestShareCeilingRate"]
                .as_f64()
                .unwrap()
                - 0.5)
                .abs()
                < 1e-12
        );
        assert!((report["stableNovelInputRate"].as_f64().unwrap() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn cache_economics_replay_attributes_stable_provider_miss_without_reclassifying_it() {
        let content = [
            with_provider_cache_telemetry(
                cache_event(
                    "run-a",
                    0,
                    "miss",
                    1_000,
                    Some(0),
                    1,
                    "window-start",
                    "bp-a",
                ),
                1_000,
                None,
            ),
            with_provider_cache_telemetry(
                cache_event("run-a", 1, "hit", 1_200, Some(800), 1, "stable", "bp-b"),
                600,
                None,
            ),
            with_provider_cache_telemetry(
                cache_event("run-a", 2, "miss", 1_500, Some(0), 1, "stable", "bp-c"),
                1_500,
                Some(("input_changed", 700, 800)),
            ),
        ]
        .join("\n");

        let report = summarize_cache_events(&content);
        assert_eq!(report["attempts"], 3);
        assert_eq!(report["hitAttempts"], 1);
        assert_eq!(report["missAttempts"], 2);
        assert_eq!(report["stableSurfaceMissAttempts"], 1);
        assert_eq!(report["stableProviderReportedMissAttempts"], 1);
        assert_eq!(report["stableProviderUnattributedMissAttempts"], 0);
        assert_eq!(report["providerCacheDiagnosticAttempts"], 1);
        assert_eq!(report["providerCacheMissAttempts"], 1);
        assert_eq!(report["providerCacheMissReasons"]["input_changed"], 1);
        assert_eq!(report["providerCacheMissedTokens"], 700);
        assert_eq!(report["providerComparisonReusableTokens"], 800);
        assert_eq!(report["costEquivalentInputTokens"], 3_100);
        assert!(
            (report["costEquivalentInputRate"].as_f64().unwrap() - (3_100.0 / 3_700.0)).abs()
                < 1e-12
        );
        assert_eq!(report["stablePreviousRequestInputTokens"], 2_200);
        assert_eq!(report["stablePreviousRequestCachedInputTokens"], 800);
        assert_eq!(report["stablePreviousRequestCurrentInputTokens"], 2_700);
        assert_eq!(report["stableNovelInputTokens"], 500);
        assert!(
            (report["stablePreviousRequestReuseRate"].as_f64().unwrap() - (800.0 / 2_200.0)).abs()
                < 1e-12
        );
        assert!(
            (report["stablePreviousRequestShareCeilingRate"]
                .as_f64()
                .unwrap()
                - (2_200.0 / 2_700.0))
                .abs()
                < 1e-12
        );
        assert!((report["cacheHitRate"].as_f64().unwrap() - (800.0 / 3_700.0)).abs() < 1e-12);
    }

    #[test]
    fn debate_cache_summary_reads_response_usage() {
        let content = [
            json!({
                "type":"debate-research","runId":"run-a",
                "payload":{"event":"response","stage":0,"pro":true,"detail":{"round":0,"response":{"usage":{
                    "inputTokens":100,"cacheMeasuredInputTokens":100,"cachedInputTokens":0,
                    "cacheWriteInputTokens":0,"costEquivalentInputTokens":100
                }}}}
            }).to_string(),
            json!({
                "type":"debate-research","runId":"run-a",
                "payload":{"event":"response","stage":0,"pro":true,"detail":{"round":1,"response":{"usage":{
                    "inputTokens":200,"cacheMeasuredInputTokens":200,"cachedInputTokens":80,
                    "cacheWriteInputTokens":0,"costEquivalentInputTokens":128
                }}}}
            }).to_string(),
        ].join("\n");
        let report = summarize_debate_cache_events(&content);
        assert_eq!(report["turns"], 1);
        assert_eq!(report["attempts"], 2);
        assert_eq!(report["measuredAttempts"], 2);
        assert_eq!(report["hitAttempts"], 1);
        assert_eq!(report["missAttempts"], 1);
        assert_eq!(report["inputTokens"], 300);
        assert_eq!(report["cachedInputTokens"], 80);
        assert_eq!(report["rootMissAttempts"], 1);
        assert_eq!(report["followupHitAttempts"], 1);
        assert_eq!(report["telemetrySource"], "debates/events.jsonl");
        assert!((report["cacheHitRate"].as_f64().unwrap() - (80.0 / 300.0)).abs() < 1e-12);
    }
}
