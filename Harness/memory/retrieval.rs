//! Encoder-specific vector storage and bounded hybrid retrieval. No network
//! access: callers must authorize inputs before asking an embedding backend.

use super::{
    store::{authorize_kind, find_record},
    *,
};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryHit {
    pub record: MemoryRecord,
    pub rank_score: f64,
}

impl MemoryStore {
    /// Profiles are immutable encoder definitions. Install/register a new ID
    /// for a different fingerprint instead of overwriting a live vector space.
    pub fn register_profile(
        &mut self,
        access: &MemoryAccess,
        profile: &EmbeddingProfile,
    ) -> Result<()> {
        ensure!(access.management, "management authorization required");
        profile.fingerprint()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old: Option<String> = tx
            .query_row(
                "SELECT body FROM profiles WHERE id=?1",
                [&profile.id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(old) = old {
            let previous: EmbeddingProfile = serde_json::from_str(&old)?;
            ensure!(
                previous == *profile,
                "profile_immutable: use a new profile ID"
            );
        } else {
            tx.execute(
                "INSERT INTO profiles VALUES (?1,?2)",
                params![profile.id, serde_json::to_string(profile)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn profiles(&self) -> Result<Vec<EmbeddingProfile>> {
        let mut stmt = self
            .connection
            .prepare("SELECT body FROM profiles ORDER BY id")?;
        let bodies = stmt.query_map([], |r| r.get::<_, String>(0))?;
        bodies
            .map(|body| Ok(serde_json::from_str(&body?)?))
            .collect()
    }

    /// Commit only for the exact still-active revision. Edits, moves and
    /// deletions cannot be undone by a late embedding response.
    pub fn put_vector(
        &mut self,
        access: &MemoryAccess,
        id: &str,
        revision: u64,
        profile: &EmbeddingProfile,
        vector: Vec<f32>,
    ) -> Result<()> {
        ensure!(access.management, "management authorization required");
        validate_vectors(std::slice::from_ref(&vector), 1, profile.dimensions)?;
        let fingerprint = profile.fingerprint()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let registered: String = tx.query_row(
            "SELECT body FROM profiles WHERE id=?1",
            [&profile.id],
            |r| r.get(0),
        )?;
        ensure!(
            serde_json::from_str::<EmbeddingProfile>(&registered)? == *profile,
            "profile_mismatch"
        );
        let mut record =
            find_record(&tx, id)?.ok_or_else(|| anyhow::anyhow!("memory_not_found"))?;
        ensure!(
            access.allows(&record.scope, record.privacy),
            "memory_not_found"
        );
        ensure!(record.revision == revision, "revision_conflict");
        ensure!(
            record.privacy != Privacy::LocalOnly
                || profile.execution == EmbeddingExecution::ManagedLocal,
            "local_only_blocked"
        );
        let schema = record.privacy.schema();
        tx.execute(&format!("INSERT INTO {schema}.vectors VALUES (?1,?2,?3,?4) ON CONFLICT(id,fingerprint) DO UPDATE SET revision=excluded.revision,vector=excluded.vector"),
            params![id, i64::try_from(revision)?, fingerprint, serde_json::to_string(&vector)?])?;
        tx.execute(
            &format!("UPDATE {schema}.jobs SET status='ready' WHERE id=?1 AND revision=?2"),
            params![id, i64::try_from(revision)?],
        )?;
        record.indexing_status = "ready".into();
        tx.execute(
            &format!("UPDATE {schema}.records SET body=?1 WHERE id=?2 AND revision=?3"),
            params![
                serde_json::to_string(&record)?,
                id,
                i64::try_from(revision)?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Fuse ranked results, never compare similarity across encoders. The
    /// query vector must have been produced by this exact profile.
    pub fn recall(
        &self,
        access: &MemoryAccess,
        query: &str,
        semantic: Option<(&EmbeddingProfile, &[f32])>,
        limit: usize,
    ) -> Result<Vec<MemoryHit>> {
        let limit = limit.clamp(1, 20);
        let lexical = self.list(
            access,
            &MemoryFilter {
                query: Some(query.into()),
                limit: 100,
                ..Default::default()
            },
        )?;
        let mut hits: BTreeMap<String, MemoryHit> = BTreeMap::new();
        for (rank, record) in lexical.records.into_iter().enumerate() {
            hits.insert(
                record.id.clone(),
                MemoryHit {
                    record,
                    rank_score: 1.0 / (60.0 + rank as f64),
                },
            );
        }
        if let Some((profile, vector)) = semantic {
            validate_vectors(&[vector.to_vec()], 1, profile.dimensions)?;
            let fingerprint = profile.fingerprint()?;
            let mut candidates = Vec::new();
            for privacy in [Privacy::Shared, Privacy::LocalOnly] {
                if privacy == Privacy::LocalOnly
                    && (!access.management || profile.execution != EmbeddingExecution::ManagedLocal)
                {
                    continue;
                }
                let schema = privacy.schema();
                let mut stmt = self.connection.prepare(&format!(
                    "SELECT r.body,v.vector FROM {schema}.vectors v JOIN {schema}.records r ON r.id=v.id AND r.revision=v.revision
                     WHERE r.body IS NOT NULL AND (r.workspace=?1 OR r.workspace='') AND v.fingerprint=?2 LIMIT 10001"))?;
                let rows = stmt.query_map(params![access.workspace_id, fingerprint], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                let mut scanned = 0;
                for row in rows {
                    scanned += 1;
                    ensure!(
                        scanned <= 10000,
                        "semantic_search_capacity: narrow the corpus or rebuild a scalable index"
                    );
                    let (body, values) = row?;
                    let record: MemoryRecord = serde_json::from_str(&body)?;
                    if authorize_kind(&self.connection, access, record.kind).is_err() {
                        continue;
                    }
                    let values: Vec<f32> = serde_json::from_str(&values)?;
                    validate_vectors(std::slice::from_ref(&values), 1, profile.dimensions)?;
                    candidates.push((cosine(vector, &values), record));
                }
            }
            candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
            for (rank, (_, record)) in candidates.into_iter().take(100).enumerate() {
                hits.entry(record.id.clone())
                    .or_insert(MemoryHit {
                        record,
                        rank_score: 0.0,
                    })
                    .rank_score += 1.0 / (60.0 + rank as f64);
            }
        }
        let mut hits: Vec<_> = hits.into_values().collect();
        hits.sort_by(|a, b| {
            b.rank_score
                .total_cmp(&a.rank_score)
                .then(a.record.id.cmp(&b.record.id))
        });
        hits.truncate(limit);
        // Fence moves/deletions between candidate generation and return.
        Ok(hits
            .into_iter()
            .filter(|hit| {
                self.get(access, &hit.record.id)
                    .is_ok_and(|r| r.revision == hit.record.revision)
            })
            .collect())
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| f64::from(*x) * f64::from(*y))
        .sum();
    let norm = |v: &[f32]| v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
    dot / (norm(a) * norm(b))
}
