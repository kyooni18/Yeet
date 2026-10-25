//! Session-owned detached commands. Workers retain bounded output and are cancelled on drop.
use super::*;
use std::{
    sync::{Arc, Condvar, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};
use uuid::Uuid;

const MAX_SHELL_JOBS: usize = 32;
const WAIT_CANCEL_CHECK_INTERVAL: Duration = Duration::from_millis(250);
const SHELL_JOB_PROGRESS_TAIL_BYTES: usize = 2 * 1024;

#[derive(Default)]
pub(super) struct ShellJobs {
    jobs: BTreeMap<String, ShellJob>,
}

#[derive(Default)]
struct ShellJobState {
    result: Option<Result<crate::shell::ShellResult, String>>,
    monitor: Option<MonitorClock>,
}

struct MonitorClock {
    interval: Duration,
    next_deadline: Instant,
}

struct ShellJob {
    command: String,
    cancel: Arc<AtomicBool>,
    state: Arc<(Mutex<ShellJobState>, Condvar)>,
    progress: ShellProgress,
    transcript: Option<(String, crate::shell::log::ShellOutputLog)>,
    /// Directory the command started in, retained for late cwd tracking.
    cwd_start: PathBuf,
    /// Set once the finished command's directory has been applied.
    cwd_applied: bool,
    started_at: Instant,
    /// Set once the workspace was refreshed after this job finished.
    settled: bool,
}

pub(super) struct ShellJobOptions {
    pub command: String,
    pub root: PathBuf,
    pub directory: PathBuf,
    pub timeout_seconds: u64,
    pub capture_bytes: usize,
    pub track_cwd: bool,
    pub allow_write: bool,
    pub unrestricted: bool,
    pub force_sandboxed: bool,
    pub artifacts: Option<ArtifactStore>,
}

pub(super) enum ShellJobWait {
    Finished,
    TimedOut,
    Cancelled,
}

pub(super) struct FinishedShellJob {
    pub result: Result<crate::shell::ShellResult, String>,
    pub transcript: Option<(String, crate::shell::log::ShellOutputLog)>,
}

impl Drop for ShellJobs {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancel.store(true, Ordering::Release);
        }
    }
}

