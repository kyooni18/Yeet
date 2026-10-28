use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
use std::collections::HashSet;
#[cfg(unix)]
use std::io::Write;

use anyhow::{Context, Result, bail};
#[cfg(target_os = "linux")]
use serde::Deserialize;
use serde::Serialize;

use crate::harness::sandbox::{
    SandboxMode, SandboxPolicy, SandboxStore, WorkspaceRead, validate_relative_path,
};
use crate::platform::{TrackedChild, configure_process_group, display_path};

pub mod log;
pub mod selection;
use log::ShellOutputLog;
use selection::{LaunchSpec, select};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellResult {
    pub command: String,
    pub working_directory: String,
    pub exit_code: i32,
    pub succeeded: bool,
    pub duration_milliseconds: u128,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_working_directory: Option<String>,
}

#[derive(Clone)]
pub struct ShellProgress {
    inner: Arc<Mutex<ShellProgressState>>,
    log: Option<ShellOutputLog>,
    stop_readers: Arc<AtomicBool>,
    #[cfg(unix)]
    terminal: Arc<Mutex<Option<std::fs::File>>>,
}

struct ShellProgressState {
    stdout_bytes: usize,
    stderr_bytes: usize,
    stdout_tail: TailBuffer,
    stderr_tail: TailBuffer,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellProgressSnapshot {
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub stdout_tail: Option<String>,
    pub stderr_tail: Option<String>,
}

#[derive(Clone, Copy)]
enum ShellStream {
    Stdout,
    Stderr,
    Pty,
}

impl ShellProgress {
    pub fn new(tail_bytes: usize) -> Self {
        let tail_bytes = tail_bytes.clamp(256, 64 * 1024);
        Self {
            log: None,
            stop_readers: Arc::new(AtomicBool::new(false)),
            #[cfg(unix)]
            terminal: Arc::new(Mutex::new(None)),
            inner: Arc::new(Mutex::new(ShellProgressState {
                stdout_bytes: 0,
                stderr_bytes: 0,
                stdout_tail: TailBuffer::new(tail_bytes),
                stderr_tail: TailBuffer::new(tail_bytes),
            })),
        }
    }

    pub fn with_log(mut self, log: ShellOutputLog) -> Self {
        self.log = Some(log);
        self
    }

    pub fn snapshot(&self) -> ShellProgressSnapshot {
        let state = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let stdout_tail = state.stdout_tail.ordered();
        let stderr_tail = state.stderr_tail.ordered();
        ShellProgressSnapshot {
            stdout_bytes: state.stdout_bytes,
            stderr_bytes: state.stderr_bytes,
            stdout_tail: (!stdout_tail.is_empty())
                .then(|| String::from_utf8_lossy(&stdout_tail).into_owned()),
            stderr_tail: (!stderr_tail.is_empty())
                .then(|| String::from_utf8_lossy(&stderr_tail).into_owned()),
        }
    }

    fn push(&self, stream: ShellStream, bytes: &[u8]) {
        if let Some(log) = &self.log {
            log.push(
                match stream {
                    ShellStream::Stdout => "stdout",
                    ShellStream::Stderr => "stderr",
                    ShellStream::Pty => "pty",
                },
                bytes,
            );
        }
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match stream {
            ShellStream::Stdout | ShellStream::Pty => {
                state.stdout_bytes = state.stdout_bytes.saturating_add(bytes.len());
                state.stdout_tail.push(bytes);
            }
            ShellStream::Stderr => {
                state.stderr_bytes = state.stderr_bytes.saturating_add(bytes.len());
                state.stderr_tail.push(bytes);
            }
        }
    }

    #[cfg(unix)]
    pub fn terminal_input(&self, data: &[u8]) -> Result<()> {
        let mut terminal = self.terminal.lock().unwrap_or_else(|p| p.into_inner());
        let file = terminal
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("PTY is not running or is not ready"))?;
        file.write_all(data)?;
        Ok(())
    }

    #[cfg(unix)]
    pub fn terminal_resize(&self, rows: u16, cols: u16) -> Result<()> {
        use std::os::fd::AsRawFd;
        if rows == 0 || cols == 0 {
            bail!("Terminal dimensions must be nonzero");
        }
        let terminal = self.terminal.lock().unwrap_or_else(|p| p.into_inner());
        let file = terminal
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("PTY is not running or is not ready"))?;
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        if unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCSWINSZ, &size) } == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    #[cfg(unix)]
    pub fn terminal_close(&self) {
        *self.terminal.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

mod policy;
pub use policy::restricted_operation;

pub fn run_shell(
    command: &str,
    workspace_root: &Path,
    working_directory: Option<&str>,
    timeout_seconds: u64,
    capture_bytes: usize,
    allow_write: bool,
    unrestricted: bool,
) -> Result<ShellResult> {
    run_shell_cancellable(ShellExecutionRequest {
        command,
        workspace_root,
        working_directory,
        timeout_seconds,
        capture_bytes,
        allow_write,
        unrestricted,
        force_sandboxed: false,
        cancel: None,
    })
}

pub struct ShellExecutionRequest<'a> {
    pub command: &'a str,
    pub workspace_root: &'a Path,
    pub working_directory: Option<&'a str>,
    /// Caller deadline in seconds, or zero for no caller deadline. Sandbox
    /// wall-time limits apply independently of this value.
    pub timeout_seconds: u64,
    pub capture_bytes: usize,
    pub allow_write: bool,
    pub unrestricted: bool,
    pub force_sandboxed: bool,
    pub cancel: Option<&'a AtomicBool>,
}

pub fn run_shell_cancellable(request: ShellExecutionRequest<'_>) -> Result<ShellResult> {
    run_shell_cancellable_with_progress(request, None)
}

pub fn run_shell_cancellable_with_progress(
    request: ShellExecutionRequest<'_>,
    progress: Option<ShellProgress>,
) -> Result<ShellResult> {
    run_shell_session(
        request,
        ShellSessionOptions {
            progress,
            ..Default::default()
        },
    )
}

#[derive(Default)]
pub struct ShellSessionOptions {
    pub progress: Option<ShellProgress>,
    pub track_cwd: bool,
    pub shell: Option<String>,
    pub load_config: bool,
    pub login: bool,
    pub direct: Option<LaunchSpec>,
    pub pty: bool,
}

