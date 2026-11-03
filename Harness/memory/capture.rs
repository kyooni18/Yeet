//! Deterministic post-commit capture. This ingests canonical text only; it never
//! downloads, embeds, summarizes or calls a provider. Transcript storage remains
//! authoritative and independently preserved.

use super::{
    store::{clear_derived, insert_record, suppressed},
    *,
};
use crate::{
    model::ConversationKind,
    session_store::{RunStatus, StoredSession},
};
use anyhow::{Result, ensure};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;

impl MemoryStore {
    /// Retry post-commit ingestion after restart. This deliberately uses the
    /// same enablement epoch as live capture; it cannot turn into backfill.
    pub fn reconcile_workspace(
        &mut self,
        sessions: &crate::session_store::SessionStore,
        workspace: &std::path::Path,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<usize> {
        let id = crate::session_store::ensure_workspace_id(workspace)?;
        let settings = self.settings(&id)?;
        if !settings.whole_session.enabled || settings.whole_session.local_only {
            return Ok(0);
        }
        let mut captured = 0;
        for summary in sessions.list(workspace)? {
            ensure!(
                !cancel.load(std::sync::atomic::Ordering::Acquire),
                "cancelled"
            );
            let session = sessions.load(&summary.id)?;
            if session.workspace_id.as_deref() != Some(&id) {
                continue;
            }
            captured += self.capture_session(&session)?;
        }
        Ok(captured)
    }
    /// Capture only runs begun after explicit enablement. Reconciliation of an
    /// existing session is NOT historical backfill. Unknown/legacy enablement
    /// is baselined at first observation rather than silently importing history.
    pub fn capture_session(&mut self, session: &StoredSession) -> Result<usize> {
        let Some(workspace) = session.workspace_id.as_deref() else {
            return Ok(0);
        };
        let settings = self.settings(workspace)?;
        if !settings.whole_session.enabled || settings.whole_session.local_only {
            return Ok(0);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Recheck under the write transaction so disable/privacy changes fence
        // a concurrently prepared snapshot before any text is committed.
        let settings = store::read_settings(&tx, workspace)?;
        if !settings.whole_session.enabled || settings.whole_session.local_only {
            return Ok(0);
        }
        tx.execute(
            "INSERT OR IGNORE INTO capture_epochs VALUES (?1,?2)",
            params![workspace, Utc::now().to_rfc3339()],
        )?;
        let since: String = tx.query_row(
            "SELECT since FROM capture_epochs WHERE workspace=?1",
            [workspace],
            |r| r.get(0),
        )?;
        let since = DateTime::parse_from_rfc3339(&since)?.with_timezone(&Utc);
        let previous: Option<String> = tx
            .query_row(
                "SELECT updated_at FROM captured_sessions WHERE workspace=?1 AND session=?2",
                params![workspace, session.id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            if DateTime::parse_from_rfc3339(&previous)?.with_timezone(&Utc) > session.updated_at {
                return Ok(0);
            }
        }

        let mut episodes = BTreeMap::new();
        for (start, entry) in session.conversation.iter().enumerate() {
            if !matches!(entry.kind, ConversationKind::User { .. }) {
                continue;
            }
            let Some(run) = session
                .runs
                .iter()
                .rev()
                .find(|run| run.user_entry_id.as_deref() == Some(entry.id.as_str()))
            else {
                continue;
            };
            if run.status == RunStatus::Running {
                episodes.insert(entry.id.clone(), (String::new(), String::new(), false));
                continue;
            }
            let end = session.conversation[start + 1..]
                .iter()
                .position(|e| matches!(e.kind, ConversationKind::User { .. }))
                .map_or(session.conversation.len(), |offset| start + 1 + offset);
            let mut text = String::new();
            for entry in &session.conversation[start..end] {
                // No hidden reasoning, request-only context, activity/status
                // messages or recursive native-memory tool envelopes.
                let kind = match &entry.kind {
                    ConversationKind::User { .. } => Some(entry.kind.clone()),
                    ConversationKind::Assistant {
                        content,
                        tool_calls,
                    } => Some(ConversationKind::Assistant {
                        content: content.clone(),
                        tool_calls: tool_calls
                            .iter()
                            .filter(|call| !call.name.starts_with("memory_"))
                            .cloned()
                            .collect(),
                    }),
                    ConversationKind::ToolCall { tool_call }
                        if !tool_call.name.starts_with("memory_") =>
                    {
                        Some(entry.kind.clone())
                    }
                    ConversationKind::Skill { .. } | ConversationKind::Mcp { .. } => {
                        Some(entry.kind.clone())
                    }
                    _ => None,
                };
                if let Some(kind) = kind {
                    text.push_str(&serde_json::to_string(&kind)?);
                    text.push('\n');
                }
            }
            // Version the deterministic rendering and hash; byte offsets refer
            // to this episode rendering, linked by immutable conversation IDs.
            let revision = format!("conversation-v1:{:x}", Sha256::digest(text.as_bytes()));
            episodes.insert(entry.id.clone(), (revision, text, run.started_at >= since));
        }
        let scope = MemoryScope::Workspace(workspace.into());
        // Retire stale automatic records atomically with publishing their
        // replacements. User overrides and source suppression always win.
        for privacy in [Privacy::Shared, Privacy::LocalOnly] {
            let schema = privacy.schema();
            let records = {
                let mut statement = tx.prepare(&format!("SELECT body FROM {schema}.records WHERE workspace=?1 AND kind='whole-session' AND body IS NOT NULL"))?;
                statement
                    .query_map([workspace], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };
            for body in records {
                let record: MemoryRecord = serde_json::from_str(&body)?;
                let Some(source) = &record.source else {
                    continue;
                };
                if source.session_id != session.id || record.edited || privacy == Privacy::LocalOnly
                {
                    continue;
                }
                if episodes
                    .get(&source.window_id)
                    .is_some_and(|(revision, _, _)| revision == &source.source_revision)
                {
                    continue;
                }
                clear_derived(&tx, privacy, &record.id)?;
                tx.execute(&format!("UPDATE {schema}.records SET body=NULL,source_key=NULL,revision=revision+1 WHERE id=?1"), [&record.id])?;
            }
        }
        let mut inserted = 0;
        for (window, (revision, text, eligible)) in episodes {
            if !eligible {
                continue;
            }
            for chunk in chunk_text(&text, 3200, 320)? {
                let source = MemorySource {
                    session_id: session.id.clone(),
                    window_id: window.clone(),
                    source_revision: revision.clone(),
                    start: chunk.start as u64,
                    end: chunk.end as u64,
                };
                let key = format!(
                    "{:x}",
                    Sha256::digest(serde_json::to_vec(&(
                        scope.key(),
                        MemoryKind::WholeSession.key(),
                        &source
                    ))?)
                );
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM main.records WHERE source_key=?1 UNION ALL SELECT 1 FROM protected.records WHERE source_key=?1)", [&key], |r| r.get(0))?;
                if exists || suppressed(&tx, &scope, &source)? {
                    continue;
                }
                let now = Utc::now().to_rfc3339();
                let input = SaveMemory {
                    kind: MemoryKind::WholeSession,
                    scope: scope.clone(),
                    privacy: Privacy::Shared,
                    text: chunk.text,
                    tags: vec![],
                    source: Some(source),
                };
                input.validate()?;
                ensure!(inserted < 100_000, "session_capture_capacity");
                let record = MemoryRecord {
                    id: Uuid::new_v4().to_string(),
                    revision: 1,
                    kind: input.kind,
                    scope: input.scope,
                    privacy: input.privacy,
                    text: input.text,
                    tags: input.tags,
                    source: input.source,
                    edited: false,
                    created_at: now.clone(),
                    updated_at: now,
                    indexing_status: "pending".into(),
                };
                insert_record(&tx, &record, Some(&key))?;
                inserted += 1;
            }
        }
        tx.execute("INSERT INTO captured_sessions VALUES (?1,?2,?3) ON CONFLICT(workspace,session) DO UPDATE SET updated_at=excluded.updated_at",params![workspace,session.id,session.updated_at.to_rfc3339()])?;
        tx.commit()?;
        Ok(inserted)
    }
}
