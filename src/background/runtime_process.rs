use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread,
    time::Duration,
};

use anyhow::{Result, anyhow};

use super::Wake;
use crate::{
    backend::{BackendEvent, BackendService, SessionCatalog},
    model::{FrontendCommand, HarnessEvent, HarnessState},
};

const RUNTIME_IDLE_WAIT: Duration = Duration::from_millis(50);
const RUNTIME_START_TIMEOUT: Duration = Duration::from_secs(30);

enum RuntimeCommand {
    Frontend(FrontendCommand),
    AdoptCatalog(SessionCatalog),
    AbandonStuckRun(String),
}

/// Daemon-side handle for one semantic backend runtime.
///
/// BackendService deliberately lives on a dedicated worker thread. The background
/// daemon owns socket acceptance, client routing, heartbeats, and runtime fan-out;
/// none of those operations may ever wait on backend mutexes, provider work,
/// persistence, session scans, or a slow command.
///
/// The daemon only touches an unbounded command sender, an event receiver, atomics,
/// and the last fully published semantic state cached below. This keeps one
/// unhealthy or busy session from stalling every Remote/TUI client in a workspace.
pub(super) struct RuntimeProcess {
    command_tx: Sender<RuntimeCommand>,
    command_wake: Wake,
    events: Receiver<HarnessEvent>,
    failed: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    last_state: HarnessState,
}

impl RuntimeProcess {
    pub(super) fn spawn(workspace: &Path, _scope: Option<&str>, daemon_wake: Wake) -> Result<Self> {
        let workspace = workspace.to_path_buf();
        let runtime_wake = Wake::new();
        let command_wake = runtime_wake.clone();
        let (command_tx, command_rx) = mpsc::channel::<RuntimeCommand>();
        let (event_tx, events) = mpsc::channel::<HarnessEvent>();
        let (startup_tx, startup_rx) = mpsc::sync_channel::<Result<HarnessState, String>>(1);
        let failed = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let worker_failed = failed.clone();
        let worker_closed = closed.clone();

        thread::Builder::new()
            .name("yeet-backend-runtime".into())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    let mut service =
                        match BackendService::spawn(workspace, Some(runtime_wake.clone())) {
                            Ok(service) => service,
                            Err(error) => {
                                let _ = startup_tx.send(Err(error.to_string()));
                                return;
                            }
                        };

                    let initial_state = service.state_snapshot();
                    if startup_tx.send(Ok(initial_state)).is_err() {
                        return;
                    }

                    loop {
                        let mut command_channel_open = true;
                        loop {
                            match command_rx.try_recv() {
                                Ok(command) => {
                                    if !handle_runtime_command(
                                        &mut service,
                                        command,
                                        &event_tx,
                                        &daemon_wake,
                                    ) {
                                        return;
                                    }
                                    // Commands often publish state synchronously. Forward
                                    // that state before accepting the next command so the
                                    // daemon routing cache cannot lag semantic transitions.
                                    drain_backend_events(&mut service, &event_tx, &daemon_wake);
                                }
                                Err(TryRecvError::Empty) => break,
                                Err(TryRecvError::Disconnected) => {
                                    command_channel_open = false;
                                    break;
                                }
                            }
                        }

                        drain_backend_events(&mut service, &event_tx, &daemon_wake);

                        if service.is_closed() || !command_channel_open {
                            return;
                        }

                        runtime_wake.wait_timeout(RUNTIME_IDLE_WAIT);
                    }
                }));

                if outcome.is_err() {
                    eprintln!("yeet: background session runtime worker panicked; retiring it");
                    worker_failed.store(true, Ordering::Release);
                }
                worker_closed.store(true, Ordering::Release);
                daemon_wake.notify();
            })
            .map_err(|error| anyhow!("spawn background session runtime worker: {error}"))?;

        let last_state = startup_rx
            .recv_timeout(RUNTIME_START_TIMEOUT)
            .map_err(|_| anyhow!("background session runtime did not become ready within 30s"))?
            .map_err(|message| anyhow!(message))?;

        Ok(Self {
            command_tx,
            command_wake,
            events,
            failed,
            closed,
            last_state,
        })
    }

    pub(super) fn send(&mut self, command: FrontendCommand) -> Result<()> {
        self.enqueue(RuntimeCommand::Frontend(command))
    }

    pub(super) fn try_recv(&mut self) -> Option<HarnessEvent> {
        match self.events.try_recv() {
            Ok(envelope) => {
                if let Some(state) = envelope.state.as_ref() {
                    let mut published = state.clone();
                    // Streaming envelopes intentionally omit the transcript on most
                    // tokens. Keep the last complete conversation in the daemon cache
                    // so reconnects always receive a complete semantic snapshot.
                    if published.conversation.is_none() {
                        published.conversation = self.last_state.conversation.clone();
                    }
                    published.session_activity = self.last_state.session_activity.clone();
                    self.last_state = published;
                }
                Some(envelope)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                if !self.closed.load(Ordering::Acquire) {
                    self.failed.store(true, Ordering::Release);
                }
                None
            }
        }
    }
    pub(super) fn session_activity(
        &self,
    ) -> &std::collections::BTreeMap<String, crate::model::SessionActivity> {
        &self.last_state.session_activity
    }

    pub(super) fn set_session_activity(
        &mut self,
        activity: &std::collections::BTreeMap<String, crate::model::SessionActivity>,
    ) -> bool {
        if self.last_state.session_activity == *activity {
            return false;
        }
        self.last_state.session_activity = activity.clone();
        true
    }
    pub(super) fn activity(&self) -> Option<(String, crate::model::SessionActivity)> {
        use crate::model::SessionActivity;
        let state = &self.last_state;
        let status = if state.pending_shell_permission.is_some()
            || state.pending_native_app_permission.is_some()
        {
            SessionActivity::WaitingForPermission
        } else if state.is_streaming {
            SessionActivity::Running
        } else {
            return None;
        };
        Some((state.current_session_id.clone()?, status))
    }

    pub(super) fn activity_snapshot(&self) -> HarnessState {
        self.last_state.without_conversation()
    }
    pub(super) fn state_snapshot(&self) -> HarnessState {
        self.last_state.clone()
    }

    pub(super) fn is_streaming(&self) -> bool {
        self.last_state.is_streaming
    }

    pub(super) fn current_session_id(&self) -> Option<String> {
        self.last_state.current_session_id.clone()
    }

    pub(super) fn is_closed(&self) -> bool {
        self.failed.load(Ordering::Acquire) || self.closed.load(Ordering::Acquire)
    }

    pub(super) fn adopt_session_catalog(&self, catalog: &SessionCatalog) {
        let _ = self.enqueue(RuntimeCommand::AdoptCatalog(catalog.clone()));
    }

    pub(super) fn abandon_stuck_run(&self, reason: &str) -> Result<()> {
        self.enqueue(RuntimeCommand::AbandonStuckRun(reason.to_owned()))
    }

    fn enqueue(&self, command: RuntimeCommand) -> Result<()> {
        if self.is_closed() {
            return Err(anyhow!("background session runtime is no longer available"));
        }
        self.command_tx
            .send(command)
            .map_err(|_| anyhow!("background session runtime command channel closed"))?;
        self.command_wake.notify();
        Ok(())
    }
}

