use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::Shutdown,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    backend::SessionCatalog,
    config::ConfigStore,
    extensions::{ExtensionHost, ExtensionRequest},
    model::{FrontendCommand, HarnessEvent, HarnessState},
    platform::{LocalStream, bind_local, connect_local, set_private_file},
};

mod client_writer;
mod daemon_lifecycle;
pub(crate) mod protocol;
mod routing;
mod runtime_process;
mod wake;
use client_writer::ClientWriter;
pub use daemon_lifecycle::cleanup_stale_artifacts;
use daemon_lifecycle::{
    BackgroundPaths, DaemonFilesCleanup, InstanceLease, LifecycleLock, current_executable_identity,
    retire_stale_daemon, spawn_daemon, stale_socket_error,
};
#[cfg(test)]
use daemon_lifecycle::{cleanup_stale_artifacts_in, cleanup_stale_daemon_files};
use protocol::DaemonHandshake;
use routing::{
    PendingIsolation, live_session_runtime_id, publish_runtime_state, route_client_to_runtime,
    send_client_error_to, start_isolated_runtime,
};
use runtime_process::RuntimeProcess;
pub(crate) use wake::Wake;

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
    /// Bumped on every session-routing command so a slow isolated startup
    /// never overrides a newer LoadSession/NewSession from the same client.
    route_seq: u64,
    writer: ClientWriter,
}

impl ClientConnection {
    fn next_route(&mut self) -> u64 {
        self.route_seq = self.route_seq.wrapping_add(1);
        self.route_seq
    }
}

struct SessionRuntime {
    id: u64,
    service: RuntimeProcess,
    idle_since: Option<Instant>,
    interrupt_requested_at: Option<Instant>,
    /// Session catalog this runtime last published.
    catalog: Option<SessionCatalog>,
}

struct RuntimeStartupEvent {
    client_id: u64,
    runtime_id: u64,
    stream: LocalStream,
    result: Result<SessionRuntime>,
}

struct RuntimeIsolationEvent {
    runtime_id: u64,
    result: Result<SessionRuntime>,
}

const CONNECT_DELAY: Duration = Duration::from_millis(50);
const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
// A dead socket is detected immediately by the reader thread. This timeout is
// only a wedged-daemon fallback, so keep enough headroom for cold starts and
// large-state serialization instead of reconnecting healthy sessions.
const CLIENT_STALE_AFTER: Duration = Duration::from_secs(30);
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_millis(500);
const RECONNECT_BACKOFF_BASE: Duration = Duration::from_millis(250);
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(5);
const INTERRUPT_RECOVERY_AFTER: Duration = Duration::from_secs(4);
// Losing every frontend transport is not an interrupt request. Background
// runs continue until they finish naturally or a user explicitly interrupts.
const RUNTIME_IDLE_RETIRE_AFTER: Duration = Duration::from_secs(60);
// Keep the lightweight socket owner around longer than its heavy session
// runtimes. This avoids daemon start/stop churn while still reclaiming inactive
// BackendService/bridge state promptly.
const DAEMON_IDLE_EXIT_AFTER: Duration = Duration::from_secs(10 * 60);
const STALE_DAEMON_EXIT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_BACKGROUND_LOG_BYTES: u64 = 8 * 1024 * 1024;
// State snapshots can contain a retained transcript, so the protocol limit is
// deliberately larger than an ordinary command. It still prevents a peer
// that never sends a newline from growing a String without bound.
const MAX_BACKGROUND_FRAME_BYTES: usize = 32 * 1024 * 1024;
const MAX_CLIENT_COMMAND_BYTES: usize = 8 * 1024 * 1024;
const CLIENT_EVENT_QUEUE_CAPACITY: usize = 256;
// Upper bound on the daemon's wait. Producers wake it immediately; this only
// bounds polling of sources that cannot signal (listener, bridge events).
const DAEMON_TICK: Duration = Duration::from_millis(25);
const MAX_RUNTIME_EVENTS_PER_TICK: usize = 1024;
const MAX_PENDING_ISOLATIONS: usize = 8;
const MAX_PENDING_STARTUP_CLIENTS: usize = 64;

/// A socket that completed a compatible handshake with a workspace daemon.
struct AttachedDaemon {
    stream: LocalStream,
    events: mpsc::Receiver<HarnessEvent>,
}

pub struct BackgroundConnection {
    workspace: PathBuf,
    scope: Option<String>,
    resume_session_id: Option<String>,
    resume_barrier: Option<String>,
    stream: LocalStream,
    events: mpsc::Receiver<HarnessEvent>,
    last_daemon_activity: Instant,
    wake: Option<Wake>,
    reconnect_failures: u32,
    next_reconnect_at: Option<Instant>,
}

impl BackgroundConnection {
    pub fn connect(workspace: &Path) -> Result<Self> {
        Self::connect_scoped(workspace, None)
    }

    pub fn connect_scoped(workspace: &Path, scope: Option<&str>) -> Result<Self> {
        Self::connect_with_wake(workspace, scope, None)
    }

    /// Connects and notifies `wake` whenever a daemon frame arrives, so an
    /// event loop can block instead of polling `try_recv` on a timer.
    pub(crate) fn connect_with_wake(
        workspace: &Path,
        scope: Option<&str>,
        wake: Option<Wake>,
    ) -> Result<Self> {
        let workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        let scope = scope.map(str::to_owned);
        let attached = Self::establish(&workspace, scope.as_deref(), wake.clone())?;
        Ok(Self {
            workspace,
            scope,
            resume_session_id: None,
            resume_barrier: None,
            stream: attached.stream,
            events: attached.events,
            last_daemon_activity: Instant::now(),
            wake,
            reconnect_failures: 0,
            next_reconnect_at: None,
        })
    }

