//! One-turn request classification and mutable execution-state initialization.

use super::*;

pub(super) struct TurnSetup<'a> {
    pub(super) model: &'a str,
    pub(super) reasoning_level: &'a str,
    pub(super) cancel: &'a AtomicBool,
    pub(super) attached_capabilities: Option<Vec<String>>,
    pub(super) goal_epoch: u64,
    pub(super) goal_progress: goal::GoalProgress,
    pub(super) current_request: Message,
    pub(super) request_input: String,
    pub(super) vision_enabled: bool,
    pub(super) web_search_enabled: bool,
    pub(super) profile: TaskProfile,
    pub(super) local_file_lookup: bool,
    pub(super) capability_discovery_requested: bool,
    pub(super) implementation_requested: bool,
    pub(super) planning_or_documentation: bool,
    pub(super) bounded_explanation: bool,
    pub(super) bounded_analysis: bool,
    pub(super) native_deferred_tools_supported: bool,
    pub(super) research_budget: ResearchBudget,
    pub(super) model_attempts: usize,
    pub(super) tool_rounds: usize,
    pub(super) malformed_repairs: usize,
    pub(super) empty_repairs: usize,
    pub(super) length_continuations: usize,
    pub(super) implementation_repairs: usize,
    pub(super) execution_evidence: turn_state::TurnExecutionEvidence,
    pub(super) consecutive_no_progress: usize,
    pub(super) progressful_inspection_rounds: usize,
    pub(super) implementation_inspection_checkpoint_used: bool,
    pub(super) retry_instruction: Option<String>,
    pub(super) final_consistency_pending: bool,
    pub(super) final_consistency_used: bool,
    pub(super) completion_gate_repairs: usize,
    pub(super) local_lookup_read_calls: usize,
    pub(super) local_lookup_read_externalized: bool,
    pub(super) local_lookup_recovery_calls: usize,
    pub(super) local_lookup_shell_calls: usize,
    pub(super) call_counts: HashMap<String, usize>,
    pub(super) workspace_generation: u64,
    pub(super) workspace_write_generation: u64,
    pub(super) tool_discovery: tool_discovery::ToolDiscovery,
    pub(super) loop_budget: LoopBudget,
    pub(super) research_stop_grace_used: bool,
    pub(super) analysis_stop_grace_used: bool,
    pub(super) runaway_detector: RunawayDetector,
    pub(super) last_provenance_checkpoint: Option<String>,
    pub(super) workspace_revision: Option<String>,
    pub(super) turn_stable_overlays: Vec<Message>,
    pub(super) turn_context_orientation: String,
    pub(super) last_context_updates: Vec<Message>,
}

