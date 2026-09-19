//! Session-runtime binding and cache-continuity lifecycle.

use super::{AgentCoordinator, Message, SYSTEM_INSTRUCTION};
use crate::{
    core::BridgeClient, tools::ToolRegistry, web_search::CAPABILITY_ID as WEB_SEARCH_CAPABILITY_ID,
};
use uuid::Uuid;

pub(super) fn runtime_attached_capabilities(values: &[String]) -> Vec<String> {
    values
        .iter()
        .filter(|value| {
            value.as_str() != WEB_SEARCH_CAPABILITY_ID
                && value.as_str() != "lead"
                && value.as_str() != crate::skyline::CAPABILITY_ID
                && !value.starts_with("skill:")
        })
        .cloned()
        .collect()
}

impl AgentCoordinator {
    pub fn new(bridge: BridgeClient, registry: ToolRegistry) -> Self {
        Self {
            bridge,
            registry,
            history: vec![Message::system(SYSTEM_INSTRUCTION)],
            retained_debate_knowledge: Vec::new(),
            context_key: Uuid::new_v4().to_string(),
            context_memory: super::context::ContextMemory::default(),
            cache_continuity: super::cache::ContinuityTracker::default(),
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
        self.cache_continuity.rebind_session(
            self.context_memory.session_id(),
            active_session_id.as_deref(),
        );
        self.context_memory.bind(
            active_session_id
                .as_ref()
                .map(|id| store.directory.join(id).join("context")),
        );
        self.registry.set_session_runtime(store, active_session_id);
    }
}
