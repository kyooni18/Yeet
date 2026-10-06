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

    /// Match the displayed request atomically so delayed UI input cannot approve
    /// a replacement request. Legacy unscoped callers retain their existing API.
    pub fn resolve_request(&self, request_id: &str, granted: bool) -> bool {
        self.resolve_matching(granted, |request| match request {
            PermissionRequest::Shell(permission) => permission.id == request_id,
            PermissionRequest::NativeApp(permission) => permission.id == request_id,
        })
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
        if state.decision.is_some() || !state.pending.as_ref().is_some_and(accepts) {
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

    #[test]
    fn delayed_decision_cannot_resolve_a_replacement_permission() {
        let broker = PermissionBroker::default();
        let requests = [
            PermissionRequest::Shell(ShellPermission {
                id: "current-shell".into(),
                kind: "shell".into(),
                command: "echo test".into(),
                operation: "execute".into(),
                reason: String::new(),
            }),
            PermissionRequest::NativeApp(NativeAppPermission {
                id: "current-native".into(),
                server: "native".into(),
                tool: "inspect".into(),
                bundle_id: None,
                app_name: None,
                operation: "inspect".into(),
                reason: String::new(),
            }),
        ];
        for request in requests {
            let id = match &request {
                PermissionRequest::Shell(value) => value.id.clone(),
                PermissionRequest::NativeApp(value) => value.id.clone(),
            };
            {
                let mut state = broker.inner.0.lock().unwrap();
                state.pending = Some(request);
                state.decision = None;
            }
            assert!(!broker.resolve_request("previous-request", true));
            assert_eq!(broker.inner.0.lock().unwrap().decision, None);
            assert!(broker.resolve_request(&id, false));
            assert_eq!(broker.inner.0.lock().unwrap().decision, Some(false));
            assert!(!broker.resolve_request(&id, true));
        }
    }
}
