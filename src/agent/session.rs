//! Session-runtime binding and cache-continuity lifecycle.

use super::AgentCoordinator;

impl AgentCoordinator {
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
