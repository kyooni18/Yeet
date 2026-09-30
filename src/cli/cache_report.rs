//! Cache telemetry aggregation and the `yeet cache` report command.

use super::*;

pub(super) fn cache(args: &[String]) -> Result<()> {
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

pub(super) fn summarize_cache_events(content: &str) -> Value {
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
    let mut turn_boundary_pruned_request_only_messages = 0u64;
    let mut turn_boundary_pruned_request_only_chars = 0u64;
    let mut turns_with_pruned_request_only_history = 0u64;

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

    let mut max_turn_cumulative_input_tokens = 0u64;
    let mut max_turn_cost_equivalent_input_tokens = 0u64;
    let mut max_turn_cumulative_request_chars = 0u64;
    let mut max_turn_rollovers = 0u64;
    let mut max_search_loaded_tool_count = 0u64;
    let mut max_window_model_calls = 0u64;
    let mut budget_stage_counts = BTreeMap::<String, u64>::new();

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

        max_turn_cumulative_input_tokens = max_turn_cumulative_input_tokens
            .max(u64_value(diagnostics.get("turnCumulativeInputTokens")).unwrap_or(0));
        max_turn_cost_equivalent_input_tokens = max_turn_cost_equivalent_input_tokens
            .max(u64_value(diagnostics.get("turnCostEquivalentInputTokens")).unwrap_or(0));
        max_turn_cumulative_request_chars = max_turn_cumulative_request_chars
            .max(u64_value(diagnostics.get("turnCumulativeRequestChars")).unwrap_or(0));
        max_turn_rollovers =
            max_turn_rollovers.max(u64_value(diagnostics.get("turnRolloverCount")).unwrap_or(0));
        max_search_loaded_tool_count = max_search_loaded_tool_count
            .max(u64_value(diagnostics.get("searchLoadedToolCount")).unwrap_or(0));
        max_window_model_calls =
            max_window_model_calls.max(u64_value(diagnostics.get("windowModelCalls")).unwrap_or(0));
        if let Some(stage) = diagnostics.get("turnBudgetStage").and_then(Value::as_str) {
            *budget_stage_counts.entry(stage.to_owned()).or_default() += 1;
        }
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
            let pruned_messages =
                u64_value(diagnostics.get("turnStartPrunedRequestOnlyMessages")).unwrap_or(0);
            turn_boundary_pruned_request_only_messages =
                turn_boundary_pruned_request_only_messages.saturating_add(pruned_messages);
            turn_boundary_pruned_request_only_chars = turn_boundary_pruned_request_only_chars
                .saturating_add(
                    u64_value(diagnostics.get("turnStartPrunedRequestOnlyChars")).unwrap_or(0),
                );
            turns_with_pruned_request_only_history += u64::from(pruned_messages > 0);
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

    put!(
        "maximumTurnCumulativeInputTokens",
        (attempts > 0).then_some(max_turn_cumulative_input_tokens)
    );
    put!(
        "maximumTurnCostEquivalentInputTokens",
        (attempts > 0).then_some(max_turn_cost_equivalent_input_tokens)
    );
    put!(
        "maximumTurnCumulativeRequestChars",
        (attempts > 0).then_some(max_turn_cumulative_request_chars)
    );
    put!(
        "maximumTurnRollovers",
        (attempts > 0).then_some(max_turn_rollovers)
    );
    put!(
        "maximumSearchLoadedToolCount",
        (attempts > 0).then_some(max_search_loaded_tool_count)
    );
    put!(
        "maximumWindowModelCalls",
        (attempts > 0).then_some(max_window_model_calls)
    );
    put!("windowLifecycleStageAttemptCounts", budget_stage_counts);
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
    put!(
        "turnBoundaryPrunedRequestOnlyMessages",
        turn_boundary_pruned_request_only_messages
    );
    put!(
        "turnBoundaryPrunedRequestOnlyChars",
        turn_boundary_pruned_request_only_chars
    );
    put!(
        "turnsWithPrunedRequestOnlyHistory",
        turns_with_pruned_request_only_history
    );
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

pub(super) fn summarize_debate_cache_events(content: &str) -> Value {
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
            u64_value(usage.get("cacheWriteInputTokens"))
                .unwrap_or(0)
                .min(input),
        );
        cost_equivalent_input_tokens = cost_equivalent_input_tokens
            .saturating_add(u64_value(usage.get("costEquivalentInputTokens")).unwrap_or(input));

        let run_id = event
            .get("runId")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let stage = event
            .pointer("/payload/stage")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let pro = event
            .pointer("/payload/pro")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        turns.insert(format!("{run_id}:{stage}:{pro}"));
        let root = event
            .pointer("/payload/detail/round")
            .and_then(Value::as_u64)
            == Some(0);

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
            followup_measured_input_tokens =
                followup_measured_input_tokens.saturating_add(measured);
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
