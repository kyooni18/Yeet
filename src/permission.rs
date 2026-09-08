use std::sync::{Arc, Condvar, Mutex};

use uuid::Uuid;

use crate::model::{NativeAppPermission, ShellPermission};

type PermissionNotifier = Arc<dyn Fn() + Send + Sync>;
type NotifierSlot = Arc<Mutex<Option<PermissionNotifier>>>;

#[derive(Clone, Default)]
pub struct PermissionBroker {
    inner: Arc<(Mutex<PermissionState>, Condvar)>,
    notifier: NotifierSlot,
}

#[derive(Default)]
struct PermissionState {
    pending: Option<PermissionRequest>,
    decision: Option<bool>,
    closed: bool,
}

#[derive(Clone)]
enum PermissionRequest {
    Shell(ShellPermission),
    NativeApp(NativeAppPermission),
}

impl PermissionBroker {
    pub fn set_notifier(&self, notifier: PermissionNotifier) {
        if let Ok(mut value) = self.notifier.lock() {
            *value = Some(notifier);
        }
    }

    pub fn request(
        &self,
        kind: String,
        command: String,
        operation: String,
        reason: String,
    ) -> bool {
        self.wait_for(PermissionRequest::Shell(ShellPermission {
            id: Uuid::new_v4().to_string(),
            kind,
            command,
            operation,
            reason,
        }))
    }

    pub fn request_native(&self, request: NativeAppPermission) -> bool {
        self.wait_for(PermissionRequest::NativeApp(request))
    }

    pub fn pending_shell(&self) -> Option<ShellPermission> {
        let (lock, _condition) = &*self.inner;
        lock.lock()
            .ok()
            .and_then(|state| match state.pending.as_ref() {
                Some(PermissionRequest::Shell(permission)) => Some(permission.clone()),
                _ => None,
            })
    }

    pub fn pending_native_app(&self) -> Option<NativeAppPermission> {
        self.inner
            .0
            .lock()
            .ok()
            .and_then(|state| match state.pending.as_ref() {
                Some(PermissionRequest::NativeApp(permission)) => Some(permission.clone()),
                _ => None,
            })
    }

    pub fn resolve(&self, granted: bool) -> bool {
        self.resolve_matching(granted, |_| true)
    }

    pub fn resolve_shell(&self, granted: bool) -> bool {
        self.resolve_matching(granted, |request| {
            matches!(request, PermissionRequest::Shell(_))
        })
    }

    pub fn resolve_native_app(&self, granted: bool) -> bool {
        self.resolve_matching(granted, |request| {
            matches!(request, PermissionRequest::NativeApp(_))
        })
    }

    fn resolve_matching(
        &self,
        granted: bool,
        accepts: impl FnOnce(&PermissionRequest) -> bool,
    ) -> bool {
        let (lock, condition) = &*self.inner;
        let Ok(mut state) = lock.lock() else {
            return false;
        };
        if !state.pending.as_ref().is_some_and(accepts) {
            return false;
        }
        state.decision = Some(granted);
        condition.notify_all();
        drop(state);
        self.notify();
        true
    }

    pub fn close(&self) {
        let (lock, condition) = &*self.inner;
        if let Ok(mut state) = lock.lock() {
            state.closed = true;
            state.decision = Some(false);
            condition.notify_all();
        }
        self.notify();
    }

    fn notify(&self) {
        let notifier = self.notifier.lock().ok().and_then(|value| value.clone());
        if let Some(notifier) = notifier {
            notifier();
        }
    }

    fn wait_for(&self, request: PermissionRequest) -> bool {
        let (lock, condition) = &*self.inner;
        let mut state = lock.lock().unwrap();
        while state.pending.is_some() && !state.closed {
            state = condition.wait(state).unwrap();
        }
        if state.closed {
            return false;
        }
        state.pending = Some(request);
        state.decision = None;
        condition.notify_all();
        drop(state);
        self.notify();
        let mut state = lock.lock().unwrap();
        while state.decision.is_none() && !state.closed {
            state = condition.wait(state).unwrap();
        }
        let decision = state.decision.take().unwrap_or(false);
        state.pending = None;
        condition.notify_all();
        drop(state);
        self.notify();
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, thread, time::Duration};

    fn native() -> NativeAppPermission {
        NativeAppPermission {
            id: "request-1".into(),
            server: "computer-use".into(),
            tool: "open".into(),
            bundle_id: Some("org.blenderfoundation.blender".into()),
            app_name: Some("Blender".into()),
            operation: "Open Blender".into(),
            reason: "The MCP tool requested native access".into(),
        }
    }

    #[test]
    fn native_request_waits_for_resolution_and_preserves_identity() {
        let broker = PermissionBroker::default();
        let (tx, rx) = mpsc::channel();
        let worker = broker.clone();
        thread::spawn(move || tx.send(worker.request_native(native())).unwrap());
        for _ in 0..20 {
            if let Some(pending) = broker.pending_native_app() {
                assert_eq!(
                    pending.bundle_id.as_deref(),
                    Some("org.blenderfoundation.blender")
                );
                assert_eq!(pending.tool, "open");
                assert!(broker.resolve(true));
                assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap());
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("native permission did not become pending");
    }

    #[test]
    fn close_denies_pending_native_request() {
        let broker = PermissionBroker::default();
        let (tx, rx) = mpsc::channel();
        let worker = broker.clone();
        thread::spawn(move || tx.send(worker.request_native(native())).unwrap());
        for _ in 0..20 {
            if broker.pending_native_app().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        broker.close();
        assert!(!rx.recv_timeout(Duration::from_secs(1)).unwrap());
    }
}
