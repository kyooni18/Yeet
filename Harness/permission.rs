use std::{
    collections::HashSet,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use uuid::Uuid;

use crate::model::{NativeAppPermission, ShellPermission, UserQuestion};

type PermissionNotifier = Arc<dyn Fn() + Send + Sync>;
type NotifierSlot = Arc<Mutex<Option<PermissionNotifier>>>;

#[derive(Clone, Default)]
pub struct PermissionBroker {
    inner: Arc<(Mutex<PermissionState>, Condvar)>,
    notifier: NotifierSlot,
    /// Approvals the user chose to keep for the rest of this session.
    session_grants: Arc<Mutex<HashSet<String>>>,
}

/// The user's answer to a permission prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    Deny,
    /// Allow this request only.
    Once,
    /// Allow this request and identical future requests in this session.
    Session,
}

impl PermissionDecision {
    pub fn allowed(self) -> bool {
        self != Self::Deny
    }
}

#[derive(Default)]
struct PermissionState {
    pending: Option<PermissionRequest>,
    decision: Option<PermissionDecision>,
    closed: bool,
    question: Option<UserQuestion>,
    /// `Some(None)` means the user dismissed the question.
    answer: Option<Option<String>>,
}

#[derive(Clone)]
enum PermissionRequest {
    Shell(ShellPermission),
    NativeApp(NativeAppPermission),
}

