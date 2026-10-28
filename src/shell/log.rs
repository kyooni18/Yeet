//! Bounded disk-backed shell transcripts, shared by readers and job snapshots.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;

pub const SHELL_LOG_LIMIT: usize = 64 * 1024 * 1024;
pub const SESSION_SHELL_LOG_LIMIT: usize = 512 * 1024 * 1024;

#[derive(Clone)]
pub struct ShellOutputLog {
    inner: Arc<Mutex<LogState>>,
    changed: Arc<Condvar>,
    budget: Arc<AtomicUsize>,
}

struct LogState {
    file: File,
    bytes: usize,
    truncated: bool,
    error: Option<String>,
    finished: bool,
    spans: Vec<LogSpan>,
    release_budget: Option<Arc<AtomicUsize>>,
}

impl Drop for LogState {
    fn drop(&mut self) {
        if let Some(budget) = &self.release_budget {
            budget.fetch_sub(self.bytes, Ordering::AcqRel);
        }
    }
}

struct LogSpan {
    start: usize,
    end: usize,
    stream: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellEventPage {
    pub events: Vec<ShellOutputEvent>,
    pub next_offset: usize,
    pub total_bytes: usize,
    pub finished: bool,
    pub truncated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellOutputEvent {
    pub offset: usize,
    pub next_offset: usize,
    pub stream: &'static str,
    /// Raw bytes, so clients can incrementally decode UTF-8 and terminal escapes.
    pub data_base64: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellLogSnapshot {
    pub bytes: usize,
    pub truncated: bool,
    pub complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellLogPage {
    pub content: String,
    pub offset: usize,
    pub next_offset: usize,
    pub total_bytes: usize,
    pub has_more: bool,
    pub finished: bool,
    pub truncated: bool,
}

impl ShellOutputLog {
    pub fn new(file: File, budget: Arc<AtomicUsize>) -> Self {
        Self::with_lifetime(file, budget, false)
    }

    /// Anonymous transcripts release their session budget when the last reader
    /// and job reference goes away. Persistent artifacts remain charged.
    pub fn new_ephemeral(file: File, budget: Arc<AtomicUsize>) -> Self {
        Self::with_lifetime(file, budget, true)
    }

    fn with_lifetime(file: File, budget: Arc<AtomicUsize>, ephemeral: bool) -> Self {
        Self {
            inner: Arc::new(Mutex::new(LogState {
                file,
                bytes: 0,
                truncated: false,
                error: None,
                finished: false,
                spans: Vec::new(),
                release_budget: ephemeral.then(|| budget.clone()),
            })),
            changed: Arc::new(Condvar::new()),
            budget,
        }
    }

    // Failure to spool must never stop draining the child's pipes: that can
    // deadlock the child. Surface loss in metadata while retaining live tails.
    pub(super) fn push(&self, stream: &'static str, bytes: &[u8]) {
        let mut state = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if state.finished || state.error.is_some() || state.truncated {
            state.truncated |= !bytes.is_empty();
            return;
        }
        // Bound index memory independently of output size. Once a stream
        // transition cannot be indexed, stop recording rather than mislabel it.
        if state.spans.len() >= 65_536
            && state.spans.last().is_some_and(|span| span.stream != stream)
        {
            state.truncated = true;
            self.changed.notify_all();
            return;
        }
        let requested = bytes.len().min(SHELL_LOG_LIMIT.saturating_sub(state.bytes));
        let mut reserved = 0;
        let _ = self
            .budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                reserved = requested.min(SESSION_SHELL_LOG_LIMIT.saturating_sub(used));
                Some(used + reserved)
            });
        state.truncated |= reserved < bytes.len();
        let mut written = 0;
        while written < reserved {
            match state.file.write(&bytes[written..reserved]) {
                Ok(0) => {
                    state.error = Some("Shell log write returned zero bytes".into());
                    break;
                }
                Ok(count) => written += count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    state.error = Some(error.to_string());
                    break;
                }
            }
        }
        let start = state.bytes;
        state.bytes += written;
        let end = state.bytes;
        if written != 0 {
            if let Some(span) = state.spans.last_mut().filter(|span| span.stream == stream) {
                span.end = end;
            } else {
                state.spans.push(LogSpan { start, end, stream });
            }
        }
        state.truncated |= written < reserved;
        self.budget.fetch_sub(reserved - written, Ordering::AcqRel);
        self.changed.notify_all();
    }

    /// Wait without polling the producer. Cancellation only cancels this read,
    /// not the job. Offsets and payloads remain identical to immediate reads.
    pub fn wait_events(
        &self,
        offset: usize,
        max_bytes: usize,
        wait: Duration,
        cancel: &AtomicBool,
    ) -> Result<ShellEventPage> {
        let deadline = Instant::now() + wait.min(Duration::from_secs(30));
        let mut state = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if offset > state.bytes {
                bail!(
                    "Shell output offset {offset} is past {} stored bytes",
                    state.bytes
                );
            }
            // Once recording is truncated (or a spool error occurs), no more
            // bytes will be added. Keep long-polling for completion rather
            // than returning empty pages in a tight loop for a live process.
            if offset < state.bytes || state.finished {
                break;
            }
            if cancel.load(Ordering::Acquire) {
                bail!("Shell output wait cancelled; the job is still running");
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let (next, _) = self
                .changed
                .wait_timeout(state, (deadline - now).min(Duration::from_millis(250)))
                .unwrap_or_else(|p| p.into_inner());
            state = next;
        }
        drop(state);
        self.events(offset, max_bytes)
    }

    pub fn events(&self, offset: usize, max_bytes: usize) -> Result<ShellEventPage> {
        let mut state = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if offset > state.bytes {
            bail!(
                "Shell output offset {offset} is past {} stored bytes",
                state.bytes
            );
        }
        let total = state.bytes;
        let end = total.min(offset.saturating_add(max_bytes.clamp(1, 8192)));
        let spans: Vec<_> = state
            .spans
            .iter()
            .filter(|span| span.end > offset && span.start < end)
            .map(|span| (span.start.max(offset), span.end.min(end), span.stream))
            .take(128)
            .collect();
        let result = (|| -> Result<Vec<ShellOutputEvent>> {
            let mut events = Vec::with_capacity(spans.len());
            for (start, end, stream) in spans {
                state.file.seek(SeekFrom::Start(start as u64))?;
                let mut bytes = vec![0; end - start];
                state.file.read_exact(&mut bytes)?;
                events.push(ShellOutputEvent {
                    offset: start,
                    next_offset: end,
                    stream,
                    data_base64: STANDARD.encode(bytes),
                });
            }
            Ok(events)
        })();
        state.file.seek(SeekFrom::Start(total as u64))?;
        let events = result?;
        let next_offset = events.last().map_or(offset, |event| event.next_offset);
        Ok(ShellEventPage {
            events,
            next_offset,
            total_bytes: total,
            finished: state.finished,
            truncated: state.truncated,
        })
    }

    pub fn finish(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .finished = true;
        self.changed.notify_all();
    }

    pub(super) fn incomplete(&self, error: impl ToString) {
        let mut state = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        state.truncated = true;
        state.error.get_or_insert_with(|| error.to_string());
        self.changed.notify_all();
    }

    pub fn snapshot(&self) -> ShellLogSnapshot {
        let state = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        ShellLogSnapshot {
            bytes: state.bytes,
            truncated: state.truncated,
            complete: state.finished && !state.truncated && state.error.is_none(),
            error: state.error.clone(),
        }
    }

    /// Offsets are raw bytes, not characters. A snapshot has a fixed endpoint
    /// even while a command is continuously appending to the transcript.
    pub fn read(&self, offset: usize, max_bytes: usize) -> Result<ShellLogPage> {
        let mut state = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if offset > state.bytes {
            bail!(
                "Shell output offset {offset} is past {} stored bytes",
                state.bytes
            );
        }
        let end = state.bytes;
        let length = max_bytes.clamp(1, 8192).min(end - offset);
        let mut bytes = vec![0; length];
        state.file.seek(SeekFrom::Start(offset as u64))?;
        let result = state.file.read_exact(&mut bytes);
        // Restore the append cursor even on a failed read.
        state.file.seek(SeekFrom::Start(end as u64))?;
        result?;
        // Don't split a valid UTF-8 suffix between pages. Invalid interior
        // bytes are still presented lossily and offsets always count bytes.
        if offset + length < end && length >= 4 {
            for trailing in 1..=3 {
                let start = length - trailing;
                if matches!(bytes[start], 0xc2..=0xf4)
                    && std::str::from_utf8(&bytes[start..])
                        .is_err_and(|error| error.error_len().is_none())
                {
                    bytes.truncate(start);
                    break;
                }
            }
        }
        let next_offset = offset + bytes.len();
        Ok(ShellLogPage {
            content: String::from_utf8_lossy(&bytes).into_owned(),
            offset,
            next_offset,
            total_bytes: end,
            has_more: next_offset < end,
            finished: state.finished,
            truncated: state.truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ephemeral_log_budget_is_released_only_after_last_reader() {
        let budget = Arc::new(AtomicUsize::new(0));
        let log = ShellOutputLog::new_ephemeral(tempfile::tempfile().unwrap(), budget.clone());
        log.push("stdout", b"bytes");
        let reader = log.clone();
        drop(log);
        assert_eq!(budget.load(Ordering::Acquire), 5);
        drop(reader);
        assert_eq!(budget.load(Ordering::Acquire), 0);
        let persistent = ShellOutputLog::new(tempfile::tempfile().unwrap(), budget.clone());
        persistent.push("stdout", b"abc");
        drop(persistent);
        assert_eq!(budget.load(Ordering::Acquire), 3);
    }

    #[test]
    fn event_wait_wakes_for_output_and_finish() {
        let log = ShellOutputLog::new(tempfile::tempfile().unwrap(), Arc::new(AtomicUsize::new(0)));
        let producer = log.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            producer.push("pty", b"hello\r");
            std::thread::sleep(Duration::from_millis(40));
            producer.finish();
        });
        let cancel = AtomicBool::new(false);
        let first = log
            .wait_events(0, 32, Duration::from_secs(2), &cancel)
            .unwrap();
        assert_eq!(first.next_offset, 6);
        assert_eq!(first.events[0].stream, "pty");
        let end = log
            .wait_events(first.next_offset, 32, Duration::from_secs(2), &cancel)
            .unwrap();
        assert!(end.finished);
        assert!(end.events.is_empty());
        thread.join().unwrap();
    }

    #[test]
    fn truncated_output_waits_for_job_completion() {
        let budget = Arc::new(AtomicUsize::new(SESSION_SHELL_LOG_LIMIT));
        let log = ShellOutputLog::new(tempfile::tempfile().unwrap(), budget);
        log.push("pty", b"not retained");
        assert!(log.events(0, 32).unwrap().truncated);
        let producer = log.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            producer.finish();
        });
        let page = log
            .wait_events(0, 32, Duration::from_secs(2), &AtomicBool::new(false))
            .unwrap();
        assert!(page.finished);
        assert!(page.truncated);
        thread.join().unwrap();
    }

