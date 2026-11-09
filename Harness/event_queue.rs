//! Coalesce cumulative snapshots before retaining them. Discrete replies stay ordered.
use super::model::HarnessEvent;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, mpsc},
};

#[derive(Default)]
struct Pending {
    events: VecDeque<HarnessEvent>,
    coalesced: u64,
}

#[derive(Clone)]
pub(crate) struct EventTx {
    pending: Arc<Mutex<Pending>>,
    signal: mpsc::SyncSender<()>,
}
pub(crate) struct EventRx {
    pending: Arc<Mutex<Pending>>,
    signal: mpsc::Receiver<()>,
}
pub(crate) fn event_channel() -> (EventTx, EventRx) {
    let pending = Arc::new(Mutex::new(Pending::default()));
    let (signal, receiver) = mpsc::sync_channel(1);
    (
        EventTx {
            pending: pending.clone(),
            signal,
        },
        EventRx {
            pending,
            signal: receiver,
        },
    )
}
impl EventTx {
    #[allow(clippy::result_large_err)]
    pub(crate) fn send(
        &self,
        mut event: HarnessEvent,
    ) -> Result<(), mpsc::SendError<HarnessEvent>> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        // Never merge across discrete replies or session transitions.
        if event.kind == "state"
            && event.message.is_none()
            && let Some(previous) = pending.events.back_mut()
            && previous.kind == "state"
            && previous.message.is_none()
            && let (Some(next), Some(old)) = (event.state.as_mut(), previous.state.as_mut())
            && next.current_session_id == old.current_session_id
        {
            if next.conversation.is_none() {
                next.conversation = old.conversation.take();
            }
            *previous = event;
            pending.coalesced += 1;
            let count = pending.coalesced;
            if count >= 4096 && count.is_power_of_two() {
                eprintln!(
                    "yeet: backend snapshot coalescing pending_events={} coalesced_snapshots={count}",
                    pending.events.len()
                );
            }
        } else {
            pending.events.push_back(event);
            let depth = pending.events.len();
            if depth >= 64 && depth.is_power_of_two() {
                eprintln!(
                    "yeet: backend event backlog pending_events={depth} coalesced_snapshots={}",
                    pending.coalesced
                );
            }
        }
        // One token represents the queue. Never block under live-state locks.
        match self.signal.try_send(()) {
            Ok(()) | Err(mpsc::TrySendError::Full(())) => Ok(()),
            Err(mpsc::TrySendError::Disconnected(())) => Err(mpsc::SendError(
                pending.events.pop_back().expect("queued event"),
            )),
        }
    }
}
impl EventRx {
    pub(crate) fn try_recv(&self) -> Result<HarnessEvent, mpsc::TryRecvError> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(event) = pending.events.pop_front() {
            return Ok(event);
        }
        match self.signal.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Empty) => Err(mpsc::TryRecvError::Empty),
            Err(mpsc::TryRecvError::Disconnected) => Err(mpsc::TryRecvError::Disconnected),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConversationEntry, ConversationKind, HarnessState};

    fn snapshot(session: &str, full: bool, revision: usize) -> HarnessEvent {
        HarnessEvent {
            kind: "state".into(),
            message: None,
            state: Some(HarnessState {
                current_session_id: Some(session.into()),
                active_assistant_text: revision.to_string(),
                conversation: full.then(|| {
                    vec![ConversationEntry {
                        id: "user".into(),
                        kind: ConversationKind::User {
                            content: "x".repeat(65_536),
                        },
                    }]
                }),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn paused_consumers_retain_one_snapshot_at_both_hops() {
        let (producer, first) = event_channel();
        let (forwarder, second) = event_channel();
        for revision in 0..4096 {
            producer.send(snapshot("session", true, revision)).unwrap();
        }
        assert_eq!(producer.pending.lock().unwrap().events.len(), 1);
        forwarder.send(first.try_recv().unwrap()).unwrap();
        for revision in 4096..8192 {
            forwarder
                .send(snapshot("session", false, revision))
                .unwrap();
        }
        assert_eq!(forwarder.pending.lock().unwrap().events.len(), 1);
        let state = second.try_recv().unwrap().state.unwrap();
        assert_eq!(state.active_assistant_text, "8191");
        assert_eq!(state.conversation.unwrap().len(), 1);
        assert!(matches!(second.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(forwarder);
        // The single wake token may precede channel disconnection.
        let _ = second.try_recv();
        assert!(matches!(
            second.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn discrete_replies_and_session_boundaries_are_not_coalesced() {
        let (tx, rx) = event_channel();
        tx.send(snapshot("a", true, 1)).unwrap();
        tx.send(HarnessEvent {
            kind: "error".into(),
            state: None,
            message: Some("reply".into()),
        })
        .unwrap();
        tx.send(snapshot("a", false, 2)).unwrap();
        tx.send(snapshot("b", false, 3)).unwrap();
        assert_eq!(
            rx.try_recv().unwrap().state.unwrap().active_assistant_text,
            "1"
        );
        assert_eq!(rx.try_recv().unwrap().message.as_deref(), Some("reply"));
        assert_eq!(
            rx.try_recv().unwrap().state.unwrap().active_assistant_text,
            "2"
        );
        let state = rx.try_recv().unwrap().state.unwrap();
        assert_eq!(state.current_session_id.as_deref(), Some("b"));
        assert!(
            state.conversation.is_none(),
            "must not inherit another session's history"
        );
        drop(rx);
        assert!(tx.send(snapshot("b", true, 4)).is_err());
    }
}
