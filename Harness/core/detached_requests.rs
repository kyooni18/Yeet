//! Bridge requests issued by detached (background) agent threads.
//!
//! Interrupting the primary run cancels every in-flight bridge request so
//! blocking calls unwind promptly. Background agents must survive that, so
//! requests started on a thread running inside `run_detached` are recorded
//! here and skipped by `interrupt_active_requests`. Those agents are stopped
//! through their own cancellation flags instead.

use std::{
    cell::Cell,
    collections::HashSet,
    sync::{Mutex, OnceLock},
};

thread_local! {
    static DETACHED: Cell<bool> = const { Cell::new(false) };
}

fn ids() -> &'static Mutex<HashSet<String>> {
    static IDS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    IDS.get_or_init(Default::default)
}

/// Runs `body` with bridge requests on this thread exempt from interrupts.
pub(crate) fn run_detached<T>(body: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            DETACHED.with(|flag| flag.set(self.0));
        }
    }
    let _reset = Reset(DETACHED.with(|flag| flag.replace(true)));
    body()
}

pub(super) fn note_request(id: &str) {
    if DETACHED.with(Cell::get)
        && let Ok(mut ids) = ids().lock()
    {
        ids.insert(id.to_owned());
    }
}

pub(super) fn forget(id: &str) {
    if let Ok(mut ids) = ids().lock() {
        ids.remove(id);
    }
}

/// Drops ids that are no longer pending and returns the detached ones in `live`.
pub(super) fn retain_live(live: &[String]) -> HashSet<String> {
    let Ok(mut ids) = ids().lock() else {
        return HashSet::new();
    };
    ids.retain(|id| live.contains(id));
    ids.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_requests_inside_detached_scope_are_exempt() {
        note_request("primary");
        run_detached(|| note_request("worker"));
        note_request("primary-after");
        let live = ["primary", "worker", "primary-after"].map(String::from);
        let detached = retain_live(&live);
        assert!(detached.contains("worker"));
        assert!(!detached.contains("primary"));
        assert!(!detached.contains("primary-after"));
        forget("worker");
    }
}
