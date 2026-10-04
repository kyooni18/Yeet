//! Frontend attachment lifecycle for a shared workspace daemon.
//!
//! This owns compatible attach, serialized recovery, session-affinity resume,
//! heartbeat liveness and retry backoff. Dropping an attachment disconnects its
//! socket; it never shuts down the shared session runtime.

use super::daemon_lifecycle::{
    BackgroundPaths, InstanceLease, LifecycleLock, retire_stale_daemon, spawn_daemon,
    stale_socket_error,
};
use super::{
    CLIENT_EVENT_QUEUE_CAPACITY, MAX_BACKGROUND_FRAME_BYTES, Wake, protocol, read_bounded_frame,
};
use crate::{
    model::{FrontendCommand, HarnessEvent},
    platform::{LocalStream, connect_local},
};
use anyhow::{Context, Result, anyhow, bail};
use std::{
    io::{BufReader, Read, Write},
    net::Shutdown,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const CONNECT_DELAY: Duration = Duration::from_millis(50);
const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
// A dead socket is detected immediately by the reader thread. This timeout is
// only a wedged-daemon fallback, so keep enough headroom for cold starts and
// large-state serialization instead of reconnecting healthy sessions.
const CLIENT_STALE_AFTER: Duration = Duration::from_secs(30);
const RECONNECT_BACKOFF_BASE: Duration = Duration::from_millis(250);
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(5);

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

#[cfg(test)]
mod tests {
    use super::protocol::DaemonHandshake;
    use super::*;
    #[cfg(unix)]
    use crate::platform::bind_local;

    #[test]
    fn reconnect_backoff_is_bounded() {
        for (attempt, expected) in [
            (1, Duration::from_millis(250)),
            (2, Duration::from_millis(500)),
            (3, Duration::from_secs(1)),
            (30, RECONNECT_BACKOFF_MAX),
        ] {
            assert_eq!(reconnect_backoff(attempt), expected);
        }
    }

    #[cfg(unix)]
    #[test]
    fn resumed_session_filters_provisional_state_and_surfaces_routing_errors() {
        let directory = tempfile::Builder::new()
            .prefix("yc")
            .tempdir_in("/tmp")
            .unwrap();
        let endpoint = directory.path().join("resume.sock");
        let listener = bind_local(&endpoint).unwrap();
        let stream = connect_local(&endpoint).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        let (tx, events) = mpsc::sync_channel(8);
        let mut connection = BackgroundConnection {
            workspace: directory.path().to_path_buf(),
            scope: None,
            resume_session_id: Some("expected".into()),
            resume_barrier: Some("expected".into()),
            stream,
            events,
            last_daemon_activity: Instant::now(),
            wake: None,
            reconnect_failures: 0,
            next_reconnect_at: None,
        };
        let state = |session: &str| HarnessEvent {
            kind: "state".into(),
            state: Some(crate::model::HarnessState {
                current_session_id: Some(session.into()),
                ..Default::default()
            }),
            message: None,
        };
        tx.send(state("provisional")).unwrap();
        tx.send(state("expected")).unwrap();
        let resumed = connection.try_recv().unwrap();
        assert_eq!(
            resumed.state.unwrap().current_session_id.as_deref(),
            Some("expected")
        );
        assert!(connection.resume_barrier.is_none());
        assert_eq!(connection.resume_session_id.as_deref(), Some("expected"));

        connection.resume_barrier = Some("missing".into());
        tx.send(HarnessEvent {
            kind: "error".into(),
            state: None,
            message: Some("load failed".into()),
        })
        .unwrap();
        assert_eq!(
            connection.try_recv().unwrap().message.as_deref(),
            Some("load failed")
        );
        assert!(connection.resume_barrier.is_none());
        tx.send(state("available")).unwrap();
        assert_eq!(
            connection
                .try_recv()
                .unwrap()
                .state
                .unwrap()
                .current_session_id
                .as_deref(),
            Some("available")
        );
    }

    #[cfg(unix)]
    /// Serves `frame` as the first line of one accepted connection.
    fn serve_first_frame(endpoint: &Path, frame: Vec<u8>) -> thread::JoinHandle<()> {
        let listener = bind_local(endpoint).unwrap();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.write_all(&frame);
                let _ = stream.flush();
                thread::sleep(Duration::from_millis(300));
            }
        })
    }

    #[cfg(unix)]
    #[test]
    fn client_attaches_only_to_compatible_daemons() {
        // Unix socket paths are short; keep the endpoint directly in a
        // short-named temp directory.
        let dir = tempfile::Builder::new()
            .prefix("yh")
            .tempdir_in("/tmp")
            .unwrap();
        let cases: [(&str, Vec<u8>, bool); 3] = [
            (
                "current",
                protocol::ready_frame(&DaemonHandshake::current()).unwrap(),
                true,
            ),
            (
                "legacy",
                br#"{"type":"heartbeat","state":null,"message":null}"#.to_vec(),
                true,
            ),
            (
                "future",
                serde_json::to_vec(&serde_json::json!({
                    "type": "heartbeat", "state": null, "message": null,
                    "handshake": {"protocolVersion": 2, "stateSchemaVersion": 1}
                }))
                .unwrap(),
                false,
            ),
        ];
        for (name, mut frame, compatible) in cases {
            frame.push(b'\n');
            let endpoint = dir.path().join(format!("{name}.sock"));
            let server = serve_first_frame(&endpoint, frame);
            let attached = BackgroundConnection::connect_existing(&endpoint, None);
            assert_eq!(attached.is_ok(), compatible, "{name}: {:?}", attached.err());
            server.join().unwrap();
        }
    }
}