impl AgentCoordinator {
    pub(super) fn prepare_turn<'a>(
        &mut self,
        request: AgentTurnRequest<'a>,
        goal_mode: &AtomicBool,
    ) -> Result<TurnSetup<'a>> {
        let AgentTurnRequest {
            input,
            images,
            model,
            reasoning_level,
            attached_capabilities,
            cancel,
            continuation,
            goal_retry_reason,
        } = request;
        if !continuation && history::prune_request_only_history(&mut self.history) > 0 {
            // Historical request-only guidance is intentionally absent from the new
            // turn's wire history. Start a fresh local continuity epoch instead of
            // treating that lifecycle cleanup as an accidental prefix rewrite.
            self.cache_continuity = Default::default();
        }
        let goal_epoch = self.context_memory.goal().map_or(0, |goal| goal.epoch);
        let goal_progress = self
            .context_memory
            .goal()
            .map(|goal| goal.progress.clone())
            .unwrap_or_default();
        // Submitted context is immutable until an explicit window rollover.
        let current_request = if continuation {
            Message::user(format!(
                "{GOAL_CONTINUATION_PROMPT}{}",
                goal_retry_reason
                    .map(|reason| format!(" Strict judge feedback: {reason}"))
                    .unwrap_or_default()
            ))
            .request_only()
        } else {
            Message::user_with_images(input, images)
        };
        let request_input = current_request
            .content
            .clone()
            .unwrap_or_else(|| input.to_owned());
        self.history.push(current_request.clone());
        self.history
            .push(Message::system(TURN_CONTEXT_BOUNDARY).request_only());
        if goal_mode.load(Ordering::Acquire) {
            self.history
                .push(Message::system(GOAL_JOB_INSTRUCTION).request_only());
            if let Some(goal) = self.context_memory.goal() {
                self.history.push(Message::system(format!(
                    "Durable goal objective: {}\nOutstanding requirements (judge assessments, not proof): {:?}\nNext action: {:?}\nUse retained tool observations and exact context-history references for verification.",
                    goal.objective, goal.remaining, goal.next_action
                )).request_only());
            }
        }
        let mut explicitly_activated_tools = self.registry.skill_tools_for(&self.attached_skills);
        explicitly_activated_tools.extend(self.registry.skyline_tool_names());
        if let Some(skill_name) = explicit_skill_name(input) {
            let activation = self.registry.activate_explicit_skill(skill_name)?;
            let parsed: Value = serde_json::from_str(&activation)?;
            if let Some(instructions) = parsed.get("instructions").and_then(Value::as_str) {
                append_skill_instruction(&mut self.history, skill_name, instructions);
            }
            explicitly_activated_tools.extend(
                parsed
                    .get("tools")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
            );
        }
        explicitly_activated_tools.sort();
        explicitly_activated_tools.dedup();
        let (vision_enabled, web_search_enabled, _web_search_explicitly_attached) =
            attached_harness_flags(attached_capabilities.as_deref());
        let profile = task_profile_with_history(
            input,
            web_search_enabled,
            &self.history,
            goal_mode.load(Ordering::Acquire),
        );
        if profile == TaskProfile::Research {
            self.history
                .push(Message::system(policy::RESEARCH_SYSTEM_INSTRUCTION).request_only());
        }
        let local_file_lookup = profile == TaskProfile::Agent
            && !goal_mode.load(Ordering::Acquire)
            && looks_like_local_file_lookup(input);
        let capability_discovery_requested =
            !local_file_lookup && looks_like_capability_request(input);
        let prior_context_requested = looks_like_prior_context_request(input);
        let foundation_guidance = (prior_context_requested && !local_file_lookup)
            .then(|| {
                self.registry
                    .foundation_memory_guidance()
                    .map(str::to_owned)
            })
            .flatten();
        let inherited_implementation = should_inherit_implementation_turn(input, &self.history);
        let implementation_requested =
            looks_like_coding_implementation_request(input) || inherited_implementation;
        let planning_or_documentation = looks_like_planning_or_documentation(input);
        let bounded_explanation = looks_like_bounded_explanation(input);
        let bounded_analysis = bounded_explanation || looks_like_bounded_analysis(input);
        let native_deferred_tools_supported = if model.starts_with("openai/") {
            supports_native_deferred_tools(model)
                && self.bridge.auth_status("openai").is_ok_and(|status| {
                    status.authenticated
                        && matches!(status.method.as_str(), "api-key" | "environment")
                })
        } else {
            supports_native_deferred_tools(model)
        };
        let research_budget = ResearchBudget::for_input(input);
        let task_guidance = task_guidance(input);
        let model_attempts = 0usize;
        let tool_rounds = 0usize;
        let malformed_repairs = 0usize;
        let empty_repairs = 0usize;
        let length_continuations = 0usize;
        let implementation_repairs = 0usize;
        let execution_evidence = turn_state::TurnExecutionEvidence::default();
        let consecutive_no_progress = 0usize;
        let progressful_inspection_rounds = 0usize;
        let implementation_inspection_checkpoint_used = false;
        let retry_instruction: Option<String> = None;
        let final_consistency_pending = false;
        let final_consistency_used = false;
        let completion_gate_repairs = 0usize;
        let local_lookup_read_calls = 0usize;
        let local_lookup_read_externalized = false;
        let local_lookup_recovery_calls = 0usize;
        let local_lookup_shell_calls = 0usize;
        let call_counts: HashMap<String, usize> = HashMap::new();
        let workspace_generation = self.registry.workspace_generation();
        let workspace_write_generation = self.registry.workspace_write_generation();
        let mut tool_discovery = match profile {
            TaskProfile::Agent if local_file_lookup => {
                tool_discovery::ToolDiscovery::direct_file_lookup()
            }
            TaskProfile::Agent if goal_mode.load(Ordering::Acquire) => {
                tool_discovery::ToolDiscovery::goal()
            }
            TaskProfile::Agent if implementation_requested => {
                tool_discovery::ToolDiscovery::coding(true)
            }
            TaskProfile::Agent if bounded_analysis => {
                tool_discovery::ToolDiscovery::bounded_analysis()
            }
            TaskProfile::Agent if looks_like_coding_request(input) => {
                tool_discovery::ToolDiscovery::coding(false)
            }
            TaskProfile::Agent => tool_discovery::ToolDiscovery::agent(),
            TaskProfile::Research => tool_discovery::ToolDiscovery::research(),
        };
        promote_agent_orchestration_tool(
            &mut tool_discovery,
            profile == TaskProfile::Agent && self.registry.agent_orchestration_enabled(),
        );
        if !local_file_lookup && profile == TaskProfile::Agent {
            tool_discovery.promote_for_input(input);
            if web_search_enabled && policy::looks_like_web_research_request(input) {
                tool_discovery.carry_web_research_surface();
            }
        }
        if capability_discovery_requested {
            tool_discovery.enable_search();
        }
        if prior_context_requested && !local_file_lookup {
            // Prior context is recoverable, but recovery schemas stay behind
            // the trailing side-tool discovery gateway.
            tool_discovery.enable_search();
        }
        if profile == TaskProfile::Agent && self.registry.has_shell_jobs() {
            tool_discovery.load(["shell_job"]);
        }
        if profile == TaskProfile::Agent && should_preserve_web_tool_surface(input, &self.history) {
            tool_discovery.carry_web_research_surface();
        }
        if !explicitly_activated_tools.is_empty() {
            tool_discovery.load(explicitly_activated_tools.iter().map(String::as_str));
        }
        let loop_budget = LoopBudget::default();
        let research_stop_grace_used = false;
        let analysis_stop_grace_used = false;
        let runaway_detector = RunawayDetector::default();
        let last_provenance_checkpoint: Option<String> = None;
        let (workspace_root, workspace_revision) = self.registry.workspace_identity();
        self.context_memory.policy =
            crate::project_settings::ProjectSettingsStore::new(&workspace_root)?
                .load()?
                .context;
        let mut turn_stable_overlays = Vec::new();
        if let Some(project_instructions) = load_project_instructions(&workspace_root) {
            turn_stable_overlays.push(Message::system(project_instructions).request_only());
        }
        let (session_cwd, context_roots) = self.registry.session_environment();
        if session_cwd != workspace_root || context_roots.len() > 1 {
            turn_stable_overlays.push(
                Message::system(format!(
                    "Session filesystem environment: primary workspace={workspace_root}; cwd={session_cwd}; context roots={}. Relative file and shell paths resolve from cwd. The primary workspace remains the project/settings identity, not a filesystem access boundary.",
                    context_roots.join(", ")
                ))
                .request_only(),
            );
        }
        let turn_context_orientation = self.context_memory.orientation();
        let last_context_updates = Vec::new();
        if prior_context_requested && !local_file_lookup {
            turn_stable_overlays.push(
                Message::system(
                    "Prior-context recovery is a side path, not the foreground action order. Reuse evidence already visible in the current window first. If one exact historical detail is missing, use trailing search_tools with an exact recovery tool name such as context_history, task_notes, or project_memory_recall, retrieve only that detail, then return to the task.",
                )
                .request_only(),
            );
        }
        let padded_input = format!(" {} ", input.trim().to_ascii_lowercase());
        let refers_to_previous_turn = inherited_implementation
            || [
                " it ",
                " this ",
                " that ",
                " them ",
                " those ",
                " same ",
                " again ",
                " previous ",
                " earlier ",
            ]
            .iter()
            .any(|needle| padded_input.contains(needle));
        if profile == TaskProfile::Agent && !refers_to_previous_turn {
            self.context_memory.clear_agent_checkpoint();
        }
        if profile == TaskProfile::Agent && refers_to_previous_turn {
            let durable_checkpoint = self.context_memory.agent_checkpoint_summary();
            if self.previous_turn_working_state.is_some() || durable_checkpoint.is_some() {
                let mut state = String::new();
                if let Some(checkpoint) = durable_checkpoint.as_deref() {
                    state.push_str(checkpoint);
                }
                if let Some(workspace) = self.previous_turn_working_state.as_deref() {
                    if !state.is_empty() {
                        state.push_str("\n\n");
                    }
                    state.push_str("Immediate previous-turn workspace evidence:\n");
                    state.push_str(workspace);
                }
                turn_stable_overlays.push(Message::system(format!(
                    "Coordinator continuation state from recent work in this session. This is historical observed state and may be stale.\n{state}"
                )).request_only());
            }
        }
        if let Some(memory) = render_matching_debate_memory(
            &self.retained_debate_knowledge,
            &workspace_root,
            workspace_revision.as_deref(),
        ) {
            turn_stable_overlays.push(Message::system(memory).request_only());
        }
        if let Some(guidance) = foundation_guidance.as_deref() {
            turn_stable_overlays.push(Message::system(guidance).request_only());
        }
        if profile == TaskProfile::Agent
            && let Some(guidance) = &task_guidance
        {
            turn_stable_overlays.push(Message::system(guidance.clone()).request_only());
        }

        Ok(TurnSetup {
            model,
            reasoning_level,
            cancel,
            attached_capabilities,
            goal_epoch,
            goal_progress,
            current_request,
            request_input,
            vision_enabled,
            web_search_enabled,
            profile,
            local_file_lookup,
            capability_discovery_requested,
            implementation_requested,
            planning_or_documentation,
            bounded_explanation,
            bounded_analysis,
            native_deferred_tools_supported,
            research_budget,
            model_attempts,
            tool_rounds,
            malformed_repairs,
            empty_repairs,
            length_continuations,
            implementation_repairs,
            execution_evidence,
            consecutive_no_progress,
            progressful_inspection_rounds,
            implementation_inspection_checkpoint_used,
            retry_instruction,
            final_consistency_pending,
            final_consistency_used,
            completion_gate_repairs,
            local_lookup_read_calls,
            local_lookup_read_externalized,
            local_lookup_recovery_calls,
            local_lookup_shell_calls,
            call_counts,
            workspace_generation,
            workspace_write_generation,
            tool_discovery,
            loop_budget,

            research_stop_grace_used,
            analysis_stop_grace_used,
            runaway_detector,
            last_provenance_checkpoint,
            workspace_revision,
            turn_stable_overlays,
            turn_context_orientation,
            last_context_updates,
        })
    }
}

