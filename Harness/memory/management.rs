//! Local-user management controller, separate from model tools and Remote
//! replay. Hosts render this state; canonical policy remains in MemoryStore.

use super::*;
use anyhow::{Context, Result, ensure};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

#[derive(Debug, Clone)]
pub enum MemoryTask {
    PrepareLocal { model: String, profile: String },
    ProbeCloud { model: String, profile: String },
    IndexLocal { profile: String },
    RemoveLocal { model: String },
    Reconcile,
}

pub struct LocalMemoryManager {
    directory: PathBuf,
    workspace: PathBuf,
    access: MemoryAccess,
    pub settings: MemorySettings,
    pub profiles: Vec<EmbeddingProfile>,
    pub page: MemoryPage,
    pub filter: MemoryFilter,
    pub status: String,
    pub busy: bool,
    cancel: Arc<AtomicBool>,
    updates: Option<mpsc::Receiver<Result<String, String>>>,
}

impl LocalMemoryManager {
    /// Construct only for an authorized LOCAL user surface. Never serialize the
    /// controller or put its records into a conversation or Remote state.
    pub fn open(directory: PathBuf, workspace: PathBuf) -> Result<Self> {
        let access =
            MemoryAccess::local_management(crate::session_store::ensure_workspace_id(&workspace)?)?;
        let mut manager = Self {
            directory,
            workspace,
            access,
            settings: Default::default(),
            profiles: vec![],
            page: MemoryPage {
                records: vec![],
                next_cursor: None,
            },
            filter: MemoryFilter {
                limit: 50,
                ..Default::default()
            },
            status: String::new(),
            busy: false,
            cancel: Arc::new(AtomicBool::new(false)),
            updates: None,
        };
        manager.refresh()?;
        Ok(manager)
    }
    fn store(&self) -> Result<MemoryStore> {
        MemoryStore::open(&self.directory.join("memory"))
    }
    pub fn refresh(&mut self) -> Result<()> {
        let store = self.store()?;
        self.settings = store.settings(&self.access.workspace_id)?;
        self.profiles = store.profiles()?;
        self.page = store.list(&self.access, &self.filter)?;
        Ok(())
    }
    pub fn toggle_enabled(&mut self, kind: MemoryKind) -> Result<()> {
        // Use the displayed revision. Never silently retry a stale toggle.
        let mut next = self.settings.clone();
        let setting = match kind {
            MemoryKind::Explicit => &mut next.explicit,
            MemoryKind::WholeSession => &mut next.whole_session,
        };
        setting.enabled = !setting.enabled;
        self.store()?.update_settings(&self.access, next)?;
        self.refresh()
    }
    pub fn cycle_shared_profile(&mut self, kind: MemoryKind) -> Result<()> {
        ensure!(
            !self.profiles.is_empty(),
            "Prepare or validate an embedding profile first"
        );
        let mut next = self.settings.clone();
        let setting = match kind {
            MemoryKind::Explicit => &mut next.explicit,
            MemoryKind::WholeSession => &mut next.whole_session,
        };
        let index = setting
            .shared_profile_id
            .as_ref()
            .and_then(|id| self.profiles.iter().position(|p| &p.id == id))
            .map_or(0, |i| (i + 1) % self.profiles.len());
        setting.shared_profile_id = Some(self.profiles[index].id.clone());
        self.store()?.update_settings(&self.access, next)?;
        self.refresh()
    }
    pub fn save(&mut self, existing: Option<&MemoryRecord>, text: String) -> Result<()> {
        let mut store = self.store()?;
        if let Some(record) = existing {
            store.edit(
                &self.access,
                &record.id,
                record.revision,
                text,
                record.tags.clone(),
            )?;
        } else {
            store.save(
                &self.access,
                SaveMemory {
                    kind: MemoryKind::Explicit,
                    scope: MemoryScope::Workspace(self.access.workspace_id.clone()),
                    privacy: Privacy::Shared,
                    text,
                    tags: vec![],
                    source: None,
                },
            )?;
        }
        self.refresh()
    }
    pub fn delete(&mut self, targets: &[(String, u64)]) -> Result<()> {
        self.store()?.delete_many(&self.access, targets)?;
        self.refresh()
    }
    pub fn privacy(
        &mut self,
        record: &MemoryRecord,
        privacy: Privacy,
        confirmed: bool,
    ) -> Result<()> {
        self.store()?.set_privacy(
            &self.access,
            &record.id,
            record.revision,
            privacy,
            confirmed,
        )?;
        self.refresh()
    }
    pub fn cancel_task(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn poll(&mut self) {
        let update = self.updates.as_ref().map(mpsc::Receiver::try_recv);
        match update {
            Some(Ok(result)) => {
                self.status = result.unwrap_or_else(|error| format!("Failed: {error}"));
                self.busy = false;
                self.updates = None;
                if let Err(error) = self.refresh() {
                    self.status = format!("Refresh failed: {error}");
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.status = "Task interrupted".into();
                self.busy = false;
                self.updates = None;
            }
            _ => {}
        }
    }
    /// Caller obtains explicit download/cloud/removal consent before invoking.
    /// This task never runs in the renderer or on the input thread.
    pub fn start_task(&mut self, task: MemoryTask) -> Result<()> {
        ensure!(!self.busy, "A memory task is already running");
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let directory = self.directory.clone();
        let workspace = self.workspace.clone();
        let access = self.access.clone();
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("memory-management".into())
            .spawn(move || {
                let result = run_task(task, &directory, &workspace, &access, &cancel)
                    .map_err(|error| error.to_string());
                let _ = tx.send(result);
            })?;
        self.updates = Some(rx);
        self.busy = true;
        self.status = "Working… (Esc closes and cancels)".into();
        Ok(())
    }
}

impl Drop for LocalMemoryManager {
    fn drop(&mut self) {
        self.cancel_task();
    }
}

fn run_task(
    task: MemoryTask,
    directory: &std::path::Path,
    workspace: &std::path::Path,
    access: &MemoryAccess,
    cancel: &AtomicBool,
) -> Result<String> {
    let models = ManagedModels::new(directory.join("memory-models"));
    let mut store = MemoryStore::open(&directory.join("memory"))?;
    match task {
        MemoryTask::PrepareLocal { model, profile } => {
            models.install(&model, cancel, &mut |_, _, _| {})?;
            ensure!(!cancel.load(Ordering::Acquire), "cancelled");
            let encoder = LocalEmbedder::load(&models, &model, profile)?;
            let vectors = encoder.embed(
                &[
                    "A puppy plays with a dog.".into(),
                    "A dog and puppy are playing.".into(),
                    "Database transaction durability.".into(),
                ],
                cancel,
            )?;
            let cosine = |a: usize, b: usize| {
                vectors[a]
                    .iter()
                    .zip(&vectors[b])
                    .map(|(x, y)| f64::from(*x) * f64::from(*y))
                    .sum::<f64>()
            };
            ensure!(cosine(0, 1) > cosine(0, 2), "retrieval validation failed");
            store.register_profile(access, encoder.profile())?;
            Ok(format!(
                "Ready: {} · {} dimensions · offline CPU",
                encoder.profile().id,
                encoder.profile().dimensions
            ))
        }
        MemoryTask::ProbeCloud { model, profile } => {
            ensure!(!cancel.load(Ordering::Acquire), "cancelled");
            let bridge = crate::core::BridgeClient::start_for_workspace(workspace)?;
            let profile = probe_cloud_profile(&bridge, profile, model, cancel)?;
            store.register_profile(access, &profile)?;
            Ok(format!(
                "Validated {} (fixture only; cloud memory dispatch remains gated)",
                profile.id
            ))
        }
        MemoryTask::RemoveLocal { model } => {
            ensure!(!cancel.load(Ordering::Acquire), "cancelled");
            models.remove(&model)?;
            Ok("Model removed; canonical memories preserved".into())
        }
        MemoryTask::IndexLocal { profile } => {
            let profile = store
                .profiles()?
                .into_iter()
                .find(|p| p.id == profile && p.execution == EmbeddingExecution::ManagedLocal)
                .context("Choose a validated managed-local profile first")?;
            let model = local_model_catalog()
                .iter()
                .find(|m| m.repository == profile.model && m.revision == profile.resolved_model)
                .context("Unsupported managed model")?;
            let encoder = LocalEmbedder::load(&models, model.id, profile.id.clone())?;
            ensure!(encoder.profile() == &profile, "encoder fingerprint changed");
            let mut filter = MemoryFilter {
                limit: 100,
                ..Default::default()
            };
            let mut indexed = 0;
            let mut blocked = 0;
            loop {
                let page = store.list(access, &filter)?;
                for record in page.records {
                    ensure!(!cancel.load(Ordering::Acquire), "cancelled");
                    let result = (|| -> Result<()> {
                        let chunks = encoder.chunks(&record.text)?;
                        let mut leaves = vec![];
                        for batch in chunks.chunks(64) {
                            let inputs = batch
                                .iter()
                                .map(|chunk| chunk.text.clone())
                                .collect::<Vec<_>>();
                            leaves
                                .extend(batch.iter().cloned().zip(encoder.embed(&inputs, cancel)?));
                        }
                        store.put_vectors(access, &record.id, record.revision, &profile, leaves)
                    })();
                    if result.is_ok() {
                        indexed += 1;
                    } else {
                        blocked += 1;
                    }
                }
                filter.after = page.next_cursor;
                if filter.after.is_none() {
                    break;
                }
            }
            Ok(format!(
                "Indexed {indexed}; blocked/stale {blocked}. Text preserved. Agent recall currently lexical."
            ))
        }
        MemoryTask::Reconcile => {
            let captured = store.reconcile_workspace(
                &crate::session_store::SessionStore::new(directory),
                workspace,
                cancel,
            )?;
            Ok(format!(
                "Captured {captured} eligible source chunks; no historical backfill"
            ))
        }
    }
}
