//! Platform-neutral scheduled agent jobs. SQLite transactions serialize UI
//! mutations and claims; the daemon holds a process-wide scheduler lease.
use crate::{config::ConfigStore, harness::Harness, model::FrontendCommand};
use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    At {
        at: DateTime<Utc>,
    },
    Interval {
        seconds: u64,
    },
    /// Resolved from provider telemetry when saved. Intentionally one-shot.
    QuotaReset,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDraft {
    pub name: String,
    pub prompt: String,
    pub folder: PathBuf,
    pub model: String,
    pub schedule: Schedule,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub draft: TaskDraft,
    pub enabled: bool,
    pub next_run: Option<DateTime<Utc>>,
    pub running: bool,
    pub last_run: Option<DateTime<Utc>>,
    pub last_result: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TaskAction {
    Save {
        id: Option<String>,
        draft: TaskDraft,
    },
    SetEnabled {
        id: String,
        enabled: bool,
    },
    Delete {
        id: String,
    },
}

pub struct TaskStore {
    db: Connection,
}
impl TaskStore {
    pub fn open() -> Result<Self> {
        Self::in_directory(&ConfigStore::default().directory)
    }
    pub fn in_directory(directory: &Path) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        let db = Connection::open(directory.join("tasks.sqlite3"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS scheduled_tasks (id TEXT PRIMARY KEY, body TEXT NOT NULL)",
        )?;
        Ok(Self { db })
    }
    pub fn list(&self) -> Result<Vec<Task>> {
        let mut statement = self
            .db
            .prepare("SELECT body FROM scheduled_tasks ORDER BY rowid")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
    fn write(db: &Connection, task: &Task) -> Result<()> {
        db.execute("INSERT INTO scheduled_tasks(id, body) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET body=excluded.body", params![task.id, serde_json::to_string(task)?])?;
        Ok(())
    }
    pub fn apply(
        &mut self,
        action: TaskAction,
        quota_reset: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let get = |id: &str| -> Result<Task> {
            let body: String = tx
                .query_row("SELECT body FROM scheduled_tasks WHERE id=?1", [id], |r| {
                    r.get(0)
                })
                .map_err(|_| anyhow!("Task no longer exists"))?;
            Ok(serde_json::from_str(&body)?)
        };
        match action {
            TaskAction::Save { id, mut draft } => {
                draft.name = draft.name.trim().to_owned();
                if draft.name.is_empty() || draft.name.len() > 200 {
                    bail!("Name must be 1–200 bytes");
                }
                if draft.prompt.trim().is_empty()
                    || draft.prompt.len() > 100_000
                    || draft.prompt.trim_start().starts_with('/')
                {
                    bail!("Enter an agent prompt (not a slash command), up to 100 KB");
                }
                if !draft.model.contains('/') || draft.model.len() > 500 {
                    bail!("Model must be provider/model");
                }
                draft.folder = draft.folder.canonicalize()?;
                if !draft.folder.is_dir() {
                    bail!("Folder must be a directory");
                }
                let next = match draft.schedule {
                    Schedule::At { at } if at > now => at,
                    Schedule::Interval { seconds } if (1..=315_360_000).contains(&seconds) => {
                        now + chrono::Duration::seconds(seconds as i64)
                    }
                    Schedule::QuotaReset => {
                        quota_reset.filter(|at| *at > now).ok_or_else(|| {
                            anyhow!(
                                "Provider has no future 5h quota reset; use a timestamp or interval"
                            )
                        })?
                    }
                    _ => bail!("Choose a future timestamp or interval of 1–315360000 seconds"),
                };
                let mut task = if let Some(id) = id {
                    get(&id)?
                } else {
                    Task {
                        id: uuid::Uuid::new_v4().to_string(),
                        draft: draft.clone(),
                        enabled: true,
                        next_run: None,
                        running: false,
                        last_run: None,
                        last_result: None,
                        session_id: None,
                    }
                };
                if task.running {
                    bail!("Cannot edit a running task");
                }
                task.draft = draft;
                task.next_run = Some(next);
                Self::write(&tx, &task)?;
            }
            TaskAction::SetEnabled { id, enabled } => {
                let mut task = get(&id)?;
                if enabled && task.next_run.is_none() {
                    bail!("Edit the completed task to schedule it again");
                }
                task.enabled = enabled;
                Self::write(&tx, &task)?;
            }
            TaskAction::Delete { id } => {
                if get(&id)?.running {
                    bail!("Cannot delete a running task; pause it first and wait for completion");
                }
                tx.execute("DELETE FROM scheduled_tasks WHERE id=?1", [id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    fn claim(&mut self, now: DateTime<Utc>) -> Result<Option<Task>> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let tasks: Vec<Task> = {
            let mut stmt = tx.prepare("SELECT body FROM scheduled_tasks")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.map(|r| Ok(serde_json::from_str(&r?)?))
                .collect::<Result<_>>()?
        };
        let mut due: Vec<_> = tasks
            .into_iter()
            .filter(|t| t.enabled && !t.running && t.next_run.is_some_and(|at| at <= now))
            .collect();
        due.sort_by_key(|t| t.next_run);
        let task = if let Some(mut task) = due.into_iter().next() {
            task.running = true;
            task.last_run = Some(now);
            task.session_id = None;
            task.last_result = Some("Running".into());
            // Advance before dispatch: a crash must not replay a side effect.
            task.next_run = match task.draft.schedule {
                Schedule::Interval { seconds } => {
                    Some(now + chrono::Duration::seconds(seconds as i64))
                }
                _ => None,
            };
            Self::write(&tx, &task)?;
            Some(task)
        } else {
            None
        };
        tx.commit()?;
        Ok(task)
    }
    fn finish(&mut self, id: &str, result: &str, session_id: Option<String>) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let body: String =
            tx.query_row("SELECT body FROM scheduled_tasks WHERE id=?1", [id], |r| {
                r.get(0)
            })?;
        let mut task: Task = serde_json::from_str(&body)?;
        task.running = false;
        task.last_result = Some(result.to_owned());
        task.session_id = session_id;
        if task.next_run.is_none() {
            task.enabled = false;
        }
        // Coalesce all ticks missed during execution, without catch-up bursts.
        if let Schedule::Interval { seconds } = task.draft.schedule {
            task.next_run = Some(Utc::now() + chrono::Duration::seconds(seconds as i64));
        }
        Self::write(&tx, &task)?;
        tx.commit()?;
        Ok(())
    }
}

/// Only accept a real five-hour window, never a weekly/default primary window.
pub fn five_hour_reset(
    usage: &crate::core::ProviderUsageStatus,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if !usage.available {
        return None;
    }
    usage
        .windows
        .iter()
        .filter(|w| w.label.eq_ignore_ascii_case("5h") || w.id == "five_hour")
        .filter_map(|w| {
            DateTime::parse_from_rfc3339(w.resets_at.as_deref()?)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        })
        .filter(|at| *at > now)
        .min()
}

pub struct Scheduler {
    busy: Arc<AtomicBool>,
}
impl Scheduler {
    /// Start only in a durable host, not in each session runtime/UI.
    pub fn start() -> Result<Self> {
        let directory = ConfigStore::default().directory;
        std::fs::create_dir_all(&directory)?;
        let lease = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("tasks.scheduler.lock"))?;
        let busy = Arc::new(AtomicBool::new(true));
        let thread_busy = busy.clone();
        // Keep the lease inside the worker: daemon shutdown cannot unlock while
        // a run is still alive. Process exit releases both runtime and lease.
        thread::Builder::new()
            .name("yeet-tasks".into())
            .spawn(move || {
                let mut owner = false;
                loop {
                    let mut run = || -> Result<()> {
                        let mut store = TaskStore::in_directory(&directory)?;
                        thread_busy.store(
                            store.list()?.iter().any(|t| t.enabled || t.running),
                            Ordering::Relaxed,
                        );
                        if !owner {
                            match fs2::FileExt::try_lock_exclusive(&lease) {
                                Ok(()) => {
                                    owner = true;
                                    for task in store.list()?.into_iter().filter(|t| t.running) {
                                        store.finish(
                                            &task.id,
                                            "Interrupted: scheduler restarted; not replayed",
                                            task.session_id,
                                        )?;
                                    }
                                }
                                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                    return Ok(());
                                }
                                Err(error) => return Err(error.into()),
                            }
                        }
                        if let Some(task) = store.claim(Utc::now())? {
                            thread_busy.store(true, Ordering::Relaxed);
                            let (result, session_id) = execute(&task);
                            store.finish(&task.id, &result, session_id)?;
                        }
                        Ok(())
                    };
                    if let Err(error) = run() {
                        // Keep the host alive and retry transient I/O failures.
                        thread_busy.store(true, Ordering::Relaxed);
                        eprintln!("Task scheduler failed: {error:#}");
                    }
                    thread::sleep(Duration::from_secs(1));
                }
            })?;
        Ok(Self { busy })
    }
    pub fn has_work(&self) -> bool {
        self.busy.load(Ordering::Relaxed)
    }
}

fn execute(task: &Task) -> (String, Option<String>) {
    let mut session_id = None;
    let result = (|| -> Result<()> {
        let mut harness = Harness::embedded(&task.draft.folder)?;
        harness.send(FrontendCommand::NewSession)?;
        harness.send(FrontendCommand::SelectModel {
            model: task.draft.model.clone(),
        })?;
        harness.send(FrontendCommand::SetGoal { enabled: false })?;
        harness.send(FrontendCommand::Submit {
            text: task.draft.prompt.clone(),
            images: vec![],
            attachment_ids: vec![],
        })?;
        let started = Instant::now();
        let mut saw_streaming = false;
        loop {
            while let Some(event) = harness.try_recv() {
                if event.kind == "error" {
                    bail!(
                        "{}",
                        event.message.unwrap_or_else(|| "Runtime error".into())
                    );
                }
                if let Some(state) = event.state {
                    if state.current_session_id.is_some() {
                        session_id = state.current_session_id.clone();
                    }
                    saw_streaming |= state.is_streaming;
                    if state.pending_shell_permission.is_some()
                        || state.pending_native_app_permission.is_some()
                        || state.pending_user_question.is_some()
                    {
                        harness.send(FrontendCommand::Interrupt)?;
                        bail!(
                            "Needs user input/permission; unattended run interrupted (no auto-approval)"
                        );
                    }
                    if let Some(error) = state.error_message {
                        bail!("{error}");
                    }
                    if saw_streaming && !state.is_streaming {
                        return Ok(());
                    }
                }
            }
            if started.elapsed() > Duration::from_secs(3600) {
                let _ = harness.send(FrontendCommand::Interrupt);
                bail!("Timed out after one hour");
            }
            thread::sleep(Duration::from_millis(30));
        }
    })();
    (
        match result {
            Ok(()) => "Completed".into(),
            Err(e) => format!("Failed: {e:#}"),
        },
        session_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durable_claims_pause_and_completion_do_not_replay() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TaskStore::in_directory(dir.path()).unwrap();
        let now = Utc::now();
        let draft = TaskDraft {
            name: "test".into(),
            prompt: "do work".into(),
            folder: dir.path().into(),
            model: "test/model".into(),
            schedule: Schedule::Interval { seconds: 60 },
        };
        store
            .apply(
                TaskAction::Save {
                    id: None,
                    draft: draft.clone(),
                },
                None,
                now,
            )
            .unwrap();
        assert!(store.claim(now).unwrap().is_none());
        let task = store
            .claim(now + chrono::Duration::seconds(180))
            .unwrap()
            .unwrap();
        assert!(
            store
                .claim(now + chrono::Duration::hours(1))
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .apply(
                    TaskAction::Delete {
                        id: task.id.clone()
                    },
                    None,
                    now
                )
                .is_err()
        );
        store
            .apply(
                TaskAction::SetEnabled {
                    id: task.id.clone(),
                    enabled: false,
                },
                None,
                now,
            )
            .unwrap();
        store
            .finish(&task.id, "Completed", Some("session".into()))
            .unwrap();
        drop(store);
        let mut store = TaskStore::in_directory(dir.path()).unwrap();
        assert!(!store.list().unwrap()[0].enabled);
        assert!(
            store
                .claim(now + chrono::Duration::days(1))
                .unwrap()
                .is_none()
        );
        let mut once = draft;
        once.schedule = Schedule::QuotaReset;
        assert!(
            store
                .apply(
                    TaskAction::Save {
                        id: None,
                        draft: once.clone()
                    },
                    None,
                    now
                )
                .is_err()
        );
        store
            .apply(
                TaskAction::Save {
                    id: None,
                    draft: once,
                },
                Some(now + chrono::Duration::seconds(1)),
                now,
            )
            .unwrap();
        let task = store
            .claim(now + chrono::Duration::seconds(2))
            .unwrap()
            .unwrap();
        store.finish(&task.id, "Completed", None).unwrap();
        assert!(
            store
                .claim(now + chrono::Duration::days(1))
                .unwrap()
                .is_none()
        );
    }
}
