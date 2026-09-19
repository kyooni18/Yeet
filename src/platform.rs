use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(windows)]
use std::{
    collections::HashSet,
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    sync::{Mutex, OnceLock},
    time::Duration,
};

#[cfg(unix)]
pub(crate) type LocalListener = std::os::unix::net::UnixListener;
#[cfg(unix)]
pub(crate) type LocalStream = std::os::unix::net::UnixStream;

#[cfg(windows)]
#[derive(Debug)]
pub(crate) struct LocalListener {
    inner: TcpListener,
    token: String,
}

#[cfg(windows)]
impl LocalListener {
    pub(crate) fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.inner.set_nonblocking(nonblocking)
    }

    pub(crate) fn accept(&self) -> io::Result<(LocalStream, SocketAddr)> {
        let (mut stream, address) = self.inner.accept()?;
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        let expected = format!("YEET_LOCAL_V1 {}", self.token);
        let mut line = Vec::with_capacity(96);
        let mut byte = [0_u8; 1];
        loop {
            if line.len() >= 256 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Yeet local authentication frame is too long",
                ));
            }
            match stream.read(&mut byte) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Yeet local connection closed before authentication",
                    ));
                }
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) => line.push(byte[0]),
                Err(error) => return Err(error),
            }
        }
        stream.set_read_timeout(None)?;
        let authenticated = std::str::from_utf8(&line)
            .ok()
            .map(str::trim_end)
            .is_some_and(|value| value == expected);
        if !authenticated {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Yeet local connection authentication failed",
            ));
        }
        Ok((stream, address))
    }
}

#[cfg(windows)]
pub(crate) type LocalStream = std::net::TcpStream;

pub(crate) fn default_config_directory() -> PathBuf {
    if let Some(explicit) = std::env::var_os("YEET_CONFIG_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(explicit);
    }

    let home = dirs::home_dir();
    if let Some(legacy) = home.as_ref().map(|home| home.join(".yeet"))
        && (legacy.exists() || cfg!(target_os = "macos"))
    {
        return legacy;
    }

    #[cfg(target_os = "windows")]
    if let Some(config) = dirs::config_dir() {
        return config.join("Yeet");
    }

    #[cfg(target_os = "linux")]
    if let Some(config) = dirs::config_dir() {
        return config.join("yeet");
    }

    home.map(|home| home.join(".yeet"))
        .unwrap_or_else(|| PathBuf::from(".yeet"))
}
#[cfg(unix)]
pub(crate) fn bind_local(endpoint: &Path) -> io::Result<LocalListener> {
    LocalListener::bind(endpoint)
}

#[cfg(windows)]
pub(crate) fn bind_local(endpoint: &Path) -> io::Result<LocalListener> {
    let inner = TcpListener::bind(("127.0.0.1", 0))?;
    let address = inner.local_addr()?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    fs::write(endpoint, format!("yeet-local-v1\n{address}\n{token}\n"))?;
    set_private_file(endpoint)?;
    Ok(LocalListener { inner, token })
}

#[cfg(unix)]
pub(crate) fn connect_local(endpoint: &Path) -> io::Result<LocalStream> {
    LocalStream::connect(endpoint)
}

#[cfg(windows)]
pub(crate) fn connect_local(endpoint: &Path) -> io::Result<LocalStream> {
    let value = fs::read_to_string(endpoint)?;
    let mut lines = value.lines();
    let first = lines.next().unwrap_or_default().trim();
    let (address_text, token) = if first == "yeet-local-v1" {
        let address = lines.next().unwrap_or_default().trim();
        let token = lines.next().unwrap_or_default().trim();
        if address.is_empty() || token.len() < 16 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid Yeet local endpoint {}", endpoint.display()),
            ));
        }
        (address, Some(token))
    } else {
        // Accept the pre-auth endpoint format during rolling upgrades so the
        // new client can retire an older daemon cleanly.
        (first, None)
    };
    let address = address_text.parse::<SocketAddr>().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "invalid Yeet local endpoint {}: {error}",
                endpoint.display()
            ),
        )
    })?;
    let mut stream = LocalStream::connect(address)?;
    if let Some(token) = token {
        stream.write_all(format!("YEET_LOCAL_V1 {token}\n").as_bytes())?;
        stream.flush()?;
    }
    Ok(stream)
}

pub(crate) fn set_private_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    set_windows_private_acl(path, true)?;
    Ok(())
}

