//! Session-scoped history, attachments, environment controls, and Goal retry helpers.

use super::*;
use uuid::Uuid;

const API_RETRY_DELAY: Duration = Duration::from_secs(60);
const API_RETRY_LIMIT: u32 = 5;

impl AgentCoordinator {
    pub fn shutdown(&self) {
        self.registry.shutdown();
    }

    pub fn model_history(&self) -> Vec<Message> {
        self.history.clone()
    }

    pub fn prepare_turn_history_checkpoint(&mut self) -> usize {
        // Keep provider-visible history append-only across ordinary user turns.
        // Request-only messages are superseded by later turn boundaries rather than
        // physically removed; deleting them would invalidate the cached prefix that
        // follows them. Explicit rewind/replace operations still reset continuity.
        self.history.len()
    }

    pub fn rewind_model_history(&mut self, len: usize) -> Result<()> {
        if len == 0 || len > self.history.len() {
            bail!("invalid model history checkpoint {len}");
        }
        self.history.truncate(len);
        self.context_memory = context::ContextMemory::default();
        self.cache_continuity = cache::ContinuityTracker::default();
        self.previous_turn_working_state = None;
        self.warm_tool_names.clear();
        self.warm_tool_search_enabled = false;
        self.context_key = Uuid::new_v4().to_string();
        Ok(())
    }

    pub fn replace_model_history(&mut self, restored: Vec<Message>) {
        let body = if restored
            .first()
            .is_some_and(is_internal_coordinator_system_message)
        {
            restored.into_iter().skip(1).collect()
        } else {
            restored
        };
        self.context_memory = context::ContextMemory::default();
        self.cache_continuity = cache::ContinuityTracker::default();
        self.previous_turn_working_state = None;
        self.warm_tool_names.clear();
        self.warm_tool_search_enabled = false;
        let attached_skills = self.attached_skills.drain().collect::<Vec<_>>();
        for skill in attached_skills {
            self.registry.deactivate_skill(&skill);
        }
        self.registry.detach_skyline();
        self.history = vec![Message::system(SYSTEM_INSTRUCTION)];
        self.history.extend(body);
        self.skill_instruction_history = self
            .history
            .iter()
            .filter_map(|message| message.content.as_deref())
            .filter_map(|content| content.strip_prefix("Attached Skill: "))
            .filter_map(|rest| rest.lines().next())
            .map(str::to_owned)
            .collect();
        self.context_key = Uuid::new_v4().to_string();
    }

    // Prompt-cache invariant: skill attachment history is append-only. Never edit or
    // delete an earlier skill instruction; later state changes are appended as markers.
    pub fn attach_skill_to_session(&mut self, name: &str) -> Result<()> {
        if self.attached_skills.contains(name) {
            return Ok(());
        }
        self.registry.enable_skill_attachment(name);
        let activation = self.registry.activate_explicit_skill(name)?;
        let parsed: Value = serde_json::from_str(&activation)?;
        if self.skill_instruction_history.contains(name) {
            self.history.push(Message::system(format!(
                "Skill attachment state: {name} reattached. Resume following the previously attached Skill instruction for {name} from this session history. Keep all earlier history unchanged for prompt-cache continuity."
            )));
        } else if let Some(instructions) = parsed.get("instructions").and_then(Value::as_str) {
            self.history.push(Message::system(format!(
                "Attached Skill: {name}\n{instructions}\nThis Skill is attached to the current session. Follow it when relevant and use its support tools only as needed; normal sandbox/approval rules apply."
            )));
            self.skill_instruction_history.insert(name.to_owned());
        }
        self.attached_skills.insert(name.to_owned());
        Ok(())
    }

    pub fn detach_skill_from_session(&mut self, name: &str) {
        self.attached_skills.remove(name);
        self.registry.deactivate_skill(name);
        if self.skill_instruction_history.contains(name) {
            self.history.push(Message::system(format!(
                "Skill attachment state: {name} detached. From this point onward, do not treat the earlier attached Skill instruction for {name} as active unless the user explicitly invokes or reattaches it. Preserve and do not reinterpret any earlier history."
            )));
        }
    }

