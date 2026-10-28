//! Session-owned detached commands. Workers retain bounded output and are cancelled on drop.
use super::*;
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

const MAX_SHELL_JOBS: usize = 32;
const WAIT_CANCEL_CHECK_INTERVAL: Duration = Duration::from_millis(250);
const SHELL_JOB_PROGRESS_TAIL_BYTES: usize = 2 * 1024;

#[derive(Default)]
pub(super) struct ShellJobs {
    jobs: BTreeMap<String, ShellJob>,
    event_budget: Arc<AtomicUsize>,
    access: ShellJobAccess,
}

/// A view into the existing job owner, not another process supervisor. It lets
/// authenticated frontends interact while the agent holds its coordinator lock.
#[derive(Clone, Default)]
pub(crate) struct ShellJobAccess {
    jobs: Arc<Mutex<BTreeMap<String, ShellJobHandle>>>,
}

#[derive(Clone)]
struct ShellJobHandle {
    #[cfg(unix)]
    progress: ShellProgress,
    log: crate::shell::log::ShellOutputLog,
    cancel: Arc<AtomicBool>,
    state: Arc<(Mutex<ShellJobState>, Condvar)>,
}

impl ShellJobAccess {
    pub(crate) fn has_running(&self) -> bool {
        self.jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .any(|job| {
                job.state
                    .0
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .result
                    .is_none()
            })
    }

    pub(crate) fn execute(
        &self,
        id: &str,
        action: &str,
        offset: usize,
        data: &str,
        rows: u16,
        cols: u16,
    ) -> Result<Value> {
        #[cfg(not(unix))]
        let _ = (data, rows, cols);
        let job = self
            .jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("Unknown or unrecoverable shell job: {id}"))?;
        match action {
            "output" => {
                let page = job.log.wait_events(
                    offset,
                    8192,
                    Duration::from_secs(15),
                    &AtomicBool::new(false),
                )?;
                let state = job.state.0.lock().unwrap_or_else(|p| p.into_inner());
                let status = match state.result.as_ref() {
                    None => "running",
                    Some(Ok(result)) if result.succeeded => "completed",
                    Some(Ok(result)) if result.exit_code == 130 => "interrupted",
                    Some(Ok(_)) => "failed",
                    Some(Err(error)) if error == "cancelled" => "interrupted",
                    Some(Err(_)) => "failed",
                };
                let mut value = serde_json::to_value(page)?;
                value["status"] = json!(status);
                Ok(value)
            }
            "stop" => {
                job.cancel.store(true, Ordering::Release);
                Ok(json!({"delivered":true}))
            }
            #[cfg(unix)]
            "input" => {
                if data.len() > 8192 {
                    bail!("Input is limited to 8192 bytes");
                }
                job.progress.terminal_input(data.as_bytes())?;
                Ok(json!({"delivered":true}))
            }
            #[cfg(unix)]
            "resize" => {
                job.progress.terminal_resize(rows, cols)?;
                Ok(json!({"delivered":true}))
            }
            #[cfg(unix)]
            "interrupt" => {
                job.progress.terminal_input(b"\x03")?;
                Ok(json!({"delivered":true}))
            }
            _ => bail!("Unsupported shell job action: {action}"),
        }
    }
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
    event_log: crate::shell::log::ShellOutputLog,
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
    pub direct: Option<crate::shell::selection::LaunchSpec>,
    pub shell: Option<String>,
    pub load_config: bool,
    pub login: bool,
    pub pty: bool,
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
        self.reset();
    }
}

impl ShellJobs {
    /// Revoke every frontend handle before transferring the registry to a new
    /// session. Running workers retain their own cancel flag and reap children.
    pub(super) fn reset(&mut self) {
        for job in self.jobs.values() {
            job.cancel.store(true, Ordering::Release);
        }
        self.access
            .jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.jobs.clear();
    }

    pub(super) fn access(&self) -> ShellJobAccess {
        self.access.clone()
    }
    pub(super) fn has_jobs(&self) -> bool {
        !self.jobs.is_empty()
    }

