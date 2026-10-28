use std::{
    collections::BTreeMap,
    fs, io,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{Mutex, OnceLock},
};

#[cfg(windows)]
use std::{
    collections::HashSet,
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
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
    LocalListener::bind(endpoint).map_err(|error| socket_path_error(endpoint, error))
}

// Unix socket paths are limited to ~108 bytes; name the path and the fix
// instead of surfacing the bare SUN_LEN error.
#[cfg(unix)]
fn socket_path_error(endpoint: &Path, error: io::Error) -> io::Error {
    if error.kind() != io::ErrorKind::InvalidInput || !error.to_string().contains("SUN_LEN") {
        return error;
    }
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "socket path is too long for a Unix socket ({} bytes): {}; set YEET_CONFIG_DIR to a shorter directory",
            endpoint.as_os_str().len(),
            endpoint.display()
        ),
    )
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
    LocalStream::connect(endpoint).map_err(|error| socket_path_error(endpoint, error))
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
    let output = Command::new("icacls")
        .arg(path)
        .arg("/inheritancelevel:r")
        .arg("/grant:r")
        .args(&grants)
        .arg("/Q")
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = stderr.trim();
        let detail = if detail.is_empty() {
            stdout.trim()
        } else {
            detail
        };
        return Err(io::Error::other(if detail.is_empty() {
            format!("icacls failed while hardening {}", path.display())
        } else {
            format!("icacls failed while hardening {}: {detail}", path.display())
        }));
    }
    hardened
        .lock()
        .map_err(|_| io::Error::other("Windows ACL cache lock poisoned"))?
        .insert(key);
    Ok(())
}

