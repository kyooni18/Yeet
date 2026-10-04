//! Session persistence coordination for the live backend session.
//!
//! Persistence is two-phase so storage never runs under the live-state mutex:
//! `prepare_session_write_locked` builds an immutable `PreparedSessionWrite`
//! from `SharedSession` while the caller holds the lock (cloning only), and
//! `commit_session_write` / `SessionWriter` write it to the `SessionStore`
//! after the lock is released. Store writes take cross-process file locks and
//! may block on I/O; Remote reconnects and the background daemon need the
//! live-state mutex to observe semantic state in the meantime.
//!
//! `persist_locked` is the one-step convenience for callers that are not
//! latency-sensitive (commands that already hold the lock).

use super::*;

/// Immutable persistence payload prepared while holding the live-state lock.
///
/// Constructing this snapshot may clone conversation/model history, but it performs
/// no filesystem or cross-process lock operations. Latency-sensitive callers can
/// therefore release SharedSession before committing it to disk.
pub(super) struct PreparedSessionWrite {
    session: StoredSession,
    goal_mode: bool,
}

pub(super) fn prepare_session_write_locked(
    state: &mut SharedSession,
    workspace: &Path,
    workspace_id: String,
    history: Vec<Message>,
) -> Option<PreparedSessionWrite> {
    let conversation = state.state.conversation.clone().unwrap_or_default();
    if !conversation
        .iter()
        .any(|entry| matches!(entry.kind, ConversationKind::User { .. }))
        && state.state.current_session_id.is_none()
    {
        return None;
    }

    let id = state
        .state
        .current_session_id
        .clone()
        .unwrap_or_else(SessionStore::new_id);
    let created_at = state.meta.created_at.unwrap_or_else(Utc::now);
    let title = state
        .meta
        .title
        .clone()
        .unwrap_or_else(|| fallback_title(&conversation));

    state.state.current_session_id = Some(id.clone());
    state.meta.created_at = Some(created_at);
    state.meta.title = Some(title.clone());

    Some(PreparedSessionWrite {
        session: StoredSession {
            version: 4,
            debate: state.state.debate.clone(),
            id,
            title,
            created_at,
            updated_at: Utc::now(),
            workspace_root: workspace.display().to_string(),
            workspace_id: Some(workspace_id),
            working_directory: state
                .meta
                .working_directory
                .as_deref()
                .map(|path| crate::session_store::workspace_relative_path(workspace, path)),
            context_roots: state
                .meta
                .context_roots
                .iter()
                .map(|path| crate::session_store::workspace_relative_path(workspace, path))
                .collect(),
            model: state.state.active_model.clone(),
            agent_mode: state.state.agent_mode,
            autonomy_mode: state.state.autonomy_mode,
            token_usage: state.state.token_usage.clone(),
            credit_usage: state.state.credit_usage,
            conversation,
            model_history: storage_model_history(history),
            runs: state.meta.runs.clone(),
            retained_debate_knowledge: state.meta.retained_debate_knowledge.clone(),
            attached_harness_capabilities: state.meta.attached_capabilities.clone(),
            disabled_capabilities: state.meta.disabled_capabilities.clone(),
        },
        goal_mode: state.state.goal_mode,
    })
}

pub(super) fn commit_session_write(
    store: &SessionStore,
    prepared: PreparedSessionWrite,
) -> Result<()> {
    let id = prepared.session.id.clone();
    store.save(&prepared.session)?;
    store.set_goal_mode(&id, prepared.goal_mode)
}

/// Compatibility wrapper for call sites that still require synchronous
/// persistence. New latency-sensitive paths should prepare, release the live-state
/// mutex, and then commit the prepared write.
pub(super) fn persist_locked(
    state: &mut SharedSession,
    store: &SessionStore,
    workspace: &Path,
    history: Vec<Message>,
) -> Result<()> {
    let workspace_id = crate::session_store::ensure_workspace_id(workspace)?;
    let Some(prepared) = prepare_session_write_locked(state, workspace, workspace_id, history)
    else {
        return Ok(());
    };
    commit_session_write(store, prepared)
}

/// Removes the coordinator's internal coding prompt before session persistence.
fn storage_model_history(mut history: Vec<Message>) -> Vec<Message> {
    if history
        .first()
        .is_some_and(is_internal_coordinator_system_message)
    {
        history.remove(0);
    }
    history
}

/// Commits prepared session snapshots and refreshes the workspace catalog.
///
/// Session-store writes take cross-process file locks and may block on
/// filesystem I/O, so callers prepare a snapshot under the live-state mutex and
/// hand it here only after releasing that mutex: Remote reconnects and the
/// background daemon need the mutex to observe semantic state.
#[derive(Clone)]
pub(super) struct SessionWriter {
    pub(super) store: SessionStore,
    pub(super) workspace: PathBuf,
    pub(super) shared: Arc<Mutex<SharedSession>>,
    pub(super) tx: EventSender,
}

impl SessionWriter {
    /// Commits a prepared snapshot; a failure is surfaced as a session error
    /// instead of failing the run that produced it.
    pub(super) fn commit_or_report(&self, prepared: PreparedSessionWrite) {
        if let Err(error) = commit_session_write(&self.store, prepared) {
            let mut state = self.shared.lock_or_recover();
            state.state.error_message = Some(format!("Save failed: {error}"));
            self.publish_compact(&state);
        }
    }

    /// Re-reads the workspace session catalog and publishes it.
    pub(super) fn refresh_catalog(&self) {
        if let Ok(catalog) = SessionCatalog::read_from_store(&self.store, &self.workspace) {
            let mut state = self.shared.lock_or_recover();
            apply_session_catalog_locked(&mut state, &catalog);
            self.publish_compact(&state);
        }
    }

    fn publish_compact(&self, state: &SharedSession) {
        let _ = self
            .tx
            .send(BackendEvent::Envelope(state_envelope_without_conversation(
                &state.state,
            )));
    }
}
