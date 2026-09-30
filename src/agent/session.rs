//! Session-runtime binding and cache-continuity lifecycle.

use super::{AgentCoordinator, Message, SYSTEM_INSTRUCTION};
use crate::{
    tools::{BridgeHandle, ToolRegistry},
    web_search::CAPABILITY_ID as WEB_SEARCH_CAPABILITY_ID,
};
use uuid::Uuid;

pub(super) fn runtime_attached_capabilities(values: &[String]) -> Vec<String> {
    values
        .iter()
        .filter(|value| {
            value.as_str() != WEB_SEARCH_CAPABILITY_ID
                && value.as_str() != "lead"
                && value.as_str() != "context-mode"
                && value.as_str() != "builtin:artifacts"
                && value.as_str() != crate::skyline::CAPABILITY_ID
                && !value.starts_with("skill:")
        })
        .cloned()
        .collect()
}

impl AgentCoordinator {
    pub(crate) fn new(bridge: BridgeHandle, registry: ToolRegistry) -> Self {
        Self {
            bridge,
            registry,
            history: vec![Message::system(SYSTEM_INSTRUCTION)],
            retained_debate_knowledge: Vec::new(),
            context_key: Uuid::new_v4().to_string(),
            context_memory: super::context::ContextMemory::default(),
            cache_continuity: super::cache::ContinuityTracker::default(),
            observation_cache: super::cache::ObservationCache::default(),
            previous_turn_working_state: None,
            attached_skills: Default::default(),
            skill_instruction_history: Default::default(),
        }
    }
    pub fn set_session_runtime(
        &mut self,
        store: crate::session_store::SessionStore,
        active_session_id: Option<String>,
    ) {
        let previous_session_id = self.context_memory.session_id().map(str::to_owned);
        let session_changed = previous_session_id.as_deref() != active_session_id.as_deref();
        self.cache_continuity
            .rebind_session(previous_session_id.as_deref(), active_session_id.as_deref());
        if session_changed {
            self.observation_cache = Default::default();
            self.context_key = active_session_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
        }
        self.context_memory.bind(
            active_session_id
                .as_ref()
                .map(|id| store.directory.join(id).join("context")),
        );
        self.registry.set_session_runtime(store, active_session_id);
    }
}
