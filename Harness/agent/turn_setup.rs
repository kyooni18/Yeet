//! Per-turn setup: record the user message and assemble the stable context.

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
    pub(super) native_deferred_tools_supported: bool,
    pub(super) tool_discovery: tool_discovery::ToolDiscovery,
    pub(super) loop_budget: LoopBudget,
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
        // Ordinary turn boundaries are append-only. Historical request-only
        // guidance is superseded by TURN_CONTEXT_BOUNDARY and newer overlays, but
        // remains on the wire so the provider can reuse the already-cached prefix.
        // Physical pruning is reserved for explicit history replacement/rollover.
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
                    .map(|reason| format!(" Judge feedback: {reason}"))
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
        let goal_block = goal_mode
            .load(Ordering::Acquire)
            .then(|| self.context_memory.goal().map(goal_block))
            .flatten();
        // A goal block whose objective no longer matches (goal ended or the
        // user amended it) would keep presenting superseded wording with
        // system authority, so drop it even though that costs a cache miss.
        let current_brief = goal_block.as_ref().map(|(brief, _)| brief.as_str());
        self.history.retain(|message| !is_stale_goal_block(message, current_brief));
        self.history.push(current_request.clone());
        self.history
            .push(Message::system(TURN_CONTEXT_BOUNDARY).request_only());
        if let Some((_, block)) = goal_block {
            self.history
                .push(Message::system(GOAL_JOB_INSTRUCTION).request_only());
            self.history.push(Message::system(block).request_only());
        }
        let mut explicitly_activated_tools = self.registry.skill_tools_for(&self.attached_skills);
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
        let (vision_enabled, web_search_enabled, _) =
            attached_harness_flags(attached_capabilities.as_deref());
        let native_deferred_tools_supported = if model.starts_with("openai/") {
            supports_native_deferred_tools(model)
                && self.bridge.auth_status("openai").is_ok_and(|status| {
                    status.authenticated
                        && matches!(status.method.as_str(), "api-key" | "environment")
                })
        } else {
            supports_native_deferred_tools(model)
        };
        // One basic surface for every turn. Tools only ever append, so the
        // provider-visible prefix stays warm across turns.
        let mut tool_discovery = tool_discovery::ToolDiscovery::basic();
        tool_discovery.restore_warm_surface(&self.warm_tool_names, self.warm_tool_search_enabled);
        promote_agent_orchestration_tool(
            &mut tool_discovery,
            self.registry.agent_group_tool_names(),
            self.registry.agent_orchestration_enabled(),
        );
        tool_discovery.load(explicitly_activated_tools.iter().map(String::as_str));
        self.warm_tool_names = tool_discovery.loaded_names().to_vec();
        self.warm_tool_search_enabled = tool_discovery.search_enabled();

        let (workspace_root, workspace_revision) = self.registry.workspace_identity();
        self.context_memory.policy =
            crate::project_settings::ProjectSettingsStore::new(&workspace_root)?
                .load()?
                .context;
        let mut turn_stable_overlays = Vec::new();
        if let Some(project_instructions) = load_project_instructions(&workspace_root) {
            turn_stable_overlays.push(Message::system(project_instructions).request_only());
        }
        if self.context_memory.policy.repo_map
            && let Some(repo_map) = repo_context::repo_map_overlay(&self.history, &workspace_root)
        {
            turn_stable_overlays.push(repo_map);
        }
        let (session_cwd, context_roots) = self.registry.session_environment();
        turn_stable_overlays.push(
            Message::system(format!(
                "Workspace: {workspace_root}; session cwd: {session_cwd}; context roots: {}. File-tool relative paths resolve from session cwd. Shell commands initially start there and remember directory changes across calls; consult shellCwd in shell results. Do not prepend a redundant cd to commands. Use workingDirectory for an explicit per-call directory override.",
                context_roots.join(", ")
            ))
            .request_only(),
        );
        if let Some(memory) = render_matching_debate_memory(
            &self.retained_debate_knowledge,
            &workspace_root,
            workspace_revision.as_deref(),
        ) {
            turn_stable_overlays.push(Message::system(memory).request_only());
        }
        if let Some(guidance) = self.registry.agent_group_guidance() {
            turn_stable_overlays.push(Message::system(guidance).request_only());
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
            native_deferred_tools_supported,
            tool_discovery,
            loop_budget: LoopBudget::default(),
            workspace_revision,
            turn_stable_overlays,
            turn_context_orientation: self.context_memory.orientation(),
            // Trust the record only while those messages are still in the
            // window; rollover, compaction, or a session swap replaces history.
            last_context_updates: if self
                .submitted_context_updates
                .iter()
                .all(|update| self.history.contains(update))
            {
                self.submitted_context_updates.clone()
            } else {
                Vec::new()
            },
        })
    }
}

/// Attaches every agent tool together so the provider-visible tool envelope
/// does not change when the model first launches or messages an agent.
fn promote_agent_orchestration_tool(
    discovery: &mut tool_discovery::ToolDiscovery,
    names: &[&'static str],
    enabled: bool,
) {
    if enabled && !names.is_empty() {
        discovery.load(names);
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
    fn adaptive_agent_tools_attach_together_only_when_enabled() {
        let mut catalog = vec![ToolDefinition::new(
            "read_file",
            "read",
            json!({"type":"object"}),
        )];
        catalog.extend(crate::agents::group::control_commands::tool_definitions());

        let mut disabled = tool_discovery::ToolDiscovery::basic();
        promote_agent_orchestration_tool(&mut disabled, &[], false);
        let attached = disabled.attached(&catalog);
        for name in crate::agents::group::control_commands::TOOL_NAMES {
            assert!(attached.iter().all(|tool| tool.name != name));
        }

        let mut enabled = tool_discovery::ToolDiscovery::basic();
        promote_agent_orchestration_tool(
            &mut enabled,
            &crate::agents::group::control_commands::TOOL_NAMES,
            true,
        );
        let attached = enabled.attached(&catalog);
        for name in crate::agents::group::control_commands::TOOL_NAMES {
            assert!(attached.iter().any(|tool| tool.name == name));
        }
    }
}

const GOAL_BLOCK_HEADER: &str = "Goal (the user's request; their newer messages take precedence):\n";

/// Renders the per-turn goal block, returning the objective part separately so
/// later turns can tell whether an older block is stale.
fn goal_block(goal: &goal::GoalJob) -> (String, String) {
    let brief = format!("{GOAL_BLOCK_HEADER}{}", goal.brief());
    let mut block = brief.clone();
    if !goal.remaining.is_empty() {
        block.push_str("\n\nJudge says still missing:");
        for item in &goal.remaining {
            block.push_str("\n- ");
            block.push_str(item);
        }
    }
    (brief, block)
}

fn is_stale_goal_block(message: &Message, current_brief: Option<&str>) -> bool {
    if message.role != MessageRole::System || message.request_only != Some(true) {
        return false;
    }
    let Some(content) = message.content.as_deref() else {
        return false;
    };
    if content.starts_with("This is a goal job:") {
        return current_brief.is_none() || content != GOAL_JOB_INSTRUCTION;
    }
    // Blocks from before this format used a bare "Goal: " prefix.
    let is_goal_block = content.starts_with(GOAL_BLOCK_HEADER) || content.starts_with("Goal: ");
    is_goal_block && current_brief.is_none_or(|brief| !content.starts_with(brief))
}
