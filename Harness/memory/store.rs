//! Canonical records and atomic lifecycle operations in private SQLite stores.
//! Attached rollback-journal databases let privacy moves commit atomically.

use std::{fs, path::Path, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::*;
use crate::platform::{set_private_directory, set_private_file};

pub struct MemoryStore {
    pub(super) connection: Connection,
}

impl MemoryStore {
    /// Open only in an application-owned directory. This never starts a model,
    /// provider process, download or network request.
    pub fn open(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory).context("create memory directory")?;
        set_private_directory(directory)?;
        let shared = directory.join("shared.sqlite3");
        let protected = directory.join("local-only.sqlite3");
        // Pre-create with private modes before SQLite can write content.
        for path in [&shared, &protected] {
            let file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            set_private_file(path)?;
            drop(file);
        }
        let mut connection = Connection::open(&shared)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute(
            "ATTACH DATABASE ?1 AS protected",
            [protected.to_string_lossy().as_ref()],
        )?;
        for schema in ["main", "protected"] {
            // Do not enable WAL: atomic attached-DB commits require journals.
            connection.execute_batch(&format!(
                "PRAGMA {schema}.journal_mode=DELETE; PRAGMA {schema}.synchronous=FULL;
                 PRAGMA {schema}.secure_delete=ON;"
            ))?;
            let version: i64 =
                connection.query_row(&format!("PRAGMA {schema}.user_version"), [], |r| r.get(0))?;
            ensure!(
                version <= 3,
                "memory database requires a newer Yeet version"
            );
        }
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for schema in ["main", "protected"] {
            let version: i64 =
                tx.query_row(&format!("PRAGMA {schema}.user_version"), [], |r| r.get(0))?;
            if version == 1 {
                // Vectors are rebuildable. Preserve canonical records and all
                // source suppressions while upgrading to per-leaf embeddings.
                tx.execute_batch(&format!("DROP TABLE {schema}.vectors;
                    UPDATE {schema}.records SET body=json_set(body,'$.indexingStatus','pending') WHERE body IS NOT NULL;
                    UPDATE {schema}.jobs SET status='pending';"))?;
            }
            tx.execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {schema}.records (
                    id TEXT PRIMARY KEY, revision INTEGER NOT NULL, workspace TEXT NOT NULL,
                    kind TEXT NOT NULL, body TEXT, source_key TEXT UNIQUE);
                 CREATE INDEX IF NOT EXISTS {schema}.records_scope ON records(workspace,kind,id);
                 CREATE VIRTUAL TABLE IF NOT EXISTS {schema}.search USING fts5(id UNINDEXED,text);
                 CREATE TABLE IF NOT EXISTS {schema}.suppressions (
                    workspace TEXT NOT NULL, session TEXT NOT NULL, window TEXT NOT NULL,
                    start INTEGER NOT NULL, end INTEGER NOT NULL,
                    PRIMARY KEY(workspace,session,window,start,end));
                 CREATE TABLE IF NOT EXISTS {schema}.vectors (
                    id TEXT NOT NULL, revision INTEGER NOT NULL, fingerprint TEXT NOT NULL,
                    start INTEGER NOT NULL, end INTEGER NOT NULL,
                    vector TEXT NOT NULL, PRIMARY KEY(id,fingerprint,start,end));
                 CREATE TABLE IF NOT EXISTS {schema}.jobs (
                    id TEXT PRIMARY KEY, revision INTEGER NOT NULL, status TEXT NOT NULL);
                 PRAGMA {schema}.user_version=3;"
            ))?;
        }
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS settings(workspace TEXT PRIMARY KEY, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS profiles(id TEXT PRIMARY KEY, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS capture_epochs(workspace TEXT PRIMARY KEY, since TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS captured_sessions(workspace TEXT NOT NULL, session TEXT NOT NULL,
                 updated_at TEXT NOT NULL, PRIMARY KEY(workspace,session));",
        )?;
        tx.commit()?;
        Ok(Self { connection })
    }

    pub fn settings(&self, workspace_id: &str) -> Result<MemorySettings> {
        read_settings(&self.connection, workspace_id)
    }

    pub fn update_settings(
        &mut self,
        access: &MemoryAccess,
        mut next: MemorySettings,
    ) -> Result<MemorySettings> {
        ensure!(access.management, "management authorization required");
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_settings(&tx, &access.workspace_id)?;
        ensure!(current.revision == next.revision, "revision_conflict");
        if next.whole_session.enabled && !current.whole_session.enabled {
            tx.execute("INSERT INTO capture_epochs VALUES (?1,?2) ON CONFLICT(workspace) DO UPDATE SET since=excluded.since",
                params![access.workspace_id, Utc::now().to_rfc3339()])?;
        }
        next.revision += 1;
        tx.execute("INSERT INTO settings VALUES (?1,?2) ON CONFLICT(workspace) DO UPDATE SET body=excluded.body",
            params![access.workspace_id, serde_json::to_string(&next)?])?;
        tx.commit()?;
        Ok(next)
    }

    pub fn save(&mut self, access: &MemoryAccess, input: SaveMemory) -> Result<MemoryRecord> {
        input.validate()?;
        ensure!(access.allows(&input.scope, input.privacy), "scope_denied");
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        authorize_kind(&tx, access, input.kind)?;
        if !access.management {
            ensure!(
                input.scope == MemoryScope::Workspace(access.workspace_id.clone()),
                "scope_denied"
            );
            ensure!(
                !read_settings(&tx, &access.workspace_id)?
                    .kind(input.kind)
                    .local_only,
                "protected_capture_unavailable: local-only capture requires guarded local execution"
            );
        }
        let source_key = input
            .source
            .as_ref()
            .map(|s| {
                serde_json::to_vec(&(input.scope.key(), input.kind.key(), s))
                    .map(|v| format!("{:x}", Sha256::digest(v)))
            })
            .transpose()?;
        if let Some(key) = &source_key {
            for privacy in [Privacy::Shared, Privacy::LocalOnly] {
                let existing: Option<String> = tx
                    .query_row(
                        &format!(
                            "SELECT body FROM {}.records WHERE source_key=?1",
                            privacy.schema()
                        ),
                        [key],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                if let Some(body) = existing {
                    let record: MemoryRecord = serde_json::from_str(&body)?;
                    ensure!(access.allows(&record.scope, record.privacy), "scope_denied");
                    tx.commit()?;
                    return Ok(record);
                }
            }
        }
        if let Some(source) = &input.source {
            ensure!(!suppressed(&tx, &input.scope, source)?, "source_suppressed");
        }
        let now = Utc::now().to_rfc3339();
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
        insert_record(&tx, &record, source_key.as_deref())?;
        tx.commit()?;
        Ok(record)
    }

    pub fn get(&self, access: &MemoryAccess, id: &str) -> Result<MemoryRecord> {
        let record = find_record(&self.connection, id)?.context("memory_not_found")?;
        // Same response for missing and unauthorized IDs.
        ensure!(
            access.allows(&record.scope, record.privacy),
            "memory_not_found"
        );
        authorize_kind(&self.connection, access, record.kind)?;
        Ok(record)
    }

    pub fn list(&self, access: &MemoryAccess, filter: &MemoryFilter) -> Result<MemoryPage> {
        ensure!(
            filter.query.as_ref().is_none_or(|q| q.len() <= 1024),
            "query_too_large"
        );
        let limit = if filter.limit == 0 {
            50
        } else {
            filter.limit.clamp(1, 100)
        };
        let mut records = Vec::new();
        let settings = read_settings(&self.connection, &access.workspace_id)?;
        let query = filter.query.as_deref().map(fts_query).transpose()?;
        for privacy in [Privacy::Shared, Privacy::LocalOnly] {
            if filter.privacy.is_some_and(|p| p != privacy)
                || (privacy == Privacy::LocalOnly && !access.management)
            {
                continue;
            }
            let schema = privacy.schema();
            let sql = format!(
                "SELECT r.body FROM {schema}.records r WHERE r.body IS NOT NULL
                 AND (r.workspace=?1 OR r.workspace='') AND r.id>?2
                 AND (?3 IS NULL OR r.kind=?3)
                 AND (?4=1 OR (r.kind='explicit' AND ?5=1) OR (r.kind='whole-session' AND ?6=1))
                 AND (?7 IS NULL OR r.id IN (SELECT id FROM {schema}.search WHERE search MATCH ?7))
                 ORDER BY r.id LIMIT ?8"
            );
            let mut stmt = self.connection.prepare(&sql)?;
            let bodies = stmt.query_map(
                params![
                    access.workspace_id,
                    filter.after.as_deref().unwrap_or(""),
                    filter.kind.map(|k| k.key()),
                    access.management,
                    settings.explicit.enabled,
                    settings.whole_session.enabled,
                    query,
                    (limit + 1) as i64
                ],
                |r| r.get::<_, String>(0),
            )?;
            for body in bodies {
                records.push(serde_json::from_str::<MemoryRecord>(&body?)?);
            }
        }
        records.sort_by(|a, b| a.id.cmp(&b.id));
        let more = records.len() > limit;
        records.truncate(limit);
        let next_cursor = more.then(|| records.last().unwrap().id.clone());
        Ok(MemoryPage {
            records,
            next_cursor,
        })
    }

    /// Corrections replace canonical text, not the transcript. The source key
    /// remains stable so reingestion returns the override instead of old text.
    pub fn edit(
        &mut self,
        access: &MemoryAccess,
        id: &str,
        revision: u64,
        text: String,
        tags: Vec<String>,
    ) -> Result<MemoryRecord> {
        ensure!(access.management, "management authorization required");
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut record = managed_record(&tx, access, id, revision)?;
        SaveMemory {
            kind: record.kind,
            scope: record.scope.clone(),
            privacy: record.privacy,
            text: text.clone(),
            tags: tags.clone(),
            source: record.source.clone(),
        }
        .validate()?;
        record.text = text;
        record.tags = tags;
        record.edited = true;
        record.revision += 1;
        record.updated_at = Utc::now().to_rfc3339();
        record.indexing_status = "pending".into();
        suppress_source(&tx, &record, Some(&record.id))?;
        replace_record(&tx, &record)?;
        tx.commit()?;
        Ok(record)
    }

    pub fn delete(&mut self, access: &MemoryAccess, id: &str, revision: u64) -> Result<()> {
        ensure!(access.management, "management authorization required");
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = managed_record(&tx, access, id, revision)?;
        suppress_source(&tx, &record, None)?;
        clear_derived(&tx, record.privacy, id)?;
        tx.execute(
            &format!(
                "UPDATE {}.records SET body=NULL,revision=revision+1 WHERE id=?1",
                record.privacy.schema()
            ),
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Narrow agent deletion: no elevated management authority, scope widening,
    /// session suppression or protected access, even when given a guessed ID.
    pub fn forget_explicit(
        &mut self,
        access: &MemoryAccess,
        id: &str,
        revision: u64,
    ) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = find_record(&tx, id)?.context("memory_not_found")?;
        ensure!(
            record.kind == MemoryKind::Explicit
                && record.privacy == Privacy::Shared
                && record.scope == MemoryScope::Workspace(access.workspace_id.clone()),
            "memory_not_found"
        );
        authorize_kind(&tx, access, record.kind)?;
        ensure!(record.revision == revision, "revision_conflict");
        suppress_source(&tx, &record, None)?;
        clear_derived(&tx, record.privacy, id)?;
        tx.execute(
            "UPDATE main.records SET body=NULL,revision=revision+1 WHERE id=?1",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Privacy downgrade requires an explicit local user action for an exact
    /// revision. This API is not exposed to agents or untrusted Remote clients.
    pub fn set_privacy(
        &mut self,
        access: &MemoryAccess,
        id: &str,
        revision: u64,
        privacy: Privacy,
        confirm_declassification: bool,
    ) -> Result<MemoryRecord> {
        ensure!(access.management, "management authorization required");
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut record = managed_record(&tx, access, id, revision)?;
        if record.privacy == privacy {
            return Ok(record);
        }
        ensure!(
            privacy != Privacy::Shared || confirm_declassification,
            "consent_required"
        );
        if privacy == Privacy::LocalOnly && record.source.is_some() {
            // Protect the entire source episode across rechunking/revisions,
            // not just this leaf. Otherwise a later transcript edit could
            // silently republish overlapping protected text as shared memory.
            let mut envelope = record.clone();
            if let Some(source) = &mut envelope.source {
                source.start = 0;
                source.end = i64::MAX as u64;
            }
            suppress_source(&tx, &envelope, Some(&record.id))?;
        }
        // Atomic attached rollback-journal commit: no crash can publish both
        // a stale shared copy and the protected revision.
        let key: Option<String> = tx.query_row(
            &format!(
                "SELECT source_key FROM {}.records WHERE id=?1",
                record.privacy.schema()
            ),
            [id],
            |r| r.get(0),
        )?;
        clear_derived(&tx, record.privacy, id)?;
        tx.execute(
            &format!(
                "DELETE FROM {}.records WHERE id=?1",
                record.privacy.schema()
            ),
            [id],
        )?;
        record.privacy = privacy;
        record.revision += 1;
        record.updated_at = Utc::now().to_rfc3339();
        record.indexing_status = "pending".into();
        insert_record(&tx, &record, key.as_deref())?;
        tx.commit()?;
        Ok(record)
    }
}

pub(super) fn read_settings(connection: &Connection, workspace: &str) -> Result<MemorySettings> {
    let body: Option<String> = connection
        .query_row(
            "SELECT body FROM settings WHERE workspace=?1",
            [workspace],
            |r| r.get(0),
        )
        .optional()?;
    Ok(body
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_default())
}

pub(super) fn authorize_kind(
    connection: &Connection,
    access: &MemoryAccess,
    kind: MemoryKind,
) -> Result<()> {
    ensure!(
        access.management
            || read_settings(connection, &access.workspace_id)?
                .kind(kind)
                .enabled,
        "memory_disabled"
    );
    Ok(())
}

pub(super) fn find_record(connection: &Connection, id: &str) -> Result<Option<MemoryRecord>> {
    for privacy in [Privacy::Shared, Privacy::LocalOnly] {
        let body: Option<String> = connection
            .query_row(
                &format!("SELECT body FROM {}.records WHERE id=?1", privacy.schema()),
                [id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if let Some(body) = body {
            return Ok(Some(serde_json::from_str(&body)?));
        }
    }
    Ok(None)
}

fn managed_record(
    connection: &Connection,
    access: &MemoryAccess,
    id: &str,
    revision: u64,
) -> Result<MemoryRecord> {
    let record = find_record(connection, id)?.context("memory_not_found")?;
    ensure!(
        access.allows(&record.scope, record.privacy),
        "memory_not_found"
    );
    ensure!(record.revision == revision, "revision_conflict");
    Ok(record)
}

/// Suppress rechunked/revised copies and immediately invalidate already indexed
/// overlapping chunks. Keep only the exact edited override, if one was supplied.
fn suppress_source(
    tx: &Transaction<'_>,
    record: &MemoryRecord,
    keep_id: Option<&str>,
) -> Result<()> {
    let Some(source) = &record.source else {
        return Ok(());
    };
    for privacy in [Privacy::Shared, Privacy::LocalOnly] {
        let schema = privacy.schema();
        tx.execute(
            &format!("INSERT OR IGNORE INTO {schema}.suppressions VALUES (?1,?2,?3,?4,?5)"),
            params![
                record.scope.key(),
                source.session_id,
                source.window_id,
                i64::try_from(source.start)?,
                i64::try_from(source.end)?
            ],
        )?;
        let mut stmt = tx.prepare(&format!(
            "SELECT id FROM {schema}.records WHERE workspace=?1 AND body IS NOT NULL
             AND json_extract(body,'$.source.sessionId')=?2 AND json_extract(body,'$.source.windowId')=?3
             AND json_extract(body,'$.source.start')<?5 AND json_extract(body,'$.source.end')>?4"))?;
        let ids: Vec<String> = stmt
            .query_map(
                params![
                    record.scope.key(),
                    source.session_id,
                    source.window_id,
                    i64::try_from(source.start)?,
                    i64::try_from(source.end)?
                ],
                |r| r.get(0),
            )?
            .collect::<rusqlite::Result<_>>()?;
        for id in ids {
            if keep_id == Some(id.as_str()) {
                continue;
            }
            clear_derived(tx, privacy, &id)?;
            tx.execute(
                &format!("UPDATE {schema}.records SET body=NULL,revision=revision+1 WHERE id=?1"),
                [id],
            )?;
        }
    }
    Ok(())
}

pub(super) fn suppressed(
    connection: &Connection,
    scope: &MemoryScope,
    source: &MemorySource,
) -> Result<bool> {
    for privacy in [Privacy::Shared, Privacy::LocalOnly] {
        let found: bool = connection.query_row(&format!(
            "SELECT EXISTS(SELECT 1 FROM {}.suppressions WHERE workspace=?1 AND session=?2 AND window=?3 AND start<?5 AND end>?4)", privacy.schema()),
            params![scope.key(), source.session_id, source.window_id, i64::try_from(source.start)?, i64::try_from(source.end)?], |r| r.get(0))?;
        if found {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn insert_record(
    tx: &Transaction<'_>,
    record: &MemoryRecord,
    source_key: Option<&str>,
) -> Result<()> {
    let schema = record.privacy.schema();
    tx.execute(
        &format!("INSERT INTO {schema}.records VALUES (?1,?2,?3,?4,?5,?6)"),
        params![
            record.id,
            i64::try_from(record.revision)?,
            record.scope.key(),
            record.kind.key(),
            serde_json::to_string(record)?,
            source_key
        ],
    )?;
    insert_derived(tx, record)
}

fn replace_record(tx: &Transaction<'_>, record: &MemoryRecord) -> Result<()> {
    clear_derived(tx, record.privacy, &record.id)?;
    tx.execute(
        &format!(
            "UPDATE {}.records SET body=?1,revision=?2 WHERE id=?3",
            record.privacy.schema()
        ),
        params![
            serde_json::to_string(record)?,
            i64::try_from(record.revision)?,
            record.id
        ],
    )?;
    insert_derived(tx, record)
}

fn insert_derived(tx: &Transaction<'_>, record: &MemoryRecord) -> Result<()> {
    let schema = record.privacy.schema();
    tx.execute(
        &format!("INSERT INTO {schema}.search VALUES (?1,?2)"),
        params![record.id, record.text],
    )?;
    tx.execute(
        &format!("INSERT INTO {schema}.jobs VALUES (?1,?2,'pending')"),
        params![record.id, i64::try_from(record.revision)?],
    )?;
    Ok(())
}

pub(super) fn clear_derived(tx: &Transaction<'_>, privacy: Privacy, id: &str) -> Result<()> {
    for table in ["search", "vectors", "jobs"] {
        tx.execute(
            &format!("DELETE FROM {}.{table} WHERE id=?1", privacy.schema()),
            [id],
        )?;
    }
    Ok(())
}

fn fts_query(query: &str) -> Result<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .take(32)
        .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
        .collect();
    if terms.is_empty() {
        bail!("empty_search_query");
    }
    Ok(terms.join(" AND "))
}
