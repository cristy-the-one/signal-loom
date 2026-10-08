use crate::error::{Error, Result};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Shared with the UI while a path is indexed. The scan checks `cancel`
/// every few dozen records and publishes how far the file has been read.
#[derive(Debug, Default)]
pub struct IndexControl {
    cancel: AtomicBool,
    bytes_done: AtomicU64,
    bytes_total: AtomicU64,
    frames: AtomicU64,
    skipped: AtomicU64,
}

impl IndexControl {
    pub fn reset(&self, bytes_total: u64) {
        self.cancel.store(false, Ordering::Relaxed);
        self.bytes_done.store(0, Ordering::Relaxed);
        self.bytes_total.store(bytes_total, Ordering::Relaxed);
        self.frames.store(0, Ordering::Relaxed);
        self.skipped.store(0, Ordering::Relaxed);
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn set_total(&self, bytes_total: u64) {
        self.bytes_total.store(bytes_total, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.bytes_done.load(Ordering::Relaxed),
            self.bytes_total.load(Ordering::Relaxed),
            self.frames.load(Ordering::Relaxed),
            self.skipped.load(Ordering::Relaxed),
        )
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Publish progress, and fail if a cancel was requested.
    pub fn observe(&self, bytes_done: u64, frames: u64, skipped: u64) -> Result<()> {
        self.bytes_done.store(bytes_done, Ordering::Relaxed);
        self.frames.store(frames, Ordering::Relaxed);
        self.skipped.store(skipped, Ordering::Relaxed);
        if self.is_cancelled() {
            Err(Error::cancelled("indexing cancelled"))
        } else {
            Ok(())
        }
    }
}

pub(super) fn pulse(
    control: Option<&IndexControl>,
    bytes_done: u64,
    frames: u64,
    skipped: u64,
) -> Result<()> {
    match control {
        Some(control) => control.observe(bytes_done, frames, skipped),
        None => Ok(()),
    }
}
