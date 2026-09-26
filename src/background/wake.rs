//! Edge-triggered wakeup shared by event producers and one polling loop.
//!
//! The daemon and the Remote gateway multiplex several independent sources
//! (client sockets, backend runtimes, extension hosts). A fixed sleep between
//! polls adds that sleep to every hop of latency; a `Wake` lets any producer cut
//! the wait short while the timeout still bounds polling of sources that
//! cannot signal.
use std::{
    sync::{Arc, Condvar, Mutex, PoisonError},
    time::Duration,
};

#[derive(Clone, Default)]
pub(crate) struct Wake {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

impl Wake {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn notify(&self) {
        let (flag, condvar) = &*self.inner;
        *flag.lock().unwrap_or_else(PoisonError::into_inner) = true;
        condvar.notify_all();
    }

    /// Waits until notified or `timeout` elapses, consuming the notification.
    pub(crate) fn wait_timeout(&self, timeout: Duration) {
        let (flag, condvar) = &*self.inner;
        let guard = flag.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut guard, _) = condvar
            .wait_timeout_while(guard, timeout, |notified| !*notified)
            .unwrap_or_else(PoisonError::into_inner);
        *guard = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Instant};

    #[test]
    fn notification_before_wait_is_not_lost() {
        let wake = Wake::new();
        wake.notify();
        let started = Instant::now();
        wake.wait_timeout(Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn cross_thread_notify_interrupts_wait() {
        let wake = Wake::new();
        let producer = wake.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            producer.notify();
        });
        let started = Instant::now();
        wake.wait_timeout(Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1));
        handle.join().unwrap();
    }

    #[test]
    fn times_out_without_notification() {
        let wake = Wake::new();
        let started = Instant::now();
        wake.wait_timeout(Duration::from_millis(20));
        assert!(started.elapsed() >= Duration::from_millis(15));
    }
}
