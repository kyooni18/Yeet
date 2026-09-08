//! Session-owned detached commands. Workers retain bounded output and are cancelled on drop.
use super::*;
use std::sync::{Arc, Mutex, atomic::Ordering};
use uuid::Uuid;

#[derive(Default)]
pub(super) struct ShellJobs {
    jobs: BTreeMap<String, ShellJob>,
}

struct ShellJob {
    command: String,
    cancel: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Result<crate::shell::ShellResult, String>>>>,
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
        protected: Vec<PathBuf>,
    ) -> Result<String> {
        if self.jobs.len() >= 32 {
            bail!("Shell job limit reached (32); forget completed jobs before starting more");
        }
        let id = Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let result = Arc::new(Mutex::new(None));
        let worker_cancel = cancel.clone();
        let worker_result = result.clone();
        let worker_command = command.clone();
        thread::Builder::new()
            .name(format!("shell-{id}"))
            .spawn(move || {
                let outcome = std::panic::catch_unwind(|| {
                    run_shell_cancellable_with_protected_paths(
                        ShellExecutionRequest {
                            command: &worker_command,
                            workspace_root: &root,
                            working_directory: directory.as_deref(),
                            timeout_seconds: timeout,
                            capture_bytes: 64 * 1024,
                            allow_write,
                            unrestricted,
                            cancel: Some(&worker_cancel),
                        },
                        &protected,
                    )
                    .map_err(|error| error.to_string())
                })
                .unwrap_or_else(|_| Err("Shell worker panicked".into()));
                *worker_result.lock().unwrap() = Some(outcome);
            })?;
        self.jobs.insert(
            id.clone(),
            ShellJob {
                command,
                cancel,
                result,
            },
        );
        Ok(id)
    }

    fn snapshot(&self, id: &str) -> Result<Value> {
        let job = self
            .jobs
            .get(id)
            .ok_or_else(|| anyhow!("Unknown shell job: {id}"))?;
        let result = job
            .result
            .lock()
            .map_err(|_| anyhow!("Shell job state unavailable"))?;
        Ok(match result.as_ref() {
            Some(Ok(output)) => {
                let mut value = serde_json::to_value(output)?;
                value["jobId"] = json!(id);
                value["status"] = json!("completed");
                value
            }
            Some(Err(error)) => json!({"jobId":id,"command":job.command,
                "status":if error == "cancelled" { "cancelled" } else { "failed" },
                "succeeded":false,"error":error}),
            None => json!({"jobId":id,"command":job.command,
                "status":if job.cancel.load(Ordering::Acquire) { "stopping" } else { "running" }}),
        })
    }
}

impl ToolRegistry {
    pub(super) fn shell_job_tool(&mut self, object: &Map<String, Value>) -> Result<String> {
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
                    Ok(value)
                })
                .collect::<Result<Vec<_>>>()?;
            return Ok(json!({"jobs":jobs}).to_string());
        }
        let id = string_arg(object, "jobId")?;
        match action {
            "check" => {}
            "stop" => {
                let job = self
                    .shell_jobs
                    .jobs
                    .get(id)
                    .ok_or_else(|| anyhow!("Unknown shell job: {id}"))?;
                job.cancel.store(true, Ordering::Release);
            }
            "forget" => {
                let value = self.shell_jobs.snapshot(id)?;
                if matches!(value["status"].as_str(), Some("running" | "stopping")) {
                    bail!("Stop the job and wait for completion before forgetting it");
                }
                self.shell_jobs.jobs.remove(id);
                return Ok(json!({"jobId":id,"status":"forgotten"}).to_string());
            }
            _ => bail!("shell_job action must be check, list, stop, or forget"),
        }
        let mut value = self.shell_jobs.snapshot(id)?;
        // A detached process can change workspace contents between any two checks.
        let generation = self.workspace_generation;
        self.invalidate_workspace_cache();
        self.workspace_generation = generation;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(jobs: &ShellJobs, id: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let value = jobs.snapshot(id).unwrap();
            if !matches!(value["status"].as_str(), Some("running" | "stopping")) {
                return value;
            }
            assert!(Instant::now() < deadline, "job did not finish");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn detached_jobs_return_immediately_and_keep_results() {
        let root = tempfile::tempdir().unwrap();
        let mut jobs = ShellJobs::default();
        let start = Instant::now();
        let id = jobs
            .start(
                "sleep 0.3; printf done; printf diagnostic >&2; exit 7".into(),
                root.path().into(),
                None,
                5,
                false,
                true,
                vec![],
            )
            .unwrap();
        assert!(start.elapsed() < Duration::from_millis(250));
        assert_eq!(jobs.snapshot(&id).unwrap()["status"], "running");
        let value = wait(&jobs, &id);
        assert_eq!(value["exitCode"], 7);
        assert_eq!(value["stdout"], "done");
        assert_eq!(value["stderr"], "diagnostic");
        assert_eq!(jobs.snapshot(&id).unwrap(), value);
        assert!(jobs.snapshot("missing").is_err());
    }

    #[test]
    fn cancellation_works_after_shell_exits_with_inherited_pipes() {
        let root = tempfile::tempdir().unwrap();
        let mut jobs = ShellJobs::default();
        let id = jobs
            .start(
                "sleep 30 & exit 0".into(),
                root.path().into(),
                None,
                900,
                false,
                true,
                vec![],
            )
            .unwrap();
        thread::sleep(Duration::from_millis(100));
        jobs.jobs[&id].cancel.store(true, Ordering::Release);
        assert_eq!(wait(&jobs, &id)["status"], "cancelled");
    }

    #[test]
    fn detached_jobs_can_be_cancelled_and_drop_cancels_workers() {
        let root = tempfile::tempdir().unwrap();
        let mut jobs = ShellJobs::default();
        let id = jobs
            .start(
                "sleep 30 & wait".into(),
                root.path().into(),
                None,
                900,
                false,
                true,
                vec![],
            )
            .unwrap();
        jobs.jobs[&id].cancel.store(true, Ordering::Release);
        assert_eq!(wait(&jobs, &id)["status"], "cancelled");
        let id = jobs
            .start(
                "sleep 30".into(),
                root.path().into(),
                None,
                900,
                false,
                true,
                vec![],
            )
            .unwrap();
        let flag = jobs.jobs[&id].cancel.clone();
        drop(jobs);
        assert!(flag.load(Ordering::Acquire));
    }
}
