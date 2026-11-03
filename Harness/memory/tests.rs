//! Behavioral regression coverage for privacy, lifecycle and source integrity.

use super::*;

fn access(workspace: &str) -> MemoryAccess {
    MemoryAccess::local_management(workspace.into()).unwrap()
}

fn input(privacy: Privacy) -> SaveMemory {
    SaveMemory {
        kind: MemoryKind::Explicit,
        scope: MemoryScope::Workspace("a".into()),
        privacy,
        text: "Use SQLite for the persistent memory store".into(),
        tags: vec!["decision".into()],
        source: None,
    }
}

#[test]
fn privacy_scope_and_disabled_kinds_are_enforced_on_all_reads() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MemoryStore::open(dir.path()).unwrap();
    let shared = store.save(&access("a"), input(Privacy::Shared)).unwrap();
    let private = store.save(&access("a"), input(Privacy::LocalOnly)).unwrap();
    let consumer = MemoryAccess::shared_consumer("a".into()).unwrap();
    assert!(store.get(&consumer, &shared.id).is_err());
    let mut settings = store.settings("a").unwrap();
    assert!(!settings.explicit.enabled && !settings.whole_session.enabled);
    settings.explicit.enabled = true;
    store.update_settings(&access("a"), settings).unwrap();
    assert!(store.get(&consumer, &shared.id).is_ok());
    assert!(store.get(&consumer, &private.id).is_err());
    assert!(store.get(&access("b"), &shared.id).is_err());
    let page = store
        .list(
            &consumer,
            &MemoryFilter {
                query: Some("SQLite".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].id, shared.id);
    assert!(
        store
            .list(&access("b"), &MemoryFilter::default())
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
fn edits_privacy_moves_and_deletion_fence_stale_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MemoryStore::open(dir.path()).unwrap();
    let original = store.save(&access("a"), input(Privacy::Shared)).unwrap();
    let edited = store
        .edit(
            &access("a"),
            &original.id,
            1,
            "Prefer local embeddings".into(),
            vec![],
        )
        .unwrap();
    assert!(store.delete(&access("a"), &original.id, 1).is_err());
    let private = store
        .set_privacy(
            &access("a"),
            &original.id,
            edited.revision,
            Privacy::LocalOnly,
            false,
        )
        .unwrap();
    assert!(
        store
            .set_privacy(
                &access("a"),
                &private.id,
                private.revision,
                Privacy::Shared,
                false
            )
            .is_err()
    );
    drop(store);
    let mut store = MemoryStore::open(dir.path()).unwrap();
    assert_eq!(
        store.get(&access("a"), &private.id).unwrap().privacy,
        Privacy::LocalOnly
    );
    assert!(
        store
            .list(
                &access("a"),
                &MemoryFilter {
                    privacy: Some(Privacy::Shared),
                    ..Default::default()
                }
            )
            .unwrap()
            .records
            .is_empty()
    );
    let shared = store
        .set_privacy(
            &access("a"),
            &private.id,
            private.revision,
            Privacy::Shared,
            true,
        )
        .unwrap();
    store
        .delete(&access("a"), &shared.id, shared.revision)
        .unwrap();
    assert!(store.get(&access("a"), &shared.id).is_err());
    assert!(
        store
            .list(
                &access("a"),
                &MemoryFilter {
                    query: Some("embeddings".into()),
                    ..Default::default()
                }
            )
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
fn source_replay_preserves_overrides_and_suppresses_deleted_spans() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MemoryStore::open(dir.path()).unwrap();
    let mut source = input(Privacy::Shared);
    source.kind = MemoryKind::WholeSession;
    source.source = Some(MemorySource {
        session_id: "session".into(),
        window_id: "window".into(),
        source_revision: "1".into(),
        start: 10,
        end: 100,
    });
    let record = store.save(&access("a"), source.clone()).unwrap();
    let edited = store
        .edit(
            &access("a"),
            &record.id,
            1,
            "Corrected decision".into(),
            vec![],
        )
        .unwrap();
    let replay = store.save(&access("a"), source.clone()).unwrap();
    assert_eq!(replay.id, edited.id);
    assert_eq!(replay.text, edited.text);
    store
        .delete(&access("a"), &edited.id, edited.revision)
        .unwrap();
    assert!(store.save(&access("a"), source.clone()).is_err());
    let provenance = source.source.as_mut().unwrap();
    provenance.source_revision = "2".into();
    provenance.start = 50;
    provenance.end = 150;
    assert!(store.save(&access("a"), source).is_err());
}

#[test]
fn settings_cas_and_pagination_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MemoryStore::open(dir.path()).unwrap();
    let settings = store.settings("a").unwrap();
    store
        .update_settings(&access("a"), settings.clone())
        .unwrap();
    assert!(store.update_settings(&access("a"), settings).is_err());
    for _ in 0..4 {
        store.save(&access("a"), input(Privacy::Shared)).unwrap();
    }
    for _ in 0..3 {
        store.save(&access("a"), input(Privacy::LocalOnly)).unwrap();
    }
    let mut ids = Vec::new();
    let mut filter = MemoryFilter {
        limit: 2,
        ..Default::default()
    };
    loop {
        let page = store.list(&access("a"), &filter).unwrap();
        ids.extend(page.records.into_iter().map(|r| r.id));
        filter.after = page.next_cursor;
        if filter.after.is_none() {
            break;
        }
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 7);
    drop(store);
    assert_eq!(
        MemoryStore::open(dir.path())
            .unwrap()
            .settings("a")
            .unwrap()
            .revision,
        1
    );
}

#[test]
fn chunks_are_utf8_safe_cover_source_and_vectors_reject_invalid_values() {
    let text = "한글🙂 words\n".repeat(100);
    let chunks = chunk_text(&text, 100, 10).unwrap();
    let mut covered = 0;
    for chunk in &chunks {
        assert!(chunk.start <= covered);
        assert_eq!(&text[chunk.start..chunk.end], chunk.text);
        assert!(chunk.text.chars().count() <= 100);
        covered = covered.max(chunk.end);
    }
    assert_eq!(covered, text.len());
    assert!(validate_vectors(&[vec![f32::NAN]], 1, 1).is_err());
    assert!(validate_vectors(&[vec![0.0, 0.0]], 1, 2).is_err());
    assert!(validate_vectors(&[vec![1.0, 0.0]], 1, 2).is_ok());
}

#[test]
fn vector_spaces_are_immutable_and_late_results_cannot_revive_edits() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MemoryStore::open(dir.path()).unwrap();
    let record = store.save(&access("a"), input(Privacy::Shared)).unwrap();
    let profile = EmbeddingProfile {
        id: "fixture".into(),
        execution: EmbeddingExecution::Cloud,
        model: "fixture/model".into(),
        source: "endpoint-hash".into(),
        resolved_model: "model-v1".into(),
        dimensions: 2,
        preprocessing_revision: "plain-v1".into(),
    };
    store.register_profile(&access("a"), &profile).unwrap();
    store
        .put_vector(
            &access("a"),
            &record.id,
            record.revision,
            &profile,
            vec![1.0, 0.0],
        )
        .unwrap();
    let hits = store
        .recall(
            &access("a"),
            "unrelated paraphrase",
            Some((&profile, &[1.0, 0.1])),
            5,
        )
        .unwrap();
    assert_eq!(hits[0].record.id, record.id);
    let mid = record.text.len() / 2;
    store
        .put_vectors(
            &access("a"),
            &record.id,
            record.revision,
            &profile,
            vec![
                (
                    Chunk {
                        start: 0,
                        end: mid + 4,
                        text: record.text[..mid + 4].into(),
                    },
                    vec![1.0, 0.0],
                ),
                (
                    Chunk {
                        start: mid,
                        end: record.text.len(),
                        text: record.text[mid..].into(),
                    },
                    vec![0.9, 0.1],
                ),
            ],
        )
        .unwrap();
    assert_eq!(
        store
            .recall(
                &access("a"),
                "unrelated paraphrase",
                Some((&profile, &[1.0, 0.1])),
                5
            )
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .put_vectors(
                &access("a"),
                &record.id,
                record.revision,
                &profile,
                vec![(
                    Chunk {
                        start: 1,
                        end: record.text.len(),
                        text: record.text[1..].into()
                    },
                    vec![1.0, 0.0]
                ),]
            )
            .is_err()
    );
    let mut incompatible = profile.clone();
    incompatible.resolved_model = "model-v2".into();
    assert!(store.register_profile(&access("a"), &incompatible).is_err());
    assert!(
        store
            .recall(
                &access("a"),
                "unrelated paraphrase",
                Some((&incompatible, &[1.0, 0.1])),
                5
            )
            .unwrap()
            .is_empty()
    );
    let private = store.save(&access("a"), input(Privacy::LocalOnly)).unwrap();
    assert!(
        store
            .put_vector(
                &access("a"),
                &private.id,
                private.revision,
                &profile,
                vec![1.0, 0.0]
            )
            .is_err()
    );
    let edited = store
        .edit(
            &access("a"),
            &record.id,
            record.revision,
            "Corrected text".into(),
            vec![],
        )
        .unwrap();
    assert!(
        store
            .put_vector(
                &access("a"),
                &record.id,
                record.revision,
                &profile,
                vec![1.0, 0.0]
            )
            .is_err()
    );
    assert!(
        store
            .recall(
                &access("a"),
                "unrelated paraphrase",
                Some((&profile, &[1.0, 0.1])),
                5
            )
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.get(&access("a"), &edited.id).unwrap().indexing_status,
        "pending"
    );
}

#[test]
fn vector_schema_upgrade_preserves_canonical_text() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MemoryStore::open(dir.path()).unwrap();
    let record = store.save(&access("a"), input(Privacy::Shared)).unwrap();
    // Simulate the previous backend checkpoint; only derived vectors change.
    store
        .connection
        .execute_batch(
            "DROP TABLE vectors;
        CREATE TABLE vectors(id TEXT,revision INTEGER,fingerprint TEXT,vector TEXT);
        PRAGMA main.user_version=1;",
        )
        .unwrap();
    drop(store);
    let mut reopened = MemoryStore::open(dir.path()).unwrap();
    assert_eq!(
        reopened.get(&access("a"), &record.id).unwrap().text,
        record.text
    );
    assert_eq!(
        reopened
            .connection
            .query_row("PRAGMA main.user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    reopened
        .delete(&access("a"), &record.id, record.revision)
        .unwrap();
}