    pub(super) fn sync_attached_skills(&mut self, attached: Option<&[String]>) -> Result<()> {
        let requested = attached
            .unwrap_or_default()
            .iter()
            .filter_map(|value| value.strip_prefix("skill:"))
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        let stale = self
            .attached_skills
            .difference(&requested)
            .cloned()
            .collect::<Vec<_>>();
        for skill in stale {
            self.registry.deactivate_skill(&skill);
        }
        for skill in requested.difference(&self.attached_skills) {
            let _ = self.registry.activate_explicit_skill(skill)?;
        }
        self.attached_skills = requested;
        Ok(())
    }

    pub(super) fn sync_attached_skyline(&mut self, attached: Option<&[String]>) -> Result<()> {
        let requested = attached
            .unwrap_or_default()
            .iter()
            .any(|value| value == crate::skyline::CAPABILITY_ID);
        self.set_skyline_attachment(requested)
    }

    pub fn set_skyline_attachment(&mut self, attached: bool) -> Result<()> {
        if attached {
            let activation = self.registry.attach_skyline()?;
            let parsed: Value = serde_json::from_str(&activation)?;
            if parsed.get("alreadyActive").and_then(Value::as_bool) != Some(true)
                && let Some(bootstrap) = parsed.get("bootstrap")
            {
                let bootstrap = serde_json::to_string(bootstrap)?;
                self.history.push(Message::system(format!(
                    "Attached Skyline:\n{bootstrap}\nUse this bootstrap as the initial shared situational-awareness snapshot. Choose work autonomously from the actual project state. Before the first project mutation after this attachment, publish one compact Skyline sync with assessment, intent, scope, and next action. After that, sync only for material changes or collision-sensitive work; do not poll unchanged state."
                )));
            }
        } else {
            self.registry.detach_skyline();
        }
        Ok(())
    }

    pub fn compact_model_history(&mut self) -> Result<(usize, usize)> {
        self.context_memory.load(&mut self.history)?;
        let before = self.history.len();
        self.context_memory.rollover(&mut self.history, None)?;
        self.registry.reset_model_evidence_window();
        self.cache_continuity = cache::ContinuityTracker::default();
        Ok((before, self.history.len()))
    }

    pub fn set_protected_write_paths(
        &mut self,
        paths: impl IntoIterator<Item = std::path::PathBuf>,
    ) {
        self.registry.set_protected_write_paths(paths);
    }

    pub(crate) fn set_agent_orchestrator(
        &mut self,
        orchestrator: Option<crate::orchestration::AdaptiveAgentOrchestrator>,
    ) {
        self.registry.set_agent_orchestrator(orchestrator);
    }

    pub fn session_environment(&self) -> (String, Vec<String>) {
        self.registry.session_environment()
    }