pub(crate) fn set_private_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    set_windows_private_acl(path, false)?;
    Ok(())
}

#[cfg(windows)]
fn set_windows_private_acl(path: &Path, directory: bool) -> io::Result<()> {
    static HARDENED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let hardened = HARDENED.get_or_init(|| Mutex::new(HashSet::new()));
    if hardened
        .lock()
        .map_err(|_| io::Error::other("Windows ACL cache lock poisoned"))?
        .contains(&key)
    {
        return Ok(());
    }

    let identity = Command::new("whoami").output()?;
    if !identity.status.success() {
        return Err(io::Error::other("whoami failed while hardening Yeet state"));
    }
    let identity = String::from_utf8_lossy(&identity.stdout).trim().to_owned();
    if identity.is_empty() {
        return Err(io::Error::other(
            "whoami returned an empty Windows identity",
        ));
    }
    let rights = if directory { "(OI)(CI)F" } else { "F" };
    let grants = [
        format!("{identity}:{rights}"),
        format!("*S-1-5-18:{rights}"),
        format!("*S-1-5-32-544:{rights}"),
    ];
    let status = Command::new("icacls")
        .arg(path)
        .arg("/inheritancelevel:r")
        .arg("/grant:r")
        .args(&grants)
        .arg("/Q")
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "icacls failed while hardening {}",
            path.display()
        )));
    }
    hardened
        .lock()
        .map_err(|_| io::Error::other("Windows ACL cache lock poisoned"))?
        .insert(key);
    Ok(())
}

pub(crate) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(source, destination)
}

pub(crate) fn configure_detached(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                libc::signal(libc::SIGHUP, libc::SIG_IGN);
                Ok(())
            });
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

pub(crate) fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    #[cfg(not(unix))]
    let _ = command;
}

pub(crate) fn force_terminate_process_tree(pid: u32) -> io::Result<()> {
    if pid == 0 || pid == std::process::id() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to terminate the current process",
        ));
    }

    #[cfg(unix)]
    {
        // Capture descendants before signalling anything because init may re-parent
        // them immediately after a supervisor exits. Yeet-owned children can each
        // lead their own process group, so killing only the root group can leave a
        // detached runtime or extension alive.
        let mut tree = unix_process_tree(pid).unwrap_or_else(|_| vec![pid]);
        if !tree.contains(&pid) {
            tree.push(pid);
        }
        for target in tree.iter().rev().copied() {
            signal_owned_process_or_group(target, libc::SIGTERM)?;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
        for target in tree.iter().rev().copied() {
            let _ = signal_owned_process_or_group(target, libc::SIGKILL);
        }
        return Ok(());
    }

    #[cfg(windows)]
    {
        // Give the process tree a non-forced termination attempt first. This
        // preserves the opportunity for console/GUI processes to run normal
        // shutdown handling instead of making every Windows recycle abrupt.
        let pid_text = pid.to_string();
        let graceful = Command::new("taskkill")
            .args(["/PID", &pid_text, "/T"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if graceful.success() {
            return Ok(());
        }

        std::thread::sleep(std::time::Duration::from_millis(150));
        let _ = Command::new("taskkill")
            .args(["/PID", &pid_text, "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        // taskkill returns a failure code when the process already exited.
        // Treat that as an idempotent success; endpoint probing decides
        // whether stale-daemon recovery actually completed.
        return Ok(());
    }

    #[allow(unreachable_code)]
    Ok(())
}

#[cfg(unix)]
fn process_group_id(pid: u32) -> io::Result<i32> {
    let pid = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "process id is out of range"))?;
    let group = unsafe { libc::getpgid(pid) };
    if group == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(group)
}

#[cfg(unix)]
fn signal_process_group(pid: u32, signal: i32) -> io::Result<()> {
    let pid = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "process id is out of range"))?;
    let result = unsafe { libc::kill(-pid, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(error)
}

#[cfg(target_os = "linux")]
fn unix_process_tree(root: u32) -> io::Result<Vec<u32>> {
    let mut pairs = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let Ok(entry) = entry else {
            continue;
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            // Processes can disappear while /proc is being scanned.
            continue;
        };
        let Some((_, fields)) = stat.rsplit_once(") ") else {
            continue;
        };
        let mut fields = fields.split_whitespace();
        let _state = fields.next();
        let Some(ppid) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        pairs.push((pid, ppid));
    }
    Ok(process_tree_from_pairs(root, &pairs))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn unix_process_tree(root: u32) -> io::Result<Vec<u32>> {
    let output = Command::new("ps").args(["-axo", "pid=,ppid="]).output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            "ps failed while discovering daemon children",
        ));
    }
    let pairs = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let ppid = fields.next()?.parse::<u32>().ok()?;
            Some((pid, ppid))
        })
        .collect::<Vec<_>>();
    Ok(process_tree_from_pairs(root, &pairs))
}

