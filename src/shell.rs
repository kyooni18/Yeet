use std::{
    collections::{BTreeMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use anyhow::{Context, Result, bail};
#[cfg(target_os = "linux")]
use serde::Deserialize;
use serde::Serialize;

use crate::sandbox::{
    SandboxMode, SandboxPolicy, SandboxStore, WorkspaceRead, validate_relative_path,
};

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
}

pub fn restricted_operation(command: &str) -> Option<String> {
    let write_executables: HashSet<&str> = [
        "rm",
        "mv",
        "cp",
        "mkdir",
        "rmdir",
        "touch",
        "truncate",
        "tee",
        "install",
        "ln",
        "chmod",
        "chown",
        "chgrp",
        "chflags",
        "xattr",
        "patch",
        "ed",
        "ex",
        "vi",
        "vim",
        "nvim",
        "nano",
        "emacs",
        "del",
        "erase",
        "copy",
        "move",
        "md",
        "rd",
        "ren",
        "rename",
        "remove-item",
        "move-item",
        "copy-item",
        "new-item",
        "set-content",
        "add-content",
        "clear-content",
        "rename-item",
        "set-item",
        "out-file",
    ]
    .into_iter()
    .collect();
    let git_mutating: HashSet<&str> = [
        "add",
        "am",
        "apply",
        "branch",
        "checkout",
        "cherry-pick",
        "clean",
        "clone",
        "commit",
        "config",
        "fetch",
        "gc",
        "init",
        "merge",
        "mv",
        "pull",
        "push",
        "rebase",
        "reset",
        "restore",
        "revert",
        "rm",
        "stash",
        "submodule",
        "switch",
        "tag",
        "worktree",
    ]
    .into_iter()
    .collect();
    let dependency_tools: HashSet<&str> = [
        "npm", "pnpm", "yarn", "bun", "cargo", "pip", "pip3", "gem", "brew",
    ]
    .into_iter()
    .collect();
    let dependency_mutating: HashSet<&str> = [
        "add",
        "install",
        "uninstall",
        "remove",
        "rm",
        "update",
        "upgrade",
        "link",
        "unlink",
        "publish",
        "pack",
        "prune",
        "dedupe",
    ]
    .into_iter()
    .collect();

    for segment in split_shell_segments(command) {
        let words = command_words(&segment);
        if words.is_empty() {
            continue;
        }
        let mut index = 0;
        while index < words.len()
            && (is_assignment(&words[index])
                || matches!(
                    words[index].as_str(),
                    "command" | "exec" | "env" | "nice" | "nohup" | "time" | "sudo"
                ))
        {
            index += 1;
        }
        let Some(raw) = words.get(index) else {
            continue;
        };
        let executable = Path::new(raw)
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or(raw)
            .to_ascii_lowercase();
        if write_executables.contains(executable.as_str()) {
            return Some(executable);
        }
        let rest = &words[index + 1..];
        if executable == "sed"
            && rest.iter().any(|word| {
                word == "--in-place"
                    || word.starts_with("--in-place=")
                    || (word.starts_with('-') && !word.starts_with("--") && word[1..].contains('i'))
            })
        {
            return Some("sed -i".into());
        }
        if executable == "git" {
            let mut skip_next = false;
            for word in rest {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if matches!(
                    word.as_str(),
                    "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path"
                ) {
                    skip_next = true;
                    continue;
                }
                if word.starts_with('-') {
                    continue;
                }
                if git_mutating.contains(word.to_ascii_lowercase().as_str()) {
                    return Some(format!("git {word}"));
                }
                break;
            }
        }
        if dependency_tools.contains(executable.as_str())
            && let Some(subcommand) = rest.iter().find(|word| !word.starts_with('-'))
            && dependency_mutating.contains(subcommand.to_ascii_lowercase().as_str())
        {
            return Some(format!("{executable} {subcommand}"));
        }
        if matches!(executable.as_str(), "sh" | "bash" | "zsh") {
            for pair in rest.windows(2) {
                if pair[0].starts_with('-')
                    && pair[0].contains('c')
                    && let Some(value) = restricted_operation(&pair[1])
                {
                    return Some(value);
                }
            }
        }
        if matches!(executable.as_str(), "cmd" | "cmd.exe") {
            for pair in rest.windows(2) {
                if matches!(pair[0].to_ascii_lowercase().as_str(), "/c" | "/k")
                    && let Some(value) = restricted_operation(&pair[1])
                {
                    return Some(value);
                }
            }
        }
        if matches!(
            executable.as_str(),
            "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
        ) {
            for pair in rest.windows(2) {
                if matches!(pair[0].to_ascii_lowercase().as_str(), "-c" | "-command")
                    && let Some(value) = restricted_operation(&pair[1])
                {
                    return Some(value);
                }
            }
        }
    }
    None
}

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
        cancel: None,
    })
}

