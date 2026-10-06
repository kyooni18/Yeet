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
    started_at: Instant,
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

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start(
        &mut self,
        command: String,
        root: PathBuf,
        directory: Option<String>,
        timeout: u64,
        allow_write: bool,
        unrestricted: bool,
        force_sandboxed: bool,
    ) -> Result<String> {
        if self.jobs.len() >= MAX_SHELL_JOBS {
            bail!(
                "Shell job limit reached ({MAX_SHELL_JOBS}); forget completed jobs before starting more"
            );
        }
        let id = Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let state = Arc::new((Mutex::new(ShellJobState::default()), Condvar::new()));
        let progress = ShellProgress::new(SHELL_JOB_PROGRESS_TAIL_BYTES);
        let worker_progress = progress.clone();
        let worker_cancel = cancel.clone();
        let worker_state = state.clone();
        let worker_command = command.clone();
        thread::Builder::new()
            .name(format!("shell-{id}"))
            .spawn(move || {
                let outcome = std::panic::catch_unwind(|| {
                    run_shell_cancellable_with_progress(
                        ShellExecutionRequest {
                            command: &worker_command,
                            workspace_root: &root,
                            working_directory: directory.as_deref(),
                            timeout_seconds: timeout,
                            capture_bytes: 64 * 1024,
                            allow_write,
                            unrestricted,
                            force_sandboxed,
                            cancel: Some(&worker_cancel),
                        },
                        Some(worker_progress),
                    )
                    .map_err(|error| error.to_string())
                })
                .unwrap_or_else(|_| Err("Shell worker panicked".into()));
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
                started_at: Instant::now(),
            },
        );
        Ok(id)
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
        Ok(match state.result.as_ref() {
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
                "error":error
            }),
            None => json!({
                "jobId":id,
                "command":job.command,
                "status":if job.cancel.load(Ordering::Acquire) { "stopping" } else { "running" },
                "elapsedMilliseconds":job.started_at.elapsed().as_millis(),
                "progress":job.progress.snapshot()
            }),
        })
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
        let action = string_arg(object, "action")?;
        if action != "wait" && object.contains_key("reportEverySeconds") {
            bail!("reportEverySeconds is only valid with shell_job action=wait");
        }
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
        let stdout = value["stdout"].as_str().unwrap_or_default();
        let stderr = value["stderr"].as_str().unwrap_or_default();
        if stdout.len() + stderr.len() > 6 * 1024 {
            let artifact = self
                .artifacts
                .store(&format!("STDOUT:\n{stdout}\nSTDERR:\n{stderr}"))?;
            value.as_object_mut().unwrap().remove("stdout");
            value.as_object_mut().unwrap().remove("stderr");
            value["artifactId"] = json!(artifact);
            value["summary"] =
                json!("Output stored as an artifact; use search_artifact or read_artifact.");
        }
        Ok(value.to_string())
    }
}