    // Used by the preserved pending remote shell-job layer.
    #[allow(dead_code)]
    pub(super) fn has_running_jobs(&self) -> bool {
        self.jobs.values().any(|job| {
            job.state
                .0
                .lock()
                .map(|state| state.result.is_none())
                .unwrap_or(true)
        })
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
            direct,
            shell,
            load_config,
            login,
            pty,
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
        let event_log = if let Some((_, log)) = &transcript {
            log.clone()
        } else {
            crate::shell::log::ShellOutputLog::new_ephemeral(
                tempfile::tempfile()?,
                self.event_budget.clone(),
            )
        };
        let progress =
            ShellProgress::new(SHELL_JOB_PROGRESS_TAIL_BYTES).with_log(event_log.clone());
        let worker_log = event_log.clone();
        let worker_progress = progress.clone();
        #[cfg(unix)]
        let terminal_progress = progress.clone();
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
                            direct,
                            shell,
                            load_config,
                            login,
                            pty,
                        },
                    )
                    .map_err(|error| error.to_string())
                })
                .unwrap_or_else(|_| Err("Shell worker panicked".into()));
                #[cfg(unix)]
                terminal_progress.terminal_close();
                let (lock, completed) = &*worker_state;
                let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                state.result = Some(outcome);
                // Publish the outcome before waking output readers at EOF. Keep
                // the state lock until the log is finished so waiters see both.
                worker_log.finish();
                completed.notify_all();
            })?;
        self.access
            .jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                id.clone(),
                ShellJobHandle {
                    #[cfg(unix)]
                    progress: progress.clone(),
                    log: event_log.clone(),
                    cancel: cancel.clone(),
                    state: state.clone(),
                },
            );
        self.jobs.insert(
            id.clone(),
            ShellJob {
                command,
                cancel,
                state,
                progress,
                transcript,
                event_log,
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
        self.access
            .jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
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
        if matches!(action, "input" | "resize" | "interrupt") {
            let job = self
                .shell_jobs
                .jobs
                .get(id)
                .ok_or_else(|| anyhow!("Unknown shell job: {id}"))?;
            #[cfg(unix)]
            match action {
                "input" => {
                    let data = string_arg(object, "data")?;
                    if data.len() > 8192 {
                        bail!("Input is limited to 8192 bytes per call");
                    }
                    job.progress.terminal_input(data.as_bytes())?;
                }
                "interrupt" => job.progress.terminal_input(b"\x03")?,
                "resize" => {
                    let rows = usize_arg(object, "rows")
                        .filter(|n| (1..=u16::MAX as usize).contains(n))
                        .ok_or_else(|| anyhow!("Invalid rows"))?
                        as u16;
                    let cols = usize_arg(object, "cols")
                        .filter(|n| (1..=u16::MAX as usize).contains(n))
                        .ok_or_else(|| anyhow!("Invalid cols"))?
                        as u16;
                    job.progress.terminal_resize(rows, cols)?;
                }
                _ => unreachable!(),
            }
            #[cfg(not(unix))]
            {
                let _ = job;
                bail!("PTY operations are not supported on this platform");
            }
            #[cfg(unix)]
            return Ok(json!({"jobId":id,"status":"delivered"}).to_string());
        }
        if action == "output" {
            let job = self
                .shell_jobs
                .jobs
                .get(id)
                .ok_or_else(|| anyhow!("Unknown shell job: {id}"))?;
            let offset = usize_arg(object, "offset").unwrap_or(0);
            let max_bytes = usize_arg(object, "maxBytes").unwrap_or(8192);
            let wait = usize_arg(object, "waitMilliseconds").unwrap_or(0);
            if wait > 30_000 {
                bail!("waitMilliseconds must be at most 30000");
            }
            return Ok(serde_json::to_string(&job.event_log.wait_events(
                offset,
                max_bytes,
                Duration::from_millis(wait as u64),
                cancel,
            )?)?);
        }
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
                self.shell_jobs
                    .access
                    .jobs
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(id);
                return Ok(json!({"jobId":id,"status":"forgotten"}).to_string());
            }
            _ => bail!(
                "shell_job action must be check, wait, list, stop, forget, output, input, resize, or interrupt"
            ),
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

    #[cfg(unix)]
    #[test]
    fn dropping_job_owner_cancels_and_reaps_running_shell() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("pid");
        let mut jobs = ShellJobs::default();
        let id = jobs
            .start(ShellJobOptions {
                command: format!(
                    "trap '' INT TERM; echo $$ > '{}'; while :; do :; done",
                    marker.display()
                ),
                direct: None,
                shell: Some("sh".into()),
                load_config: false,
                login: false,
                pty: false,
                root: root.path().to_path_buf(),
                directory: root.path().to_path_buf(),
                timeout_seconds: 0,
                capture_bytes: 4096,
                track_cwd: false,
                allow_write: false,
                unrestricted: true,
                force_sandboxed: false,
                artifacts: None,
            })
            .unwrap();
        let state = jobs.jobs[&id].state.clone();
        let start = Instant::now();
        let pid: i32 = loop {
            if let Ok(text) = std::fs::read_to_string(&marker)
                && let Ok(pid) = text.trim().parse()
            {
                break pid;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "shell did not start"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        drop(jobs);
        let (lock, completed) = &*state;
        let guard = lock.lock().unwrap();
        let (guard, _) = completed
            .wait_timeout_while(guard, Duration::from_secs(8), |s| s.result.is_none())
            .unwrap();
        assert!(
            guard
                .result
                .as_ref()
                .expect("worker did not finish")
                .is_err()
        );
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "shell still exists after owner shutdown"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_jobs_have_independent_lifetimes() {
        let root = tempfile::tempdir().unwrap();
        let mut jobs = ShellJobs::default();
        let options = |command: &str| ShellJobOptions {
            command: command.into(),
            direct: None,
            shell: None,
            load_config: false,
            login: false,
            pty: false,
            root: root.path().to_path_buf(),
            directory: root.path().to_path_buf(),
            timeout_seconds: 10,
            capture_bytes: 4096,
            track_cwd: false,
            allow_write: false,
            unrestricted: true,
            force_sandboxed: false,
            artifacts: None,
        };
        let stopped = jobs.start(options("sleep 30")).unwrap();
        let kept = jobs.start(options("sleep 1; printf done")).unwrap();
        let access = jobs.access();
        assert!(access.has_running());
        jobs.jobs[&stopped].cancel.store(true, Ordering::Release);
        assert!(matches!(
            jobs.wait_finished(&stopped, Duration::from_secs(8), &AtomicBool::new(false))
                .unwrap(),
            ShellJobWait::Finished
        ));
        assert_eq!(
            jobs.access()
                .execute(&stopped, "output", 0, "", 0, 0)
                .unwrap()["status"],
            "interrupted"
        );
        assert!(jobs.take_finished(&stopped).unwrap().result.is_err());
        assert!(matches!(
            jobs.wait_finished(&kept, Duration::from_secs(8), &AtomicBool::new(false))
                .unwrap(),
            ShellJobWait::Finished
        ));
        assert_eq!(
            jobs.access().execute(&kept, "output", 0, "", 0, 0).unwrap()["status"],
            "completed"
        );
        let output = jobs.take_finished(&kept).unwrap().result.unwrap();
        assert_eq!(output.stdout.as_deref(), Some("done"));
        let failed = jobs.start(options("exit 7")).unwrap();
        assert!(matches!(
            jobs.wait_finished(&failed, Duration::from_secs(8), &AtomicBool::new(false))
                .unwrap(),
            ShellJobWait::Finished
        ));
        assert_eq!(
            jobs.access()
                .execute(&failed, "output", 0, "", 0, 0)
                .unwrap()["status"],
            "failed"
        );
        assert!(!access.has_running());
        jobs.reset();
        assert!(access.execute(&failed, "output", 0, "", 0, 0).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn pty_input_resize_and_merged_output() {
        let root = tempfile::tempdir().unwrap();
        let mut jobs = ShellJobs::default();
        let id = jobs.start(ShellJobOptions {
            command: "stty size; read line; stty size; printf 'input=%s\\n' \"$line\"; printf 'err\\n' >&2".into(),
            direct: None, shell: None, load_config: false, login: false, pty: true,
            root: root.path().to_path_buf(), directory: root.path().to_path_buf(),
            timeout_seconds: 10, capture_bytes: 4096, track_cwd: false,
            allow_write: false, unrestricted: true, force_sandboxed: false, artifacts: None,
        }).unwrap();
        let progress = jobs.jobs[&id].progress.clone();
        let start = Instant::now();
        while progress.terminal_resize(41, 99).is_err() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
        }
        let access = jobs.access();
        access.execute(&id, "resize", 0, "", 41, 99).unwrap();
        access.execute(&id, "input", 0, "hello\n", 0, 0).unwrap();
        assert!(matches!(
            jobs.wait_finished(&id, Duration::from_secs(10), &AtomicBool::new(false))
                .unwrap(),
            ShellJobWait::Finished
        ));
        let page = access.execute(&id, "output", 0, "", 0, 0).unwrap();
        assert_eq!(page["finished"], true);
        assert_eq!(page["status"], "completed");
        let events = jobs.jobs[&id].event_log.events(0, 8192).unwrap();
        assert!(events.finished);
        assert!(events.events.iter().all(|event| event.stream == "pty"));
        assert!(!events.events.is_empty());
        let result = jobs.take_finished(&id).unwrap().result.unwrap();
        let output = result.stdout.unwrap();
        assert!(output.contains("41 99"), "{output}");
        assert!(output.contains("input=hello"), "{output}");
        assert!(output.contains("err"), "{output}");
        assert_eq!(result.stderr_bytes, 0);
    }

    #[cfg(unix)]
    #[test]
    fn pty_control_c_interrupts_foreground_command() {
        let root = tempfile::tempdir().unwrap();
        let mut jobs = ShellJobs::default();
        let id = jobs
            .start(ShellJobOptions {
                command: "printf ready; sleep 30".into(),
                direct: None,
                shell: None,
                load_config: false,
                login: false,
                pty: true,
                root: root.path().to_path_buf(),
                directory: root.path().to_path_buf(),
                timeout_seconds: 10,
                capture_bytes: 4096,
                track_cwd: false,
                allow_write: false,
                unrestricted: true,
                force_sandboxed: false,
                artifacts: None,
            })
            .unwrap();
        let progress = jobs.jobs[&id].progress.clone();
        let start = Instant::now();
        while !progress
            .snapshot()
            .stdout_tail
            .unwrap_or_default()
            .contains("ready")
        {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
        }
        progress.terminal_input(b"\x03").unwrap();
        assert!(matches!(
            jobs.wait_finished(&id, Duration::from_secs(4), &AtomicBool::new(false))
                .unwrap(),
            ShellJobWait::Finished
        ));
        let result = jobs.take_finished(&id).unwrap().result.unwrap();
        assert!(!result.succeeded);
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_an_output_wait_does_not_stop_the_job() {
        let root = tempfile::tempdir().unwrap();
        let mut jobs = ShellJobs::default();
        let id = jobs
            .start(ShellJobOptions {
                command: "sleep 1; printf finished".into(),
                direct: None,
                shell: None,
                load_config: false,
                login: false,
                pty: false,
                root: root.path().to_path_buf(),
                directory: root.path().to_path_buf(),
                timeout_seconds: 10,
                capture_bytes: 4096,
                track_cwd: false,
                allow_write: false,
                unrestricted: true,
                force_sandboxed: false,
                artifacts: None,
            })
            .unwrap();
        let cancel_wait = AtomicBool::new(true);
        let error = jobs.jobs[&id]
            .event_log
            .wait_events(0, 128, Duration::from_secs(5), &cancel_wait)
            .err()
            .expect("output wait should be cancelled");
        assert!(error.to_string().contains("job is still running"));
        assert!(matches!(
            jobs.wait_finished(&id, Duration::from_secs(5), &AtomicBool::new(false))
                .unwrap(),
            ShellJobWait::Finished
        ));
        let result = jobs.take_finished(&id).unwrap().result.unwrap();
        assert_eq!(result.stdout.as_deref(), Some("finished"));
    }

    #[test]
    fn finished_jobs_stop_requesting_workspace_refresh() {
        let root = tempfile::tempdir().unwrap();
        let mut jobs = ShellJobs::default();
        jobs.start(ShellJobOptions {
            command: "true".into(),
            direct: None,
            shell: None,
            load_config: false,
            login: false,
            pty: false,
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