impl PermissionRequest {
    fn grant_key(&self) -> String {
        match self {
            Self::Shell(p) => format!("{}\u{1}{}", p.kind, p.operation),
            Self::NativeApp(p) => format!(
                "native\u{1}{}\u{1}{}\u{1}{}",
                p.server,
                p.bundle_id.as_deref().unwrap_or(""),
                p.operation
            ),
        }
    }
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
        self.ask_user(PermissionRequest::Shell(ShellPermission {
            id: Uuid::new_v4().to_string(),
            kind,
            command,
            operation,
            reason,
        }))
    }

    pub fn request_native(&self, request: NativeAppPermission) -> bool {
        self.ask_user(PermissionRequest::NativeApp(request))
    }

    /// Skips the prompt when the user already allowed this kind of request for
    /// the session. A "session" answer is recorded by `resolve_matching`.
    fn ask_user(&self, request: PermissionRequest) -> bool {
        if self.has_session_grant(&request.grant_key()) {
            return true;
        }
        self.wait_for(request).allowed()
    }

    /// Approvals kept for this session, for persistence with the session.
    pub fn session_grants(&self) -> Vec<String> {
        let mut grants: Vec<_> = self
            .session_grants
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect();
        grants.sort();
        grants
    }

    pub fn restore_session_grants(&self, grants: Vec<String>) {
        *self.session_grants.lock().unwrap() = grants.into_iter().collect();
    }

    fn has_session_grant(&self, key: &str) -> bool {
        self.session_grants.lock().unwrap().contains(key)
    }

    /// Forgets every "allow for this session" approval.
    pub fn clear_session_grants(&self) {
        self.session_grants.lock().unwrap().clear();
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
        self.resolve_matching(Self::decision_for(granted), |_| true)
    }

    fn decision_for(granted: bool) -> PermissionDecision {
        if granted {
            PermissionDecision::Once
        } else {
            PermissionDecision::Deny
        }
    }

    /// Like `resolve_request`, but lets the caller keep the approval for the session.
    pub fn resolve_request_scoped(&self, request_id: &str, decision: PermissionDecision) -> bool {
        self.resolve_matching(decision, |request| match request {
            PermissionRequest::Shell(permission) => permission.id == request_id,
            PermissionRequest::NativeApp(permission) => permission.id == request_id,
        })
    }

    /// Match the displayed request atomically so delayed UI input cannot approve
    /// a replacement request. Legacy unscoped callers retain their existing API.
    pub fn resolve_request(&self, request_id: &str, granted: bool) -> bool {
        self.resolve_matching(Self::decision_for(granted), |request| match request {
            PermissionRequest::Shell(permission) => permission.id == request_id,
            PermissionRequest::NativeApp(permission) => permission.id == request_id,
        })
    }

    pub fn resolve_shell(&self, granted: bool) -> bool {
        self.resolve_matching(Self::decision_for(granted), |request| {
            matches!(request, PermissionRequest::Shell(_))
        })
    }

    pub fn resolve_native_app(&self, granted: bool) -> bool {
        self.resolve_matching(Self::decision_for(granted), |request| {
            matches!(request, PermissionRequest::NativeApp(_))
        })
    }

    fn resolve_matching(
        &self,
        decision: PermissionDecision,
        accepts: impl FnOnce(&PermissionRequest) -> bool,
    ) -> bool {
        let (lock, condition) = &*self.inner;
        let Ok(mut state) = lock.lock() else {
            return false;
        };
        if state.decision.is_some() || !state.pending.as_ref().is_some_and(accepts) {
            return false;
        }
        if decision == PermissionDecision::Session
            && let Some(request) = state.pending.as_ref()
        {
            let key = request.grant_key();
            self.session_grants.lock().unwrap().insert(key);
        }
        state.decision = Some(decision);
        condition.notify_all();
        drop(state);
        self.notify();
        true
    }

    /// Blocks until the user answers, dismisses, or `cancel` is raised.
    /// Returns `None` when dismissed/cancelled.
    pub fn ask(&self, question: UserQuestion, cancel: &AtomicBool) -> Option<String> {
        let (lock, condition) = &*self.inner;
        let mut state = lock.lock().ok()?;
        while state.question.is_some() && !state.closed {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            state = condition
                .wait_timeout(state, Duration::from_millis(200))
                .ok()?
                .0;
        }
        if state.closed {
            return None;
        }
        state.question = Some(question);
        state.answer = None;
        condition.notify_all();
        drop(state);
        self.notify();
        let mut state = lock.lock().ok()?;
        while state.answer.is_none() && !state.closed && !cancel.load(Ordering::Relaxed) {
            state = condition
                .wait_timeout(state, Duration::from_millis(200))
                .ok()?
                .0;
        }
        let answer = state.answer.take().flatten();
        state.question = None;
        condition.notify_all();
        drop(state);
        self.notify();
        answer
    }

    pub fn pending_question(&self) -> Option<UserQuestion> {
        self.inner
            .0
            .lock()
            .ok()
            .and_then(|state| state.question.clone())
    }

    /// Answers only the question the caller displayed; `None` dismisses it.
    pub fn answer_question(&self, request_id: &str, answer: Option<String>) -> bool {
        let (lock, condition) = &*self.inner;
        let Ok(mut state) = lock.lock() else {
            return false;
        };
        if state.answer.is_some()
            || state
                .question
                .as_ref()
                .is_none_or(|question| question.id != request_id)
        {
            return false;
        }
        state.answer = Some(answer);
        condition.notify_all();
        drop(state);
        self.notify();
        true
    }

    /// Dismisses any pending question (used by interrupts).
    pub fn dismiss_question(&self) {
        let id = self.pending_question().map(|question| question.id);
        if let Some(id) = id {
            self.answer_question(&id, None);
        }
    }

    pub fn close(&self) {
        let (lock, condition) = &*self.inner;
        if let Ok(mut state) = lock.lock() {
            state.closed = true;
            state.decision = Some(PermissionDecision::Deny);
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

    fn wait_for(&self, request: PermissionRequest) -> PermissionDecision {
        let (lock, condition) = &*self.inner;
        let mut state = lock.lock().unwrap();
        while state.pending.is_some() && !state.closed {
            state = condition.wait(state).unwrap();
        }
        if state.closed {
            return PermissionDecision::Deny;
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
        let decision = state.decision.take().unwrap_or(PermissionDecision::Deny);
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
            assert_eq!(
                broker.inner.0.lock().unwrap().decision,
                Some(PermissionDecision::Deny)
            );
            assert!(!broker.resolve_request(&id, true));
        }
    }

    #[test]
    fn session_grant_skips_later_prompts_until_cleared() {
        let broker = PermissionBroker::default();
        let asker = broker.clone();
        let first = std::thread::spawn(move || {
            asker.request(
                "capability".into(),
                "mcp x".into(),
                "use x".into(),
                String::new(),
            )
        });
        let id = loop {
            if let Some(pending) = broker.pending_shell() {
                break pending.id;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(broker.resolve_request_scoped(&id, PermissionDecision::Session));
        assert!(first.join().unwrap());
        // Same operation no longer prompts.
        assert!(broker.request(
            "capability".into(),
            "mcp x".into(),
            "use x".into(),
            String::new()
        ));
        assert!(broker.pending_shell().is_none());
        broker.clear_session_grants();
        assert!(!broker.has_session_grant("capability\u{1}use x"));
    }

    #[test]
    fn question_answers_only_the_displayed_request() {
        let broker = PermissionBroker::default();
        let asker = broker.clone();
        let handle = std::thread::spawn(move || {
            asker.ask(
                UserQuestion {
                    id: "q1".into(),
                    question: "Which?".into(),
                    options: vec!["A".into(), "B".into()],
                    allow_custom: true,
                },
                &AtomicBool::new(false),
            )
        });
        while broker.pending_question().is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!broker.answer_question("stale", Some("A".into())));
        assert!(broker.answer_question("q1", Some("custom".into())));
        assert_eq!(handle.join().unwrap().as_deref(), Some("custom"));
        assert!(broker.pending_question().is_none());
    }
}