pub(crate) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    // fs::rename replaces atomically on Unix and on Windows (MoveFileExW with
    // MOVEFILE_REPLACE_EXISTING). The remove-then-rename fallback only covers
    // destinations that reject replacement while held open.
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(first) => {
            #[cfg(windows)]
            if destination.exists() {
                fs::remove_file(destination)?;
                return fs::rename(source, destination);
            }
            Err(first)
        }
    }
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Renders a path for display, dropping the Windows `\\?\` verbatim prefix
/// that `fs::canonicalize` adds. Boundary checks re-canonicalize their inputs,
/// so display strings and internal forms stay interchangeable.
pub(crate) fn display_path(path: &Path) -> String {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return rest.to_owned();
        }
    }
    path.display().to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnedChildInfo {
    pub pid: u32,
    pub role: &'static str,
}

#[cfg(windows)]
mod job_object {
    //! Windows Job Objects give `TrackedChild` reliable descendant containment.
    //! Every spawned descendant joins the job automatically, and
    //! `KILL_ON_JOB_CLOSE` reaps surviving descendants when the owning handle
    //! closes — including when the Yeet process dies unexpectedly.
    use std::ptr;

    pub(super) type Handle = *mut std::ffi::c_void;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        operations: [u64; 6],
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimitInformation {
        basic: BasicLimitInformation,
        io: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    pub(super) const CTRL_BREAK_EVENT: u32 = 1;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(job_attributes: *const std::ffi::c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            info_class: i32,
            info: *const std::ffi::c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        pub(super) fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
        pub(super) fn CloseHandle(handle: Handle) -> i32;
        pub(super) fn GenerateConsoleCtrlEvent(event: u32, process_group: u32) -> i32;
    }

    #[derive(Debug)]
    pub(super) struct Job {
        handle: Handle,
    }

    impl Job {
        /// Containment is best effort: when the OS refuses the assignment (for
        /// example inside a non-nesting parent job on older Windows), callers
        /// fall back to the process-tree termination path instead of failing
        /// the spawn.
        pub(super) fn contain(process: Handle) -> Option<Self> {
            unsafe {
                let handle = CreateJobObjectW(ptr::null(), ptr::null());
                if handle.is_null() {
                    return None;
                }
                let mut info = ExtendedLimitInformation::default();
                info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let configured = SetInformationJobObject(
                    handle,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    (&info as *const ExtendedLimitInformation).cast(),
                    std::mem::size_of::<ExtendedLimitInformation>() as u32,
                );
                if configured == 0 || AssignProcessToJobObject(handle, process) == 0 {
                    let _ = CloseHandle(handle);
                    return None;
                }
                Some(Self { handle })
            }
        }

        /// Forces every process in the job to exit without process-tree
        /// discovery races.
        pub(super) fn terminate(&self) {
            unsafe {
                let _ = TerminateJobObject(self.handle, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // Closing the last handle reaps survivors via KILL_ON_JOB_CLOSE,
            // mirroring the Unix process-group kill after a leader exits.
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }

    // The job handle is exclusively owned and every kernel32 call is safe from
    // any thread, so the raw pointer may move with its owner.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}
}

static OWNED_CHILDREN: OnceLock<Mutex<BTreeMap<u32, &'static str>>> = OnceLock::new();

fn owned_children() -> &'static Mutex<BTreeMap<u32, &'static str>> {
    OWNED_CHILDREN.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn shell_signal_grace(value: Option<&str>) -> u64 {
    value
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(500)
        .min(5_000)
}

#[derive(Debug)]
pub(crate) struct TrackedChild {
    child: Child,
    pid: u32,
    #[cfg(windows)]
    job: Option<job_object::Job>,
}

impl TrackedChild {
    pub(crate) fn new(child: Child, role: &'static str) -> Self {
        let pid = child.id();
        if let Ok(mut children) = owned_children().lock() {
            children.insert(pid, role);
        }
        #[cfg(windows)]
        let job = {
            use std::os::windows::io::AsRawHandle;
            job_object::Job::contain(child.as_raw_handle())
        };
        Self {
            child,
            pid,
            #[cfg(windows)]
            job,
        }
    }

    pub(crate) fn kill(&mut self) -> io::Result<()> {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            // Terminating the job covers descendants spawned after launch,
            // which a PID-based tree walk can miss.
            job.terminate();
            return Ok(());
        }
        force_terminate_process_tree(self.pid)
    }

    pub(crate) fn terminate_shell(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            let tree = unix_process_tree(self.pid).unwrap_or_else(|_| vec![self.pid]);
            let grace =
                shell_signal_grace(std::env::var("YEET_SHELL_SIGNAL_GRACE_MS").ok().as_deref());
            for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGKILL] {
                let _ = signal_process_group(self.pid, signal);
                for target in tree.iter().rev().copied().filter(|pid| *pid != self.pid) {
                    let _ = signal_owned_process_or_group(target, signal);
                }
                if signal != libc::SIGKILL {
                    std::thread::sleep(std::time::Duration::from_millis(grace));
                }
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            let grace =
                shell_signal_grace(std::env::var("YEET_SHELL_SIGNAL_GRACE_MS").ok().as_deref());
            // Graceful escalation mirrors Unix SIGINT → SIGTERM → SIGKILL.
            // A console break interrupts shell traps and foreground children;
            // taskkill without /F then allows GUI descendants a WM_CLOSE
            // shutdown. Both are best effort and never block forced cleanup.
            unsafe {
                let _ =
                    job_object::GenerateConsoleCtrlEvent(job_object::CTRL_BREAK_EVENT, self.pid);
            }
            let pid_text = self.pid.to_string();
            let _ = Command::new("taskkill")
                .args(["/PID", &pid_text, "/T"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            std::thread::sleep(std::time::Duration::from_millis(grace));
            if let Some(job) = &self.job {
                job.terminate();
            } else {
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid_text, "/T", "/F"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        {
            force_terminate_process_tree(self.pid)
        }
    }
}

impl Deref for TrackedChild {
    type Target = Child;
    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl DerefMut for TrackedChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

impl Drop for TrackedChild {
    fn drop(&mut self) {
        let still_running = self
            .child
            .try_wait()
            .map(|status| status.is_none())
            .unwrap_or(true);
        if still_running {
            if self.kill().is_err() {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
        } else {
            #[cfg(unix)]
            {
                // The group leader may have exited after starting background
                // descendants. Its PGID remains addressable even though the
                // original PID no longer exists.
                let _ = signal_process_group(self.pid, libc::SIGKILL);
            }
        }
        if let Ok(mut children) = owned_children().lock() {
            children.remove(&self.pid);
        }
    }
}

pub(crate) fn tracked_children_snapshot() -> Vec<OwnedChildInfo> {
    owned_children()
        .lock()
        .map(|children| {
            children
                .iter()
                .map(|(&pid, &role)| OwnedChildInfo { pid, role })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn systemd_managed_process() -> bool {
    #[cfg(target_os = "linux")]
    {
        // systemd keeps descendants in the unit cgroup even after setsid().
        std::env::var_os("INVOCATION_ID").is_some() || std::env::var_os("NOTIFY_SOCKET").is_some()
    }

    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}
pub(crate) fn configure_detached(command: &mut Command) {
    #[cfg(unix)]
    {
        if systemd_managed_process() {
            return;
        }
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

#[cfg(windows)]
struct StandardHandleInheritanceGuard {
    handles: Vec<*mut std::ffi::c_void>,
}

#[cfg(windows)]
impl StandardHandleInheritanceGuard {
    fn new() -> io::Result<Self> {
        const STD_INPUT_HANDLE: u32 = (-10_i32) as u32;
        const STD_OUTPUT_HANDLE: u32 = (-11_i32) as u32;
        const STD_ERROR_HANDLE: u32 = (-12_i32) as u32;
        const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
        const INVALID_HANDLE_VALUE: *mut std::ffi::c_void = (-1_isize) as *mut std::ffi::c_void;

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetStdHandle(n_std_handle: u32) -> *mut std::ffi::c_void;
            fn GetHandleInformation(handle: *mut std::ffi::c_void, flags: *mut u32) -> i32;
            fn SetHandleInformation(handle: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
        }

        let mut guard = Self {
            handles: Vec::with_capacity(3),
        };
        for stream in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = unsafe { GetStdHandle(stream) };
            if handle.is_null() || handle == INVALID_HANDLE_VALUE || guard.handles.contains(&handle)
            {
                continue;
            }

            let mut flags = 0_u32;
            if unsafe { GetHandleInformation(handle, &mut flags) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if flags & HANDLE_FLAG_INHERIT == 0 {
                continue;
            }
            if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
                return Err(io::Error::last_os_error());
            }
            guard.handles.push(handle);
        }
        Ok(guard)
    }
}

#[cfg(windows)]
impl Drop for StandardHandleInheritanceGuard {
    fn drop(&mut self) {
        const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetHandleInformation(handle: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
        }

        for handle in self.handles.drain(..) {
            let _ =
                unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
        }
    }
}

pub(crate) fn spawn_detached(command: &mut Command) -> io::Result<Child> {
    configure_detached(command);

    #[cfg(windows)]
    {
        static DETACHED_SPAWN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = DETACHED_SPAWN_LOCK.get_or_init(|| Mutex::new(()));
        let _spawn_guard = lock
            .lock()
            .map_err(|_| io::Error::other("detached process spawn lock poisoned"))?;
        let _inheritance_guard = StandardHandleInheritanceGuard::new()?;
        command.spawn()
    }

    #[cfg(not(windows))]
    {
        command.spawn()
    }
}

pub(crate) fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        #[cfg(target_os = "linux")]
        let owner_pid = std::process::id() as libc::pid_t;
        #[cfg(target_os = "linux")]
        let arm_parent_death_signal =
            unsafe { libc::syscall(libc::SYS_gettid) as libc::pid_t } == owner_pid;
        unsafe {
            command.pre_exec(move || {
                if libc::setpgid(0, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                #[cfg(target_os = "linux")]
                {
                    // PR_SET_PDEATHSIG tracks the specific thread that forked the
                    // child, not the Rust process as a whole. Yeet starts some
                    // owned children from short-lived worker threads, so arming
                    // it there kills a healthy child as soon as that worker
                    // returns. Only use PDEATHSIG when the process-leader thread
                    // performs the spawn; TrackedChild still owns normal cleanup.
                    if arm_parent_death_signal {
                        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                            return Err(io::Error::last_os_error());
                        }
                        // Close the fork -> prctl race. If the original process
                        // died before PDEATHSIG was armed, refuse to start orphaned.
                        if libc::getppid() != owner_pid {
                            return Err(io::Error::from_raw_os_error(libc::ECHILD));
                        }
                    }
                }
                Ok(())
            });
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // A dedicated process group isolates console control events from the
        // parent and gives `terminate_shell` a Ctrl-Break target, mirroring the
        // Unix setpgid() setup above.
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }

    #[cfg(not(any(unix, windows)))]
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

#[cfg(test)]
mod portable_tests {
    use super::*;

    #[test]
    fn shell_signal_grace_is_configurable_and_bounded() {
        assert_eq!(shell_signal_grace(None), 500);
        assert_eq!(shell_signal_grace(Some("not-a-number")), 500);
        assert_eq!(shell_signal_grace(Some("0")), 0);
        assert_eq!(shell_signal_grace(Some("175")), 175);
        assert_eq!(shell_signal_grace(Some("9000")), 5_000);
    }

    #[test]
    fn display_path_hides_internal_verbatim_prefixes() {
        #[cfg(windows)]
        {
            assert_eq!(display_path(Path::new(r"\\?\C:\proj\sub")), r"C:\proj\sub");
            assert_eq!(
                display_path(Path::new(r"\\?\UNC\server\share\x")),
                r"\\server\share\x"
            );
            assert_eq!(display_path(Path::new(r"C:\proj")), r"C:\proj");
        }
        #[cfg(not(windows))]
        {
            assert_eq!(display_path(Path::new("/a/b")), "/a/b");
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::time::{Duration, Instant};

    /// Runs a cmd script as one raw command line so tests are independent of
    /// `Command::arg` quoting behavior under test elsewhere.
    fn raw_cmd(script: &str) -> Command {
        let mut command = Command::new("cmd");
        command
            .raw_arg("/D")
            .raw_arg("/S")
            .raw_arg("/C")
            .raw_arg(format!("\"{script}\""))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        configure_process_group(&mut command);
        command
    }

    fn image_count(image: &str) -> usize {
        let output = Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {image}"), "/FO", "CSV", "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.starts_with(&format!("\"{image}\"")))
            .count()
    }

    fn wait_for(description: &str, mut condition: impl FnMut() -> bool) {
        let start = Instant::now();
        while !condition() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out waiting for {description}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn exit_codes_and_quoted_commands_survive_cmd_execution() {
        let script = r#"echo "quoted & spaced" >nul & cmd /c exit 7"#;
        let status = raw_cmd(script).status().unwrap();
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn tracked_child_drop_reaps_descendants_after_leader_exit() {
        let baseline = image_count("PING.EXE");
        // The leader exits immediately after backgrounding a long-lived ping;
        // only job-object cleanup can still reclaim that descendant.
        let mut child = TrackedChild::new(
            raw_cmd(r#"start "" /b ping -n 60 127.0.0.1 >nul & exit /b 0"#)
                .spawn()
                .unwrap(),
            "test-job-containment",
        );
        child.wait().unwrap();
        wait_for("background descendant to start", || {
            image_count("PING.EXE") > baseline
        });
        drop(child);
        wait_for("background descendant to be reaped", || {
            image_count("PING.EXE") <= baseline
        });
    }

    #[test]
    fn shell_termination_cleans_up_the_process_tree_promptly() {
        let baseline = image_count("PING.EXE");
        let mut child = TrackedChild::new(
            raw_cmd(r#"ping -n 60 127.0.0.1 >nul"#).spawn().unwrap(),
            "test-termination",
        );
        wait_for("child process to start", || {
            image_count("PING.EXE") > baseline
        });
        let start = Instant::now();
        child.terminate_shell().unwrap();
        let _ = child.wait().unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "forced termination was not bounded"
        );
        wait_for("child process tree to be reaped", || {
            image_count("PING.EXE") <= baseline
        });
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn process_tree_is_bounded_and_cycle_safe() {
        for (pairs, expected) in [
            (
                vec![(11, 10), (12, 10), (21, 11), (22, 11), (31, 21), (99, 1)],
                vec![10, 11, 12, 21, 22, 31],
            ),
            (vec![(11, 10), (10, 11), (12, 11)], vec![10, 11, 12]),
        ] {
            assert_eq!(process_tree_from_pairs(10, &pairs), expected);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_group_child_spawned_from_worker_survives_worker_exit() {
        let mut child = std::thread::spawn(|| {
            let mut command = Command::new("/bin/sh");
            command
                .arg("-c")
                .arg("sleep 30")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            configure_process_group(&mut command);
            command.spawn().unwrap()
        })
        .join()
        .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            child.try_wait().unwrap().is_none(),
            "child was killed when its spawning worker thread exited"
        );

        let pid = child.id();
        let _ = force_terminate_process_tree(pid);
        let _ = child.wait();
    }

    #[test]
    fn shell_termination_escalates_through_interrupt_term_and_kill() {
        let directory = tempfile::tempdir().unwrap();
        let ready = directory.path().join("ready");
        let signals = directory.path().join("signals");
        let mut command = Command::new("/bin/sh");
        command.arg("-c")
            .arg("trap 'echo INT >> \"$SIGNALS\"' INT; trap 'echo TERM >> \"$SIGNALS\"' TERM; echo ready > \"$READY\"; while :; do :; done")
            .env("SIGNALS", &signals).env("READY", &ready)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        configure_process_group(&mut command);
        let mut child = TrackedChild::new(command.spawn().unwrap(), "test-escalation");
        let start = std::time::Instant::now();
        while !ready.exists() {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "child did not start"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.terminate_shell().unwrap();
        let status = child.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        assert_eq!(std::fs::read_to_string(signals).unwrap(), "INT\nTERM\n");
    }

    fn process_group_exists(pgid: u32) -> bool {
        let result = unsafe { libc::kill(-(pgid as i32), 0) };
        let signalable =
            result == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
        signalable && live_group_member(pgid)
    }

    // Killed members reparented to a PID 1 that never reaps (common in
    // containers) stay signalable as zombies; only running members count.
    #[cfg(target_os = "linux")]
    fn live_group_member(pgid: u32) -> bool {
        let Ok(entries) = fs::read_dir("/proc") else {
            return true;
        };
        entries.flatten().any(|entry| {
            let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
                return false;
            };
            let Some((_, fields)) = stat.rsplit_once(") ") else {
                return false;
            };
            let fields = fields.split_whitespace().collect::<Vec<_>>();
            fields.first() != Some(&"Z") && fields.get(2) == Some(&pgid.to_string().as_str())
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn live_group_member(_pgid: u32) -> bool {
        true
    }

    #[test]
    fn tracked_child_drop_kills_group_after_leader_exits() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("background.pid");
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("trap '' HUP; sleep 30 & echo $! > \"$YEET_TEST_PID_FILE\"")
            .env("YEET_TEST_PID_FILE", &pid_file)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        configure_process_group(&mut command);
        let raw_child = command.spawn().unwrap();
        let pgid = raw_child.id();
        let mut child = TrackedChild::new(raw_child, "test-background");
        child.wait().unwrap();

        assert!(process_group_exists(pgid));
        drop(child);
        for _ in 0..40 {
            if !process_group_exists(pgid) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!process_group_exists(pgid));
    }
}