pub fn run_shell_session(
    request: ShellExecutionRequest<'_>,
    options: ShellSessionOptions,
) -> Result<ShellResult> {
    let progress = options.progress.clone();
    let ShellExecutionRequest {
        command,
        workspace_root,
        working_directory,
        timeout_seconds,
        capture_bytes,
        allow_write,
        unrestricted,
        force_sandboxed,
        cancel,
    } = request;
    #[cfg(not(unix))]
    if options.pty {
        bail!("PTY execution is not supported on this platform");
    }
    let command = command.trim();
    if command.is_empty() {
        bail!("Shell command cannot be empty");
    }
    if command.len() > 1_048_576 || command.contains('\0') {
        bail!("Shell command is too long");
    }
    let root = workspace_root.canonicalize()?;
    let requested = SandboxStore::new(&root)?.load()?;
    let unrestricted =
        !force_sandboxed && (unrestricted || requested.mode == SandboxMode::Unlimited);
    if !allow_write
        && !unrestricted
        && let Some(operation) = if options.direct.is_some() {
            Some("direct process execution".to_owned())
        } else {
            restricted_operation(command)
        }
    {
        bail!(
            "Shell operation '{operation}' requires explicit permission. Use request_shell_permission with the exact command and a short reason, then retry after the user grants it."
        );
    }
    let cwd = resolve_working_directory(&root, working_directory, unrestricted)?;
    let maximum = trusted_maximum(&requested, allow_write);
    let mut effective = requested.restricted_to(&maximum);
    if allow_write {
        effective.workspace_writable = true;
    }
    if !unrestricted && timeout_seconds != 0 {
        effective.limits.wall_time_seconds = effective
            .limits
            .wall_time_seconds
            .min(timeout_seconds.clamp(1, 86_400));
    }
    if !unrestricted {
        if !effective.network_allow.is_empty() {
            bail!(
                "This sandbox backend cannot safely enforce the requested network allowlist yet."
            );
        }
        if !effective.secret_ids.is_empty() {
            bail!("This sandbox backend cannot inject requested secrets yet.");
        }
        if !working_directory_allowed(&effective.workspace_read, &root, &cwd) {
            bail!(
                "Shell sandbox denied execution: working directory is not granted by the project policy"
            );
        }

        #[cfg(target_os = "windows")]
        {
            if let Some(log) = progress.as_ref().and_then(|p| p.log.as_ref()) {
                log.incomplete("Full transcripts are not supported by the Windows sandbox backend");
            }
            let marker = if options.track_cwd && options.direct.is_none() {
                Some(format!("__YEET_CWD_{}__", uuid::Uuid::new_v4().simple()))
            } else {
                None
            };
            let wrapped = if let Some(marker) = &marker {
                let shell = select(options.shell.as_deref().unwrap_or("auto"))?;
                Some(shell.wrap_cwd_trailer(command, marker))
            } else {
                None
            };
            let spec = execution_spec(wrapped.as_deref().unwrap_or(command), &options, None)?;
            return run_windows_sandboxed_shell(
                command,
                &spec,
                &cwd,
                &effective,
                capture_bytes,
                cancel,
                marker.as_deref(),
            );
        }
    }

    let scratch = tempfile::Builder::new()
        .prefix("yeet-model-shell-")
        .tempdir()?;
    // Keep cwd reporting out of stdout and the full transcript. Retain the
    // original open file rather than reopening a child-controlled path (which
    // could have been replaced with a symlink outside the sandbox).
    let mut cwd_report = if options.direct.is_none()
        && options.track_cwd
        && (unrestricted || effective.scratch_writable)
    {
        Some(tempfile::NamedTempFile::new_in(scratch.path())?)
    } else {
        None
    };
    let spec = execution_spec(command, &options, cwd_report.as_ref().map(|f| f.path()))?;
    #[cfg(unix)]
    let terminal = if options.pty {
        Some(open_terminal()?)
    } else {
        None
    };
    #[cfg(target_os = "macos")]
    let terminal_path = terminal
        .as_ref()
        .map(|(_, slave)| terminal_name(slave))
        .transpose()?;
    #[cfg(not(target_os = "macos"))]
    let terminal_path: Option<PathBuf> = None;
    let mut launch = if unrestricted {
        spec.command()
    } else {
        sandboxed_shell_command(
            &spec,
            &effective,
            &root,
            &cwd,
            scratch.path(),
            terminal_path.as_deref(),
        )?
    };
    if !options.load_config {
        launch.env_remove("ENV").env_remove("BASH_ENV");
    }
    if let Some(file) = &cwd_report
        && select(options.shell.as_deref().unwrap_or("auto"))?.kind == selection::ShellKind::Cmd
    {
        launch.env("YEET_INTERNAL_CWD_REPORT", file.path());
    }
    let progress = progress.unwrap_or_else(|| ShellProgress::new(8 * 1024));
    #[cfg(unix)]
    let mut master = if options.pty {
        use std::os::unix::process::CommandExt;
        let (master, slave) = terminal.expect("PTY allocated before sandbox setup");
        launch
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        unsafe {
            launch.pre_exec(|| {
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Some(master)
    } else {
        None
    };
    if !options.pty {
        configure_process_group(&mut launch);
        launch
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    }
    let child = launch.current_dir(&cwd).spawn().context(if unrestricted {
        "Unable to launch unrestricted shell command"
    } else {
        "Unable to launch sandboxed shell command"
    })?;
    let mut child = TrackedChild::new(child, "shell");
    let capture_limit = capture_bytes.clamp(1024, 4 * 1024 * 1024);
    let stdout_limit = capture_limit.min(effective.limits.max_stdout_bytes);
    let stderr_limit = capture_limit.min(effective.limits.max_stderr_bytes);
    #[cfg(unix)]
    let (stdout, stderr): (Box<dyn Read + Send>, Box<dyn Read + Send>) = if let Some(master) =
        master.take()
    {
        set_nonblocking(&master)?;
        *progress.terminal.lock().unwrap_or_else(|p| p.into_inner()) = Some(master.try_clone()?);
        (Box::new(master), Box::new(std::io::empty()))
    } else {
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        set_nonblocking(&stdout)?;
        set_nonblocking(&stderr)?;
        (Box::new(stdout), Box::new(stderr))
    };
    #[cfg(not(unix))]
    let (stdout, stderr): (Box<dyn Read + Send>, Box<dyn Read + Send>) = (
        Box::new(child.stdout.take().unwrap()),
        Box::new(child.stderr.take().unwrap()),
    );
    // Keep diagnostics even when execution ends before reader threads return.
    let stdout_progress = Some(progress.clone());
    let stderr_progress = Some(progress.clone());
    let output_stream = if options.pty {
        ShellStream::Pty
    } else {
        ShellStream::Stdout
    };
    let stdout_thread = thread::spawn(move || {
        read_bounded_with_progress(stdout, stdout_limit, stdout_progress, output_stream)
    });
    let stderr_thread = thread::spawn(move || {
        read_bounded_with_progress(stderr, stderr_limit, stderr_progress, ShellStream::Stderr)
    });
    let start = Instant::now();
    let timeout_seconds = if unrestricted && timeout_seconds == 0 {
        None
    } else if unrestricted {
        Some(timeout_seconds.clamp(1, 86_400))
    } else {
        Some(effective.limits.wall_time_seconds)
    };
    let timeout = timeout_seconds.map(Duration::from_secs);
    let mut cancelled = false;
    let mut timed_out = false;
    let status = loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            cancelled = true;
            break None;
        }
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if timeout.is_some_and(|timeout| start.elapsed() >= timeout) {
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let mut drain_deadline = None;
    // An exited shell can leave descendants holding its output descriptors.
    // Do not let a job with no caller deadline wait forever for those pipes.
    let exited_drain_deadline = status.map(|_| Instant::now() + Duration::from_secs(1));
    let mut abandoned_pipes = false;
    if cancelled || timed_out {
        let _ = child.terminate_shell();
        let _ = child.wait();
        drain_deadline = Some(Instant::now() + Duration::from_millis(250));
    }
    // Drain after termination, but do not leak a blocked reader if an escaped
    // descendant retains a pipe. Unix readers are nonblocking and cancellable.
    while !stdout_thread.is_finished() || !stderr_thread.is_finished() {
        if drain_deadline.is_none() {
            cancelled = cancel.is_some_and(|flag| flag.load(Ordering::Acquire));
            timed_out = timeout.is_some_and(|timeout| start.elapsed() >= timeout);
            abandoned_pipes =
                exited_drain_deadline.is_some_and(|deadline| Instant::now() >= deadline);
            if cancelled || timed_out || abandoned_pipes {
                let _ = child.terminate_shell();
                let _ = child.wait();
                drain_deadline = Some(Instant::now() + Duration::from_millis(250));
            }
        }
        if drain_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            progress.stop_readers.store(true, Ordering::Release);
            if let Some(log) = &progress.log {
                log.incomplete("Output pipe did not close after process termination");
            }
            #[cfg(unix)]
            {
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
            }
            if cancelled {
                bail!("cancelled");
            }
            if abandoned_pipes {
                bail!("Shell exited but descendants retained its output pipes");
            }
            bail!(shell_timeout_message(
                timeout_seconds.unwrap_or(0),
                &progress
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
    let stdout = stdout_thread
        .join()
        .unwrap_or_else(|_| BoundedOutput::new(stdout_limit))
        .into_capture();
    let stderr = stderr_thread
        .join()
        .unwrap_or_else(|_| BoundedOutput::new(stderr_limit))
        .into_capture();
    #[cfg(unix)]
    {
        *progress.terminal.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
    if cancelled {
        bail!("cancelled");
    }
    if timed_out {
        bail!(shell_timeout_message(
            timeout_seconds.unwrap_or(0),
            &progress
        ));
    }
    if abandoned_pipes {
        bail!("Shell exited but descendants retained its output pipes");
    }
    let status = status.expect("non-terminated shell has an exit status");
    Ok(ShellResult {
        command: command.into(),
        working_directory: display_path(&cwd),
        exit_code: status
            .code()
            .unwrap_or_else(|| 128 + status_signal(&status)),
        succeeded: status.success(),
        duration_milliseconds: start.elapsed().as_millis(),
        stdout_bytes: stdout.total_bytes,
        stderr_bytes: stderr.total_bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        stdout: (!stdout.text.is_empty()).then_some(stdout.text),
        stderr: (!stderr.text.is_empty()).then_some(stderr.text),
        final_working_directory: cwd_report.as_mut().and_then(|file| {
            file.seek(SeekFrom::Start(0)).ok()?;
            let mut path = String::new();
            file.take(16 * 1024).read_to_string(&mut path).ok()?;
            if cfg!(windows) {
                path = path
                    .trim_start_matches('\u{feff}')
                    .trim_end_matches(['\r', '\n'])
                    .to_owned();
            }
            (!path.is_empty() && !path.contains('\0')).then_some(path)
        }),
    })
}

fn shell_timeout_message(seconds: u64, progress: &ShellProgress) -> String {
    let snapshot = progress.snapshot();
    format!(
        "Shell command timed out after {seconds} seconds. Partial output (tails; stdout {} bytes, stderr {} bytes):\nSTDOUT:\n{}\nSTDERR:\n{}\nFor long-running work, use background=true and shell_job action=wait.",
        snapshot.stdout_bytes,
        snapshot.stderr_bytes,
        snapshot.stdout_tail.as_deref().unwrap_or_default(),
        snapshot.stderr_tail.as_deref().unwrap_or_default(),
    )
}

fn execution_spec(
    command: &str,
    options: &ShellSessionOptions,
    cwd_report: Option<&Path>,
) -> Result<LaunchSpec> {
    if let Some(spec) = &options.direct {
        #[cfg(windows)]
        spec.ensure_command_line_fits("Direct execution")?;
        return Ok(spec.clone());
    }
    let shell = select(options.shell.as_deref().unwrap_or("auto"))?;
    if options.login
        && matches!(
            shell.kind,
            selection::ShellKind::Powershell | selection::ShellKind::Cmd
        )
    {
        bail!("Login-shell behavior is not supported by the selected shell");
    }
    if options.login && !options.load_config && shell.kind == selection::ShellKind::Sh {
        bail!("A login sh may load user profiles; set loadConfig=true explicitly");
    }
    let wrapped = cwd_report.and_then(|path| shell.wrap_cwd(command, path));
    let spec = shell.launch(
        wrapped.as_deref().unwrap_or(command),
        options.load_config,
        options.login,
    );
    #[cfg(windows)]
    spec.ensure_command_line_fits("Shell command")?;
    Ok(spec)
}

#[allow(clippy::needless_return)]
fn sandboxed_shell_command(
    spec: &LaunchSpec,
    policy: &SandboxPolicy,
    root: &Path,
    cwd: &Path,
    scratch: &Path,
    terminal_path: Option<&Path>,
) -> Result<Command> {
    #[cfg(not(target_os = "macos"))]
    let _ = terminal_path;
    #[cfg(target_os = "macos")]
    {
        if !Path::new("/usr/bin/sandbox-exec").exists() {
            bail!("macOS sandbox-exec is unavailable; refusing unsandboxed model shell execution.");
        }
        let profile = sandbox_profile(policy, root, scratch, terminal_path)?;
        let mut value = Command::new("/usr/bin/sandbox-exec");
        value
            .arg("-p")
            .arg(profile)
            .arg(&spec.executable)
            .args(&spec.args);
        value
            .env_clear()
            .envs(sandbox_environment(policy, scratch, cwd));
        return Ok(value);
    }

    #[cfg(target_os = "linux")]
    {
        let command = serde_json::to_string(spec)?;
        let plan = linux_sandbox_plan(policy, root, scratch)?;
        let plan_path = scratch.join("sandbox-plan.json");
        std::fs::write(&plan_path, serde_json::to_vec(&plan)?)?;
        let executable =
            std::env::current_exe().context("resolve Yeet executable for Linux sandbox")?;
        let mut value = Command::new(executable);

        #[cfg(test)]
        value
            .arg("shell::tests::linux_sandbox_helper_entry")
            .arg("--exact")
            .arg("--nocapture")
            .env_clear()
            .envs(sandbox_environment(policy, scratch, cwd))
            .env("YEET_TEST_LINUX_SANDBOX_PLAN", &plan_path)
            .env("YEET_TEST_LINUX_SANDBOX_COMMAND", &command);

        #[cfg(not(test))]
        value
            .arg("__linux-sandbox-shell")
            .arg(&plan_path)
            .arg(command)
            .env_clear()
            .envs(sandbox_environment(policy, scratch, cwd));
        return Ok(value);
    }

    #[cfg(target_os = "windows")]
    {
        let _ = (spec, policy, root, cwd, scratch);
        bail!("internal error: Windows sandboxed shell must use the AppContainer path")
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    bail!("Shell sandboxing is not implemented on this platform")
}

#[cfg(target_os = "linux")]
#[derive(Debug, Serialize, Deserialize)]
struct LinuxSandboxGrant {
    path: PathBuf,
    writable: bool,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Serialize, Deserialize)]
struct LinuxSandboxPlan {
    grants: Vec<LinuxSandboxGrant>,
    block_network: bool,
}

#[cfg(target_os = "linux")]
fn linux_sandbox_plan(
    policy: &SandboxPolicy,
    root: &Path,
    scratch: &Path,
) -> Result<LinuxSandboxPlan> {
    let mut grants = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |path: PathBuf, writable: bool| {
        let path = path.canonicalize().unwrap_or(path);
        if path.exists() && seen.insert(path.clone()) {
            grants.push(LinuxSandboxGrant { path, writable });
        }
    };

    for path in trusted_tool_roots() {
        push(path, false);
    }
    for path in [
        PathBuf::from("/lib"),
        PathBuf::from("/lib64"),
        PathBuf::from("/etc/ld.so.cache"),
        PathBuf::from("/etc/ssl/certs"),
        PathBuf::from("/dev/null"),
        PathBuf::from("/dev/urandom"),
    ] {
        push(path, false);
    }
    push(scratch.to_path_buf(), policy.scratch_writable);

    match &policy.workspace_read {
        WorkspaceRead::None => {}
        WorkspaceRead::All => push(root.to_path_buf(), policy.workspace_writable),
        WorkspaceRead::Paths(paths) => {
            for relative in paths {
                let candidate = root.join(relative);
                let canonical = candidate.canonicalize().unwrap_or(candidate);
                if !canonical.starts_with(root) {
                    bail!("workspace read grant escaped workspace root: {relative}");
                }
                push(canonical, policy.workspace_writable);
            }
        }
    }

    Ok(LinuxSandboxPlan {
        grants,
        block_network: true,
    })
}

#[cfg(target_os = "linux")]
pub fn run_linux_sandbox_shell_helper(plan_path: &Path, command: &str) -> Result<i32> {
    use nono::{AccessMode, CapabilitySet, Sandbox};

    let plan: LinuxSandboxPlan = serde_json::from_slice(&std::fs::read(plan_path)?)?;
    let mut caps = CapabilitySet::new();
    for grant in plan.grants {
        let mode = if grant.writable {
            AccessMode::ReadWrite
        } else {
            AccessMode::Read
        };
        caps = if grant.path.is_dir() {
            caps.allow_path(&grant.path, mode)?
        } else if grant.path.is_file() {
            caps.allow_file(&grant.path, mode)?
        } else {
            continue;
        };
    }
    if plan.block_network {
        caps = caps.block_network();
    }
    Sandbox::apply_auto(&caps).map_err(|error| {
        anyhow::anyhow!(
            "Linux sandbox unavailable: {error}. Sandboxed shell execution fails closed. If this container is intentionally trusted and unrestricted shell execution is acceptable, opt in explicitly with `yeet sandbox mode unlimited`."
        )
    })?;

    let spec: LaunchSpec = serde_json::from_str(command)?;
    let status = spec
        .command()
        .status()
        .context("launch Linux sandbox shell")?;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status_signal(&status)))
}

#[cfg(any(target_os = "windows", test))]
fn split_cwd_trailer<'a>(output: &'a str, marker: &str) -> (&'a str, Option<String>) {
    // A missing or malformed trailer never changes the session cwd or output.
    let separator = format!("\n{marker}\n");
    let normalized = output.replace("\r\n", "\n");
    let Some(index) = normalized.rfind(&separator) else {
        return (output, None);
    };
    let path = &normalized[index + separator.len()..];
    let path = path.strip_suffix('\n').unwrap_or(path);
    if path.is_empty() || path.contains(['\r', '\n', '\0']) {
        return (output, None);
    }
    // Find the corresponding separator in the original without altering user output.
    let windows_separator = format!("\r\n{marker}\r\n");
    let Some(original_index) = output
        .rfind(&windows_separator)
        .or_else(|| output.rfind(&separator))
    else {
        return (output, None);
    };
    (&output[..original_index], Some(path.to_owned()))
}

#[cfg(target_os = "windows")]
fn run_windows_sandboxed_shell(
    command: &str,
    spec: &LaunchSpec,
    cwd: &Path,
    policy: &SandboxPolicy,
    capture_bytes: usize,
    cancel: Option<&AtomicBool>,
    cwd_marker: Option<&str>,
) -> Result<ShellResult> {
    use io_harness::{
        ExecMode,
        sandbox::{RunSpec, Sandbox, SandboxConfig, SandboxLimits, select},
    };

    let executable =
        std::env::current_exe().context("resolve Yeet executable for Windows sandbox")?;
    let environment = serde_json::to_string(&policy.environment)?;
    if environment.encode_utf16().count() > 6_000 {
        bail!("Sandbox environment is too large for the Windows process command line");
    }
    let argv = vec![
        executable.to_string_lossy().into_owned(),
        "__windows-sandbox-shell".to_owned(),
        serde_json::to_string(spec)?,
        environment,
    ];
    let command_line_units: usize = argv.iter().map(|arg| arg.encode_utf16().count() + 3).sum();
    if command_line_units > 30_000 {
        bail!(
            "Sandboxed shell command is too long for a Windows process command line ({command_line_units} UTF-16 units, limit 30,000). Write the script to a file in the workspace and run it with a short command instead."
        );
    }
    let mode = if policy.workspace_writable {
        ExecMode::WorkspaceWrite
    } else {
        ExecMode::ReadOnly
    };
    let limits = SandboxLimits {
        max_cpu_secs: None,
        max_wall_secs: Some(policy.limits.wall_time_seconds),
        max_memory_bytes: Some(policy.limits.max_memory_bytes),
        max_processes: Some(policy.limits.max_processes as u64),
        max_open_files: None,
    };
    let mut config = SandboxConfig::new()
        .with_access_confinement()
        .with_mode(mode);
    config.limits = limits.clone();
    let sandbox = select(&config);
    let spec = RunSpec::new(&argv, cwd, &limits)
        .with_mode(mode)
        .with_network(false);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .context("initialize Windows sandbox runtime")?;
    let started = Instant::now();
    let outcome = runtime.block_on(async {
        let future = sandbox.run(spec);
        tokio::pin!(future);
        loop {
            tokio::select! {
                result = &mut future => break result.map_err(anyhow::Error::from),
                _ = tokio::time::sleep(Duration::from_millis(20)) => {
                    if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                        break Err(anyhow::anyhow!("cancelled"));
                    }
                }
            }
        }
    })?;

    let succeeded = outcome.success();
    let exit_code = outcome.exit_code.unwrap_or(1);
    let capture_limit = capture_bytes.clamp(1024, 4 * 1024 * 1024);
    let stdout_limit = capture_limit.min(policy.limits.max_stdout_bytes);
    let stderr_limit = capture_limit.min(policy.limits.max_stderr_bytes);
    let (output, final_working_directory) = match cwd_marker {
        Some(marker) => split_cwd_trailer(&outcome.stdout, marker),
        None => (outcome.stdout.as_str(), None),
    };
    let mut stdout = BoundedOutput::new(stdout_limit);
    stdout.push(output.as_bytes());
    let stdout = stdout.into_capture();
    let mut stderr = BoundedOutput::new(stderr_limit);
    stderr.push(outcome.stderr.as_bytes());
    let stderr = stderr.into_capture();

    Ok(ShellResult {
        command: command.to_owned(),
        working_directory: display_path(cwd),
        exit_code,
        succeeded,
        duration_milliseconds: started.elapsed().as_millis(),
        stdout_bytes: stdout.total_bytes,
        stderr_bytes: stderr.total_bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        stdout: (!stdout.text.is_empty()).then_some(stdout.text),
        stderr: (!stderr.text.is_empty()).then_some(stderr.text),
        final_working_directory,
    })
}

#[cfg(target_os = "windows")]
pub fn run_windows_sandbox_shell_helper(command: &str, environment_json: &str) -> Result<i32> {
    let requested: BTreeMap<String, String> = serde_json::from_str(environment_json)?;
    let original_home = std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir);
    let mut inherited = [
        "PATH",
        "PATHEXT",
        "SystemRoot",
        "WINDIR",
        "ComSpec",
        "TEMP",
        "TMP",
        "PROCESSOR_ARCHITECTURE",
        "NUMBER_OF_PROCESSORS",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "PNPM_HOME",
        "BUN_INSTALL",
    ]
    .into_iter()
    .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
    .collect::<BTreeMap<_, _>>();
    if let Some(home) = original_home {
        inherited
            .entry("CARGO_HOME".to_owned())
            .or_insert_with(|| home.join(".cargo").display().to_string());
        inherited
            .entry("RUSTUP_HOME".to_owned())
            .or_insert_with(|| home.join(".rustup").display().to_string());
    }
    let temporary = inherited
        .get("TEMP")
        .or_else(|| inherited.get("TMP"))
        .cloned()
        .unwrap_or_else(|| ".".to_owned());
    let spec: LaunchSpec = serde_json::from_str(command)?;
    let mut child = spec.command();
    child
        .env_clear()
        .envs(inherited)
        .envs(requested)
        .env("HOME", &temporary)
        .env("USERPROFILE", &temporary)
        .env("YEET_SANDBOX", "1");
    let status = child
        .status()
        .context("launch Windows AppContainer shell")?;
    Ok(status.code().unwrap_or(1))
}

fn trusted_maximum(requested: &SandboxPolicy, allow_write: bool) -> SandboxPolicy {
    let safe_environment = requested
        .environment
        .iter()
        .filter(|(key, _)| safe_environment_key(key))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    SandboxPolicy {
        mode: SandboxMode::Sandboxed,
        auto_approve: false,
        workspace_read: WorkspaceRead::All,
        workspace_writable: allow_write,
        scratch_writable: true,
        network_allow: Default::default(),
        environment: safe_environment,
        secret_ids: Default::default(),
        limits: crate::harness::sandbox::SandboxLimits {
            wall_time_seconds: 86_400,
            max_stdout_bytes: 4 * 1024 * 1024,
            max_stderr_bytes: 4 * 1024 * 1024,
            max_memory_bytes: 4 * 1024 * 1024 * 1024,
            max_processes: 128,
        },
    }
}

fn safe_environment_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    !matches!(
        upper.as_str(),
        "HOME" | "PATH" | "SHELL" | "TMPDIR" | "TMP" | "TEMP"
    ) && !upper.starts_with("DYLD_")
        && !upper.starts_with("LD_")
        && !upper.starts_with("__XPC_")
}

fn resolve_working_directory(
    root: &Path,
    requested: Option<&str>,
    allow_outside: bool,
) -> Result<PathBuf> {
    let value = requested.unwrap_or(".").trim();
    if value.is_empty() || value == "." {
        return Ok(root.to_path_buf());
    }
    let path = Path::new(value);
    if path.is_absolute() {
        let candidate = path
            .canonicalize()
            .context("Invalid shell working directory")?;
        if !candidate.is_dir() {
            bail!("Invalid shell working directory: {value}");
        }
        if !allow_outside && candidate != root && !candidate.starts_with(root) {
            bail!("Shell working directory is outside the current workspace: {value}");
        }
        return Ok(candidate);
    }
    if allow_outside {
        let candidate = root
            .join(path)
            .canonicalize()
            .context("Invalid shell working directory")?;
        if !candidate.is_dir() {
            bail!("Invalid shell working directory: {value}");
        }
        return Ok(candidate);
    }
    let relative = validate_relative_path(value)?;
    let candidate = root
        .join(relative)
        .canonicalize()
        .context("Invalid shell working directory")?;
    if !candidate.is_dir() || !(candidate == root || candidate.starts_with(root)) {
        bail!("Invalid shell working directory: {value}");
    }
    Ok(candidate)
}

fn working_directory_allowed(scope: &WorkspaceRead, root: &Path, cwd: &Path) -> bool {
    if cwd == root {
        // Choosing the workspace root as cwd does not itself grant access to
        // its contents; the OS sandbox still enforces WorkspaceRead below.
        // Rejecting `.` here made otherwise-valid commands fail before launch.
        return true;
    }
    let Ok(relative) = cwd.strip_prefix(root) else {
        return false;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    scope.allows(&relative)
}

#[cfg(target_os = "macos")]
fn sandbox_profile(
    policy: &SandboxPolicy,
    root: &Path,
    scratch: &Path,
    terminal_path: Option<&Path>,
) -> Result<String> {
    let mut rules = vec![
        "(version 1)".into(),
        "(deny default)".into(),
        "(import \"system.sb\")".into(),
        "(allow process-exec)".into(),
        "(allow process-fork)".into(),
        "(allow signal (target self))".into(),
        "(allow file-read-metadata file-test-existence)".into(),
    ];
    // Grant terminal ioctls only on this job's already-open slave, not on
    // arbitrary devices. Applications need these for termios and window size.
    if let Some(path) = terminal_path {
        rules.push(format!(
            "(allow file-ioctl (literal \"{}\"))",
            escape_path(path)
        ));
    }
    for tool_root in trusted_tool_roots() {
        rules.push(format!(
            "(allow file-read* file-test-existence file-map-executable (subpath \"{}\"))",
            escape_path(&tool_root)
        ));
    }
    let scratch = scratch
        .canonicalize()
        .unwrap_or_else(|_| scratch.to_path_buf());
    rules.push(format!(
        "(allow file-read-metadata file-test-existence (path-ancestors \"{}\"))",
        escape_path(&scratch)
    ));
    rules.push(format!(
        "(allow file-read* {} file-test-existence (subpath \"{}\"))",
        if policy.scratch_writable {
            "file-write*"
        } else {
            ""
        },
        escape_path(&scratch)
    ));
    match &policy.workspace_read {
        WorkspaceRead::None => {}
        WorkspaceRead::All => add_workspace_rule(&mut rules, root, policy.workspace_writable),
        WorkspaceRead::Paths(paths) => {
            for path in paths {
                let absolute = root
                    .join(path)
                    .canonicalize()
                    .unwrap_or_else(|_| root.join(path));
                if !absolute.starts_with(root) {
                    bail!("workspace read grant escaped workspace root: {path}");
                }
                add_workspace_rule(&mut rules, &absolute, policy.workspace_writable);
            }
        }
    }
    Ok(rules.join("\n"))
}

#[cfg(target_os = "macos")]
fn add_workspace_rule(rules: &mut Vec<String>, path: &Path, writable: bool) {
    let escaped = escape_path(path);
    rules.push(format!(
        "(allow file-read-metadata file-test-existence (path-ancestors \"{escaped}\"))"
    ));
    rules.push(format!(
        "(allow file-read* file-test-existence file-map-executable (subpath \"{escaped}\"))"
    ));
    if writable {
        rules.push(format!("(allow file-write* (subpath \"{escaped}\"))"));
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn sandbox_environment(
    policy: &SandboxPolicy,
    scratch: &Path,
    cwd: &Path,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let mut path_entries = Vec::new();
    for root in trusted_tool_roots() {
        if root.ends_with("bin") || root.ends_with("sbin") {
            path_entries.push(root.display().to_string());
        } else if root == Path::new("/opt/homebrew") || root == Path::new("/usr/local") {
            path_entries.push(root.join("bin").display().to_string());
            path_entries.push(root.join("sbin").display().to_string());
        }
    }
    env.insert("HOME".into(), scratch.display().to_string());
    env.insert("TMPDIR".into(), format!("{}/", scratch.display()));
    env.insert("PATH".into(), path_entries.join(":"));
    env.insert("LANG".into(), "en_US.UTF-8".into());
    env.insert("TERM".into(), "dumb".into());
    env.insert("YEET_SANDBOX".into(), "1".into());
    env.insert("PWD".into(), cwd.display().to_string());
    if let Some(home) = dirs::home_dir() {
        let cargo_home = home.join(".cargo");
        let rustup_home = home.join(".rustup");
        if cargo_home.is_dir() {
            env.insert("CARGO_HOME".into(), cargo_home.display().to_string());
        }
        if rustup_home.is_dir() {
            env.insert("RUSTUP_HOME".into(), rustup_home.display().to_string());
        }
    }
    for (key, value) in &policy.environment {
        if safe_environment_key(key) {
            env.insert(key.clone(), value.clone());
        }
    }
    env
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn trusted_tool_roots() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = [
        "/bin",
        "/sbin",
        "/usr/bin",
        "/usr/sbin",
        "/usr/libexec",
        "/usr/local",
        "/opt/homebrew",
        "/Applications/Xcode.app",
        "/Library/Developer",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = dirs::home_dir() {
        for path in [
            ".cargo/bin",
            ".cargo/registry",
            ".cargo/git",
            ".rustup",
            ".rustup/toolchains",
            ".local/bin",
            ".bun/bin",
            "Library/pnpm",
        ] {
            candidates.push(home.join(path));
        }
    }
    if let Some(developer) = std::env::var_os("DEVELOPER_DIR") {
        candidates.push(PathBuf::from(developer));
    }
    candidates
        .into_iter()
        .filter(|path| path.exists())
        .collect()
}

#[cfg(target_os = "macos")]
fn escape_path(path: &Path) -> String {
    path.display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

#[derive(Debug)]
struct BoundedOutput {
    head: Vec<u8>,
    tail: TailBuffer,
    total_bytes: usize,
    limit: usize,
}

impl BoundedOutput {
    fn new(limit: usize) -> Self {
        let head_limit = limit / 2;
        Self {
            head: Vec::with_capacity(head_limit),
            tail: TailBuffer::new(limit - head_limit),
            total_bytes: 0,
            limit,
        }
    }

    fn push(&mut self, mut bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        let head_limit = self.limit / 2;
        if self.head.len() < head_limit {
            let take = bytes.len().min(head_limit - self.head.len());
            self.head.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
        }
        self.tail.push(bytes);
    }

    fn into_capture(self) -> CapturedOutput {
        let truncated = self.total_bytes > self.limit;
        let total_bytes = self.total_bytes;
        let tail = self.tail.into_ordered();
        let text = match utf16_bom(&self.head) {
            Some(endian) => {
                // Windows console tools (wmic, reg query, some installers)
                // emit UTF-16 with a BOM into pipes; decoding that as UTF-8
                // would mangle every character.
                let head = utf16_decode(self.head.get(2..).unwrap_or_default(), endian);
                let tail_text = utf16_decode(
                    if (total_bytes - tail.len()) % 2 == 1 {
                        tail.get(1..).unwrap_or_default()
                    } else {
                        &tail
                    },
                    endian,
                );
                if truncated {
                    format!(
                        "{}\n... [{} bytes omitted] ...\n{}",
                        head,
                        total_bytes.saturating_sub(self.head.len() + tail.len()),
                        tail_text
                    )
                } else {
                    format!("{head}{tail_text}")
                }
            }
            None if truncated => {
                let head = utf8_prefix_without_partial_suffix(&self.head);
                let tail = utf8_suffix_without_partial_prefix(&tail);
                let omitted = total_bytes.saturating_sub(head.len() + tail.len());
                format!(
                    "{}\n... [{omitted} bytes omitted] ...\n{}",
                    String::from_utf8_lossy(head),
                    String::from_utf8_lossy(tail)
                )
            }
            None => {
                let mut bytes = self.head;
                bytes.extend_from_slice(&tail);
                String::from_utf8_lossy(&bytes).into_owned()
            }
        };
        CapturedOutput {
            total_bytes,
            truncated,
            text,
        }
    }
}

#[derive(Clone, Copy)]
enum Utf16Endian {
    Le,
    Be,
}

fn utf16_bom(bytes: &[u8]) -> Option<Utf16Endian> {
    if bytes.starts_with(&[0xff, 0xfe]) {
        Some(Utf16Endian::Le)
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        Some(Utf16Endian::Be)
    } else {
        None
    }
}

fn utf16_decode(bytes: &[u8], endian: Utf16Endian) -> String {
    let units = bytes
        .chunks_exact(2)
        .map(|pair| match endian {
            Utf16Endian::Le => u16::from_le_bytes([pair[0], pair[1]]),
            Utf16Endian::Be => u16::from_be_bytes([pair[0], pair[1]]),
        })
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&units)
}

#[derive(Debug)]
struct CapturedOutput {
    total_bytes: usize,
    truncated: bool,
    text: String,
}

#[derive(Debug)]
struct TailBuffer {
    bytes: Vec<u8>,
    capacity: usize,
    start: usize,
}

impl TailBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
            capacity,
            start: 0,
        }
    }

    fn push(&mut self, mut bytes: &[u8]) {
        if self.capacity == 0 || bytes.is_empty() {
            return;
        }
        if self.bytes.len() < self.capacity {
            let take = bytes.len().min(self.capacity - self.bytes.len());
            self.bytes.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if bytes.is_empty() {
                return;
            }
        }
        if bytes.len() >= self.capacity {
            self.bytes.clear();
            self.bytes
                .extend_from_slice(&bytes[bytes.len() - self.capacity..]);
            self.start = 0;
            return;
        }
        let first = bytes.len().min(self.capacity - self.start);
        self.bytes[self.start..self.start + first].copy_from_slice(&bytes[..first]);
        if first < bytes.len() {
            self.bytes[..bytes.len() - first].copy_from_slice(&bytes[first..]);
        }
        self.start = (self.start + bytes.len()) % self.capacity;
    }

    fn ordered(&self) -> Vec<u8> {
        let mut bytes = self.bytes.clone();
        if bytes.len() == self.capacity && self.start != 0 {
            bytes.rotate_left(self.start);
        }
        bytes
    }

    fn into_ordered(mut self) -> Vec<u8> {
        if self.bytes.len() == self.capacity && self.start != 0 {
            self.bytes.rotate_left(self.start);
        }
        self.bytes
    }
}

#[cfg(target_os = "macos")]
fn terminal_name(file: &std::fs::File) -> Result<PathBuf> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let mut buffer = [0u8; 1024];
    let error =
        unsafe { libc::ttyname_r(file.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
    if error != 0 {
        return Err(std::io::Error::from_raw_os_error(error).into());
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| anyhow::anyhow!("PTY name is not terminated"))?;
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&buffer[..end])))
}

