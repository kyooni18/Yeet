//! Session runtime ownership, retirement and interrupt/idle lifecycle policy.

use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;

use super::{Wake, runtime_process::RuntimeProcess};
use crate::harness::SessionCatalog;

const INTERRUPT_RECOVERY_AFTER: Duration = Duration::from_secs(4);
// Transport loss is not cancellation: a detached streaming runtime survives.
pub(super) const RUNTIME_IDLE_RETIRE_AFTER: Duration = Duration::from_secs(60);

pub(super) struct SessionRuntime {
    pub(super) id: u64,
    pub(super) service: RuntimeProcess,
    pub(super) lifecycle: SessionRuntimeLifecycle,
    /// Session catalog this runtime last published.
    pub(super) catalog: Option<SessionCatalog>,
}

impl SessionRuntime {
    pub(super) fn spawn(
        workspace: &Path,
        id: u64,
        scope: Option<&str>,
        wake: Wake,
    ) -> Result<Self> {
        Ok(Self {
            id,
            service: RuntimeProcess::spawn(workspace, scope, wake)?,
            lifecycle: SessionRuntimeLifecycle::default(),
            catalog: None,
        })
    }

    /// Runtime teardown can wait on backend work; never block the daemon loop.
    pub(super) fn retire_async(self) {
        thread::spawn(move || drop(self));
    }
}

#[derive(Default)]
pub(super) struct SessionRuntimeLifecycle {
    idle_since: Option<Instant>,
    interrupt_requested_at: Option<Instant>,
}

impl SessionRuntimeLifecycle {
    pub(super) fn attached(&mut self) {
        self.idle_since = None;
    }

    pub(super) fn note_interrupt(&mut self, streaming: bool, now: Instant) {
        self.interrupt_requested_at = streaming.then_some(now);
    }

    pub(super) fn observe_streaming(&mut self, streaming: bool) {
        if !streaming {
            self.interrupt_requested_at = None;
        }
    }

    pub(super) fn requires_interrupt_recovery(&self, streaming: bool, now: Instant) -> bool {
        streaming
            && self.interrupt_requested_at.is_some_and(|requested| {
                now.saturating_duration_since(requested) >= INTERRUPT_RECOVERY_AFTER
            })
    }

    pub(super) fn observe_idle(&mut self, attached: bool, streaming: bool, now: Instant) {
        if attached || streaming {
            self.idle_since = None;
        } else {
            self.idle_since.get_or_insert(now);
        }
    }

    pub(super) fn idle_expired(&self, now: Instant) -> bool {
        self.idle_since
            .is_some_and(|since| now.saturating_duration_since(since) >= RUNTIME_IDLE_RETIRE_AFTER)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_loss_preserves_streaming_and_idle_retirement_restarts_on_attach() {
        let now = Instant::now();
        let later = now + RUNTIME_IDLE_RETIRE_AFTER;
        let mut lifecycle = SessionRuntimeLifecycle::default();
        lifecycle.observe_idle(false, true, now);
        assert!(!lifecycle.idle_expired(later));
        lifecycle.observe_idle(false, false, now);
        lifecycle.observe_idle(false, false, now + Duration::from_secs(1));
        assert!(lifecycle.idle_expired(later));
        lifecycle.attached();
        assert!(!lifecycle.idle_expired(later));
        lifecycle.observe_idle(false, false, later);
        assert!(!lifecycle.idle_expired(later));
        assert!(lifecycle.idle_expired(later + RUNTIME_IDLE_RETIRE_AFTER));
        lifecycle.observe_idle(true, false, later);
        assert!(!lifecycle.idle_expired(later + RUNTIME_IDLE_RETIRE_AFTER));
    }

    #[test]
    fn only_a_still_streaming_interrupted_run_requires_recovery() {
        let now = Instant::now();
        let deadline = now + INTERRUPT_RECOVERY_AFTER;
        let mut lifecycle = SessionRuntimeLifecycle::default();
        assert!(!lifecycle.requires_interrupt_recovery(true, deadline));
        lifecycle.note_interrupt(true, now);
        assert!(!lifecycle.requires_interrupt_recovery(true, deadline - Duration::from_millis(1)));
        assert!(lifecycle.requires_interrupt_recovery(true, deadline));
        assert!(!lifecycle.requires_interrupt_recovery(false, deadline));
        lifecycle.observe_streaming(false);
        assert!(!lifecycle.requires_interrupt_recovery(true, deadline));
        lifecycle.note_interrupt(false, now);
        assert!(!lifecycle.requires_interrupt_recovery(true, deadline));
    }
}
