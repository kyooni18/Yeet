//! Legacy layout readers and opportunistic migration.
//!
//! Current readers understand every legacy layout, so migration is best-effort
//! and never blocks startup behind a contended store lock.

use super::*;

pub(super) fn cleanup_legacy_layout(root: &Path) -> Result<()> {
    remove_file_if_exists(&root.join("current"))?;
    remove_file_if_exists(&root.join("tasks.json"))?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if matches!(name, TASKS_DIR | DEBATES_DIR | RUNS_DIR) {
            continue;
        }
        let looks_like_revision = path.join("metadata.json").is_file()
            && path.join("conversation.json").is_file()
            && path.join("model-history.json").is_file();
        if looks_like_revision {
            fs::remove_dir_all(path)?;
        }
    }
    Ok(())
}

impl SessionStore {
    pub(super) fn load_semantic_layout_legacy(&self, root: &Path) -> Result<StoredSession> {
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("metadata.json"))?)?;
        value["conversation"] = serde_json::from_slice(&fs::read(root.join("conversation.json"))?)?;
        value["modelHistory"] =
            serde_json::from_slice(&fs::read(root.join("model-history.json"))?)?;
        let runs = root.join(RUNS_DIR).join("index.json");
        if runs.exists() {
            value["runs"] = serde_json::from_slice(&fs::read(runs)?)?;
        }
        let knowledge = root.join(DEBATES_DIR).join("knowledge.json");
        if knowledge.exists() {
            value["retainedDebateKnowledge"] = serde_json::from_slice(&fs::read(knowledge)?)?;
        }
        let debate_path = root.join(DEBATES_DIR).join("state.json");
        if debate_path.exists() {
            value["debate"] = serde_json::from_slice(&fs::read(debate_path)?)?;
        }
        Ok(serde_json::from_value(value)?)
    }

    pub(super) fn load_revision_layout(&self, root: &Path) -> Result<StoredSession> {
        let revision = fs::read_to_string(root.join("current"))?;
        validate_id(&revision)?;
        let snapshot = root.join(revision);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(snapshot.join("metadata.json"))?)?;
        value["conversation"] =
            serde_json::from_slice(&fs::read(snapshot.join("conversation.json"))?)?;
        value["modelHistory"] =
            serde_json::from_slice(&fs::read(snapshot.join("model-history.json"))?)?;
        Ok(serde_json::from_value(value)?)
    }

    pub(super) fn migrate_legacy_layouts(&self) {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return;
        };
        let entries = entries
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();

        // Revision-directory sessions first.
        for root in entries.iter().filter(|path| path.is_dir()) {
            if root.join("metadata.json").exists() {
                if root.join(MANIFEST_FILE).is_file() {
                    if let Ok(bytes) = fs::read(root.join(MANIFEST_FILE))
                        && let Ok(manifest) = serde_json::from_slice::<SemanticManifest>(&bytes)
                    {
                        let _ = materialize_manifest(root, &manifest);
                        let _ = gc_semantic_objects(root, &manifest);
                    }
                } else if let Ok(session) = self.load_semantic_layout_legacy(root) {
                    let _ = self.save_unlocked(&session);
                }
                let _ = self.migrate_legacy_runtime_log(root);
                let _ = cleanup_legacy_layout(root);
                continue;
            }
            if !root.join("current").exists() {
                continue;
            }
            let Ok(session) = self.load_revision_layout(root) else {
                continue;
            };
            if self.save_unlocked(&session).is_err() {
                continue;
            }
        }

        // Old single-file sessions become folders too.
        for path in entries.iter().filter(|path| {
            path.is_file() && path.extension().and_then(|value| value.to_str()) == Some("json")
        }) {
            let Some(id) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if validate_id(id).is_err() {
                continue;
            }
            let Ok(data) = fs::read(path) else {
                continue;
            };
            let Ok(session) = serde_json::from_slice::<StoredSession>(&data) else {
                continue;
            };
            if session.id != id {
                continue;
            }
            let root = self.directory.join(id);
            if !root.join("metadata.json").exists() && self.save_unlocked(&session).is_err() {
                continue;
            }
            let _ = fs::remove_file(path);
        }
    }

    /// Stamps pre-identity sessions with their workspace's identity while the
    /// recorded folder still exists, so they follow later moves.
    pub(super) fn backfill_workspace_ids(&self) {
        let Ok(records) = self.scan_records() else {
            return;
        };
        for record in records
            .iter()
            .filter(|record| record.workspace_id.is_none())
        {
            let Ok(root) = Path::new(&record.workspace_root).canonicalize() else {
                continue;
            };
            if !root.is_dir() {
                continue;
            }
            let Ok(workspace_id) = ensure_workspace_id(&root) else {
                continue;
            };
            let Ok(mut session) = self.load_unlocked(&record.id) else {
                continue;
            };
            session.workspace_id = Some(workspace_id);
            session.working_directory = session
                .working_directory
                .map(|path| workspace_relative_path(&root, &path));
            for path in &mut session.context_roots {
                *path = workspace_relative_path(&root, path);
            }
            let _ = self.save_unlocked(&session);
        }
    }

    pub(super) fn migrate_legacy_runtime_log(&self, root: &Path) -> Result<()> {
        use std::io::Write;

        let legacy = root.join("runtime.jsonl");
        if !legacy.exists() {
            return Ok(());
        }
        let target_directory = root.join(TASKS_DIR);
        create_private_dir(&target_directory)?;
        let target = target_directory.join("events.jsonl");
        if target.exists() {
            let data = fs::read(&legacy)?;
            let mut options = fs::OpenOptions::new();
            options.append(true);
            let mut file = options.open(&target)?;
            file.write_all(&data)?;
            file.flush()?;
        } else {
            fs::rename(&legacy, &target)?;
        }
        remove_file_if_exists(&legacy)?;
        Ok(())
    }
}
