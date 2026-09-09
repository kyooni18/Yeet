use std::{fs, io, path::Path, process::Command};

#[cfg(unix)]
pub(crate) type LocalListener = std::os::unix::net::UnixListener;
#[cfg(unix)]
pub(crate) type LocalStream = std::os::unix::net::UnixStream;

#[cfg(windows)]
pub(crate) type LocalListener = std::net::TcpListener;
#[cfg(windows)]
pub(crate) type LocalStream = std::net::TcpStream;

#[cfg(unix)]
pub(crate) fn bind_local(endpoint: &Path) -> io::Result<LocalListener> {
    LocalListener::bind(endpoint)
}

#[cfg(windows)]
pub(crate) fn bind_local(endpoint: &Path) -> io::Result<LocalListener> {
    let listener = LocalListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    fs::write(endpoint, format!("{address}\n"))?;
    Ok(listener)
}

#[cfg(unix)]
pub(crate) fn connect_local(endpoint: &Path) -> io::Result<LocalStream> {
    LocalStream::connect(endpoint)
}

#[cfg(windows)]
pub(crate) fn connect_local(endpoint: &Path) -> io::Result<LocalStream> {
    let value = fs::read_to_string(endpoint)?;
    let address = value
        .trim()
        .parse::<std::net::SocketAddr>()
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid Yeet local endpoint {}: {error}",
                    endpoint.display()
                ),
            )
        })?;
    LocalStream::connect(address)
}

pub(crate) fn set_private_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub(crate) fn set_private_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
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
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()?;
        if status.success() {
            return Ok(());
        }
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
