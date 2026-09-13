use std::{
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::{
    backend::{BackendEvent, BackendService},
    model::{BridgeEnvelope, BridgeState, FrontendCommand},
    platform::force_terminate_process_tree,
};

const RUNTIME_START_TIMEOUT: Duration = Duration::from_secs(10);
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
const ABANDON_SETTLE_TIMEOUT: Duration = Duration::from_millis(750);
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(8);

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RuntimeCommand {
    Frontend { command: FrontendCommand },
    AbandonStuckRun { reason: String },
}

enum WorkerEvent {
    Envelope(BridgeEnvelope),
    ProtocolError(String),
    Closed,
}

enum WorkerInput {
    Command(RuntimeCommand),
    ProtocolError(String),
    Closed,
}

pub(super) struct RuntimeProcess {
    child: Child,
    command: Option<BufWriter<ChildStdin>>,
    events: Receiver<WorkerEvent>,
    state: BridgeState,
    closed: bool,
}

impl RuntimeProcess {
    pub(super) fn spawn(workspace: &Path) -> Result<Self> {
        let executable =
            std::env::current_exe().context("locate yeet executable for background runtime")?;
        let mut child = Command::new(executable)
            .arg("__background-runtime")
            .arg(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("start Yeet background runtime process")?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("background runtime stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("background runtime stdout was not piped"))?;
        let (tx, events) = mpsc::channel();
        thread::spawn(move || runtime_reader(stdout, tx));

        let state = match events.recv_timeout(RUNTIME_START_TIMEOUT) {
            Ok(WorkerEvent::Envelope(envelope)) => envelope
                .state
                .ok_or_else(|| anyhow!("background runtime handshake did not contain state"))?,
            Ok(WorkerEvent::ProtocolError(error)) => {
                let _ = terminate_child(&mut child);
                bail!("background runtime protocol failed during startup: {error}");
            }
            Ok(WorkerEvent::Closed) => {
                let status = child.try_wait().ok().flatten();
                bail!("background runtime exited during startup{status:?}");
            }
            Err(error) => {
                let _ = terminate_child(&mut child);
                return Err(anyhow!("background runtime did not become ready: {error}"));
            }
        };

        Ok(Self {
            child,
            command: Some(BufWriter::new(stdin)),
            events,
            state,
            closed: false,
        })
    }

    pub(super) fn send(&mut self, command: FrontendCommand) -> Result<()> {
        self.send_runtime_command(&RuntimeCommand::Frontend { command })
    }

    pub(super) fn try_recv(&mut self) -> Option<BackendEvent> {
        match self.events.try_recv() {
            Ok(WorkerEvent::Envelope(envelope)) => {
                self.observe_envelope(&envelope);
                Some(BackendEvent::Envelope(envelope))
            }
            Ok(WorkerEvent::ProtocolError(error)) => {
                self.closed = true;
                Some(BackendEvent::Envelope(error_envelope(error)))
            }
            Ok(WorkerEvent::Closed) | Err(TryRecvError::Disconnected) => {
                self.closed = true;
                None
            }
            Err(TryRecvError::Empty) => None,
        }
    }

    pub(super) fn state_snapshot(&self) -> BridgeState {
        self.state.clone()
    }

    pub(super) fn is_streaming(&self) -> bool {
        self.state.is_streaming
    }

    pub(super) fn current_session_id(&self) -> Option<String> {
        self.state.current_session_id.clone()
    }

    pub(super) fn is_closed(&self) -> bool {
        self.closed
    }

    pub(super) fn abandon_stuck_run(&mut self, reason: &str) -> Result<()> {
        self.send_runtime_command(&RuntimeCommand::AbandonStuckRun {
            reason: reason.to_owned(),
        })?;
        let deadline = Instant::now() + ABANDON_SETTLE_TIMEOUT;
        while Instant::now() < deadline && !self.closed && self.state.is_streaming {
            match self.events.recv_timeout(Duration::from_millis(20)) {
                Ok(WorkerEvent::Envelope(envelope)) => self.observe_envelope(&envelope),
                Ok(WorkerEvent::ProtocolError(error)) => {
                    self.closed = true;
                    return Err(anyhow!(error));
                }
                Ok(WorkerEvent::Closed) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.closed = true;
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        Ok(())
    }

    fn observe_envelope(&mut self, envelope: &BridgeEnvelope) {
        if let Some(state) = envelope.state.as_ref() {
            self.state = state.clone();
        }
    }

    fn send_runtime_command(&mut self, command: &RuntimeCommand) -> Result<()> {
        if self.closed {
            bail!("background runtime process is closed");
        }
        let writer = self
            .command
            .as_mut()
            .ok_or_else(|| anyhow!("background runtime command pipe is closed"))?;
        serde_json::to_writer(&mut *writer, command)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        Ok(())
    }

    fn shutdown(&mut self) {
        if self.closed {
            let _ = self.child.wait();
            return;
        }

        let _ = self.send(FrontendCommand::Shutdown);
        self.command.take();
        let deadline = Instant::now() + RUNTIME_SHUTDOWN_TIMEOUT;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    self.closed = true;
                    return;
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        let _ = terminate_child(&mut self.child);
        self.closed = true;
    }
}

impl Drop for RuntimeProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) fn run_worker(workspace: &Path) -> Result<()> {
    let mut service = BackendService::spawn(workspace.to_path_buf())?;
    let stdout = std::io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    write_envelope(
        &mut output,
        &BridgeEnvelope {
            kind: "state".into(),
            state: Some(service.state_snapshot()),
            message: None,
        },
    )?;

    let (input_tx, input_rx) = mpsc::channel();
    thread::spawn(move || worker_input_reader(input_tx));
    let mut input_closed = false;

    loop {
        while let Ok(input) = input_rx.try_recv() {
            match input {
                WorkerInput::Command(RuntimeCommand::Frontend { command }) => {
                    if let Err(error) = service.send(command) {
                        write_envelope(&mut output, &error_envelope(error.to_string()))?;
                    }
                }
                WorkerInput::Command(RuntimeCommand::AbandonStuckRun { reason }) => {
                    if let Err(error) = service.abandon_stuck_run(&reason) {
                        write_envelope(&mut output, &error_envelope(error.to_string()))?;
                    }
                }
                WorkerInput::ProtocolError(error) => {
                    write_envelope(&mut output, &error_envelope(error))?;
                }
                WorkerInput::Closed => input_closed = true,
            }
        }

        if input_closed && !service.is_closed() {
            let _ = service.send(FrontendCommand::Shutdown);
        }

        while let Some(event) = service.try_recv() {
            let BackendEvent::Envelope(envelope) = event;
            write_envelope(&mut output, &envelope)?;
        }

        if service.is_closed() {
            break;
        }
        thread::sleep(WORKER_POLL_INTERVAL);
    }

    Ok(())
}

fn runtime_reader(stdout: std::process::ChildStdout, tx: mpsc::Sender<WorkerEvent>) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => match serde_json::from_str::<BridgeEnvelope>(line.trim_end()) {
                Ok(envelope) => {
                    if tx.send(WorkerEvent::Envelope(envelope)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = tx.send(WorkerEvent::ProtocolError(format!(
                        "invalid background runtime frame: {error}"
                    )));
                    return;
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                let _ = tx.send(WorkerEvent::ProtocolError(format!(
                    "background runtime read failed: {error}"
                )));
                return;
            }
        }
    }
    let _ = tx.send(WorkerEvent::Closed);
}

fn worker_input_reader(tx: mpsc::Sender<WorkerInput>) {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => match serde_json::from_str::<RuntimeCommand>(line.trim_end()) {
                Ok(command) => {
                    if tx.send(WorkerInput::Command(command)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    if tx
                        .send(WorkerInput::ProtocolError(format!(
                            "invalid supervisor command: {error}"
                        )))
                        .is_err()
                    {
                        return;
                    }
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                let _ = tx.send(WorkerInput::ProtocolError(format!(
                    "supervisor command read failed: {error}"
                )));
                return;
            }
        }
    }
    let _ = tx.send(WorkerInput::Closed);
}

fn write_envelope(writer: &mut impl Write, envelope: &BridgeEnvelope) -> Result<()> {
    serde_json::to_writer(&mut *writer, envelope)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn error_envelope(message: String) -> BridgeEnvelope {
    BridgeEnvelope {
        kind: "error".into(),
        state: None,
        message: Some(message),
    }
}

fn terminate_child(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_none() {
        let pid = child.id();
        if force_terminate_process_tree(pid).is_err() {
            let _ = child.kill();
        }
    }
    let _ = child.wait();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_command_round_trips_frontend_commands() {
        let encoded = serde_json::to_string(&RuntimeCommand::Frontend {
            command: FrontendCommand::Interrupt,
        })
        .unwrap();
        let decoded: RuntimeCommand = serde_json::from_str(&encoded).unwrap();
        assert!(matches!(
            decoded,
            RuntimeCommand::Frontend {
                command: FrontendCommand::Interrupt
            }
        ));
    }
}
