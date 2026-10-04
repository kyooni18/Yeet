use std::{net::TcpListener, path::PathBuf, process::Command, sync::Mutex, thread, time::Duration};

use crate::remote::{
    RemoteOptions, remote_pid, remote_status, start_remote_background, stop_remote,
};

static LIFECYCLE_TEST_LOCK: Mutex<()> = Mutex::new(());

fn pick_unused_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

fn locate_yeet_binary() -> PathBuf {
    if let Some(explicit) = std::env::var_os("YEET_EXE").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return path;
        }
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate =
        manifest_dir
            .join("target/debug")
            .join(if cfg!(windows) { "yeet.exe" } else { "yeet" });
    if candidate.is_file() {
        return candidate;
    }
    let status = Command::new("cargo")
        .args(["build", "--bin", "yeet"])
        .current_dir(&manifest_dir)
        .status()
        .expect("cargo build --bin yeet");
    assert!(status.success(), "cargo build --bin yeet failed");
    candidate
}

struct TestContext {
    /// Kept for the rest of the test process: while this context is alive
    /// `YEET_CONFIG_DIR` points here, and sidecars spawned meanwhile by
    /// unrelated parallel tests inherit it and may outlive the context.
    config_dir: PathBuf,
    binary: PathBuf,
    /// Process-wide values restored when the context ends.
    previous_env: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl TestContext {
    fn new() -> Self {
        let config_dir = tempfile::tempdir().expect("temp config dir").keep();
        let binary = locate_yeet_binary();
        let previous_env = ["YEET_CONFIG_DIR", "YEET_EXE"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        unsafe {
            std::env::set_var("YEET_CONFIG_DIR", config_dir.as_path());
            std::env::set_var("YEET_EXE", &binary);
        }
        Self {
            config_dir,
            binary,
            previous_env,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        command.env("YEET_CONFIG_DIR", self.config_dir.as_path());
        command.env("YEET_EXE", &self.binary);
        command
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        unsafe {
            std::env::set_var("YEET_CONFIG_DIR", self.config_dir.as_path());
        }
        let _ = stop_remote();
        for (key, value) in &self.previous_env {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

#[test]
fn background_start_status_and_duplicate_prevention() {
    let _lock = LIFECYCLE_TEST_LOCK.lock().unwrap();
    let ctx = TestContext::new();
    let port = pick_unused_port();
    let bind_addr = format!("127.0.0.1:{port}");

    // 1. Initial status reports not running
    let status_output = ctx
        .command()
        .args(["remote", "status"])
        .output()
        .expect("status check");
    assert!(status_output.status.success());
    let status_text = String::from_utf8_lossy(&status_output.stdout);
    assert!(status_text.contains("Yeet remote UI is not running"));
    assert!(remote_status().unwrap().is_none());

    // 2. Start remote in background via CLI
    let start_output = ctx
        .command()
        .args(["remote", "--background", "--bind", &bind_addr])
        .output()
        .expect("start background remote");
    assert!(
        start_output.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&start_output.stderr)
    );
    let start_text = String::from_utf8_lossy(&start_output.stdout);
    assert!(start_text.contains(&format!(":{port}")));
    assert!(start_text.contains("Running in background"));

    // 3. Status confirms remote is running
    let status_output = ctx
        .command()
        .args(["remote", "status"])
        .output()
        .expect("status check after start");
    assert!(status_output.status.success());
    let status_text = String::from_utf8_lossy(&status_output.stdout);
    assert!(status_text.contains(&format!(":{port}")));
    assert!(status_text.contains("Owned children: none"));

    let api_status = remote_status().unwrap().expect("status from API");
    assert!(api_status.address.contains(&format!(":{port}")));

    let pid = remote_pid().unwrap().expect("remote pid");
    assert!(pid > 0);

    // 4. Duplicate start prevention via CLI: does not start a second instance
    let dup_output = ctx
        .command()
        .args(["remote", "--background", "--bind", &bind_addr])
        .output()
        .expect("duplicate start attempt");
    assert!(dup_output.status.success());
    let dup_text = String::from_utf8_lossy(&dup_output.stdout);
    assert!(
        dup_text.contains("Yeet remote UI already running"),
        "expected already running notice, got: {dup_text}"
    );

    // 5. Duplicate start prevention via API
    let dup_api_result = start_remote_background(&RemoteOptions {
        bind: bind_addr,
        ..Default::default()
    });
    assert!(dup_api_result.is_err());
    let err_msg = dup_api_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("already running"),
        "expected already running error, got: {err_msg}"
    );

    // Original remote remains alive and unchanged
    assert_eq!(remote_pid().unwrap(), Some(pid));

    // 6. Stop remote
    let stop_output = ctx
        .command()
        .args(["remote", "stop"])
        .output()
        .expect("stop remote");
    assert!(stop_output.status.success());
    assert!(String::from_utf8_lossy(&stop_output.stdout).contains("Stopped Yeet remote UI"));

    assert!(remote_status().unwrap().is_none());
}

#[test]
fn stop_and_direct_termination_do_not_respawn() {
    let _lock = LIFECYCLE_TEST_LOCK.lock().unwrap();
    let ctx = TestContext::new();

    // Part A: Normal stop does not respawn
    let port_a = pick_unused_port();
    let bind_a = format!("127.0.0.1:{port_a}");
    let start_output = ctx
        .command()
        .args(["remote", "--background", "--bind", &bind_a])
        .output()
        .expect("start A");
    assert!(start_output.status.success());
    assert!(remote_status().unwrap().is_some());

    assert!(stop_remote().unwrap());
    assert!(remote_status().unwrap().is_none());

    // Sleep to verify no supervisor or service manager resurrects the stopped process
    thread::sleep(Duration::from_millis(500));
    assert!(
        remote_status().unwrap().is_none(),
        "stopped process was unexpectedly respawned"
    );

    let status_output = ctx
        .command()
        .args(["remote", "status"])
        .output()
        .expect("status check A");
    assert!(
        String::from_utf8_lossy(&status_output.stdout).contains("Yeet remote UI is not running")
    );

    // Part B: Direct process termination (SIGKILL) does not respawn
    let port_b = pick_unused_port();
    let bind_b = format!("127.0.0.1:{port_b}");
    let start_output = ctx
        .command()
        .args(["remote", "--background", "--bind", &bind_b])
        .output()
        .expect("start B");
    assert!(start_output.status.success());
    let pid = remote_pid().unwrap().expect("pid for B");

    // Directly kill the process without using yeet remote stop
    #[cfg(unix)]
    unsafe {
        let ret = libc::kill(pid as libc::pid_t, libc::SIGKILL);
        assert_eq!(ret, 0, "kill -9 failed");
    }
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("taskkill");
        assert!(status.success());
    }

    // Wait for the process to exit.
    let dead = {
        #[cfg(unix)]
        {
            let mut dead = false;
            for _ in 0..50 {
                unsafe {
                    if libc::kill(pid as libc::pid_t, 0) != 0 {
                        dead = true;
                        break;
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }
            dead
        }
        #[cfg(windows)]
        {
            true
        }
    };
    assert!(dead, "process did not terminate after forced termination");

    // remote_status must immediately reflect not running
    assert!(remote_status().unwrap().is_none());

    let status_output = ctx
        .command()
        .args(["remote", "status"])
        .output()
        .expect("status check B");
    assert!(
        String::from_utf8_lossy(&status_output.stdout).contains("Yeet remote UI is not running")
    );

    // Sleep to verify no supervisor (launchd/systemd/monitor) respawns it
    thread::sleep(Duration::from_millis(500));
    assert!(
        remote_status().unwrap().is_none(),
        "process was unexpectedly respawned after direct kill"
    );
}

#[test]
fn shell_cwd_neutral_start_and_workspace() {
    let _lock = LIFECYCLE_TEST_LOCK.lock().unwrap();
    let ctx = TestContext::new();
    let port = pick_unused_port();
    let bind = format!("127.0.0.1:{port}");

    // Create an ephemeral temporary directory to serve as caller shell cwd
    let ephemeral_dir = tempfile::tempdir().expect("ephemeral cwd");
    let ephemeral_path = ephemeral_dir.path().to_path_buf();

    // Start background remote from the ephemeral directory
    let mut cmd = ctx.command();
    cmd.args(["remote", "--background", "--bind", &bind]);
    cmd.current_dir(&ephemeral_path);
    let start_output = cmd.output().expect("start from ephemeral cwd");
    assert!(
        start_output.status.success(),
        "failed to start from ephemeral cwd: {}",
        String::from_utf8_lossy(&start_output.stderr)
    );

    assert!(remote_status().unwrap().is_some());

    // Delete the ephemeral directory from which remote was started
    drop(ephemeral_dir);
    assert!(!ephemeral_path.exists());

    // Remote process must remain alive and responsive
    assert!(remote_status().unwrap().is_some());
    let status_output = ctx
        .command()
        .args(["remote", "status"])
        .output()
        .expect("status check after cwd deletion");
    assert!(status_output.status.success());
    assert!(String::from_utf8_lossy(&status_output.stdout).contains(&format!(":{port}")));

    // Verify workspace neutrality contract: RemoteHub default workspace resolves
    // to user home directory, never to the shell's ephemeral current working directory.
    let home = dirs::home_dir()
        .expect("home directory")
        .canonicalize()
        .expect("canonical home");
    let hub = crate::remote::websocket::RemoteHub::new();
    assert_eq!(hub.resolve_workspace(None).unwrap(), home);
    assert_eq!(hub.resolve_workspace(Some(".")).unwrap(), home);

    // Clean up
    assert!(stop_remote().unwrap());
    assert!(remote_status().unwrap().is_none());
}
