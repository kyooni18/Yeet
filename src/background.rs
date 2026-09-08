use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Write},
    net::Shutdown,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use fs2::FileExt;

use crate::{
    backend::{BackendEvent, BackendService},
    config::ConfigStore,
    model::{BridgeEnvelope, FrontendCommand},
    platform::{
        LocalStream, bind_local, configure_detached, connect_local, set_private_directory,
        set_private_file,
    },
};

enum ClientEvent {
    Command {
        client_id: u64,
        command: FrontendCommand,
    },
    Disconnected(u64),
}

struct ClientConnection {
    id: u64,
    runtime_id: u64,
    stream: LocalStream,
}

struct SessionRuntime {
    id: u64,
    service: BackendService,
    idle_since: Option<Instant>,
}

const CONNECT_RETRIES: usize = 60;
const CONNECT_DELAY: Duration = Duration::from_millis(50);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(750);
const IDLE_EXIT_AFTER: Duration = Duration::from_secs(60);
const STALE_DAEMON_EXIT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_BACKGROUND_LOG_BYTES: u64 = 8 * 1024 * 1024;

pub struct BackgroundConnection {
    workspace: PathBuf,
    scope: Option<String>,
    resume_session_id: Option<String>,
    stream: LocalStream,
    events: mpsc::Receiver<BridgeEnvelope>,
}

impl BackgroundConnection {
    pub fn connect(workspace: &Path) -> Result<Self> {
        Self::connect_scoped(workspace, None)
    }

    pub fn connect_scoped(workspace: &Path, scope: Option<&str>) -> Result<Self> {
        let workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        let scope = scope.map(str::to_owned);
        let (stream, events) = Self::establish(&workspace, scope.as_deref())?;
        Ok(Self {
            workspace,
            scope,
            resume_session_id: None,
            stream,
            events,
        })
    }

    fn establish(
        workspace: &Path,
        scope: Option<&str>,
    ) -> Result<(LocalStream, mpsc::Receiver<BridgeEnvelope>)> {
        let paths = BackgroundPaths::new(workspace, scope)?;
        let executable_identity = current_executable_identity()?;

        if daemon_identity_matches(&paths, &executable_identity)
            && let Ok(connection) = Self::connect_existing(&paths.socket)
        {
            return Ok(connection);
        }

        // Serialise recovery/startup for this workspace. Without this, two
        // terminals that arrive together can both decide the socket is stale,
        // unlink each other's listener, and leave two independent daemons.
        let _lifecycle_lock = LifecycleLock::acquire(&paths.lifecycle_lock)?;

        // Another terminal may have finished starting the daemon while this
        // process was waiting for the lifecycle lock.
        if daemon_identity_matches(&paths, &executable_identity)
            && let Ok(connection) = Self::connect_existing(&paths.socket)
        {
            return Ok(connection);
        }

        if paths.socket.exists() || paths.identity.exists() {
            retire_stale_daemon(&paths)?;
        }

        spawn_daemon(workspace, scope, &paths)?;
        for _ in 0..CONNECT_RETRIES {
            match Self::connect_existing(&paths.socket) {
                Ok(connection) => return Ok(connection),
                Err(_) => thread::sleep(CONNECT_DELAY),
            }
        }
        bail!(
            "background service did not become ready; see {}",
            paths.log.display()
        )
    }