impl Drop for RuntimeProcess {
    fn drop(&mut self) {
        // Preserve command ordering: a queued abandon/interrupt is observed before
        // shutdown. Retirement itself already happens off the daemon thread.
        let _ = self
            .command_tx
            .send(RuntimeCommand::Frontend(FrontendCommand::Shutdown));
        self.command_wake.notify();
    }
}

fn handle_runtime_command(
    service: &mut BackendService,
    command: RuntimeCommand,
    event_tx: &Sender<HarnessEvent>,
    daemon_wake: &Wake,
) -> bool {
    let result = match command {
        RuntimeCommand::Frontend(FrontendCommand::Shutdown) => {
            let result = service.send(FrontendCommand::Shutdown);
            if let Err(error) = result {
                send_runtime_error(event_tx, daemon_wake, error);
            }
            return false;
        }
        RuntimeCommand::Frontend(command) => service.send(command),
        RuntimeCommand::AdoptCatalog(catalog) => {
            service.adopt_session_catalog(&catalog);
            Ok(())
        }
        RuntimeCommand::AbandonStuckRun(reason) => service.abandon_stuck_run(&reason),
    };

    if let Err(error) = result {
        send_runtime_error(event_tx, daemon_wake, error);
    }
    true
}

fn drain_backend_events(
    service: &mut BackendService,
    event_tx: &Sender<HarnessEvent>,
    daemon_wake: &Wake,
) {
    while let Some(event) = service.try_recv() {
        let BackendEvent::Envelope(envelope) = event;
        if event_tx.send(envelope).is_err() {
            return;
        }
        daemon_wake.notify();
    }
}

fn send_runtime_error(event_tx: &Sender<HarnessEvent>, daemon_wake: &Wake, error: anyhow::Error) {
    let _ = event_tx.send(HarnessEvent {
        kind: "error".into(),
        state: None,
        message: Some(error.to_string()),
    });
    daemon_wake.notify();
}

#[cfg(test)]
mod activity_tests {
    use super::*;

    #[test]
    fn daemon_activity_survives_worker_updates_and_clears_on_completion() {
        let (command_tx, _) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let mut runtime = RuntimeProcess {
            command_tx,
            command_wake: Wake::new(),
            events,
            failed: Arc::new(AtomicBool::new(false)),
            closed: Arc::new(AtomicBool::new(false)),
            last_state: HarnessState {
                current_session_id: Some("detached".into()),
                is_streaming: true,
                ..Default::default()
            },
        };
        let activity = std::collections::BTreeMap::from([runtime.activity().unwrap()]);
        assert!(runtime.set_session_activity(&activity));
        assert!(!runtime.set_session_activity(&activity));
        event_tx
            .send(HarnessEvent {
                kind: "state".into(),
                state: Some(HarnessState {
                    current_session_id: Some("detached".into()),
                    ..Default::default()
                }),
                message: None,
            })
            .unwrap();
        runtime.try_recv().unwrap();
        assert_eq!(runtime.activity_snapshot().session_activity, activity);
        assert!(runtime.activity().is_none());
        assert!(runtime.set_session_activity(&Default::default()));
        assert!(runtime.state_snapshot().session_activity.is_empty());
    }
}