#[cfg(unix)]
fn open_terminal() -> Result<(std::fs::File, std::fs::File)> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let (mut master, mut slave) = (-1, -1);
    let mut size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    } == -1
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let master = unsafe { std::fs::File::from_raw_fd(master) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave) };
    for file in [&master, &slave] {
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok((master, slave))
}

#[cfg(unix)]
fn set_nonblocking(file: &impl std::os::fd::AsRawFd) -> std::io::Result<()> {
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn read_bounded_with_progress(
    mut reader: impl Read,
    limit: usize,
    progress: Option<ShellProgress>,
    stream: ShellStream,
) -> BoundedOutput {
    let mut output = BoundedOutput::new(limit);
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        if progress
            .as_ref()
            .is_some_and(|p| p.stop_readers.load(Ordering::Acquire))
        {
            break;
        }
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let bytes = &buffer[..read];
                if let Some(progress) = progress.as_ref() {
                    progress.push(stream, bytes);
                }
                output.push(bytes);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            #[cfg(target_os = "linux")]
            Err(error)
                if matches!(stream, ShellStream::Pty)
                    && error.raw_os_error() == Some(libc::EIO) =>
            {
                break;
            }
            Err(error) => {
                if let Some(log) = progress.as_ref().and_then(|p| p.log.as_ref()) {
                    log.incomplete(error);
                }
                break;
            }
        }
    }
    output
}