    #[test]
    fn transcript_pages_preserve_append_cursor_and_report_budget_loss() {
        let budget = Arc::new(AtomicUsize::new(SESSION_SHELL_LOG_LIMIT - 12));
        let log = ShellOutputLog::new(tempfile::tempfile().unwrap(), budget);
        log.push("stdout", "abc€def".as_bytes());
        let page = log.read(0, 4).unwrap();
        assert_eq!(page.content, "abc");
        assert_eq!(page.next_offset, 3);
        log.push("stderr", b"ghijkl");
        log.finish();
        assert_eq!(log.read(3, 8192).unwrap().content, "€defghi");
        assert_eq!(log.snapshot().bytes, 12);
        assert!(log.snapshot().truncated);
        assert!(!log.snapshot().complete);
    }

    #[test]
    fn event_pages_preserve_streams_and_raw_terminal_bytes() {
        let log = ShellOutputLog::new(tempfile::tempfile().unwrap(), Arc::new(AtomicUsize::new(0)));
        let stdout = "\x1b[31m€\rprogress".as_bytes();
        log.push("stdout", stdout);
        log.push("stderr", b"error\n");
        let mut offset = 0;
        let mut out = Vec::new();
        let mut err = Vec::new();
        loop {
            let page = log.events(offset, 2).unwrap();
            for event in page.events {
                assert_eq!(event.offset, offset);
                let bytes = STANDARD.decode(event.data_base64).unwrap();
                match event.stream {
                    "stdout" => out.extend(bytes),
                    "stderr" => err.extend(bytes),
                    _ => panic!("unexpected stream"),
                }
                offset = event.next_offset;
            }
            if offset == page.total_bytes {
                break;
            }
        }
        assert_eq!(out, stdout);
        assert_eq!(err, b"error\n");
        log.push("stdout", b"later");
        assert_eq!(log.events(offset, 20).unwrap().events[0].stream, "stdout");
    }
}
