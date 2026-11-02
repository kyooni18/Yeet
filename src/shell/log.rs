//! Bounded disk-backed shell transcripts, shared by readers and job snapshots.

use std::{
    collections::BTreeMap,
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
    pub has_more: bool,
    pub capture_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_error: Option<String>,
}

/// Decoded model output. Cursors count retained raw bytes, exactly as the raw
/// terminal protocol does; decoding state is reconstructed from the log.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellTextPage {
    pub chunks: Vec<ShellTextChunk>,
    pub offset: usize,
    pub next_offset: usize,
    pub total_bytes: usize,
    pub has_more: bool,
    pub finished: bool,
    pub truncated: bool,
    pub capture_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_error: Option<String>,
}

#[derive(Serialize)]
pub struct ShellTextChunk {
    pub stream: &'static str,
    pub content: String,
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
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
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
        self.wait_available(offset, wait, cancel)?;
        self.events(offset, max_bytes)
    }

    pub fn wait_text(
        &self,
        offset: usize,
        max_bytes: usize,
        wait: Duration,
        cancel: &AtomicBool,
    ) -> Result<ShellTextPage> {
        self.wait_available(offset, wait, cancel)?;
        self.text(offset, max_bytes)
    }

    fn wait_available(&self, offset: usize, wait: Duration, cancel: &AtomicBool) -> Result<()> {
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
        Ok(())
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
            has_more: next_offset < total,
            capture_complete: !state.truncated && state.error.is_none(),
            capture_error: state.error.clone(),
        })
    }

    /// Decode each stream separately, retaining split UTF-8 scalars across
    /// chunks, pages, and interleaved stdout/stderr. At most three earlier bytes
    /// per stream are needed. The bounded span index is scanned, but previous
    /// raw transcript bytes are not reread beyond these decoder prefixes.
    pub fn text(&self, offset: usize, max_bytes: usize) -> Result<ShellTextPage> {
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
        let next_offset = spans.last().map_or(offset, |(_, end, _)| *end);
        let final_page =
            next_offset == total && (state.finished || state.truncated || state.error.is_some());
        let result = (|| -> Result<Vec<ShellTextChunk>> {
            let mut decoders = BTreeMap::new();
            // Include streams absent from this page so a final incomplete scalar
            // can be flushed even if the last bytes belonged to another stream.
            let streams: Vec<_> = state
                .spans
                .iter()
                .map(|span| span.stream)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            for stream in streams {
                decoders.insert(stream, pending_utf8_before(&mut state, stream, offset)?);
            }
            let mut chunks = Vec::new();
            for (start, end, stream) in spans {
                state.file.seek(SeekFrom::Start(start as u64))?;
                let mut bytes = vec![0; end - start];
                state.file.read_exact(&mut bytes)?;
                let content = decode_utf8(decoders.entry(stream).or_default(), &bytes, false);
                append_text_chunk(&mut chunks, stream, content);
            }
            if final_page {
                for (stream, pending) in &mut decoders {
                    append_text_chunk(&mut chunks, stream, decode_utf8(pending, &[], true));
                }
            }
            Ok(chunks)
        })();
        // Reading the prefix and the page shares the append handle.
        state.file.seek(SeekFrom::Start(total as u64))?;
        let chunks = result?;
        Ok(ShellTextPage {
            chunks,
            offset,
            next_offset,
            total_bytes: total,
            has_more: next_offset < total,
            finished: state.finished,
            truncated: state.truncated,
            capture_complete: !state.truncated && state.error.is_none(),
            capture_error: state.error.clone(),
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

fn pending_utf8_before(state: &mut LogState, stream: &str, offset: usize) -> Result<Vec<u8>> {
    let mut remaining = 3;
    let mut ranges = Vec::new();
    for span in state
        .spans
        .iter()
        .rev()
        .filter(|span| span.stream == stream && span.start < offset)
    {
        let end = span.end.min(offset);
        let start = span.start.max(end.saturating_sub(remaining));
        ranges.push((start, end));
        remaining -= end - start;
        if remaining == 0 {
            break;
        }
    }
    let mut tail = Vec::with_capacity(3);
    for (start, end) in ranges.into_iter().rev() {
        state.file.seek(SeekFrom::Start(start as u64))?;
        let mut bytes = vec![0; end - start];
        state.file.read_exact(&mut bytes)?;
        tail.extend_from_slice(&bytes);
    }
    for start in 0..tail.len() {
        if matches!(tail[start], 0xc2..=0xf4)
            && std::str::from_utf8(&tail[start..])
                .is_err_and(|error| error.valid_up_to() == 0 && error.error_len().is_none())
        {
            return Ok(tail[start..].to_vec());
        }
    }
    Ok(Vec::new())
}

fn decode_utf8(pending: &mut Vec<u8>, bytes: &[u8], finished: bool) -> String {
    pending.extend_from_slice(bytes);
    let mut content = String::new();
    let mut consumed = 0;
    while consumed < pending.len() {
        match std::str::from_utf8(&pending[consumed..]) {
            Ok(text) => {
                content.push_str(text);
                consumed = pending.len();
            }
            Err(error) => {
                let valid_end = consumed + error.valid_up_to();
                content.push_str(std::str::from_utf8(&pending[consumed..valid_end]).unwrap());
                consumed = valid_end;
                match error.error_len() {
                    Some(length) => {
                        content.push('\u{fffd}');
                        consumed += length;
                    }
                    None => break,
                }
            }
        }
    }
    pending.drain(..consumed);
    if finished && !pending.is_empty() {
        content.push('\u{fffd}');
        pending.clear();
    }
    content
}

fn append_text_chunk(chunks: &mut Vec<ShellTextChunk>, stream: &'static str, content: String) {
    if content.is_empty() {
        return;
    }
    if let Some(last) = chunks.last_mut().filter(|chunk| chunk.stream == stream) {
        last.content.push_str(&content);
    } else {
        chunks.push(ShellTextChunk { stream, content });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_pages_decode_split_utf8_per_stream_with_one_byte_cursors() {
        let log = ShellOutputLog::new(tempfile::tempfile().unwrap(), Arc::new(AtomicUsize::new(0)));
        log.push("stdout", b"first \xe2");
        log.push("stderr", b"error \xf0\x9f");
        log.push("stdout", b"\x82\xac\n");
        log.push("stderr", b"\x98\x80\n");
        log.finish();
        let mut offset = 0;
        let mut streams: BTreeMap<&str, String> = BTreeMap::new();
        loop {
            let page = log.text(offset, 1).unwrap();
            assert!(page.capture_complete);
            assert!(page.next_offset > offset);
            for chunk in page.chunks {
                streams
                    .entry(chunk.stream)
                    .or_default()
                    .push_str(&chunk.content);
            }
            offset = page.next_offset;
            if !page.has_more {
                break;
            }
        }
        assert_eq!(streams["stdout"], "first €\n");
        assert_eq!(streams["stderr"], "error 😀\n");
        assert!(log.text(offset + 1, 4).is_err());
    }

    #[test]
    fn text_pages_defer_incomplete_live_scalars_and_preserve_append_cursor() {
        let log = ShellOutputLog::new(tempfile::tempfile().unwrap(), Arc::new(AtomicUsize::new(0)));
        log.push("stdout", b"\xe2");
        let first = log.text(0, 1).unwrap();
        assert!(first.chunks.is_empty());
        assert!(first.capture_complete);
        assert!(!first.finished);
        log.push("stderr", b"x");
        log.push("stdout", b"\x82\xac");
        log.finish();
        let last = log.text(first.next_offset, 8).unwrap();
        assert_eq!(last.chunks[0].stream, "stderr");
        assert_eq!(last.chunks[0].content, "x");
        assert_eq!(last.chunks[1].content, "€");
        assert_eq!(last.next_offset, 4);
        assert!(last.capture_complete);
    }

    #[test]
    fn text_page_reports_lost_capture_and_flushes_terminal_incomplete_bytes() {
        let log = ShellOutputLog::new(tempfile::tempfile().unwrap(), Arc::new(AtomicUsize::new(0)));
        log.push("stdout", b"ok\xe2\x82");
        log.incomplete("pipe abandoned");
        log.finish();
        let page = log.text(0, 8192).unwrap();
        assert_eq!(page.chunks[0].content, "ok\u{fffd}");
        assert!(page.truncated);
        assert!(!page.capture_complete);
        assert_eq!(page.capture_error.as_deref(), Some("pipe abandoned"));
        assert!(!page.has_more);
    }

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
