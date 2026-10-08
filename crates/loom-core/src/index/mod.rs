//! The sparse time index over one log, and every read of it.
//!
//! `build` scans a log once and keeps checkpoints. Every read goes through
//! `replay`: seek to the checkpoint before the window, rebuild what each signal
//! held, and stop after the window. The read modules (`query`, `window`,
//! `export`, `navigate`, `bus_load`) each state only what they do with the records.

mod build;
mod bus_load;
mod checkpoint;
mod checksum;
mod control;
mod export;
mod integrity;
mod navigate;
mod query;
mod replay;
mod signal;
mod source;
mod time;
mod window;

#[cfg(test)]
mod testing;

use crate::error::Result;
use crate::map::{SignalMap, TimeoutFactor};
use crate::scan::{sniff, LogFormat};
use checkpoint::Checkpoint;
use signal::SignalMeta;
use source::{sniff_path, Source};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

#[cfg(test)]
pub(crate) use checksum::ChecksumAlgo;
pub use control::IndexControl;
pub use export::Export;
pub(crate) use export::{csv_header, truncation_note, EXPORT_ROW_CAP};
pub(crate) use query::{decimate_points, QueryWindow, Series};

/// Sparse time index. Sample payloads stay in the file; a query seeks to the
/// nearest checkpoint and downsamples that window.
pub struct IndexedLog {
    source: Source,
    /// Format shown to the user. A BLF stays a BLF; containers are inflated one at a time.
    format: LogFormat,
    /// Format the scanner reads on resume. Same as `format`.
    body: LogFormat,
    checkpoints: Vec<Checkpoint>,
    snapshots: Vec<Vec<Option<f64>>>,
    frame_count: u64,
    event_count: u64,
    events_truncated: bool,
    t_start_us: u64,
    t_end_us: u64,
    byte_len: u64,
    events: Vec<(u64, String)>,
    skipped: u64,
    warnings: Vec<String>,
    signals: Vec<SignalMeta>,
    name_index: HashMap<String, usize>,
    msg_index: HashMap<u32, Vec<usize>>,
    message_names: HashMap<u32, String>,
    /// Every CAN id that has at least one frame in the log.
    seen_ids: HashSet<u32>,
}

impl IndexedLog {
    /// Index with the default timeout factor.
    #[cfg(test)]
    pub fn open_path(path: &Path, map: Option<&SignalMap>) -> Result<Self> {
        Self::open_path_controlled(path, map, None)
    }

    #[cfg(test)]
    pub fn open_path_controlled(
        path: &Path,
        map: Option<&SignalMap>,
        control: Option<&IndexControl>,
    ) -> Result<Self> {
        Self::open_path_timed(path, map, TimeoutFactor::default(), control)
    }

    #[cfg(test)]
    pub fn open_bytes(bytes: Vec<u8>, map: Option<&SignalMap>) -> Result<Self> {
        Self::open_bytes_timed(bytes, map, TimeoutFactor::default())
    }

    /// Index `path`, marking a message late after `timeout` of its cycles.
    pub fn open_path_timed(
        path: &Path,
        map: Option<&SignalMap>,
        timeout: TimeoutFactor,
        control: Option<&IndexControl>,
    ) -> Result<Self> {
        let format = sniff_path(path)?;
        Self::build(
            Source::Path(path.to_path_buf()),
            format,
            map,
            timeout,
            control,
        )
    }

    pub fn open_bytes_timed(
        bytes: Vec<u8>,
        map: Option<&SignalMap>,
        timeout: TimeoutFactor,
    ) -> Result<Self> {
        let format = sniff(&bytes)?;
        Self::build(Source::Memory(Arc::new(bytes)), format, map, timeout, None)
    }

    pub fn open_shared(
        bytes: Arc<Vec<u8>>,
        map: Option<&SignalMap>,
        timeout: TimeoutFactor,
    ) -> Result<Self> {
        let format = sniff(bytes.as_slice())?;
        Self::build(Source::Memory(bytes), format, map, timeout, None)
    }

    pub fn path(&self) -> Option<&Path> {
        match &self.source {
            Source::Path(path) => Some(path),
            Source::Memory(_) => None,
        }
    }

    pub fn shared_bytes(&self) -> Option<Arc<Vec<u8>>> {
        match &self.source {
            Source::Memory(bytes) => Some(Arc::clone(bytes)),
            Source::Path(_) => None,
        }
    }

    pub fn format(&self) -> LogFormat {
        self.format
    }

    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    /// Whether any frame in the log carries this CAN id.
    pub fn carries_id(&self, id: u32) -> bool {
        self.seen_ids.contains(&id)
    }

    pub fn event_count(&self) -> u64 {
        self.event_count
    }

    pub fn events_truncated(&self) -> bool {
        self.events_truncated
    }

    pub fn t_start_us(&self) -> u64 {
        self.t_start_us
    }

    pub fn t_end_us(&self) -> u64 {
        self.t_end_us
    }

    pub fn byte_len(&self) -> u64 {
        self.byte_len
    }

    pub fn checkpoint_count(&self) -> usize {
        self.checkpoints.len()
    }

    pub fn events(&self) -> &[(u64, String)] {
        &self.events
    }

    pub fn skipped(&self) -> u64 {
        self.skipped
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}