fn utf8_prefix_without_partial_suffix(bytes: &[u8]) -> &[u8] {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes,
        Err(error) if error.error_len().is_none() => &bytes[..error.valid_up_to()],
        // Invalid bytes inside the output are data, not a split UTF-8 boundary.
        Err(_) => bytes,
    }
}

fn utf8_suffix_without_partial_prefix(bytes: &[u8]) -> &[u8] {
    // Only discard leading continuation bytes from a split codepoint. Decode
    // invalid interior bytes lossily rather than throwing away the entire tail.
    let skip = bytes
        .iter()
        .take(3)
        .take_while(|byte| (**byte & 0xc0) == 0x80)
        .count();
    &bytes[skip..]
}

#[cfg(unix)]
fn status_signal(status: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status.signal().unwrap_or(0)
}
#[cfg(not(unix))]
fn status_signal(_: &std::process::ExitStatus) -> i32 {
    0
}

pub(crate) fn split_shell_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            current.push(ch);
            continue;
        }
        if let Some(active) = quote {
            current.push(ch);
            if ch == active {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            current.push(ch);
            continue;
        }
        if matches!(ch, ';' | '|' | '&' | '\n' | '\r') {
            if !current.trim().is_empty() {
                segments.push(current.trim().to_owned());
            }
            current.clear();
        } else {
            current.push(ch);
        }
    }
    if !current.trim().is_empty() {
        segments.push(current.trim().to_owned());
    }
    segments
}