#[cfg(unix)]
fn process_tree_from_pairs(root: u32, pairs: &[(u32, u32)]) -> Vec<u32> {
    let mut tree = vec![root];
    let mut cursor = 0usize;
    while cursor < tree.len() {
        let parent = tree[cursor];
        for (child, ppid) in pairs {
            if *ppid == parent && !tree.contains(child) {
                tree.push(*child);
            }
        }
        cursor += 1;
    }
    tree
}

#[cfg(unix)]
fn signal_owned_process_or_group(pid: u32, signal: i32) -> io::Result<()> {
    if process_group_id(pid).is_ok_and(|group| group == pid as i32) {
        signal_process_group(pid, signal)
    } else {
        signal_process(pid, signal)
    }
}

#[cfg(unix)]
fn signal_process(pid: u32, signal: i32) -> io::Result<()> {
    let result = unsafe { libc::kill(pid as i32, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(error)
}

/// Retire an internal runtime and its children if its spawning daemon exits.
/// Parentage, rather than a PID-existence check, also detects PID reuse.
pub(crate) fn watch_runtime_parent(expected_parent: u32) {
    #[cfg(unix)]
    {
        if expected_parent <= 1 || expected_parent == std::process::id() {
            return;
        }
        std::thread::spawn(move || {
            loop {
                let actual_parent = unsafe { libc::getppid() } as u32;
                if runtime_parent_changed(expected_parent, actual_parent) {
                    // Snapshot before signalling: children may be reparented as soon
                    // as their own supervisor exits. Never signal our own group.
                    let pid = std::process::id();
                    if let Ok(tree) = unix_process_tree(pid) {
                        for child in tree.iter().rev().copied().filter(|child| *child != pid) {
                            let _ = signal_owned_process_or_group(child, libc::SIGKILL);
                        }
                    }
                    // Drop handlers may be stuck on the same abandoned work. The
                    // owning daemon is gone, so no live session can use this worker.
                    std::process::exit(0);
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
    }
    #[cfg(not(unix))]
    let _ = expected_parent;
}

#[cfg(unix)]
fn runtime_parent_changed(expected_parent: u32, actual_parent: u32) -> bool {
    expected_parent != actual_parent
}

#[cfg(all(test, unix))]
mod runtime_parent_tests {
    use super::*;

    #[test]
    fn only_obsolete_runtime_parentage_triggers_cleanup() {
        assert!(!runtime_parent_changed(42, 42));
        assert!(runtime_parent_changed(42, 1));
        // Linux subreapers can adopt an orphan instead of init.
        assert!(runtime_parent_changed(42, 99));
    }

    // Run the exit-capable watchdog only in an isolated test subprocess.
    #[test]
    fn watchdog_fixture() {
        let Ok(mode) = std::env::var("YEET_TEST_PARENT_WATCHDOG") else {
            return;
        };
        let parent = unsafe { libc::getppid() } as u32;
        let expected = if mode == "live" { parent } else { parent + 1 };
        // Avoid the current-PID guard when selecting a simulated old parent.
        let expected = if expected == std::process::id() {
            expected + 1
        } else {
            expected
        };
        watch_runtime_parent(expected);
        std::thread::sleep(std::time::Duration::from_millis(1500));
        // A live owner must survive, while an obsolete owner must exit via
        // the watchdog before reaching this distinct status.
        std::process::exit(23);
    }

    #[test]
    fn watchdog_retires_only_a_runtime_with_obsolete_parentage() {
        for (mode, expected_code) in [("live", 23), ("obsolete", 0)] {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "platform::runtime_parent_tests::watchdog_fixture",
                    "--nocapture",
                ])
                .env("YEET_TEST_PARENT_WATCHDOG", mode)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert_eq!(status.code(), Some(expected_code), "{mode}");
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("watchdog fixture timed out: {mode}");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}
