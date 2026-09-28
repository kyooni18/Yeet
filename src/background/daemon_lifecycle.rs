//! Owns background daemon path leases, stale recovery, spawning, and lease cleanup.

use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    net::Shutdown,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use fs2::{FileExt, lock_contended_error};

use crate::{
    config::ConfigStore,
    platform::{connect_local, set_private_directory, spawn_detached},
};

use super::{MAX_BACKGROUND_LOG_BYTES, STALE_DAEMON_EXIT_TIMEOUT};
fn lock_is_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error() == lock_contended_error().raw_os_error()
}

pub(super) struct BackgroundPaths {
    pub(super) socket: PathBuf,
    pub(super) log: PathBuf,
    pub(super) identity: PathBuf,
    pub(super) pid: PathBuf,
    pub(super) lifecycle_lock: PathBuf,
    pub(super) owner_lock: PathBuf,
}

impl BackgroundPaths {
    pub(super) fn new(workspace: &Path, scope: Option<&str>) -> Result<Self> {
        let config = ConfigStore::default();
        config.ensure()?;
        let directory = config.directory.join("background");
        fs::create_dir_all(&directory)?;
        set_private_directory(&directory)?;
        let key = background_key(workspace, scope);
        Ok(Self {
            socket: directory.join(format!("{key}.sock")),
            log: directory.join(format!("{key}.log")),
            identity: directory.join(format!("{key}.identity")),
            pid: directory.join(format!("{key}.pid")),
            lifecycle_lock: directory.join(format!("{key}.lock")),
            owner_lock: directory.join(format!("{key}.owner.lock")),
        })
    }
}

pub(super) struct LifecycleLock(File);

impl LifecycleLock {
    pub(super) fn acquire(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("open background lifecycle lock {}", path.display()))?;
        file.lock_exclusive()
            .with_context(|| format!("lock background lifecycle {}", path.display()))?;
        Ok(Self(file))
    }
    fn try_acquire(path: &Path) -> Result<Option<Self>> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("open background lifecycle lock {}", path.display()))?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self(file))),
            Err(error) if lock_is_contended(&error) => Ok(None),
            Err(error) => {
                Err(error).with_context(|| format!("probe background lifecycle {}", path.display()))
            }
        }
    }
}

impl Drop for LifecycleLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub(super) struct InstanceLease(File);

impl InstanceLease {
    fn open(path: &Path) -> Result<File> {
        OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("open background owner lock {}", path.display()))
    }

    pub(super) fn acquire(path: &Path) -> Result<Self> {
        let file = Self::open(path)?;
        file.try_lock_exclusive().with_context(|| {
            format!(
                "another Yeet background service already owns {}",
                path.display()
            )
        })?;
        Ok(Self(file))
    }

    pub(super) fn is_held(path: &Path) -> Result<bool> {
        let file = Self::open(path)?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                let _ = FileExt::unlock(&file);
                Ok(false)
            }
            Err(error) if lock_is_contended(&error) => Ok(true),
            Err(error) => Err(error)
                .with_context(|| format!("probe background owner lock {}", path.display())),
        }
    }
}