pub fn command_words(segment: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in segment.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod output_regressions {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn exited_shell_with_inherited_pipe_does_not_hang() {
        let root = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let result = run_shell("sleep 30 & exit", root.path(), None, 0, 4096, false, true);
        assert!(started.elapsed() < Duration::from_secs(6));
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("descendants retained")
        );
    }

    #[test]
    fn bounded_output_preserves_invalid_bytes_and_diagnostics() {
        let mut output = BoundedOutput::new(32);
        output.push(b"start\xff important header");
        output.push(&[b'x'; 100]);
        output.push(b"\xff fatal: broken\n");
        let captured = output.into_capture();
        assert!(captured.truncated);
        assert!(captured.text.contains("important"));
        assert!(captured.text.contains("fatal: broken"));
    }

    #[test]
    fn split_utf8_boundaries_are_removed_without_losing_text() {
        assert_eq!(utf8_prefix_without_partial_suffix(b"ok\xe2\x82"), b"ok");
        assert_eq!(utf8_suffix_without_partial_prefix(b"\x82\xacok"), b"ok");
    }

    #[test]
    fn utf16_console_output_decodes_and_respects_code_unit_boundaries() {
        let mut wide = vec![0xff, 0xfe];
        for unit in "héllo".encode_utf16() {
            wide.extend_from_slice(&unit.to_le_bytes());
        }
        let mut output = BoundedOutput::new(2048);
        output.push(&wide);
        let captured = output.into_capture();
        assert!(!captured.truncated);
        assert_eq!(captured.text, "héllo");

        // A truncated capture keeps decoding the retained tail from a
        // code-unit boundary instead of leaking half a code point.
        let mut output = BoundedOutput::new(8);
        output.push(&wide);
        let captured = output.into_capture();
        assert!(captured.truncated, "{captured:?}");
        assert!(captured.text.starts_with('h'), "{captured:?}");
        assert!(captured.text.ends_with("lo"), "{captured:?}");
    }

    #[cfg(unix)]
    #[test]
    fn deadline_preserves_both_output_streams() {
        let root = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let error = run_shell(
            "printf 'before timeout'; printf 'failure detail' >&2; sleep 10",
            root.path(),
            None,
            1,
            4096,
            false,
            true,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("timed out after 1 seconds"), "{error}");
        assert!(error.contains("before timeout"), "{error}");
        assert!(error.contains("failure detail"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_sandbox_helper_entry() {
        if let (Ok(plan), Ok(command)) = (
            std::env::var("YEET_TEST_LINUX_SANDBOX_PLAN"),
            std::env::var("YEET_TEST_LINUX_SANDBOX_COMMAND"),
        ) {
            std::process::exit(
                run_linux_sandbox_shell_helper(Path::new(&plan), &command).unwrap_or(1),
            );
        }
    }
    #[test]
    fn sandbox_cwd_trailer_keeps_output_and_rejects_incomplete_reports() {
        let (output, cwd) = split_cwd_trailer("hello\r\nMARK\r\nC:\\a b\r\n", "MARK");
        assert_eq!(output, "hello");
        assert_eq!(cwd.as_deref(), Some("C:\\a b"));
        assert_eq!(
            split_cwd_trailer("hello\nMARK\n", "MARK"),
            ("hello\nMARK\n", None)
        );
        assert_eq!(split_cwd_trailer("untouched", "MARK"), ("untouched", None));
    }
    #[cfg(unix)]
    #[test]
    fn failed_shell_command_is_not_retried() {
        let root = tempfile::tempdir().unwrap();
        let request = ShellExecutionRequest {
            command: "printf x >> attempts; false",
            workspace_root: root.path(),
            working_directory: None,
            timeout_seconds: 10,
            capture_bytes: 4096,
            allow_write: false,
            unrestricted: true,
            force_sandboxed: false,
            cancel: None,
        };
        let result = run_shell_session(
            request,
            ShellSessionOptions {
                shell: Some("sh".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!result.succeeded);
        assert_eq!(std::fs::read(root.path().join("attempts")).unwrap(), b"x");
    }
    #[cfg(windows)]
    #[test]
    fn windows_shells_round_trip_unicode_and_quoted_arguments() {
        for (shell, command) in [
            ("cmd", "echo \"quoted & spaced\" & echo ünïcødé ✓"),
            (
                "powershell",
                "Write-Output \"quoted & spaced\"; Write-Output \"ünïcødé ✓\"",
            ),
            ("bash", "echo 'quoted & spaced'; echo 'ünïcødé ✓'"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let request = ShellExecutionRequest {
                command,
                workspace_root: root.path(),
                working_directory: None,
                timeout_seconds: 30,
                capture_bytes: 8192,
                allow_write: false,
                unrestricted: true,
                force_sandboxed: false,
                cancel: None,
            };
            let outcome = run_shell_session(
                request,
                ShellSessionOptions {
                    shell: Some(shell.into()),
                    ..Default::default()
                },
            );
            let result = match outcome {
                Ok(result) => result,
                // Git Bash is optional on a Windows machine.
                Err(error) if shell == "bash" && error.to_string().contains("unavailable") => {
                    eprintln!("Git Bash not installed; skipping bash round trip");
                    continue;
                }
                Err(error) => panic!("{shell}: {error}"),
            };
            assert!(result.succeeded, "{shell}: {result:?}");
            let stdout = result.stdout.unwrap_or_default();
            assert!(stdout.contains("quoted & spaced"), "{shell}: {stdout:?}");
            assert!(stdout.contains("ünïcødé"), "{shell}: {stdout:?}");
        }
    }
    #[cfg(windows)]
    #[test]
    fn windows_working_directories_are_case_corrected_and_boundary_checked() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("MixedCase")).unwrap();
        // Drive-letter absolute paths resolve across case differences.
        let resolved = resolve_working_directory(
            &root,
            Some(&root.join("mixedcase").display().to_string()),
            false,
        )
        .unwrap();
        assert_eq!(resolved, root.join("MixedCase"));
        // Drive-relative and traversal inputs cannot smuggle a working
        // directory outside the workspace.
        for bad in [
            format!(
                "{}sub",
                display_path(&root).chars().take(2).collect::<String>()
            ),
            r"..\..".to_owned(),
            r"sub\..\..".to_owned(),
        ] {
            assert!(
                resolve_working_directory(&root, Some(&bad), false).is_err(),
                "accepted {bad}"
            );
        }
        // A junction pointing outside the workspace is resolved and refused.
        let outside = tempfile::tempdir().unwrap();
        let link = root.join("escape");
        let status = std::process::Command::new("cmd")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        if status.success() {
            assert!(
                resolve_working_directory(&root, Some(&link.display().to_string()), false).is_err(),
                "junction escape was accepted"
            );
        }
    }
    // ConPTY is not implemented yet. Both policies must fail closed rather
    // than starting an ordinary, potentially unrestricted process.
    #[cfg(windows)]
    #[test]
    fn windows_pty_is_rejected_before_any_command_is_launched() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("should-not-exist");
        let command = format!("echo launched > \"{}\"", marker.display());
        for unrestricted in [false, true] {
            let request = ShellExecutionRequest {
                command: &command,
                workspace_root: root.path(),
                working_directory: None,
                timeout_seconds: 10,
                capture_bytes: 4096,
                allow_write: true,
                unrestricted,
                force_sandboxed: !unrestricted,
                cancel: None,
            };
            let error = run_shell_session(
                request,
                ShellSessionOptions {
                    pty: true,
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert!(error.to_string().contains("PTY execution is not supported"));
            assert!(!marker.exists());
        }
    }
    #[cfg(unix)]
    #[test]
    fn direct_arguments_and_sandbox_permission() {
        let root = tempfile::tempdir().unwrap();
        let request = |unrestricted, allow_write| ShellExecutionRequest {
            command: "direct process",
            workspace_root: root.path(),
            working_directory: None,
            timeout_seconds: 10,
            capture_bytes: 4096,
            allow_write,
            unrestricted,
            force_sandboxed: !unrestricted,
            cancel: None,
        };
        let options = || ShellSessionOptions {
            direct: Some(LaunchSpec {
                executable: "/bin/echo".into(),
                args: vec!["a b;$(echo injected)".into()],
                verbatim: false,
            }),
            track_cwd: true,
            ..Default::default()
        };
        let output = run_shell_session(request(true, false), options()).unwrap();
        assert_eq!(output.stdout.as_deref(), Some("a b;$(echo injected)\n"));
        assert!(output.final_working_directory.is_none());
        assert!(
            run_shell_session(request(false, false), options())
                .unwrap_err()
                .to_string()
                .contains("requires explicit permission")
        );
    }
    #[cfg(unix)]
    #[test]
    fn direct_process_cannot_write_outside_sandbox() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("forbidden");
        let request = ShellExecutionRequest {
            command: "direct process",
            workspace_root: root.path(),
            working_directory: None,
            timeout_seconds: 10,
            capture_bytes: 4096,
            allow_write: true,
            unrestricted: false,
            force_sandboxed: true,
            cancel: None,
        };
        let options = ShellSessionOptions {
            direct: Some(LaunchSpec {
                executable: "/usr/bin/touch".into(),
                args: vec![target.to_string_lossy().into_owned()],
                verbatim: false,
            }),
            ..Default::default()
        };
        let result = run_shell_session(request, options).unwrap();
        assert!(!result.succeeded);
        assert!(!target.exists());
    }
}