impl ShellJobs {
    pub(super) fn has_jobs(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// True while a job may still be changing the workspace, and once more
    /// after each job finishes so its final writes are observed. Completed
    /// jobs kept for inspection do not keep invalidating the workspace cache.
    pub(super) fn take_workspace_refresh(&mut self) -> bool {
        let mut refresh = false;
        for job in self.jobs.values_mut().filter(|job| !job.settled) {
            let (lock, _) = &*job.state;
            let finished = lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .result
                .is_some();
            job.settled = finished;
            refresh = true;
        }
        refresh
    }

    pub(super) fn start(&mut self, options: ShellJobOptions) -> Result<String> {
        let ShellJobOptions {
            command,
            root,
            directory,
            timeout_seconds,
            capture_bytes,
            track_cwd,
            allow_write,
            unrestricted,
            force_sandboxed,
            artifacts,
        } = options;
        if self.jobs.len() >= MAX_SHELL_JOBS {
            bail!(
                "Shell job limit reached ({MAX_SHELL_JOBS}); forget completed jobs before starting more"
            );
        }
        let id = Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let state = Arc::new((Mutex::new(ShellJobState::default()), Condvar::new()));
        let transcript = artifacts
            .as_ref()
            .map(|store| store.shell_log(&command))
            .transpose()?;
        let mut progress = ShellProgress::new(SHELL_JOB_PROGRESS_TAIL_BYTES);
        if let Some((_, log)) = &transcript {
            progress = progress.with_log(log.clone());
        }
        let worker_log = transcript.as_ref().map(|(_, log)| log.clone());
        let worker_progress = progress.clone();
        let worker_cancel = cancel.clone();
        let worker_state = state.clone();
        let worker_command = command.clone();
        let worker_directory = directory.to_string_lossy().into_owned();
        thread::Builder::new()
            .name(format!("shell-{id}"))
            .spawn(move || {
                let _artifacts = artifacts;
                let outcome = std::panic::catch_unwind(|| {
                    crate::shell::run_shell_session(
                        ShellExecutionRequest {
                            command: &worker_command,
                            workspace_root: &root,
                            working_directory: Some(&worker_directory),
                            timeout_seconds,
                            capture_bytes,
                            allow_write,
                            unrestricted,
                            force_sandboxed,
                            cancel: Some(&worker_cancel),
                        },
                        crate::shell::ShellSessionOptions {
                            progress: Some(worker_progress),
                            track_cwd,
                        },
                    )
                    .map_err(|error| error.to_string())
                })
                .unwrap_or_else(|_| Err("Shell worker panicked".into()));
                if let Some(log) = worker_log {
                    log.finish();
                }
                let (lock, completed) = &*worker_state;
                let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                state.result = Some(outcome);
                completed.notify_all();
            })?;
        self.jobs.insert(
            id.clone(),
            ShellJob {
                command,
                cancel,
                state,
                progress,
                transcript,
                cwd_start: directory,
                cwd_applied: false,
                started_at: Instant::now(),
                settled: false,
            },
        );
        Ok(id)
    }

    pub(super) fn elapsed(&self, id: &str) -> Option<u128> {
        self.jobs
            .get(id)
            .map(|job| job.started_at.elapsed().as_millis())
    }

    /// Blocks until the job finishes, the deadline passes, or the caller cancels.
    /// Returning early leaves the job running in the background.
    pub(super) fn wait_finished(
        &self,
        id: &str,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<ShellJobWait> {
        let job = self
            .jobs
            .get(id)
            .ok_or_else(|| anyhow!("Unknown shell job: {id}"))?;
        let stop = job.cancel.clone();
        let deadline = Instant::now() + timeout;
        let (lock, completed) = &*job.state;
        let mut state = lock
            .lock()
            .map_err(|_| anyhow!("Shell job state unavailable"))?;
        loop {
            if state.result.is_some() {
                return Ok(ShellJobWait::Finished);
            }
            if cancel.load(Ordering::Acquire) {
                stop.store(true, Ordering::Release);
                return Ok(ShellJobWait::Cancelled);
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(ShellJobWait::TimedOut);
            }
            let (next, _) = completed
                .wait_timeout(state, (deadline - now).min(WAIT_CANCEL_CHECK_INTERVAL))
                .map_err(|_| anyhow!("Shell job state unavailable"))?;
            state = next;
        }
    }

    /// Removes a finished job and hands back its typed result.
    pub(super) fn take_finished(&mut self, id: &str) -> Result<FinishedShellJob> {
        let job = self
            .jobs
            .remove(id)
            .ok_or_else(|| anyhow!("Unknown shell job: {id}"))?;
        let (lock, _) = &*job.state;
        let state = lock
            .lock()
            .map_err(|_| anyhow!("Shell job state unavailable"))?;
        Ok(FinishedShellJob {
            result: state
                .result
                .clone()
                .ok_or_else(|| anyhow!("Shell job {id} has not finished"))?,
            transcript: job.transcript,
        })
    }

    /// Reports a finished command's directory once so a late handoff can still
    /// move the session cwd.
    pub(super) fn take_cwd(&mut self, id: &str) -> Option<(PathBuf, PathBuf)> {
        let job = self.jobs.get_mut(id)?;
        if job.cwd_applied {
            return None;
        }
        let (lock, _) = &*job.state;
        let ended = lock
            .lock()
            .ok()?
            .result
            .as_ref()?
            .as_ref()
            .ok()?
            .final_working_directory
            .clone()?;
        job.cwd_applied = true;
        Some((job.cwd_start.clone(), PathBuf::from(ended)))
    }

    pub(super) fn transcript_id(&self, id: &str) -> Option<String> {
        self.jobs
            .get(id)
            .and_then(|job| job.transcript.as_ref().map(|(id, _)| id.clone()))
    }

    pub(super) fn progress(&self, id: &str) -> Option<crate::shell::ShellProgressSnapshot> {
        self.jobs.get(id).map(|job| job.progress.snapshot())
    }

    fn snapshot(&self, id: &str) -> Result<Value> {
        let job = self
            .jobs
            .get(id)
            .ok_or_else(|| anyhow!("Unknown shell job: {id}"))?;
        let (lock, _) = &*job.state;
        let state = lock
            .lock()
            .map_err(|_| anyhow!("Shell job state unavailable"))?;
        Self::snapshot_from_state(id, job, &state)
    }

    fn snapshot_from_state(id: &str, job: &ShellJob, state: &ShellJobState) -> Result<Value> {
        let mut value = match state.result.as_ref() {
            Some(Ok(output)) => {
                let mut value = serde_json::to_value(output)?;
                value["jobId"] = json!(id);
                value["status"] = json!("completed");
                value
            }
            Some(Err(error)) => json!({
                "jobId":id,
                "command":job.command,
                "status":if error == "cancelled" { "cancelled" } else { "failed" },
                "succeeded":false,
                "durationMilliseconds":job.started_at.elapsed().as_millis(),
                "error":error,
                "progress":job.progress.snapshot()
            }),
            None => json!({
                "jobId":id,
                "command":job.command,
                "status":if job.cancel.load(Ordering::Acquire) { "stopping" } else { "running" },
                "elapsedMilliseconds":job.started_at.elapsed().as_millis(),
                "progress":job.progress.snapshot()
            }),
        };
        if let Some((id, log)) = &job.transcript {
            value["artifactId"] = json!(id);
            value["artifactComplete"] = json!(log.snapshot().complete);
            value["transcript"] = serde_json::to_value(log.snapshot())?;
        }
        Ok(value)
    }

    fn wait(
        &self,
        id: &str,
        report_every: Option<Duration>,
        wait_cancel: &AtomicBool,
    ) -> Result<(Value, &'static str, u128)> {
        if report_every.is_some_and(|interval| interval.is_zero()) {
            bail!("Shell job report interval must be greater than zero");
        }
        let job = self
            .jobs
            .get(id)
            .ok_or_else(|| anyhow!("Unknown shell job: {id}"))?;
        let wait_started = Instant::now();
        let (lock, completed) = &*job.state;
        let mut state = lock
            .lock()
            .map_err(|_| anyhow!("Shell job state unavailable"))?;

        match report_every {
            Some(interval) => {
                let reset_clock = state
                    .monitor
                    .as_ref()
                    .is_none_or(|monitor| monitor.interval != interval);
                if reset_clock {
                    state.monitor = Some(MonitorClock {
                        interval,
                        next_deadline: Instant::now() + interval,
                    });
                }
            }
            None => state.monitor = None,
        }

        loop {
            if state.result.is_some() {
                let value = Self::snapshot_from_state(id, job, &state)?;
                return Ok((value, "terminal", wait_started.elapsed().as_millis()));
            }
            if wait_cancel.load(Ordering::Acquire) {
                bail!("Shell job wait cancelled; the detached job is still running");
            }

            let now = Instant::now();
            let sleep_for = if let Some(monitor) = state.monitor.as_mut() {
                if now >= monitor.next_deadline {
                    while monitor.next_deadline <= now {
                        monitor.next_deadline += monitor.interval;
                    }
                    let value = Self::snapshot_from_state(id, job, &state)?;
                    return Ok((value, "interval", wait_started.elapsed().as_millis()));
                }
                monitor
                    .next_deadline
                    .saturating_duration_since(now)
                    .min(WAIT_CANCEL_CHECK_INTERVAL)
            } else {
                WAIT_CANCEL_CHECK_INTERVAL
            };

            let (next_state, _) = completed
                .wait_timeout(state, sleep_for)
                .map_err(|_| anyhow!("Shell job state unavailable"))?;
            state = next_state;
        }
    }
}

impl ToolRegistry {
    pub(super) fn shell_job_tool(
        &mut self,
        object: &Map<String, Value>,
        cancel: &AtomicBool,
    ) -> Result<String> {
        // reportEverySeconds only affects wait; other actions ignore it.
        let action = string_arg(object, "action")?;
        if action == "list" {
            let jobs = self
                .shell_jobs
                .jobs
                .keys()
                .map(|id| {
                    let mut value = self.shell_jobs.snapshot(id)?;
                    value.as_object_mut().unwrap().remove("stdout");
                    value.as_object_mut().unwrap().remove("stderr");
                    value.as_object_mut().unwrap().remove("progress");
                    Ok(value)
                })
                .collect::<Result<Vec<_>>>()?;
            return Ok(json!({"jobs":jobs}).to_string());
        }

        let id = string_arg(object, "jobId")?;
        let mut value = match action {
            "check" => self.shell_jobs.snapshot(id)?,
            "wait" => {
                let report_every = match usize_arg(object, "reportEverySeconds") {
                    Some(seconds) if !(1..=86_400).contains(&seconds) => {
                        bail!("reportEverySeconds must be between 1 and 86400")
                    }
                    Some(seconds) => Some(Duration::from_secs(seconds as u64)),
                    None => None,
                };
                let (mut value, wake_reason, waited_milliseconds) =
                    self.shell_jobs.wait(id, report_every, cancel)?;
                value["wakeReason"] = json!(wake_reason);
                value["waitedMilliseconds"] = json!(waited_milliseconds);
                if wake_reason == "interval" {
                    value["hint"] = json!(
                        "The job is still running. Call shell_job action=wait with the same reportEverySeconds to keep the fixed monitoring cadence, or omit it to sleep until termination."
                    );
                }
                value
            }
            "stop" => {
                let job = self
                    .shell_jobs
                    .jobs
                    .get(id)
                    .ok_or_else(|| anyhow!("Unknown shell job: {id}"))?;
                job.cancel.store(true, Ordering::Release);
                self.shell_jobs.snapshot(id)?
            }
            "forget" => {
                let value = self.shell_jobs.snapshot(id)?;
                if matches!(value["status"].as_str(), Some("running" | "stopping")) {
                    bail!("Stop the job and wait for completion before forgetting it");
                }
                self.shell_jobs.jobs.remove(id);
                return Ok(json!({"jobId":id,"status":"forgotten"}).to_string());
            }
            _ => bail!("shell_job action must be check, wait, list, stop, or forget"),
        };

        // A detached process can change workspace contents while the model is asleep.
        let generation = self.evidence.workspace_generation;
        self.invalidate_workspace_cache();
        self.evidence.workspace_generation = generation;
        // A command that was handed off from the foreground still owns the cwd
        // once it finally exits.
        if !matches!(value["status"].as_str(), Some("running" | "stopping"))
            && let Some((started, ended)) = self.shell_jobs.take_cwd(id)
        {
            let note = self.apply_shell_cwd(ended, &started);
            value["shellCwd"] = json!(
                self.shell_cwd
                    .as_deref()
                    .unwrap_or(&self.context.working_directory)
                    .display()
                    .to_string()
            );
            if let Some(note) = note {
                value["cwdNote"] = json!(note);
            }
        }
        let stdout = value["stdout"].as_str().unwrap_or_default().to_owned();
        let stderr = value["stderr"].as_str().unwrap_or_default().to_owned();
        if stdout.len() + stderr.len() > 16 * 1024 {
            let artifact = self
                .artifacts
                .store(&format!("STDOUT:\n{stdout}\nSTDERR:\n{stderr}"))?;
            value["stdout"] = json!(super::shell_runtime::output_window(&stdout, 12 * 1024));
            value["stderr"] = json!(super::shell_runtime::output_window(&stderr, 4 * 1024));
            if value.get("transcript").is_none() {
                value["artifactId"] = json!(artifact);
            }
            value["outputWindowed"] = json!(true);
            if value.get("transcript").is_none() {
                value["artifactComplete"] = json!(
                    !value["stdoutTruncated"].as_bool().unwrap_or(false)
                        && !value["stderrTruncated"].as_bool().unwrap_or(false)
                );
            }
        }
        Ok(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_jobs_stop_requesting_workspace_refresh() {
        let root = tempfile::tempdir().unwrap();
        let mut jobs = ShellJobs::default();
        jobs.start(ShellJobOptions {
            command: "true".into(),
            root: root.path().to_path_buf(),
            directory: root.path().to_path_buf(),
            timeout_seconds: 30,
            capture_bytes: 64 * 1024,
            track_cwd: false,
            allow_write: false,
            unrestricted: true,
            force_sandboxed: false,
            artifacts: None,
        })
        .unwrap();
        let started = Instant::now();
        while jobs.take_workspace_refresh() {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "job never settled"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(jobs.has_jobs());
        assert!(!jobs.take_workspace_refresh());
    }
}