pub struct ShellExecutionRequest<'a> {
    pub command: &'a str,
    pub workspace_root: &'a Path,
    pub working_directory: Option<&'a str>,
    pub timeout_seconds: u64,
    pub capture_bytes: usize,
    pub allow_write: bool,
    pub unrestricted: bool,
    pub cancel: Option<&'a AtomicBool>,
}

pub fn run_shell_cancellable(request: ShellExecutionRequest<'_>) -> Result<ShellResult> {
    let ShellExecutionRequest {
        command,
        workspace_root,
        working_directory,
        timeout_seconds,
        capture_bytes,
        allow_write,
        unrestricted,
        cancel,
    } = request;
    let command = command.trim();
    if command.is_empty() {
        bail!("Shell command cannot be empty");
    }
    if command.len() > 1_048_576 || command.contains('\0') {
        bail!("Shell command is too long");
    }
    let root = workspace_root.canonicalize()?;
    let requested = SandboxStore::new(&root)?.load()?;
    let unrestricted = unrestricted || requested.mode == SandboxMode::Unlimited;
    if !allow_write
        && !unrestricted
        && let Some(operation) = restricted_operation(command)
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
    if !unrestricted {
        effective.limits.wall_time_seconds = effective
            .limits
            .wall_time_seconds
            .min(timeout_seconds.clamp(1, 900));
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
        return run_windows_sandboxed_shell(command, &cwd, &effective, capture_bytes, cancel);
    }

    let mut scratch = None;
    let mut launch = if unrestricted {
        unrestricted_shell_command(command)?
    } else {
        let directory = tempfile::Builder::new()
            .prefix("yeet-model-sandbox-")
            .tempdir()?;
        let value = sandboxed_shell_command(command, &effective, &root, &cwd, directory.path())?;
        scratch = Some(directory);
        value
    };
    #[cfg(unix)]
    unsafe {
        launch.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let _scratch = scratch;
    let mut child = launch
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(if unrestricted {
            "Unable to launch unrestricted shell command"
        } else {
            "Unable to launch sandboxed shell command"
        })?;
    let capture_limit = capture_bytes.clamp(1024, 4 * 1024 * 1024);
    let stdout_limit = capture_limit.min(effective.limits.max_stdout_bytes);
    let stderr_limit = capture_limit.min(effective.limits.max_stderr_bytes);
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let stdout_thread = thread::spawn(move || read_bounded(stdout, stdout_limit));
    let stderr_thread = thread::spawn(move || read_bounded(stderr, stderr_limit));
    let start = Instant::now();
    let timeout = (!unrestricted).then(|| Duration::from_secs(effective.limits.wall_time_seconds));
    let status = loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            kill_process_group(&mut child);
            let _ = child.wait();
            bail!("cancelled");
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if timeout.is_some_and(|limit| start.elapsed() >= limit) {
            kill_process_group(&mut child);
            let _ = child.wait();
            bail!(
                "Shell command timed out after {} seconds",
                effective.limits.wall_time_seconds
            );
        }
        thread::sleep(Duration::from_millis(20));
    };
    // Descendants may still hold the pipes after the shell exits. Keep cancellation
    // and the sandbox deadline effective while the reader threads drain them.
    while !stdout_thread.is_finished() || !stderr_thread.is_finished() {
        if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            kill_process_group(&mut child);
            bail!("cancelled");
        }
        if timeout.is_some_and(|limit| start.elapsed() >= limit) {
            kill_process_group(&mut child);
            bail!(
                "Shell command timed out after {} seconds",
                effective.limits.wall_time_seconds
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
    let stdout = stdout_thread
        .join()
        .unwrap_or_else(|_| BoundedOutput::new(stdout_limit))
        .into_capture();
    let stderr = stderr_thread
        .join()
        .unwrap_or_else(|_| BoundedOutput::new(stderr_limit))
        .into_capture();
    Ok(ShellResult {
        command: command.into(),
        working_directory: cwd.display().to_string(),
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
    })
}

#[allow(clippy::needless_return)]
fn unrestricted_shell_command(command: &str) -> Result<Command> {
    #[cfg(target_os = "macos")]
    {
        let mut value = Command::new("/bin/zsh");
        value.arg("-f").arg("-c").arg(command);
        return Ok(value);
    }

    #[cfg(target_os = "linux")]
    {
        let mut value = Command::new("/bin/sh");
        value.arg("-c").arg(command);
        return Ok(value);
    }

    #[cfg(target_os = "windows")]
    {
        let mut value = Command::new("cmd.exe");
        value.arg("/D").arg("/S").arg("/C").arg(command);
        return Ok(value);
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    bail!("Shell execution is not implemented on this platform")
}

#[allow(clippy::needless_return)]
fn sandboxed_shell_command(
    command: &str,
    policy: &SandboxPolicy,
    root: &Path,
    cwd: &Path,
    scratch: &Path,
) -> Result<Command> {
    #[cfg(target_os = "macos")]
    {
        if !Path::new("/usr/bin/sandbox-exec").exists() {
            bail!("macOS sandbox-exec is unavailable; refusing unsandboxed model shell execution.");
        }
        let profile = sandbox_profile(policy, root, scratch)?;
        let mut value = Command::new("/usr/bin/sandbox-exec");
        value
            .arg("-p")
            .arg(profile)
            .arg("/bin/zsh")
            .arg("-f")
            .arg("-c")
            .arg(format!("unset PWD; {command}"));
        value
            .env_clear()
            .envs(sandbox_environment(policy, scratch, cwd));
        return Ok(value);
    }

    #[cfg(target_os = "linux")]
    {
        let plan = linux_sandbox_plan(policy, root, scratch)?;
        let plan_path = scratch.join("sandbox-plan.json");
        std::fs::write(&plan_path, serde_json::to_vec(&plan)?)?;
        let executable =
            std::env::current_exe().context("resolve Yeet executable for Linux sandbox")?;
        let mut value = Command::new(executable);
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
        let _ = (command, policy, root, cwd, scratch);
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
    Sandbox::apply_auto(&caps).context("apply Linux Landlock shell sandbox")?;

    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .status()
        .context("launch Linux sandbox shell")?;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status_signal(&status)))
}

#[cfg(target_os = "windows")]
fn run_windows_sandboxed_shell(
    command: &str,
    cwd: &Path,
    policy: &SandboxPolicy,
    capture_bytes: usize,
    cancel: Option<&AtomicBool>,
) -> Result<ShellResult> {
    use io_harness::{
        ExecMode,
        sandbox::{RunSpec, Sandbox, SandboxConfig, SandboxLimits, select},
    };

    if command.encode_utf16().count() > 24_000 {
        bail!("Shell command is too long for the Windows process command line");
    }

    let executable =
        std::env::current_exe().context("resolve Yeet executable for Windows sandbox")?;
    let environment = serde_json::to_string(&policy.environment)?;
    if environment.encode_utf16().count() > 6_000 {
        bail!("Sandbox environment is too large for the Windows process command line");
    }
    let argv = vec![
        executable.to_string_lossy().into_owned(),
        "__windows-sandbox-shell".to_owned(),
        command.to_owned(),
        environment,
    ];
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
    let mut stdout = BoundedOutput::new(stdout_limit);
    stdout.push(outcome.stdout.as_bytes());
    let stdout = stdout.into_capture();
    let mut stderr = BoundedOutput::new(stderr_limit);
    stderr.push(outcome.stderr.as_bytes());
    let stderr = stderr.into_capture();

    Ok(ShellResult {
        command: command.to_owned(),
        working_directory: cwd.display().to_string(),
        exit_code,
        succeeded,
        duration_milliseconds: started.elapsed().as_millis(),
        stdout_bytes: stdout.total_bytes,
        stderr_bytes: stderr.total_bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        stdout: (!stdout.text.is_empty()).then_some(stdout.text),
        stderr: (!stderr.text.is_empty()).then_some(stderr.text),
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
    let command_processor = inherited
        .get("ComSpec")
        .cloned()
        .unwrap_or_else(|| "cmd.exe".to_owned());
    let mut child = Command::new(command_processor);
    child
        .arg("/D")
        .arg("/S")
        .arg("/C")
        .arg(command)
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

fn kill_process_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        let pid = child.id() as i32;
        if pid > 0 && libc::kill(-pid, libc::SIGKILL) == 0 {
            return;
        }
    }
    #[cfg(windows)]
    {
        let pid = child.id().to_string();
        let _ = Command::new("taskkill.exe")
            .args(["/PID", &pid, "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
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
        limits: crate::sandbox::SandboxLimits {
            wall_time_seconds: 900,
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
fn sandbox_profile(policy: &SandboxPolicy, root: &Path, scratch: &Path) -> Result<String> {
    let mut rules = vec![
        "(version 1)".into(),
        "(deny default)".into(),
        "(import \"system.sb\")".into(),
        "(allow process-exec)".into(),
        "(allow process-fork)".into(),
        "(allow signal (target self))".into(),
        "(allow file-read-metadata file-test-existence)".into(),
    ];
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
        let text = if truncated {
            let head = utf8_prefix_without_partial_suffix(&self.head);
            let tail = utf8_suffix_without_partial_prefix(&tail);
            let omitted = total_bytes.saturating_sub(head.len() + tail.len());
            format!(
                "{}\n... [{omitted} bytes omitted] ...\n{}",
                String::from_utf8_lossy(head),
                String::from_utf8_lossy(tail)
            )
        } else {
            let mut bytes = self.head;
            bytes.extend_from_slice(&tail);
            String::from_utf8_lossy(&bytes).into_owned()
        };
        CapturedOutput {
            total_bytes,
            truncated,
            text,
        }
    }
}

#[cfg(test)]
impl BoundedOutput {
    fn retained_bytes(&self) -> usize {
        self.head.len() + self.tail.bytes.len()
    }
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

    fn into_ordered(mut self) -> Vec<u8> {
        if self.bytes.len() == self.capacity && self.start != 0 {
            self.bytes.rotate_left(self.start);
        }
        self.bytes
    }
}

fn read_bounded(mut reader: impl Read, limit: usize) -> BoundedOutput {
    let mut output = BoundedOutput::new(limit);
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => output.push(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    output
}

fn utf8_prefix_without_partial_suffix(bytes: &[u8]) -> &[u8] {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes,
        Err(error) => &bytes[..error.valid_up_to()],
    }
}

fn utf8_suffix_without_partial_prefix(bytes: &[u8]) -> &[u8] {
    if std::str::from_utf8(bytes).is_ok() {
        return bytes;
    }
    for skip in 1..=bytes.len().min(4) {
        let candidate = &bytes[skip..];
        if std::str::from_utf8(candidate).is_ok() {
            return candidate;
        }
    }
    &[]
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

fn split_shell_segments(command: &str) -> Vec<String> {
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
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use crate::sandbox::SandboxStore;
    #[cfg(unix)]
    use std::sync::Arc;

    #[test]
    fn bounded_output_preserves_untruncated_utf8_across_internal_split() {
        let capture = read_bounded("abé".as_bytes(), 6).into_capture();

        assert_eq!(capture.total_bytes, 4);
        assert!(!capture.truncated);
        assert_eq!(capture.text, "abé");
    }

    #[test]
    fn bounded_output_keeps_head_and_tail_and_counts_all_bytes() {
        let output = read_bounded(&b"0123456789abcdef"[..], 10);

        assert_eq!(output.total_bytes, 16);
        assert_eq!(output.retained_bytes(), 10);

        let capture = output.into_capture();
        assert!(capture.truncated);
        assert_eq!(capture.text, "01234\n... [6 bytes omitted] ...\nbcdef");
    }

    #[test]
    fn bounded_output_drops_partial_utf8_at_truncation_boundaries() {
        let output = read_bounded("ééééé".as_bytes(), 5);

        assert_eq!(output.total_bytes, 10);
        assert_eq!(output.retained_bytes(), 5);

        let capture = output.into_capture();
        assert!(capture.truncated);
        assert_eq!(capture.text, "é\n... [6 bytes omitted] ...\né");
        assert!(!capture.text.contains('\u{fffd}'));
    }

    #[test]
    fn bounded_output_drains_large_stream_without_retaining_it_all() {
        let input = vec![b'x'; 2 * 1024 * 1024];
        let output = read_bounded(input.as_slice(), 1_025);

        assert_eq!(output.total_bytes, input.len());
        assert_eq!(output.retained_bytes(), 1_025);
        assert!(output.total_bytes > output.retained_bytes());

        let capture = output.into_capture();
        assert!(capture.truncated);
        assert!(capture.text.contains("[2096127 bytes omitted]"));
    }

    #[test]
    fn bounded_output_matches_existing_capture_format_across_chunk_boundaries() {
        for limit in 0..32 {
            for length in 0..96 {
                let input: Vec<u8> = (0..length).map(|index| b'a' + index % 26).collect();
                let expected = if input.len() <= limit {
                    (String::from_utf8_lossy(&input).into_owned(), false)
                } else {
                    let head = limit / 2;
                    let tail = limit - head;
                    (
                        format!(
                            "{}\n... [{} bytes omitted] ...\n{}",
                            String::from_utf8_lossy(&input[..head]),
                            input.len() - head - tail,
                            String::from_utf8_lossy(&input[input.len() - tail..])
                        ),
                        true,
                    )
                };
                for chunk_size in 1..17 {
                    let mut output = BoundedOutput::new(limit);
                    for chunk in input.chunks(chunk_size) {
                        output.push(chunk);
                        assert!(output.retained_bytes() <= limit);
                    }
                    let capture = output.into_capture();
                    assert_eq!(capture.total_bytes, input.len());
                    assert_eq!((capture.text, capture.truncated), expected);
                }
            }
        }
    }

    #[test]
    fn shell_policy_distinguishes_inspection_and_mutation() {
        assert_eq!(restricted_operation("cat Cargo.toml"), None);
        assert_eq!(restricted_operation("sed -n '1,20p' file"), None);
        assert_eq!(
            restricted_operation("sed -i '' 's/a/b/' file").as_deref(),
            Some("sed -i")
        );
        assert_eq!(restricted_operation("git status"), None);
        assert_eq!(
            restricted_operation("git checkout main").as_deref(),
            Some("git checkout")
        );
        assert_eq!(restricted_operation("cargo test"), None);
        assert_eq!(
            restricted_operation("cargo add serde").as_deref(),
            Some("cargo add")
        );
        assert_eq!(
            restricted_operation("del build.log").as_deref(),
            Some("del")
        );
        assert_eq!(
            restricted_operation("cmd.exe /c \"del build.log\"").as_deref(),
            Some("del")
        );
        assert_eq!(
            restricted_operation("powershell -Command \"Remove-Item build.log\"").as_deref(),
            Some("remove-item")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn shell_honors_distinct_policy_output_limits_while_draining() {
        let workspace = tempfile::tempdir().unwrap();
        let store = SandboxStore::new(workspace.path()).unwrap();
        let mut policy = SandboxPolicy::default();
        policy.limits.max_stdout_bytes = 8;
        policy.limits.max_stderr_bytes = 7;
        store.save(&policy).unwrap();

        let result = run_shell(
            "printf '0123456789abcdef'; printf 'abcdefghijklmnop' >&2",
            workspace.path(),
            None,
            5,
            16 * 1024,
            false,
            true,
        )
        .unwrap();

        assert!(result.succeeded);
        assert_eq!(result.stdout_bytes, 16);
        assert_eq!(result.stderr_bytes, 16);
        assert!(result.stdout_truncated);
        assert!(result.stderr_truncated);
        assert_eq!(
            result.stdout.as_deref(),
            Some("0123\n... [8 bytes omitted] ...\ncdef")
        );
        assert_eq!(
            result.stderr.as_deref(),
            Some("abc\n... [9 bytes omitted] ...\nmnop")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sandboxed_shell_runs_with_explicit_workspace_read_access() {
        if !Path::new("/usr/bin/sandbox-exec").exists() {
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let store = SandboxStore::new(workspace.path()).unwrap();
        let policy = SandboxPolicy {
            workspace_read: WorkspaceRead::All,
            ..SandboxPolicy::default()
        };
        store.save(&policy).unwrap();

        let result = run_shell("pwd", workspace.path(), None, 5, 16 * 1024, false, false).unwrap();
        assert!(result.succeeded);
        let expected = workspace.path().canonicalize().unwrap();
        assert_eq!(
            result.stdout.unwrap().trim(),
            expected.display().to_string()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sandboxed_shell_accepts_absolute_workspace_working_directory() {
        if !Path::new("/usr/bin/sandbox-exec").exists() {
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let store = SandboxStore::new(workspace.path()).unwrap();
        let policy = SandboxPolicy {
            workspace_read: WorkspaceRead::All,
            ..SandboxPolicy::default()
        };
        store.save(&policy).unwrap();
        let absolute = workspace.path().canonicalize().unwrap();

        let result = run_shell(
            "pwd",
            workspace.path(),
            absolute.to_str(),
            5,
            16 * 1024,
            false,
            false,
        )
        .unwrap();

        assert!(result.succeeded);
        assert_eq!(
            result.stdout.unwrap().trim(),
            absolute.display().to_string()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sandboxed_shell_can_use_workspace_root_as_cwd_without_read_grant() {
        if !Path::new("/usr/bin/sandbox-exec").exists() {
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let store = SandboxStore::new(workspace.path()).unwrap();
        store.save(&SandboxPolicy::default()).unwrap();

        let result = run_shell(
            "pwd",
            workspace.path(),
            Some("."),
            5,
            16 * 1024,
            false,
            false,
        )
        .unwrap();

        assert!(result.succeeded);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unlimited_shell_can_leave_the_project_without_workspace_grants() {
        let parent = tempfile::tempdir().unwrap();
        let workspace = parent.path().join("project");
        std::fs::create_dir(&workspace).unwrap();
        let store = SandboxStore::new(&workspace).unwrap();
        let policy = SandboxPolicy {
            mode: SandboxMode::Unlimited,
            ..SandboxPolicy::default()
        };
        store.save(&policy).unwrap();

        let result = run_shell("pwd", &workspace, Some(".."), 5, 16 * 1024, false, false).unwrap();
        assert!(result.succeeded);
        assert_eq!(
            result.stdout.unwrap().trim(),
            parent.path().canonicalize().unwrap().display().to_string()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unrestricted_shell_allows_nested_sandbox() {
        if !Path::new("/usr/bin/sandbox-exec").exists() {
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let store = SandboxStore::new(workspace.path()).unwrap();
        store
            .save(&SandboxPolicy {
                mode: SandboxMode::Unlimited,
                ..SandboxPolicy::default()
            })
            .unwrap();

        let result = run_shell_cancellable(ShellExecutionRequest {
            command: "/usr/bin/sandbox-exec -p '(version 1)(allow default)' /usr/bin/true",
            workspace_root: workspace.path(),
            working_directory: None,
            timeout_seconds: 5,
            capture_bytes: 16 * 1024,
            allow_write: true,
            unrestricted: true,
            cancel: None,
        })
        .unwrap();

        assert!(result.succeeded);
    }

    #[cfg(unix)]
    #[test]
    fn cancellable_shell_force_kills_a_long_running_process_group() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().to_path_buf();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let started = Instant::now();

        let worker = thread::spawn(move || {
            run_shell_cancellable(ShellExecutionRequest {
                command: "sleep 30 & wait",
                workspace_root: &root,
                working_directory: None,
                timeout_seconds: 900,
                capture_bytes: 16 * 1024,
                allow_write: false,
                unrestricted: true,
                cancel: Some(&worker_cancel),
            })
        });

        thread::sleep(Duration::from_millis(80));
        cancel.store(true, Ordering::Release);
        let error = worker.join().unwrap().unwrap_err().to_string();

        assert_eq!(error, "cancelled");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
