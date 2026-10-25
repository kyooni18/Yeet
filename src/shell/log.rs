//! Bounded disk-backed shell transcripts, shared by readers and job snapshots.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Result, bail};
use serde::Serialize;

pub const SHELL_LOG_LIMIT: usize = 64 * 1024 * 1024;
pub const SESSION_SHELL_LOG_LIMIT: usize = 512 * 1024 * 1024;

#[derive(Clone)]
pub struct ShellOutputLog {
    inner: Arc<Mutex<LogState>>,
    budget: Arc<AtomicUsize>,
}

struct LogState {
    file: File,
    bytes: usize,
    truncated: bool,
    error: Option<String>,
    finished: bool,
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
        Self {
            inner: Arc::new(Mutex::new(LogState {
                file,
                bytes: 0,
                truncated: false,
                error: None,
                finished: false,
            })),
            budget,
        }
    }

    // Failure to spool must never stop draining the child's pipes: that can
    // deadlock the child. Surface loss in metadata while retaining live tails.
    pub(super) fn push(&self, bytes: &[u8]) {
        let mut state = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if state.finished || state.error.is_some() {
            state.truncated |= !bytes.is_empty();
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
        state.bytes += written;
        state.truncated |= written < reserved;
        self.budget.fetch_sub(reserved - written, Ordering::AcqRel);
    }

    pub fn finish(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .finished = true;
    }

    pub(super) fn incomplete(&self, error: impl ToString) {
        let mut state = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        state.truncated = true;
        state.error.get_or_insert_with(|| error.to_string());
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
    fn transcript_pages_preserve_append_cursor_and_report_budget_loss() {
        let budget = Arc::new(AtomicUsize::new(SESSION_SHELL_LOG_LIMIT - 12));
        let log = ShellOutputLog::new(tempfile::tempfile().unwrap(), budget);
        log.push("abc€def".as_bytes());
        let page = log.read(0, 4).unwrap();
        assert_eq!(page.content, "abc");
        assert_eq!(page.next_offset, 3);
        log.push(b"ghijkl");
        log.finish();
        assert_eq!(log.read(3, 8192).unwrap().content, "€defghi");
        assert_eq!(log.snapshot().bytes, 12);
        assert!(log.snapshot().truncated);
        assert!(!log.snapshot().complete);
    }
}