    pub fn set_working_directory(
        &mut self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<std::path::PathBuf> {
        self.registry.set_working_directory(path)
    }

    pub fn add_context_root(
        &mut self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<std::path::PathBuf> {
        self.registry.add_context_root(path)
    }

    pub fn remove_context_root(&mut self, path: impl AsRef<std::path::Path>) -> Result<bool> {
        self.registry.remove_context_root(path)
    }

    pub fn restore_session_environment(
        &mut self,
        working_directory: Option<&str>,
        context_roots: &[String],
    ) -> Result<()> {
        self.registry
            .restore_session_environment(working_directory, context_roots)
    }
    pub fn configure_foundation_memory(
        &mut self,
        enabled: bool,
        backend: crate::project_settings::ServiceBackend,
        server: impl Into<String>,
        project: impl Into<String>,
    ) {
        self.registry
            .configure_foundation_memory(enabled, backend, server, project);
    }

    pub fn configure_web_backend(
        &mut self,
        backend: crate::project_settings::ServiceBackend,
        server: impl Into<String>,
    ) {
        self.registry.configure_web_backend(backend, server);
    }

    pub fn set_retained_debate_knowledge(
        &mut self,
        knowledge: Vec<crate::debate::RetainedDebateKnowledge>,
    ) {
        self.retained_debate_knowledge = knowledge;
        self.cache_continuity = cache::ContinuityTracker::default();
    }
}

pub(super) fn goal_retry_delay(attempt: u32) -> Option<Duration> {
    (1..=API_RETRY_LIMIT)
        .contains(&attempt)
        .then_some(API_RETRY_DELAY)
}

pub(super) fn wait_for_goal(cancel: &AtomicBool, enabled: &AtomicBool, delay: Duration) -> bool {
    let deadline = Instant::now() + delay;
    while Instant::now() < deadline {
        if cancel.load(Ordering::Acquire) || !enabled.load(Ordering::Acquire) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(remaining.min(Duration::from_millis(100)));
    }
    !cancel.load(Ordering::Acquire) && enabled.load(Ordering::Acquire)
}

pub(super) fn retryable_goal_error(message: &str) -> bool {
    let value = message.to_ascii_lowercase();
    if [
        "cancelled",
        "canceled",
        "authentication",
        "not authenticated",
        "unauthorized",
        "forbidden",
        "api key",
        "no model is configured",
        "no model selected",
        "unknown provider",
        "unsupported provider",
        "vision capability is not attached",
        "exceed the fresh working-context budget",
        "workspace mutation lease",
        "previously submitted context changed",
    ]
    .iter()
    .any(|needle| value.contains(needle))
    {
        return false;
    }

    contains_retryable_http_status(&value)
        || [
            "rate limit",
            "rate-limited",
            "too many requests",
            "temporarily unavailable",
            "temporary unavailable",
            "service unavailable",
            "bad gateway",
            "gateway timeout",
            "provider overloaded",
            "overloaded",
            "timed out",
            "timeout",
            "broken pipe",
            "connection reset",
            "connection aborted",
            "connection refused",
            "connection closed",
            "stream ended unexpectedly",
            "provider bridge closed",
            "unexpected eof",
        ]
        .iter()
        .any(|needle| value.contains(needle))
}

fn contains_retryable_http_status(message: &str) -> bool {
    message
        .split(|ch: char| !ch.is_ascii_digit())
        .filter_map(|part| {
            (part.len() == 3)
                .then(|| part.parse::<u16>().ok())
                .flatten()
        })
        .any(|status| matches!(status, 408 | 409 | 425 | 429) || (500..=599).contains(&status))
}

pub(super) fn bridge_transport_error(message: &str) -> bool {
    let value = message.to_ascii_lowercase();
    [
        "provider bridge closed",
        "provider bridge stream ended unexpectedly",
        "broken pipe",
        "connection reset",
        "connection aborted",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

pub(super) fn tool_call_indicates_implementation_intent(
    call: &ToolCall,
    content: &str,
    succeeded: bool,
) -> bool {
    if call.name == "apply_file_edits" {
        return true;
    }
    if !succeeded || call.name != tool_discovery::SEARCH_TOOL {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return false;
    };
    ["loaded", "alreadyLoaded", "deferred"].iter().any(|field| {
        value
            .get(*field)
            .and_then(Value::as_array)
            .is_some_and(|tools| {
                tools
                    .iter()
                    .any(|tool| tool.as_str() == Some("apply_file_edits"))
            })
    })
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    #[test]
    fn api_retry_allows_five_one_minute_cooldowns() {
        assert_eq!(goal_retry_delay(0), None);
        for attempt in 1..=5 {
            assert_eq!(goal_retry_delay(attempt), Some(Duration::from_secs(60)));
        }
        assert_eq!(goal_retry_delay(6), None);
    }

    #[test]
    fn api_retry_accepts_only_transient_provider_errors() {
        for error in [
            "API error: HTTP 503 Service Unavailable",
            "API error: 429 Too Many Requests",
            "provider bridge stream ended unexpectedly",
            "connection reset",
            "request timed out",
        ] {
            assert!(retryable_goal_error(error), "{error}");
        }
        for error in [
            "cancelled",
            "unauthorized",
            "no model selected",
            "HTTP 400 invalid request",
            "workspace mutation lease is held by another active Yeet task",
            "Previously submitted context changed within this window",
        ] {
            assert!(!retryable_goal_error(error), "{error}");
        }
    }

    #[test]
    fn api_retry_wait_is_cancellable() {
        let cancel = AtomicBool::new(true);
        assert!(!wait_for_goal(
            &cancel,
            &AtomicBool::new(true),
            API_RETRY_DELAY
        ));
    }
}