fn promote_agent_orchestration_tool(discovery: &mut tool_discovery::ToolDiscovery, enabled: bool) {
    if enabled {
        discovery.load(["propose_agent_tasks"]);
    }
}

/// Read conventional root-level agent instruction files for this project's turns.
/// Missing, unreadable, non-regular, and oversized files are ignored.
fn load_project_instructions(workspace_root: &str) -> Option<String> {
    const MAX_FILE_BYTES: u64 = 64 * 1024;
    let root = std::path::Path::new(workspace_root);
    let mut sections = Vec::new();
    for name in ["AGENTS.md", "YEET.md"] {
        let path = root.join(name);
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !contents.trim().is_empty() {
            sections.push(format!("--- {name} ---\n{}", contents.trim()));
        }
    }
    (!sections.is_empty()).then(|| {
        format!(
            "Project instructions from root-level AGENTS.md and YEET.md files (apply where relevant):\n{}",
            sections.join("\n\n")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_agent_tool_is_foreground_only_when_enabled() {
        let catalog = vec![
            ToolDefinition::new("read_file", "read", json!({"type":"object"})),
            ToolDefinition::new("propose_agent_tasks", "delegate", json!({"type":"object"})),
        ];

        let mut disabled = tool_discovery::ToolDiscovery::goal();
        promote_agent_orchestration_tool(&mut disabled, false);
        assert!(
            disabled
                .attached(&catalog)
                .iter()
                .all(|tool| tool.name != "propose_agent_tasks")
        );

        let mut enabled = tool_discovery::ToolDiscovery::goal();
        promote_agent_orchestration_tool(&mut enabled, true);
        assert!(
            enabled
                .attached(&catalog)
                .iter()
                .any(|tool| tool.name == "propose_agent_tasks")
        );
    }
}