    fn establish(
        workspace: &Path,
        scope: Option<&str>,
        wake: Option<Wake>,
    ) -> Result<AttachedDaemon> {
        let paths = BackgroundPaths::new(workspace, scope)?;

        // A completed, compatible handshake (see `protocol`) is the
        // compatibility test. A daemon started by the previous Yeet binary can
        // keep active sessions alive across an atomic local install, so never
        // retire it merely because the executable identity (mtime/size)
        // changed.
        if let Ok(connection) = Self::connect_existing(&paths.socket, wake.clone()) {
            return Ok(connection);
        }

        // Serialise recovery/startup for this workspace. Without this, two
        // terminals that arrive together can both decide the socket is stale,
        // unlink each other's listener, and leave two independent daemons.
        let _lifecycle_lock = LifecycleLock::acquire(&paths.lifecycle_lock)?;

        // Another client or an older build may have made the endpoint ready
        // while this process waited for the lifecycle lock. Try the protocol
        // again regardless of executable identity.
        match Self::connect_existing(&paths.socket, wake.clone()) {
            Ok(connection) => return Ok(connection),
            Err(handshake_error) => match connect_local(&paths.socket) {
                Ok(stream) => {
                    let _ = stream.shutdown(Shutdown::Both);
                    // Reachable but slow/incompatible is still live. Preserving
                    // its sessions is safer than converting an attach failure or
                    // rolling upgrade into a workspace-wide shutdown.
                    return Err(handshake_error).context(format!(
                        "background service is reachable but did not complete a compatible handshake; existing sessions were left running; workspace={}, socket={}, pid={}, identity={}, lifecycle_lock={}, log={}",
                        workspace.display(),
                        paths.socket.display(),
                        paths.pid.display(),
                        paths.identity.display(),
                        paths.lifecycle_lock.display(),
                        paths.log.display(),
                    ));
                }
                Err(error) if stale_socket_error(&error) => {}
                Err(error) => return Err(error.into()),
            },
        }

        if paths.socket.exists() || paths.identity.exists() || paths.pid.exists() {
            retire_stale_daemon(&paths)?;
        }

        if InstanceLease::is_held(&paths.owner_lock)? {
            bail!(
                "background owner lease is still held while its endpoint metadata is unavailable; refusing duplicate spawn; workspace={}, owner_lock={}, socket={}, pid={}, identity={}",
                workspace.display(),
                paths.owner_lock.display(),
                paths.socket.display(),
                paths.pid.display(),
                paths.identity.display(),
            );
        }

        spawn_daemon(workspace, scope, &paths)?;
        let deadline = Instant::now() + DAEMON_START_TIMEOUT;
        let mut last_attach_error = String::from("background socket has not accepted a connection");
        while Instant::now() < deadline {
            match Self::connect_existing(&paths.socket, wake.clone()) {
                Ok(connection) => return Ok(connection),
                Err(error) => {
                    last_attach_error = error.to_string();
                    thread::sleep(
                        CONNECT_DELAY.min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
            }
        }
        bail!(
            "background service did not become ready within {DAEMON_START_TIMEOUT:?}; workspace={}, socket={}, pid={}, identity={}, lifecycle_lock={}, log={}; last attach error: {last_attach_error}",
            workspace.display(),
            paths.socket.display(),
            paths.pid.display(),
            paths.identity.display(),
            paths.lifecycle_lock.display(),
            paths.log.display(),
        )
    }

    fn connect_existing(socket: &Path, wake: Option<Wake>) -> Result<AttachedDaemon> {
        let stream = connect_local(socket)?;
        let mut reader_stream = stream.try_clone()?;
        let first_line = Self::read_handshake_line(&mut reader_stream)?;
        let (first_envelope, daemon) = protocol::parse_first_frame(&first_line)
            .context("invalid background service handshake")?;
        if let Err(error) = daemon.ensure_compatible() {
            let _ = stream.shutdown(Shutdown::Both);
            return Err(error.context("incompatible background service"));
        }
        let mut reader = BufReader::new(reader_stream);
        let (tx, events) = mpsc::sync_channel(CLIENT_EVENT_QUEUE_CAPACITY);
        if first_envelope.kind != "heartbeat" {
            tx.send(first_envelope)
                .map_err(|_| anyhow!("background event channel closed"))?;
        }
        thread::spawn(move || {
            while let Ok(Some(frame)) = read_bounded_frame(&mut reader, MAX_BACKGROUND_FRAME_BYTES)
            {
                let envelope = match serde_json::from_slice::<HarnessEvent>(&frame) {
                    Ok(envelope) => envelope,
                    Err(_) => break,
                };
                if tx.send(envelope).is_err() {
                    break;
                }
                if let Some(wake) = &wake {
                    wake.notify();
                }
            }
            // Wake the owner so it observes the disconnect promptly.
            if let Some(wake) = &wake {
                wake.notify();
            }
        });
        Ok(AttachedDaemon { stream, events })
    }

    fn read_handshake_line(stream: &mut LocalStream) -> Result<Vec<u8>> {
        stream.set_nonblocking(true)?;
        let result = Self::read_handshake_line_nonblocking(stream);
        let restore = stream.set_nonblocking(false);
        if let Err(error) = restore {
            return Err(error.into());
        }
        result
    }

    fn read_handshake_line_nonblocking(stream: &mut LocalStream) -> Result<Vec<u8>> {
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        let mut line = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            match stream.read(&mut byte) {
                Ok(0) => bail!("background service closed during handshake"),
                Ok(_) if byte[0] == b'\n' => return Ok(line),
                Ok(_) => {
                    if line.len() >= MAX_BACKGROUND_FRAME_BYTES {
                        bail!("background service handshake exceeded frame limit");
                    }
                    line.push(byte[0]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        bail!("background service handshake timed out");
                    }
                    thread::sleep(Duration::from_millis(4));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub fn send(&mut self, command: &FrontendCommand) -> Result<()> {
        if matches!(command, FrontendCommand::Shutdown) {
            // A BackgroundConnection is only a frontend attachment to a shared
            // workspace service. Frontends may disconnect, but they must never
            // terminate the daemon and every other attached/detached session.
            // Daemon replacement uses the private lifecycle socket path below.
            bail!("frontend sessions cannot shut down the shared background service");
        }
        self.observe_command(command);
        let mut frame = serde_json::to_vec(command)?;
        frame.push(b'\n');
        match self.stream.write_all(&frame) {
            Ok(()) => Ok(()),
            Err(error) if reconnectable_socket_error(&error) => {
                self.reconnect_with_backoff()?;
                self.stream.write_all(&frame).map_err(Into::into)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn try_recv(&mut self) -> Option<HarnessEvent> {
        loop {
            match self.events.try_recv() {
                Ok(envelope) => {
                    self.last_daemon_activity = Instant::now();
                    if envelope.kind == "heartbeat" {
                        continue;
                    }
                    if let Some(expected_session) = self.resume_barrier.clone() {
                        if envelope.kind == "state" {
                            let matches_resume = envelope
                                .state
                                .as_ref()
                                .and_then(|state| state.current_session_id.as_deref())
                                == Some(expected_session.as_str());
                            if !matches_resume {
                                // A newly attached socket receives the provisional
                                // runtime snapshot before the daemon handles our
                                // LoadSession resume command. Never let that stale
                                // snapshot rewrite client affinity.
                                continue;
                            }
                            self.resume_barrier = None;
                        } else if envelope.kind == "error" {
                            // Surface routing/load failures instead of filtering
                            // every future frame behind a barrier that cannot clear.
                            self.resume_barrier = None;
                        }
                    }
                    self.observe_envelope(&envelope);
                    return Some(envelope);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if self.last_daemon_activity.elapsed() >= CLIENT_STALE_AFTER {
                        // A successful write is not proof that the daemon is
                        // alive: a wedged process can leave a connected socket
                        // with buffered input. Heartbeats make main-loop
                        // liveness explicit. Do not reset the activity clock on
                        // a failed reconnect: the retry backoff should govern the
                        // next attempt, not another full stale interval.
                        self.reconnect_with_backoff().ok()?;
                        continue;
                    }
                    return None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.reconnect_with_backoff().ok()?;
                    continue;
                }
            }
        }
    }

    fn observe_command(&mut self, command: &FrontendCommand) {
        match command {
            FrontendCommand::LoadSession { session_id } => {
                self.resume_session_id = Some(session_id.clone());
                self.resume_barrier = None;
            }
            FrontendCommand::NewSession => {
                self.resume_session_id = None;
                self.resume_barrier = None;
            }
            _ => {}
        }
    }

    fn observe_envelope(&mut self, envelope: &HarnessEvent) {
        if envelope.kind != "state" {
            return;
        }
        if let Some(state) = &envelope.state {
            self.resume_session_id = state.current_session_id.clone();
        }
    }

    /// Reconnects unless a recent attempt failed. Without the backoff a dead
    /// daemon turns every poll into a blocking spawn-and-retry cycle, which
    /// freezes the TUI render loop and pins a CPU in the Remote gateway.
    fn reconnect_with_backoff(&mut self) -> Result<()> {
        if let Some(at) = self.next_reconnect_at
            && Instant::now() < at
        {
            bail!("background service reconnect is backing off");
        }
        match self.reconnect() {
            Ok(()) => {
                self.reconnect_failures = 0;
                self.next_reconnect_at = None;
                Ok(())
            }
            Err(error) => {
                self.reconnect_failures = self.reconnect_failures.saturating_add(1);
                self.next_reconnect_at =
                    Some(Instant::now() + reconnect_backoff(self.reconnect_failures));
                Err(error)
            }
        }
    }

    fn reconnect(&mut self) -> Result<()> {
        // Cloning the local stream duplicates the same endpoint. Merely
        // dropping the writer does not wake the reader thread because its
        // duplicate descriptor remains open. Explicit shutdown tears down the
        // endpoint for every duplicate so the old client is actually detached.
        let _ = self.stream.shutdown(Shutdown::Both);
        let AttachedDaemon { mut stream, events } =
            Self::establish(&self.workspace, self.scope.as_deref(), self.wake.clone())?;
        if let Some(session_id) = self.resume_session_id.clone() {
            let mut frame = serde_json::to_vec(&FrontendCommand::LoadSession {
                session_id: session_id.clone(),
            })?;
            frame.push(b'\n');
            stream.write_all(&frame)?;
            stream.flush()?;
            self.resume_barrier = Some(session_id);
        } else {
            self.resume_barrier = None;
        }
        self.stream = stream;
        self.events = events;
        self.last_daemon_activity = Instant::now();
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

fn reconnect_backoff(failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(5);
    (RECONNECT_BACKOFF_BASE * 2u32.pow(exponent)).min(RECONNECT_BACKOFF_MAX)
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

pub fn run_daemon(workspace: PathBuf, scope: Option<String>) -> Result<()> {
    let workspace = workspace.canonicalize().unwrap_or(workspace);
    let paths = BackgroundPaths::new(&workspace, scope.as_deref())?;
    // Hold a lifetime lease in addition to the startup/recovery lock. Socket
    // path removal or concurrent direct daemon launches must never create two
    // backend owners for the same workspace.
    let _instance_lease = InstanceLease::acquire(&paths.owner_lock)?;
    let executable_identity = current_executable_identity()?;
    // The parent recovery path is responsible for retiring and unlinking a
    // stale endpoint before this child is spawned. Never steal a pathname from
    // a still-running daemon here: doing so leaves the old listener alive on an
    // unlinked Unix socket and splits clients across two background services.
    let listener = bind_local(&paths.socket)
        .with_context(|| format!("bind background socket {}", paths.socket.display()))?;
    let _cleanup = DaemonFilesCleanup::new(&paths);
    set_private_file(&paths.socket)?;
    listener.set_nonblocking(true)?;

    fs::write(&paths.pid, format!("{}\n", std::process::id()))?;
    set_private_file(&paths.pid)?;
    fs::write(&paths.identity, format!("{executable_identity}\n"))?;
    set_private_file(&paths.identity)?;
    let config = ConfigStore::default();
    config.ensure()?;
    let extensions = ExtensionHost::discover_and_start(&config.directory, &workspace);
    let wake = Wake::new();
    let (client_tx, client_rx) = mpsc::sync_channel::<ClientEvent>(CLIENT_EVENT_QUEUE_CAPACITY);

    let (runtime_start_tx, runtime_start_rx) = mpsc::channel::<RuntimeStartupEvent>();
    let (runtime_isolation_tx, runtime_isolation_rx) = mpsc::channel::<RuntimeIsolationEvent>();
    let mut clients: Vec<ClientConnection> = Vec::new();
    let mut runtimes: Vec<SessionRuntime> = Vec::new();
    let mut pending_isolations: Vec<PendingIsolation> = Vec::new();
    let mut session_catalog: Option<SessionCatalog> = None;
    let mut next_client_id = 1u64;
    // Every accepted socket first receives this heartbeat + handshake frame.
    let ready_frame = protocol::ready_frame(&DaemonHandshake::current())?;
    let mut next_runtime_id = 1u64;
    let mut idle_since: Option<Instant> = None;
    let mut last_heartbeat = Instant::now();

    let mut accept_startup_pending = false;
    let mut startup_waiters: Vec<(u64, LocalStream)> = Vec::new();

    loop {
        while let Ok(event) = runtime_start_rx.try_recv() {
            accept_startup_pending = false;
            match event.result {
                Ok(runtime) => {
                    runtimes.push(runtime);
                    let mut attached_any = attach_client_to_runtime(
                        &mut clients,
                        &mut runtimes,
                        &extensions,
                        &client_tx,
                        &wake,
                        event.client_id,
                        event.runtime_id,
                        event.stream,
                    );
                    for (client_id, stream) in startup_waiters.drain(..) {
                        attached_any |= attach_client_to_runtime(
                            &mut clients,
                            &mut runtimes,
                            &extensions,
                            &client_tx,
                            &wake,
                            client_id,
                            event.runtime_id,
                            stream,
                        );
                    }
                    if attached_any {
                        idle_since = None;
                    }
                }
                Err(error) => {
                    let message = error.to_string();
                    send_unattached_client_error(event.stream, anyhow!(message.clone()));
                    for (_, stream) in startup_waiters.drain(..) {
                        send_unattached_client_error(stream, anyhow!(message.clone()));
                    }
                }
            }
        }

        while let Ok(event) = runtime_isolation_rx.try_recv() {
            let Some(index) = pending_isolations
                .iter()
                .position(|pending| pending.runtime_id == event.runtime_id)
            else {
                if let Ok(runtime) = event.result {
                    retire_runtime_async(runtime);
                }
                continue;
            };
            let pending = pending_isolations.swap_remove(index);
            // A waiter that issued a newer LoadSession/NewSession (or left)
            // no longer wants this runtime; honouring it would undo that
            // newer choice when a slow startup finally completes.
            let waiters = pending
                .waiters
                .iter()
                .filter(|(client_id, route_seq)| {
                    clients
                        .iter()
                        .any(|client| client.id == *client_id && client.route_seq == *route_seq)
                })
                .map(|(client_id, _)| *client_id)
                .collect::<Vec<_>>();
            let mut runtime = match event.result {
                Ok(runtime) => runtime,
                Err(error) => {
                    let message = format!("{error:#}");
                    for client_id in waiters {
                        send_client_error_to(&mut clients, client_id, anyhow!(message.clone()));
                    }
                    continue;
                }
            };
            if waiters.is_empty() {
                retire_runtime_async(runtime);
                continue;
            }
            let mut target_runtime_id = event.runtime_id;
            if let Some(session_id) = pending.session_id {
                if let Some(live_runtime_id) = live_session_runtime_id(&runtimes, &session_id) {
                    // The session became live elsewhere while this runtime
                    // started. Never keep two runtimes on one session: their
                    // transcripts would diverge and overwrite each other.
                    retire_runtime_async(runtime);
                    target_runtime_id = live_runtime_id;
                } else if let Err(error) = runtime
                    .service
                    .send(FrontendCommand::LoadSession { session_id })
                {
                    let message = format!("{error:#}");
                    for client_id in waiters {
                        send_client_error_to(&mut clients, client_id, anyhow!(message.clone()));
                    }
                    retire_runtime_async(runtime);
                    continue;
                } else {
                    runtimes.push(runtime);
                }
            } else {
                runtimes.push(runtime);
            }
            for client_id in waiters {
                route_client_to_runtime(
                    &mut clients,
                    &mut runtimes,
                    &extensions,
                    client_id,
                    target_runtime_id,
                );
            }
            idle_since = None;
        }

        while let Ok(event) = client_rx.try_recv() {
            match event {
                ClientEvent::Command { client_id, command } => {
                    let Some(client_index) =
                        clients.iter().position(|client| client.id == client_id)
                    else {
                        continue;
                    };
                    let runtime_id = clients[client_index].runtime_id;
                    if std::env::var_os("YEET_BACKGROUND_TRACE").is_some() {
                        eprintln!(
                            "background client command: client={client_id} runtime={runtime_id}"
                        );
                    }

                    match command {
                        FrontendCommand::Shutdown => {
                            // Frontend lifetime is not daemon lifetime. Older
                            // clients may still emit Shutdown during a rolling
                            // upgrade; ignore it so one UI cannot terminate every
                            // session in the shared workspace service.
                            if std::env::var_os("YEET_BACKGROUND_TRACE").is_some() {
                                eprintln!(
                                    "background ignored frontend shutdown: client={client_id} runtime={runtime_id}"
                                );
                            }
                            continue;
                        }
                        FrontendCommand::LoadSession { session_id } => {
                            let route_seq = clients[client_index].next_route();
                            if let Some(target_runtime_id) =
                                live_session_runtime_id(&runtimes, &session_id)
                            {
                                route_client_to_runtime(
                                    &mut clients,
                                    &mut runtimes,
                                    &extensions,
                                    client_id,
                                    target_runtime_id,
                                );
                                continue;
                            }
                            if let Some(pending) = pending_isolations.iter_mut().find(|pending| {
                                pending.session_id.as_deref() == Some(session_id.as_str())
                            }) {
                                pending.waiters.push((client_id, route_seq));
                                continue;
                            }

                            if runtime_requires_isolation(&runtimes, &clients, runtime_id) {
                                if let Err(error) = start_isolated_runtime(
                                    &mut pending_isolations,
                                    &mut next_runtime_id,
                                    &runtime_isolation_tx,
                                    &workspace,
                                    scope.as_deref(),
                                    &wake,
                                    Some(session_id),
                                    (client_id, route_seq),
                                ) {
                                    send_client_error_to(&mut clients, client_id, error);
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
                        FrontendCommand::NewSession => {
                            let route_seq = clients[client_index].next_route();
                            if runtime_requires_isolation(&runtimes, &clients, runtime_id) {
                                if let Err(error) = start_isolated_runtime(
                                    &mut pending_isolations,
                                    &mut next_runtime_id,
                                    &runtime_isolation_tx,
                                    &workspace,
                                    scope.as_deref(),
                                    &wake,
                                    None,
                                    (client_id, route_seq),
                                ) {
                                    send_client_error_to(&mut clients, client_id, error);
                                }
                                continue;
                            }
                            if let Err(error) = dispatch_runtime_command(
                                &mut runtimes,
                                runtime_id,
                                FrontendCommand::NewSession,
                            ) {
                                broadcast_runtime_error(&mut clients, runtime_id, error);
                            }
                        }
                        FrontendCommand::ExtensionCommand { command, args } => {
                            match extensions.invoke_command(&command, &args) {
                                Ok(true) => {}
                                Ok(false) => broadcast_runtime_error(
                                    &mut clients,
                                    runtime_id,
                                    anyhow!("unknown extension command: /{command}"),
                                ),
                                Err(error) => {
                                    broadcast_runtime_error(&mut clients, runtime_id, error)
                                }
                            }
                        }
                        FrontendCommand::Interrupt => {
                            match dispatch_runtime_command(
                                &mut runtimes,
                                runtime_id,
                                FrontendCommand::Interrupt,
                            ) {
                                Ok(()) => {
                                    if let Some(runtime) =
                                        runtimes.iter_mut().find(|runtime| runtime.id == runtime_id)
                                    {
                                        runtime.interrupt_requested_at =
                                            runtime.service.is_streaming().then(Instant::now);
                                    }
                                }
                                Err(error) => {
                                    broadcast_runtime_error(&mut clients, runtime_id, error);
                                }
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
                    if std::env::var_os("YEET_BACKGROUND_TRACE").is_some() {
                        eprintln!("background client disconnected: client={client_id}");
                    }
                    clients.retain(|client| client.id != client_id);
                }
            }
        }

        while let Some(request) = extensions.try_recv_request() {
            let runtime_id = if let Some(runtime_id) =
                clients.last().map(|client| client.runtime_id)
            {
                runtime_id
            } else if let Some(runtime) = runtimes
                .iter()
                .rev()
                .find(|runtime| !runtime.service.is_closed())
            {
                runtime.id
            } else {
                let runtime_id = allocate_runtime_id(&mut next_runtime_id);
                match spawn_runtime(&workspace, runtime_id, scope.as_deref(), wake.clone()) {
                    Ok(runtime) => runtimes.push(runtime),
                    Err(error) => {
                        eprintln!("yeet: could not start runtime for extension request: {error:#}");
                        continue;
                    }
                }
                runtime_id
            };
            if let Some(runtime) = runtimes.iter_mut().find(|runtime| runtime.id == runtime_id) {
                runtime.idle_since = None;
            }
            let (extension_id, command) = match request {
                ExtensionRequest::Submit { extension_id, text } => (
                    extension_id,
                    FrontendCommand::Submit {
                        text,
                        images: Vec::new(),
                        attachment_ids: Vec::new(),
                    },
                ),
                ExtensionRequest::SelectModel {
                    extension_id,
                    model,
                } => (extension_id, FrontendCommand::SelectModel { model }),
                ExtensionRequest::SelectReasoning {
                    extension_id,
                    level,
                } => (extension_id, FrontendCommand::SelectReasoning { level }),
                ExtensionRequest::Interrupt { extension_id } => {
                    (extension_id, FrontendCommand::Interrupt)
                }
                ExtensionRequest::NewSession { extension_id } => {
                    (extension_id, FrontendCommand::NewSession)
                }
                ExtensionRequest::RequestModels { extension_id } => {
                    (extension_id, FrontendCommand::RequestModels)
                }
            };
            if let Err(error) = dispatch_runtime_command(&mut runtimes, runtime_id, command) {
                eprintln!("yeet: extension {extension_id} request failed: {error}");
                broadcast_runtime_error(&mut clients, runtime_id, error);
            }
        }

        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // Accepted Unix-domain sockets can share listener flags on some
                    // platforms. Client readers are blocking, while a best-effort
                    // write timeout prevents an unresponsive local client from
                    // stalling the daemon. Darwin rejects SO_SNDTIMEO on AF_UNIX
                    // with EINVAL, so treat that specific unsupported option as
                    // non-fatal rather than killing the whole background service.
                    if stream.set_nonblocking(false).is_err() {
                        let _ = stream.shutdown(Shutdown::Both);
                        continue;
                    }
                    if let Err(error) = stream.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))
                        && error.kind() != std::io::ErrorKind::InvalidInput
                    {
                        let _ = stream.shutdown(Shutdown::Both);
                        continue;
                    }
                    // Transport readiness must not depend on provider/runtime startup.
                    // A cold runtime can legitimately spend seconds loading the session
                    // catalog or starting the provider bridge; acknowledge the accepted
                    // socket immediately so clients do not misclassify that work as a
                    // dead background daemon.
                    if send_frame(&mut stream, ready_frame.clone()).is_err() {
                        let _ = stream.shutdown(Shutdown::Both);
                        continue;
                    }
                    let client_id = next_client_id;
                    next_client_id = next_client_id.wrapping_add(1).max(1);

                    if let Some(runtime_id) = reusable_runtime_id(&runtimes, &clients) {
                        if attach_client_to_runtime(
                            &mut clients,
                            &mut runtimes,
                            &extensions,
                            &client_tx,
                            &wake,
                            client_id,
                            runtime_id,
                            stream,
                        ) {
                            idle_since = None;
                        }
                        continue;
                    }

                    if accept_startup_pending {
                        if startup_waiters.len() >= MAX_PENDING_STARTUP_CLIENTS {
                            send_unattached_client_error(
                                stream,
                                anyhow!(
                                    "background runtime startup already has {MAX_PENDING_STARTUP_CLIENTS} waiting clients"
                                ),
                            );
                        } else {
                            startup_waiters.push((client_id, stream));
                        }
                        continue;
                    }

                    let runtime_id = allocate_runtime_id(&mut next_runtime_id);
                    let startup_tx = runtime_start_tx.clone();
                    let startup_workspace = workspace.clone();
                    let startup_scope = scope.clone();
                    let startup_wake = wake.clone();
                    accept_startup_pending = true;
                    thread::spawn(move || {
                        let result = spawn_runtime(
                            &startup_workspace,
                            runtime_id,
                            startup_scope.as_deref(),
                            startup_wake.clone(),
                        );
                        let _ = startup_tx.send(RuntimeStartupEvent {
                            client_id,
                            runtime_id,
                            stream,
                            result,
                        });
                        startup_wake.notify();
                    });
                    // Only one runtime startup runs at a time. Further accepted
                    // clients are acknowledged immediately and queued above to share it.
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    // Transient accept failures (EMFILE, ECONNABORTED, ...) must
                    // not take down every running session in the workspace.
                    eprintln!("yeet: background accept failed: {error}");
                    break;
                }
            }
        }

        let mut catalog_changed = false;
        for runtime in &mut runtimes {
            let mut pending_state: Option<HarnessEvent> = None;
            for _ in 0..MAX_RUNTIME_EVENTS_PER_TICK {
                let Some(mut envelope) = runtime.service.try_recv() else {
                    break;
                };
                if envelope.kind != "state" || envelope.state.is_none() {
                    if let Some(state) = pending_state.take() {
                        publish_runtime_state(
                            &mut clients,
                            &extensions,
                            runtime,
                            state,
                            &mut session_catalog,
                            &mut catalog_changed,
                        );
                    }
                    broadcast_runtime_envelope(&mut clients, runtime.id, &envelope);
                    continue;
                }
                // Streaming produces one full state per token. Only the newest
                // is worth sending, but a compact (conversation-less) state
                // must inherit any transcript an earlier state in this batch
                // carried, or clients would miss committed entries.
                if let Some(previous) = pending_state.take()
                    && let (Some(next), Some(previous)) = (envelope.state.as_mut(), previous.state)
                    && next.conversation.is_none()
                {
                    next.conversation = previous.conversation;
                }
                pending_state = Some(envelope);
            }
            if let Some(state) = pending_state {
                publish_runtime_state(
                    &mut clients,
                    &extensions,
                    runtime,
                    state,
                    &mut session_catalog,
                    &mut catalog_changed,
                );
            }
            if !runtime.service.is_streaming() {
                runtime.interrupt_requested_at = None;
            }
        }
        if let Some(catalog) = session_catalog.as_ref()
            && (catalog_changed
                || runtimes
                    .iter()
                    .any(|runtime| runtime.catalog.as_ref() != Some(catalog)))
        {
            for runtime in &runtimes {
                if runtime.catalog.as_ref() != Some(catalog) {
                    runtime.service.adopt_session_catalog(catalog);
                }
            }
        }

        let stuck_runtime_ids = runtimes
            .iter()
            .filter(|runtime| {
                runtime.service.is_streaming()
                    && runtime
                        .interrupt_requested_at
                        .is_some_and(|requested| requested.elapsed() >= INTERRUPT_RECOVERY_AFTER)
            })
            .map(|runtime| runtime.id)
            .collect::<Vec<_>>();
        for runtime_id in stuck_runtime_ids {
            if let Some(runtime) = take_runtime(&mut runtimes, runtime_id) {
                let _ = runtime.service.abandon_stuck_run(
                    "Run did not stop after interrupt; Yeet recycled the stuck background runtime.",
                );
                disconnect_runtime_clients(&mut clients, runtime_id);
                retire_runtime_async(runtime);
            }
        }

        let closed_runtime_ids = runtimes
            .iter()
            .filter(|runtime| runtime.service.is_closed())
            .map(|runtime| runtime.id)
            .collect::<Vec<_>>();
        if !closed_runtime_ids.is_empty() {
            for runtime_id in closed_runtime_ids {
                disconnect_runtime_clients(&mut clients, runtime_id);
                if let Some(runtime) = take_runtime(&mut runtimes, runtime_id) {
                    retire_runtime_async(runtime);
                }
            }
        }

        let now = Instant::now();
        for runtime in &mut runtimes {
            if runtime_client_count(&clients, runtime.id) > 0 || runtime.service.is_streaming() {
                // A streaming runtime deliberately survives with zero clients:
                // transport loss must not become semantic cancellation.
                runtime.idle_since = None;
            } else {
                runtime.idle_since.get_or_insert(now);
            }
        }
        let expired_runtime_ids = runtimes
            .iter()
            .filter(|runtime| {
                runtime
                    .idle_since
                    .is_some_and(|since| since.elapsed() >= RUNTIME_IDLE_RETIRE_AFTER)
            })
            .map(|runtime| runtime.id)
            .collect::<Vec<_>>();
        for runtime_id in expired_runtime_ids {
            if let Some(runtime) = take_runtime(&mut runtimes, runtime_id) {
                retire_runtime_async(runtime);
            }
        }

        // Activity belongs to the daemon, not the persisted session catalog or
        // the currently attached frontend. Include detached runtimes and remove
        // retired/failed runs immediately.
        let activity = runtimes
            .iter()
            .filter_map(|runtime| runtime.service.activity())
            .collect::<std::collections::BTreeMap<_, _>>();
        for runtime in &mut runtimes {
            if runtime.service.set_session_activity(&activity) {
                let state =
                    state_with_extension_commands(runtime.service.activity_snapshot(), &extensions);
                broadcast_runtime_envelope(
                    &mut clients,
                    runtime.id,
                    &HarnessEvent {
                        kind: "state".into(),
                        state: Some(state),
                        message: None,
                    },
                );
            }
        }
        if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
            let heartbeat = HarnessEvent {
                kind: "heartbeat".into(),
                state: None,
                message: None,
            };
            clients.retain_mut(|client| match client.writer.send(&heartbeat) {
                Ok(()) => true,
                Err(error) => {
                    if std::env::var_os("YEET_BACKGROUND_TRACE").is_some() {
                        eprintln!(
                            "background client send failed: client={} runtime={} envelope=heartbeat error={error:#}",
                            client.id, client.runtime_id
                        );
                    }
                    false
                }
            });
            last_heartbeat = Instant::now();
        }

        let any_streaming = runtimes
            .iter()
            .any(|runtime| runtime.service.is_streaming());
        if clients.is_empty() && !any_streaming && pending_isolations.is_empty() {
            let since = idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= DAEMON_IDLE_EXIT_AFTER {
                break;
            }
        } else {
            idle_since = None;
        }
        wake.wait_timeout(DAEMON_TICK);
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

fn allocate_runtime_id(next_runtime_id: &mut u64) -> u64 {
    let runtime_id = *next_runtime_id;
    *next_runtime_id = next_runtime_id.wrapping_add(1).max(1);
    runtime_id
}

fn spawn_runtime(
    workspace: &Path,
    id: u64,
    scope: Option<&str>,
    wake: Wake,
) -> Result<SessionRuntime> {
    Ok(SessionRuntime {
        id,
        service: RuntimeProcess::spawn(workspace, scope, wake)?,
        idle_since: None,
        interrupt_requested_at: None,
        catalog: None,
    })
}

fn send_client_error(
    clients: &mut Vec<ClientConnection>,
    client_index: usize,
    error: anyhow::Error,
) {
    let envelope = HarnessEvent {
        kind: "error".into(),
        state: None,
        message: Some(error.to_string()),
    };
    if clients
        .get_mut(client_index)
        .is_some_and(|client| client.writer.send(&envelope).is_err())
    {
        clients.remove(client_index);
    }
}

fn disconnect_runtime_clients(clients: &mut Vec<ClientConnection>, runtime_id: u64) {
    clients.retain_mut(|client| {
        if client.runtime_id != runtime_id {
            return true;
        }
        client.writer.disconnect();
        false
    });
}

fn take_runtime(runtimes: &mut Vec<SessionRuntime>, runtime_id: u64) -> Option<SessionRuntime> {
    let index = runtimes
        .iter()
        .position(|runtime| runtime.id == runtime_id)?;
    Some(runtimes.swap_remove(index))
}

fn retire_runtime_async(runtime: SessionRuntime) {
    thread::spawn(move || drop(runtime));
}

fn runtime_client_count(clients: &[ClientConnection], runtime_id: u64) -> usize {
    clients
        .iter()
        .filter(|client| client.runtime_id == runtime_id)
        .count()
}

fn runtime_requires_isolation(
    runtimes: &[SessionRuntime],
    clients: &[ClientConnection],
    runtime_id: u64,
) -> bool {
    let client_count = runtime_client_count(clients, runtime_id);
    let is_streaming = runtimes
        .iter()
        .find(|runtime| runtime.id == runtime_id)
        .is_some_and(|runtime| runtime.service.is_streaming());
    should_isolate_runtime(client_count, is_streaming)
}

fn should_isolate_runtime(client_count: usize, is_streaming: bool) -> bool {
    client_count > 1 || is_streaming
}

#[cfg(test)]
mod runtime_lifecycle_tests {
    use super::*;

    #[test]
    fn lifecycle_policy_is_consistent() {
        assert!(DAEMON_IDLE_EXIT_AFTER > RUNTIME_IDLE_RETIRE_AFTER);

        for (attempt, expected) in [
            (1, Duration::from_millis(250)),
            (2, Duration::from_millis(500)),
            (3, Duration::from_secs(1)),
            (30, RECONNECT_BACKOFF_MAX),
        ] {
            assert_eq!(reconnect_backoff(attempt), expected);
        }

        for (clients, streaming, isolate) in [
            (0, false, false),
            (1, false, false),
            (2, false, true),
            (1, true, true),
        ] {
            assert_eq!(should_isolate_runtime(clients, streaming), isolate);
        }
    }

    #[test]
    fn daemon_file_cleanup_only_removes_files_owned_by_that_daemon() {
        let directory = tempfile::tempdir().unwrap();
        let make_files = |pid_value: &str| {
            let socket = directory.path().join("daemon.sock");
            let identity = directory.path().join("daemon.identity");
            let pid = directory.path().join("daemon.pid");
            fs::write(&socket, b"socket").unwrap();
            fs::write(&identity, b"identity").unwrap();
            fs::write(&pid, pid_value).unwrap();
            (socket, identity, pid)
        };

        let (socket, identity, pid) = make_files(
            "222
",
        );
        drop(DaemonFilesCleanup {
            socket: socket.clone(),
            identity: identity.clone(),
            pid: pid.clone(),
            owner_pid: 111,
        });
        assert!(socket.exists() && identity.exists() && pid.exists());

        let (socket, identity, pid) = make_files(
            "111
",
        );
        drop(DaemonFilesCleanup {
            socket: socket.clone(),
            identity: identity.clone(),
            pid: pid.clone(),
            owner_pid: 111,
        });
        assert!(!socket.exists() && !identity.exists() && !pid.exists());
    }

    #[test]
    fn owner_lease_is_exclusive_and_observable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owner.lock");

        let owner = InstanceLease::acquire(&path).unwrap();
        assert!(InstanceLease::is_held(&path).unwrap());
        assert!(InstanceLease::acquire(&path).is_err());

        drop(owner);
        assert!(!InstanceLease::is_held(&path).unwrap());
        assert!(InstanceLease::acquire(&path).is_ok());
    }

    #[test]
    fn stale_cleanup_preserves_live_owner_and_removes_dead_metadata() {
        let directory = tempfile::tempdir().unwrap();

        let live = BackgroundPaths {
            socket: directory.path().join("live.sock"),
            log: directory.path().join("live.log"),
            identity: directory.path().join("live.identity"),
            pid: directory.path().join("live.pid"),
            lifecycle_lock: directory.path().join("live.lock"),
            owner_lock: directory.path().join("live.owner.lock"),
        };
        fs::write(&live.socket, b"socket").unwrap();
        fs::write(&live.identity, b"identity").unwrap();
        fs::write(
            &live.pid, b"123
",
        )
        .unwrap();
        let live_owner = InstanceLease::acquire(&live.owner_lock).unwrap();

        assert!(cleanup_stale_daemon_files(&live).is_err());
        assert!(live.socket.exists() && live.identity.exists() && live.pid.exists());

        let dead_socket = directory.path().join("dead.sock");
        let dead_owner = directory.path().join("dead.owner.lock");
        let dead_identity = directory.path().join("dead.identity");
        let dead_pid = directory.path().join("dead.pid");
        fs::write(&dead_owner, b"").unwrap();
        let listener = bind_local(&dead_socket).unwrap();
        drop(listener);
        fs::write(&dead_identity, b"stale").unwrap();
        fs::write(
            &dead_pid, b"456
",
        )
        .unwrap();

        assert_eq!(cleanup_stale_artifacts_in(directory.path()).unwrap(), 1);
        assert!(!dead_socket.exists() && !dead_identity.exists() && !dead_pid.exists());
        assert!(live.socket.exists() && live.identity.exists() && live.pid.exists());

        drop(live_owner);
        cleanup_stale_daemon_files(&live).unwrap();
        assert!(!live.socket.exists() && !live.identity.exists() && !live.pid.exists());
    }
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
    choose_reusable_runtime_id(&states).or_else(|| {
        // A reconnect can arrive before the daemon reader observes the old
        // socket closing. In that window there is no detached runtime, and the
        // old implementation eagerly booted a second BackendService/provider
        // bridge just to discard it after LoadSession. Prefer the most recently
        // attached live runtime; session routing will isolate later only when a
        // client actually selects a different session.
        clients
            .iter()
            .rev()
            .map(|client| client.runtime_id)
            .find(|runtime_id| states.iter().any(|(id, _, _)| id == runtime_id))
    })
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

#[allow(clippy::too_many_arguments)]
fn attach_client_to_runtime(
    clients: &mut Vec<ClientConnection>,
    runtimes: &mut [SessionRuntime],
    extensions: &ExtensionHost,
    client_tx: &mpsc::SyncSender<ClientEvent>,
    wake: &Wake,
    client_id: u64,
    runtime_id: u64,
    stream: LocalStream,
) -> bool {
    let Some(runtime) = runtimes.iter_mut().find(|runtime| runtime.id == runtime_id) else {
        send_unattached_client_error(
            stream,
            anyhow!("background session runtime is no longer available"),
        );
        return false;
    };
    let reader = match stream.try_clone() {
        Ok(reader) => reader,
        Err(error) => {
            send_unattached_client_error(
                stream,
                anyhow!("clone background client stream: {error}"),
            );
            return false;
        }
    };
    runtime.idle_since = None;
    let writer = match ClientWriter::new(stream) {
        Ok(writer) => writer,
        Err(_) => return false,
    };
    let initial = HarnessEvent {
        kind: "state".into(),
        state: Some(state_with_extension_commands(
            runtime.service.state_snapshot(),
            extensions,
        )),
        message: None,
    };
    if let Some(state) = initial.state.as_ref() {
        extensions.publish_state(state);
    }
    if writer.send(&initial).is_err() {
        return false;
    }
    let tx = client_tx.clone();
    let wake = wake.clone();
    thread::spawn(move || client_reader(client_id, reader, tx, wake));
    clients.push(ClientConnection {
        id: client_id,
        runtime_id,
        route_seq: 0,
        writer,
    });
    if std::env::var_os("YEET_BACKGROUND_TRACE").is_some() {
        eprintln!("background client attached: client={client_id} runtime={runtime_id}");
    }
    true
}

fn send_unattached_client_error(mut stream: LocalStream, error: anyhow::Error) {
    let envelope = HarnessEvent {
        kind: "error".into(),
        state: None,
        message: Some(error.to_string()),
    };
    let _ = send_envelope(&mut stream, &envelope);
    let _ = stream.shutdown(Shutdown::Both);
}

fn state_with_extension_commands(
    mut state: HarnessState,
    extensions: &ExtensionHost,
) -> HarnessState {
    state.extension_commands = extensions.command_items();
    state
}

fn broadcast_runtime_error(
    clients: &mut Vec<ClientConnection>,
    runtime_id: u64,
    error: anyhow::Error,
) {
    let envelope = HarnessEvent {
        kind: "error".into(),
        state: None,
        message: Some(error.to_string()),
    };
    broadcast_runtime_envelope(clients, runtime_id, &envelope);
}

fn broadcast_runtime_envelope(
    clients: &mut Vec<ClientConnection>,
    runtime_id: u64,
    envelope: &HarnessEvent,
) {
    let frame = match ClientWriter::encode(envelope) {
        Ok(frame) => frame,
        Err(error) => {
            eprintln!(
                "yeet: could not encode background envelope {} for runtime {runtime_id}: {error:#}",
                envelope.kind
            );
            return;
        }
    };
    clients.retain_mut(|client| {
        if client.runtime_id != runtime_id {
            return true;
        }
        match client.writer.send_frame(frame.clone()) {
            Ok(()) => true,
            Err(error) => {
                if std::env::var_os("YEET_BACKGROUND_TRACE").is_some() {
                    eprintln!(
                        "background client send failed: client={} runtime={} envelope={} error={error:#}",
                        client.id, client.runtime_id, envelope.kind
                    );
                }
                false
            }
        }
    });
}

fn client_reader(
    client_id: u64,
    stream: LocalStream,
    tx: mpsc::SyncSender<ClientEvent>,
    wake: Wake,
) {
    let mut reader = BufReader::new(stream);
    while let Ok(Some(frame)) = read_bounded_frame(&mut reader, MAX_CLIENT_COMMAND_BYTES) {
        let Ok(command) = serde_json::from_slice::<FrontendCommand>(&frame) else {
            continue;
        };
        if tx
            .send(ClientEvent::Command { client_id, command })
            .is_err()
        {
            return;
        }
        wake.notify();
    }
    let _ = tx.send(ClientEvent::Disconnected(client_id));
    wake.notify();
}

/// Reads one newline-delimited protocol frame without allowing a partial
/// frame to grow memory indefinitely. The returned bytes exclude the newline.
fn read_bounded_frame(
    reader: &mut impl BufRead,
    max_bytes: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    loop {
        let buffered = reader.fill_buf()?;
        if buffered.is_empty() {
            if frame.is_empty() {
                return Ok(None);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "background protocol frame ended before newline",
            ));
        }

        if let Some(newline) = buffered.iter().position(|byte| *byte == b'\n') {
            if frame.len().saturating_add(newline) > max_bytes {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "background protocol frame exceeded its limit",
                ));
            }
            frame.extend_from_slice(&buffered[..newline]);
            reader.consume(newline + 1);
            return Ok(Some(frame));
        }

        if frame.len().saturating_add(buffered.len()) > max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "background protocol frame exceeded its limit",
            ));
        }
        frame.extend_from_slice(buffered);
        let consumed = buffered.len();
        reader.consume(consumed);
    }
}

fn send_envelope(stream: &mut LocalStream, envelope: &HarnessEvent) -> Result<()> {
    send_frame(stream, serde_json::to_vec(envelope)?)
}

fn send_frame(stream: &mut LocalStream, mut frame: Vec<u8>) -> Result<()> {
    if frame.len() > MAX_BACKGROUND_FRAME_BYTES {
        bail!("background protocol frame exceeded its limit");
    }
    frame.push(b'\n');
    let result = write_client_frame(stream, &frame, CLIENT_WRITE_TIMEOUT);
    if result.is_err() {
        // A partially written JSON frame cannot be resumed on another connection.
        let _ = stream.shutdown(Shutdown::Both);
    }
    result.map_err(Into::into)
}

fn write_client_frame(
    writer: &mut impl Write,
    frame: &[u8],
    timeout: Duration,
) -> std::io::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut offset = 0usize;

    while offset < frame.len() {
        // Bound the whole frame, not just WouldBlock retries. A slow client
        // making partial progress (or repeated EINTR) must not monopolize the
        // daemon loop and delay heartbeats for every other session.
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "background client frame write exceeded its deadline",
            ));
        }
        match writer.write(&frame[offset..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "background client socket accepted zero-byte write",
                ));
            }
            Ok(written) => offset += written,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(
                    Duration::from_millis(4)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod client_frame_tests {
    use super::*;
    use std::io::Cursor;

    struct SlowWriter {
        error: Option<std::io::ErrorKind>,
        calls: usize,
    }

    impl Write for SlowWriter {
        fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
            self.calls += 1;
            thread::sleep(Duration::from_millis(10));
            match self.error {
                Some(kind) => Err(std::io::Error::from(kind)),
                None => Ok(1),
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn outbound_frame_contract() {
        let payload = b"{\"kind\":\"heartbeat\"}\n";
        let mut output = Vec::new();
        write_client_frame(&mut output, payload, CLIENT_WRITE_TIMEOUT).unwrap();
        assert_eq!(output, payload);

        let mut empty = &mut [][..];
        assert_eq!(
            write_client_frame(&mut empty, b"frame\n", CLIENT_WRITE_TIMEOUT)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::WriteZero
        );

        for error in [
            None,
            Some(std::io::ErrorKind::Interrupted),
            Some(std::io::ErrorKind::WouldBlock),
        ] {
            let mut writer = SlowWriter { error, calls: 0 };
            assert_eq!(
                write_client_frame(&mut writer, b"frame\n", Duration::from_millis(5))
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::TimedOut
            );
            assert!(writer.calls <= 1);
        }
    }

    #[test]
    fn inbound_frame_contract() {
        let mut oversized = BufReader::new(Cursor::new(b"12345".to_vec()));
        assert_eq!(
            read_bounded_frame(&mut oversized, 4).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );

        let mut reader = BufReader::new(Cursor::new(b"first\nsecond\n".to_vec()));
        assert_eq!(
            read_bounded_frame(&mut reader, 16).unwrap(),
            Some(b"first".to_vec())
        );
        assert_eq!(
            read_bounded_frame(&mut reader, 16).unwrap(),
            Some(b"second".to_vec())
        );
        assert_eq!(read_bounded_frame(&mut reader, 16).unwrap(), None);
    }
}