    fn connect_existing(socket: &Path) -> Result<(LocalStream, mpsc::Receiver<BridgeEnvelope>)> {
        let stream = connect_local(socket)?;
        let reader_stream = stream.try_clone()?;
        reader_stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        let mut reader = BufReader::new(reader_stream);
        let mut first_line = String::new();
        match reader.read_line(&mut first_line) {
            Ok(0) => bail!("background service closed during handshake"),
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
        let first_envelope = serde_json::from_str::<BridgeEnvelope>(first_line.trim_end())
            .context("invalid background service handshake")?;
        reader.get_mut().set_read_timeout(None)?;
        let (tx, events) = mpsc::channel();
        tx.send(first_envelope)
            .map_err(|_| anyhow!("background event channel closed"))?;
        thread::spawn(move || {
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Ok(envelope) =
                            serde_json::from_str::<BridgeEnvelope>(line.trim_end())
                            && tx.send(envelope).is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok((stream, events))
    }

    pub fn send(&mut self, command: &FrontendCommand) -> Result<()> {
        self.observe_command(command);
        let mut frame = serde_json::to_vec(command)?;
        frame.push(b'\n');
        match self.stream.write_all(&frame) {
            Ok(()) => Ok(()),
            Err(error) if reconnectable_socket_error(&error) => {
                self.reconnect()?;
                self.stream.write_all(&frame).map_err(Into::into)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn try_recv(&mut self) -> Option<BridgeEnvelope> {
        match self.events.try_recv() {
            Ok(envelope) => {
                self.observe_envelope(&envelope);
                Some(envelope)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.reconnect().ok()?;
                let envelope = self.events.try_recv().ok()?;
                self.observe_envelope(&envelope);
                Some(envelope)
            }
        }
    }

    fn observe_command(&mut self, command: &FrontendCommand) {
        match command {
            FrontendCommand::LoadSession { session_id } => {
                self.resume_session_id = Some(session_id.clone());
            }
            FrontendCommand::NewSession => self.resume_session_id = None,
            _ => {}
        }
    }

    fn observe_envelope(&mut self, envelope: &BridgeEnvelope) {
        if envelope.kind != "state" {
            return;
        }
        if let Some(state) = &envelope.state {
            self.resume_session_id = state.current_session_id.clone();
        }
    }

    fn reconnect(&mut self) -> Result<()> {
        // Cloning the local stream duplicates the same endpoint. Merely
        // dropping the writer does not wake the reader thread because its
        // duplicate descriptor remains open. Explicit shutdown tears down the
        // endpoint for every duplicate so the old client is actually detached.
        let _ = self.stream.shutdown(Shutdown::Both);
        let (mut stream, events) = Self::establish(&self.workspace, self.scope.as_deref())?;
        if let Some(session_id) = self.resume_session_id.as_deref() {
            // `establish` always queues the daemon's initial snapshot. When a
            // connection is resuming a known session that snapshot may belong
            // to another detached runtime selected before LoadSession arrives.
            // Drop it so neither the UI nor our affinity can briefly switch to
            // the wrong session while the explicit resume command is handled.
            let _ = events.try_recv();
            let mut frame = serde_json::to_vec(&FrontendCommand::LoadSession {
                session_id: session_id.to_owned(),
            })?;
            frame.push(b'\n');
            stream.write_all(&frame)?;
            stream.flush()?;
        }
        self.stream = stream;
        self.events = events;
        Ok(())
    }
}

impl Drop for BackgroundConnection {
    fn drop(&mut self) {
        // See reconnect(): this is required to release the reader thread's
        // cloned descriptor and let the daemon observe a real disconnect.
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

fn reconnectable_socket_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected
    )
}

struct BackgroundPaths {
    socket: PathBuf,
    log: PathBuf,
    identity: PathBuf,
    lifecycle_lock: PathBuf,
}

impl BackgroundPaths {
    fn new(workspace: &Path, scope: Option<&str>) -> Result<Self> {
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
            lifecycle_lock: directory.join(format!("{key}.lock")),
        })
    }
}

struct LifecycleLock(File);

impl LifecycleLock {
    fn acquire(path: &Path) -> Result<Self> {
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
}

impl Drop for LifecycleLock {
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

fn current_executable_identity() -> Result<String> {
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

fn daemon_identity_matches(paths: &BackgroundPaths, expected: &str) -> bool {
    fs::read_to_string(&paths.identity)
        .ok()
        .is_some_and(|value| value.trim() == expected)
}

fn retire_stale_daemon(paths: &BackgroundPaths) -> Result<()> {
    match connect_local(&paths.socket) {
        Ok(mut stream) => {
            let mut frame = serde_json::to_vec(&FrontendCommand::Shutdown)?;
            frame.push(b'\n');
            let _ = stream.write_all(&frame);
            let _ = stream.flush();
            let _ = stream.shutdown(Shutdown::Both);
        }
        Err(error) if stale_socket_error(&error) => {
            cleanup_stale_daemon_files(paths);
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    }

    let started = Instant::now();
    while paths.socket.exists() && started.elapsed() < STALE_DAEMON_EXIT_TIMEOUT {
        thread::sleep(Duration::from_millis(25));
    }
    if paths.socket.exists() {
        // A daemon that died without running its cleanup leaves a pathname
        // behind, but connect(2) reports it as refused. Treat that as an orphan
        // rather than making every post-update restart fail until manual rm.
        match connect_local(&paths.socket) {
            Err(error) if stale_socket_error(&error) => {
                cleanup_stale_daemon_files(paths);
                return Ok(());
            }
            Err(error) => return Err(error.into()),
            Ok(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        bail!(
            "stale background service did not stop after Yeet was updated; remove {} or terminate the old Yeet daemon",
            paths.socket.display()
        );
    }
    cleanup_stale_daemon_files(paths);
    Ok(())
}

fn stale_socket_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
    )
}

fn cleanup_stale_daemon_files(paths: &BackgroundPaths) {
    let _ = fs::remove_file(&paths.socket);
    let _ = fs::remove_file(&paths.identity);
}

fn spawn_daemon(workspace: &Path, scope: Option<&str>, paths: &BackgroundPaths) -> Result<()> {
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
    configure_detached(&mut command);
    command.spawn().context("start Yeet background service")?;
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

pub fn run_daemon(workspace: PathBuf, scope: Option<String>) -> Result<()> {
    let workspace = workspace.canonicalize().unwrap_or(workspace);
    let paths = BackgroundPaths::new(&workspace, scope.as_deref())?;
    let executable_identity = current_executable_identity()?;
    let _ = fs::remove_file(&paths.socket);
    let _ = fs::remove_file(&paths.identity);
    let listener = bind_local(&paths.socket)
        .with_context(|| format!("bind background socket {}", paths.socket.display()))?;
    let _cleanup = DaemonFilesCleanup::new(&paths);
    set_private_file(&paths.socket)?;
    listener.set_nonblocking(true)?;

    fs::write(&paths.identity, format!("{executable_identity}\n"))?;
    set_private_file(&paths.identity)?;
    let (client_tx, client_rx) = mpsc::channel::<ClientEvent>();
    let mut clients: Vec<ClientConnection> = Vec::new();
    let mut runtimes: Vec<SessionRuntime> = Vec::new();
    let mut next_client_id = 1u64;
    let mut next_runtime_id = 1u64;
    let mut idle_since: Option<Instant> = None;
    let mut shutdown_requested = false;

    loop {
        while let Ok(event) = client_rx.try_recv() {
            match event {
                ClientEvent::Command { client_id, command } => {
                    let Some(client_index) =
                        clients.iter().position(|client| client.id == client_id)
                    else {
                        continue;
                    };
                    let runtime_id = clients[client_index].runtime_id;

                    match command {
                        FrontendCommand::Shutdown => {
                            shutdown_requested = true;
                        }
                        FrontendCommand::LoadSession { session_id } => {
                            let live_runtime_id = runtimes
                                .iter()
                                .find(|runtime| {
                                    !runtime.service.is_closed()
                                        && runtime.service.current_session_id().as_deref()
                                            == Some(session_id.as_str())
                                })
                                .map(|runtime| runtime.id);

                            if let Some(target_runtime_id) = live_runtime_id {
                                clients[client_index].runtime_id = target_runtime_id;
                                if let Some(runtime) = runtimes
                                    .iter_mut()
                                    .find(|runtime| runtime.id == target_runtime_id)
                                {
                                    runtime.idle_since = None;
                                    let envelope = BridgeEnvelope {
                                        kind: "state".into(),
                                        state: Some(runtime.service.state_snapshot()),
                                        message: None,
                                    };
                                    if send_envelope(&mut clients[client_index].stream, &envelope)
                                        .is_err()
                                    {
                                        clients.remove(client_index);
                                    }
                                }
                                continue;
                            }

                            if let Err(error) = dispatch_runtime_command(
                                &mut runtimes,
                                runtime_id,
                                FrontendCommand::LoadSession { session_id },
                            ) {
                                broadcast_runtime_error(&mut clients, runtime_id, error);
                            }
                        }
                        FrontendCommand::NewSession
                            if runtime_client_count(&clients, runtime_id) > 1 =>
                        {
                            let new_runtime_id = next_runtime_id;
                            next_runtime_id = next_runtime_id.wrapping_add(1).max(1);
                            let runtime = SessionRuntime {
                                id: new_runtime_id,
                                service: BackendService::spawn(workspace.clone())?,
                                idle_since: None,
                            };
                            let envelope = BridgeEnvelope {
                                kind: "state".into(),
                                state: Some(runtime.service.state_snapshot()),
                                message: None,
                            };
                            runtimes.push(runtime);
                            clients[client_index].runtime_id = new_runtime_id;
                            if send_envelope(&mut clients[client_index].stream, &envelope).is_err()
                            {
                                clients.remove(client_index);
                            }
                        }
                        command => {
                            if let Err(error) =
                                dispatch_runtime_command(&mut runtimes, runtime_id, command)
                            {
                                broadcast_runtime_error(&mut clients, runtime_id, error);
                            }
                        }
                    }
                }
                ClientEvent::Disconnected(client_id) => {
                    clients.retain(|client| client.id != client_id);
                }
            }
        }
        if shutdown_requested {
            break;
        }

        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    // On macOS, an accepted Unix-domain socket can inherit the
                    // listener's nonblocking status. Client readers are meant
                    // to block until the TUI sends a command; otherwise the
                    // initial WouldBlock is mistaken for a disconnect and the
                    // daemon closes the client immediately after handshake.
                    stream.set_nonblocking(false)?;
                    let reader = stream.try_clone()?;
                    let client_id = next_client_id;
                    next_client_id = next_client_id.wrapping_add(1).max(1);
                    let tx = client_tx.clone();
                    thread::spawn(move || client_reader(client_id, reader, tx));

                    let runtime_id =
                        reusable_runtime_id(&runtimes, &clients).unwrap_or_else(|| {
                            let runtime_id = next_runtime_id;
                            next_runtime_id = next_runtime_id.wrapping_add(1).max(1);
                            runtime_id
                        });
                    if !runtimes.iter().any(|runtime| runtime.id == runtime_id) {
                        runtimes.push(SessionRuntime {
                            id: runtime_id,
                            service: BackendService::spawn(workspace.clone())?,
                            idle_since: None,
                        });
                    }
                    let runtime = runtimes
                        .iter_mut()
                        .find(|runtime| runtime.id == runtime_id)
                        .expect("selected background runtime must exist");
                    runtime.idle_since = None;
                    let mut writer = stream;
                    let initial = BridgeEnvelope {
                        kind: "state".into(),
                        state: Some(runtime.service.state_snapshot()),
                        message: None,
                    };
                    if send_envelope(&mut writer, &initial).is_ok() {
                        clients.push(ClientConnection {
                            id: client_id,
                            runtime_id,
                            stream: writer,
                        });
                        idle_since = None;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            }
        }

        for runtime in &mut runtimes {
            while let Some(event) = runtime.service.try_recv() {
                let BackendEvent::Envelope(envelope) = event;
                broadcast_runtime_envelope(&mut clients, runtime.id, &envelope);
            }
        }

        let closed_runtime_ids = runtimes
            .iter()
            .filter(|runtime| runtime.service.is_closed())
            .map(|runtime| runtime.id)
            .collect::<Vec<_>>();
        if !closed_runtime_ids.is_empty() {
            clients.retain(|client| !closed_runtime_ids.contains(&client.runtime_id));
            runtimes.retain(|runtime| !closed_runtime_ids.contains(&runtime.id));
        }

        for runtime in &mut runtimes {
            if runtime_client_count(&clients, runtime.id) > 0 || runtime.service.is_streaming() {
                runtime.idle_since = None;
            } else {
                runtime.idle_since.get_or_insert_with(Instant::now);
            }
        }
        runtimes.retain(|runtime| {
            runtime
                .idle_since
                .is_none_or(|since| since.elapsed() < IDLE_EXIT_AFTER)
        });

        let any_streaming = runtimes
            .iter()
            .any(|runtime| runtime.service.is_streaming());
        if clients.is_empty() && !any_streaming {
            let since = idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= IDLE_EXIT_AFTER {
                break;
            }
        } else {
            idle_since = None;
        }
        thread::sleep(Duration::from_millis(16));
    }

    Ok(())
}

fn dispatch_runtime_command(
    runtimes: &mut [SessionRuntime],
    runtime_id: u64,
    command: FrontendCommand,
) -> Result<()> {
    let runtime = runtimes
        .iter_mut()
        .find(|runtime| runtime.id == runtime_id)
        .ok_or_else(|| anyhow!("background session runtime is no longer available"))?;
    runtime.service.send(command)
}

fn runtime_client_count(clients: &[ClientConnection], runtime_id: u64) -> usize {
    clients
        .iter()
        .filter(|client| client.runtime_id == runtime_id)
        .count()
}

fn reusable_runtime_id(runtimes: &[SessionRuntime], clients: &[ClientConnection]) -> Option<u64> {
    let states = runtimes
        .iter()
        .filter(|runtime| !runtime.service.is_closed())
        .map(|runtime| {
            (
                runtime.id,
                runtime_client_count(clients, runtime.id) > 0,
                runtime.service.is_streaming(),
            )
        })
        .collect::<Vec<_>>();
    choose_reusable_runtime_id(&states)
}

fn choose_reusable_runtime_id(states: &[(u64, bool, bool)]) -> Option<u64> {
    let detached = states
        .iter()
        .copied()
        .filter(|(_, attached, _)| !attached)
        .collect::<Vec<_>>();
    let streaming = detached
        .iter()
        .copied()
        .filter(|(_, _, streaming)| *streaming)
        .collect::<Vec<_>>();
    if streaming.len() == 1 {
        return Some(streaming[0].0);
    }
    if detached.len() == 1 {
        return Some(detached[0].0);
    }
    None
}

fn broadcast_runtime_error(
    clients: &mut Vec<ClientConnection>,
    runtime_id: u64,
    error: anyhow::Error,
) {
    let envelope = BridgeEnvelope {
        kind: "error".into(),
        state: None,
        message: Some(error.to_string()),
    };
    broadcast_runtime_envelope(clients, runtime_id, &envelope);
}

fn broadcast_runtime_envelope(
    clients: &mut Vec<ClientConnection>,
    runtime_id: u64,
    envelope: &BridgeEnvelope,
) {
    clients.retain_mut(|client| {
        client.runtime_id != runtime_id || send_envelope(&mut client.stream, envelope).is_ok()
    });
}

struct DaemonFilesCleanup {
    socket: PathBuf,
    identity: PathBuf,
}

impl DaemonFilesCleanup {
    fn new(paths: &BackgroundPaths) -> Self {
        Self {
            socket: paths.socket.clone(),
            identity: paths.identity.clone(),
        }
    }
}

impl Drop for DaemonFilesCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_file(&self.identity);
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn scoped_background_keys_do_not_collide_with_terminal_session() {
        let workspace = Path::new("/tmp/yeet-workspace");
        let terminal = background_key(workspace, None);
        let remote = background_key(workspace, Some("remote"));
        assert_ne!(terminal, remote);
        assert_eq!(terminal, background_key(workspace, None));
        assert_eq!(remote, background_key(workspace, Some("remote")));
    }

    #[test]
    fn live_clients_never_reuse_the_same_session_runtime() {
        assert_eq!(choose_reusable_runtime_id(&[(1, true, false)]), None);
        assert_eq!(
            choose_reusable_runtime_id(&[(1, true, true), (2, true, false)]),
            None
        );
    }

    #[test]
    fn detached_runtime_can_be_reused_for_reconnect() {
        assert_eq!(
            choose_reusable_runtime_id(&[(1, false, false), (2, true, false)]),
            Some(1)
        );
        assert_eq!(
            choose_reusable_runtime_id(&[(1, false, false), (2, false, true)]),
            Some(2)
        );
        assert_eq!(
            choose_reusable_runtime_id(&[(1, false, true), (2, false, true)]),
            None
        );
    }

    #[test]
    fn dropping_connection_really_detaches_cloned_socket_reader() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("background.sock");
        let listener = bind_local(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            send_envelope(
                &mut stream,
                &BridgeEnvelope {
                    kind: "state".into(),
                    state: None,
                    message: None,
                },
            )
            .unwrap();
            stream
                .set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let mut byte = [0u8; 1];
            stream.read(&mut byte)
        });

        let (stream, events) = BackgroundConnection::connect_existing(&socket).unwrap();
        let connection = BackgroundConnection {
            workspace: directory.path().to_path_buf(),
            scope: None,
            resume_session_id: None,
            stream,
            events,
        };
        drop(connection);

        assert_eq!(server.join().unwrap().unwrap(), 0);
    }

    #[test]
    fn stale_orphan_socket_is_removed_without_manual_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let paths = BackgroundPaths {
            socket: directory.path().join("background.sock"),
            log: directory.path().join("background.log"),
            identity: directory.path().join("background.identity"),
            lifecycle_lock: directory.path().join("background.lock"),
        };
        let listener = bind_local(&paths.socket).unwrap();
        fs::write(&paths.identity, b"stale\n").unwrap();
        drop(listener);

        assert!(paths.socket.exists());
        assert!(paths.identity.exists());
        retire_stale_daemon(&paths).unwrap();
        assert!(!paths.socket.exists());
        assert!(!paths.identity.exists());
    }
}

fn client_reader(client_id: u64, stream: LocalStream, tx: mpsc::Sender<ClientEvent>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => match serde_json::from_str::<FrontendCommand>(line.trim_end()) {
                Ok(command) => {
                    if tx
                        .send(ClientEvent::Command { client_id, command })
                        .is_err()
                    {
                        return;
                    }
                }
                Err(_) => continue,
            },
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(4));
                continue;
            }
            Err(_) => break,
        }
    }
    let _ = tx.send(ClientEvent::Disconnected(client_id));
}

fn send_envelope(stream: &mut LocalStream, envelope: &BridgeEnvelope) -> Result<()> {
    serde_json::to_writer(&mut *stream, envelope)?;
    stream.write_all(b"\n").map_err(|error| anyhow!(error))?;
    stream.flush().map_err(|error| anyhow!(error))
}