impl Drop for InstanceLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn background_key(workspace: &Path, scope: Option<&str>) -> String {
    let mut hasher = DefaultHasher::new();
    workspace.to_string_lossy().hash(&mut hasher);
    if let Some(scope) = scope {
        "yeet-background-scope-v1".hash(&mut hasher);
        scope.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

pub(super) fn current_executable_identity() -> Result<String> {
    let executable = std::env::current_exe().context("locate yeet executable")?;
    let metadata = fs::metadata(&executable)
        .with_context(|| format!("inspect Yeet executable metadata: {}", executable.display()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    Ok(format!(
        "v2:{}:{}:{modified}",
        executable.display(),
        metadata.len()
    ))
}

pub(super) fn retire_stale_daemon(paths: &BackgroundPaths) -> Result<()> {
    match connect_local(&paths.socket) {
        Ok(stream) => {
            let _ = stream.shutdown(Shutdown::Both);
            bail!(
                "background service became reachable during stale cleanup; preserving live daemon at {}",
                paths.socket.display()
            );
        }
        Err(error) if stale_socket_error(&error) => {}
        Err(error) => return Err(error.into()),
    }

    // The lifetime owner lock is authoritative. A missing/refused socket while
    // that lock is held can mean startup, temporary endpoint damage, or a live
    // daemon from another build; none justify killing it from an attaching
    // frontend. Only remove lease files after ownership has actually ended.
    wait_for_instance_release(paths, STALE_DAEMON_EXIT_TIMEOUT)?;
    cleanup_stale_daemon_files(paths)?;
    Ok(())
}

pub(super) fn stale_socket_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
    )
}

fn wait_for_instance_release(paths: &BackgroundPaths, timeout: Duration) -> Result<()> {
    let started = Instant::now();
    while InstanceLease::is_held(&paths.owner_lock)? {
        if started.elapsed() >= timeout {
            bail!(
                "background owner lease is still held; refusing to unlink live daemon files: {}",
                paths.owner_lock.display()
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

pub(super) fn cleanup_stale_daemon_files(paths: &BackgroundPaths) -> Result<()> {
    if InstanceLease::is_held(&paths.owner_lock)? {
        bail!(
            "background owner lease is still held; refusing to unlink live daemon files: {}",
            paths.owner_lock.display()
        );
    }
    let _ = fs::remove_file(&paths.socket);
    let _ = fs::remove_file(&paths.identity);
    let _ = fs::remove_file(&paths.pid);
    Ok(())
}

pub fn cleanup_stale_artifacts() -> Result<usize> {
    let config = ConfigStore::default();
    config.ensure()?;
    cleanup_stale_artifacts_in(&config.directory.join("background"))
}

pub(super) fn cleanup_stale_artifacts_in(directory: &Path) -> Result<usize> {
    if !directory.is_dir() {
        return Ok(0);
    }

    let mut cleaned = 0usize;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(key) = name.strip_suffix(".owner.lock") else {
            continue;
        };
        let lifecycle = directory.join(format!("{key}.lock"));
        let Some(_lifecycle_guard) = LifecycleLock::try_acquire(&lifecycle)? else {
            continue;
        };
        if InstanceLease::is_held(&path)? {
            continue;
        }

        let socket = directory.join(format!("{key}.sock"));
        match connect_local(&socket) {
            Ok(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
                continue;
            }
            Err(error) if stale_socket_error(&error) => {}
            Err(_) => continue,
        }

        let mut removed = false;
        for stale in [
            socket,
            directory.join(format!("{key}.identity")),
            directory.join(format!("{key}.pid")),
        ] {
            match fs::remove_file(&stale) {
                Ok(()) => removed = true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("remove stale background artifact {}", stale.display())
                    });
                }
            }
        }
        if removed {
            cleaned += 1;
        }
    }
    Ok(cleaned)
}

pub(super) fn spawn_daemon(
    workspace: &Path,
    scope: Option<&str>,
    paths: &BackgroundPaths,
) -> Result<()> {
    let executable = std::env::current_exe().context("locate yeet executable")?;
    rotate_log(&paths.log, MAX_BACKGROUND_LOG_BYTES)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)?;
    let log_err = log.try_clone()?;
    let mut command = Command::new(executable);
    command
        .arg("__background-daemon")
        .arg(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    if let Some(scope) = scope {
        command.arg(scope);
    }
    let mut child = spawn_detached(&mut command).context("start Yeet background service")?;
    // Only the daemon writes the PID lease, after it has acquired the lifetime
    // owner lock and bound its endpoint. Writing it here lets a rejected child
    // overwrite the live daemon's PID and makes later recovery target the wrong
    // process.
    // Detached daemons are still direct children of the foreground Yeet
    // process. Keep a tiny waiter so a daemon replaced by stale recovery is
    // reaped immediately instead of sitting as a zombie until the TUI exits.
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn rotate_log(path: &Path, max_bytes: u64) -> Result<()> {
    if fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
        <= max_bytes
    {
        return Ok(());
    }
    let rotated = path.with_extension("log.1");
    let _ = fs::remove_file(&rotated);
    fs::rename(path, rotated)?;
    Ok(())
}

pub(super) struct DaemonFilesCleanup {
    pub(super) socket: PathBuf,
    pub(super) identity: PathBuf,
    pub(super) pid: PathBuf,
    pub(super) owner_pid: u32,
}

impl DaemonFilesCleanup {
    pub(super) fn new(paths: &BackgroundPaths) -> Self {
        Self {
            socket: paths.socket.clone(),
            identity: paths.identity.clone(),
            pid: paths.pid.clone(),
            owner_pid: std::process::id(),
        }
    }

    fn still_owns_lease(&self) -> bool {
        fs::read_to_string(&self.pid)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .is_some_and(|pid| pid == self.owner_pid)
    }
}

impl Drop for DaemonFilesCleanup {
    fn drop(&mut self) {
        // A newer daemon may already own these shared pathnames. An older
        // process can remain alive temporarily because its Unix listener stays
        // valid after unlink. It must never remove the newer daemon's lease
        // when it eventually exits.
        if !self.still_owns_lease() {
            return;
        }
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_file(&self.identity);
        let _ = fs::remove_file(&self.pid);
    }
}
