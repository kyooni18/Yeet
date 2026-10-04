//! Cross-process store and session file locks.
//!
//! The store lock is shared by session operations and exclusive for
//! store-wide maintenance; each session has its own lock for writers.

use super::*;

pub(super) fn lock_is_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error() == lock_contended_error().raw_os_error()
}

pub(super) struct StoreFileLock {
    file: fs::File,
}

impl StoreFileLock {
    pub(super) fn acquire(path: &Path, exclusive: bool) -> Result<Self> {
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        if exclusive {
            file.lock_exclusive().context("lock session store")?;
        } else {
            file.lock_shared().context("lock session store")?;
        }
        Ok(Self { file })
    }

    pub(super) fn try_acquire_exclusive(path: &Path) -> Result<Option<Self>> {
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { file })),
            Err(error) if lock_is_contended(&error) => Ok(None),
            Err(error) => Err(error).context("lock session store"),
        }
    }
}

impl Drop for StoreFileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
