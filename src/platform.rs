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

pub(crate) fn force_terminate_process_tree(pid: u32) -> io::Result<()> {
    if pid == 0 || pid == std::process::id() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to terminate the current process",
        ));
    }

    #[cfg(unix)]
    {
        // Capture descendants before signalling the daemon. Once the parent
        // exits, launchd/init may immediately re-parent the Node bridge and MCP
        // children, making later PPID-based cleanup impossible.
        let mut tree = unix_process_tree(pid).unwrap_or_else(|_| vec![pid]);
        if !tree.contains(&pid) {
            tree.push(pid);
        }
        for target in tree.iter().rev().copied() {
            signal_process(target, libc::SIGTERM)?;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
        for target in tree.iter().rev().copied() {
            let _ = signal_process(target, libc::SIGKILL);
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
    let mut tree = vec![root];
    let mut cursor = 0usize;
    while cursor < tree.len() {
        let parent = tree[cursor];
        for (child, ppid) in &pairs {
            if *ppid == parent && !tree.contains(child) {
                tree.push(*child);
            }
        }
        cursor += 1;
    }
    Ok(tree)
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{process::Stdio, time::Instant};

    #[test]
    fn forced_tree_termination_reaps_a_process_with_a_child() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 30 & wait"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();

        let discovery_started = Instant::now();
        loop {
            if unix_process_tree(pid).is_ok_and(|tree| tree.len() >= 2) {
                break;
            }
            assert!(
                discovery_started.elapsed() < std::time::Duration::from_secs(2),
                "child process never appeared in the process tree"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        force_terminate_process_tree(pid).unwrap();
        let exit_started = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if exit_started.elapsed() >= std::time::Duration::from_secs(2) {
                let _ = child.kill();
                panic!("forced process-tree termination did not stop the root process");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn authenticated_local_endpoint_preserves_application_bytes() {
        let root = tempfile::tempdir().unwrap();
        let endpoint = root.path().join("local.endpoint");
        let listener = bind_local(&endpoint).unwrap();
        listener.set_nonblocking(false).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut payload = [0_u8; 4];
            stream.read_exact(&mut payload).unwrap();
            assert_eq!(&payload, b"ping");
        });

        let mut client = connect_local(&endpoint).unwrap();
        client.write_all(b"ping").unwrap();
        server.join().unwrap();
    }
}
