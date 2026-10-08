use crate::decode::{DecodeSpec, Endian};
use crate::error::{Error, Result};
use crate::map::{SignalMap, TimeoutFactor};
use crate::project::TriggerOp;
use crate::scan::{hex_payload, sniff, FrameData, LogFormat, ReadSeek, Rec, RecKind, Scanner};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

const CHECKPOINT_EVERY: u64 = 256;
const MAX_CHECKPOINTS: usize = 4_096;
const MAX_EVENTS: usize = 5_000;
const MAX_WARNINGS: usize = 32;
const MAX_QUERY_POINTS: usize = 8_000;
/// Full-resolution reads for statistics and export. 16 bytes per sample.
const MAX_WINDOW_SAMPLES: usize = 4_000_000;
/// Most data rows one export writes. A longer window is cut here, and the
/// export says so.
pub(crate) const EXPORT_ROW_CAP: usize = 500_000;

/// One export: its text, how many data rows it holds, and whether the row cap
/// cut it short. A cut export also ends with a `#` line saying so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub text: String,
    pub rows: usize,
    pub truncated: bool,
}

pub(crate) fn truncation_note(cap: usize) -> String {
    format!("# Truncated by Signal Loom at {cap} rows. Narrow the window to export the rest.\n")
}

/// The first line of a wide CSV export: `t_us`, then one column per name. A
/// name holding a comma, a double quote, CR or LF is quoted, and its quotes
/// doubled (RFC 4180).
pub(crate) fn csv_header<'a>(columns: impl IntoIterator<Item = &'a str>) -> String {
    let mut line = String::from("t_us");
    for name in columns {
        line.push(',');
        if name.contains([',', '"', '\r', '\n']) {
            line.push('"');
            line.push_str(&name.replace('"', "\"\""));
            line.push('"');
        } else {
            line.push_str(name);
        }
    }
    line.push('\n');
    line
}

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

    fn observe(&self, bytes_done: u64, frames: u64, skipped: u64) -> Result<()> {
        self.bytes_done.store(bytes_done, Ordering::Relaxed);
        self.frames.store(frames, Ordering::Relaxed);
        self.skipped.store(skipped, Ordering::Relaxed);
        if self.cancel.load(Ordering::Relaxed) {
            Err(Error::msg("indexing cancelled"))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone)]
enum Source {
    Path(PathBuf),
    Memory(Arc<Vec<u8>>),
}

impl Source {
    fn byte_len(&self) -> Result<u64> {
        match self {
            Self::Path(path) => std::fs::metadata(path)
                .map(|meta| meta.len())
                .map_err(|err| Error::read(path, err)),
            Self::Memory(bytes) => Ok(bytes.len() as u64),
        }
    }

    fn with_reader<T>(&self, body: impl FnOnce(&mut dyn ReadSeek) -> Result<T>) -> Result<T> {
        match self {
            Self::Path(path) => {
                let mut file = File::open(path).map_err(|err| Error::read(path, err))?;
                body(&mut file)
            }
            Self::Memory(bytes) => {
                let mut cursor = Cursor::new(bytes.as_slice());
                body(&mut cursor)
            }
        }
    }
}

/// What a signal does for frame integrity, decided once from its name and layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Integrity {
    None,
    /// An 8-bit signal that is exactly one payload byte.
    Checksum {
        byte: usize,
    },
    /// A rolling counter of `bits` raw bits.
    Counter {
        bits: u16,
    },
}

impl Integrity {
    fn classify(name: &str, spec: &DecodeSpec) -> Self {
        let name = name.to_ascii_lowercase();
        if name.contains("checksum") && spec.bit_length == 8 {
            let aligned = match spec.endian {
                Endian::Little => spec.start_bit.is_multiple_of(8),
                Endian::Big => spec.start_bit % 8 == 7,
            };
            if aligned {
                return Self::Checksum {
                    byte: usize::from(spec.start_bit / 8),
                };
            }
        }
        if name.contains("counter") {
            return Self::Counter {
                bits: spec.bit_length,
            };
        }
        Self::None
    }
}

#[derive(Clone)]
struct SignalMeta {
    name: String,
    unit: String,
    message_name: String,
    message_id: Option<u32>,
    spec: Option<DecodeSpec>,
    min: Option<f64>,
    max: Option<f64>,
    channel: u8,
    mux_switch: bool,
    mux_value: Option<u32>,
    table: Vec<(i64, String)>,
    integrity: Integrity,
}

#[derive(Clone)]
struct Checkpoint {
    t_us: u64,
    offset: u64,
    frame_ordinal: u64,
}

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

#[derive(Debug, Clone)]
pub struct SignalInfo {
    pub name: String,
    pub unit: String,
    pub message_name: String,
    pub message_id: Option<u32>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// The decoder's scale factor: the smallest change the value can show.
    pub step: Option<f64>,
    pub from_map: bool,
}

#[derive(Debug, Clone)]
pub struct Series {
    pub name: String,
    pub unit: String,
    pub points: Vec<(u64, f64)>,
}

#[derive(Debug, Clone)]
pub struct HeldValue {
    pub name: String,
    pub unit: String,
    pub value: f64,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FrameHit {
    pub t_us: u64,
    pub ordinal: u64,
    pub message_id: Option<u32>,
    pub message_name: String,
    pub extended: bool,
    pub dlc: u8,
    pub data_hex: String,
}

pub struct QueryWindow {
    pub t0_us: u64,
    pub t1_us: u64,
    pub signals: Vec<String>,
    pub max_points: usize,
}

struct Built {
    checkpoints: Vec<Checkpoint>,
    snapshots: Vec<Vec<Option<f64>>>,
    frame_count: u64,
    event_count: u64,
    events_truncated: bool,
    t_start_us: u64,
    t_end_us: u64,
    events: Vec<(u64, String)>,
    skipped: u64,
    /// Records kept at the previous time after a small step back.
    reordered: u64,
    warnings: Vec<String>,
    signals: Vec<SignalMeta>,
    name_index: HashMap<String, usize>,
    msg_index: HashMap<u32, Vec<usize>>,
    message_names: HashMap<u32, String>,
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

    pub fn signals(&self) -> impl Iterator<Item = SignalInfo> + '_ {
        self.signals.iter().map(SignalMeta::info)
    }

    pub fn query(&self, query: &QueryWindow) -> Result<Vec<Series>> {
        if self.checkpoints.is_empty() {
            return Ok(Vec::new());
        }
        let (t0, t1) = ordered_range(query.t0_us, query.t1_us);
        let max_points = query.max_points.clamp(2, MAX_QUERY_POINTS);
        let wanted: Vec<usize> = if query.signals.is_empty() {
            (0..self.signals.len()).collect()
        } else {
            query
                .signals
                .iter()
                .filter_map(|name| self.name_index.get(name).copied())
                .collect()
        };
        let mut buckets: Vec<Vec<Option<Bucket>>> = vec![Vec::new(); wanted.len()];
        let mut lead = vec![None; wanted.len()];
        let mut slot_of: Vec<Option<usize>> = vec![None; self.signals.len()];
        for (slot, &signal) in wanted.iter().enumerate() {
            slot_of[signal] = Some(slot);
        }

        let idx = self.floor_checkpoint(t0);
        let mut held = self.snapshot(idx);
        let mut seeded = false;
        self.scan_from(idx, |rec| {
            if rec.t_us < t0 {
                self.touch(rec, &mut held, |_, _| {});
                return true;
            }
            if !seeded {
                capture_lead(&held, &wanted, &mut lead);
                seeded = true;
            }
            if rec.t_us > t1 {
                return false;
            }
            self.touch(rec, &mut held, |signal, value| {
                if let Some(slot) = slot_of.get(signal).copied().flatten() {
                    let row = &mut buckets[slot];
                    if row.is_empty() {
                        row.resize(bucket_count(max_points, lead[slot].is_some()), None);
                    }
                    push_bucket(row, t0, t1, rec.t_us, value);
                }
            });
            true
        })?;
        if !seeded {
            capture_lead(&held, &wanted, &mut lead);
        }

        let mut series = Vec::with_capacity(wanted.len());
        for (slot, &signal_i) in wanted.iter().enumerate() {
            let signal = &self.signals[signal_i];
            let mut points = Vec::new();
            for bucket in buckets[slot].iter().flatten() {
                bucket.emit(&mut points);
            }
            if let Some(value) = lead[slot] {
                let replace = match points.first() {
                    None => true,
                    Some((t, existing)) => {
                        *t > t0 || (*t == t0 && (*existing - value).abs() > 1e-9)
                    }
                };
                if replace {
                    // Only a budget of two forces a choice: the lead and one bucket point.
                    // The earlier point gives way so the series still ends on the latest one.
                    let excess = (points.len() + 1).saturating_sub(max_points);
                    points.drain(..excess);
                    points.insert(0, (t0, value));
                }
            }
            points.truncate(max_points);
            series.push(Series {
                name: signal.name.clone(),
                unit: signal.unit.clone(),
                points,
            });
        }
        Ok(series)
    }

    pub fn values_at(&self, t_us: u64) -> Result<Vec<HeldValue>> {
        if self.checkpoints.is_empty() {
            return Ok(Vec::new());
        }
        let idx = self.floor_checkpoint(t_us);
        let mut held = self.snapshot(idx);
        self.scan_from(idx, |rec| {
            if rec.t_us > t_us {
                return false;
            }
            self.touch(rec, &mut held, |_, _| {});
            true
        })?;
        Ok(self
            .signals
            .iter()
            .enumerate()
            .filter_map(|(i, signal)| {
                held.get(i).copied().flatten().map(|value| HeldValue {
                    name: signal.name.clone(),
                    unit: signal.unit.clone(),
                    value,
                    label: signal
                        .spec
                        .and_then(|spec| spec.raw_of(value))
                        .and_then(|raw| {
                            signal
                                .table
                                .iter()
                                .find_map(|(key, text)| (*key == raw).then(|| text.clone()))
                        }),
                })
            })
            .collect())
    }

    pub fn step_frame(&self, t_us: u64, next: bool) -> Result<Option<FrameHit>> {
        if self.checkpoints.is_empty() {
            return Ok(None);
        }
        if next {
            let idx = self.floor_checkpoint(t_us);
            let mut ordinal = self.checkpoints[idx].frame_ordinal;
            let mut found = None;
            self.scan_from(idx, |rec| {
                let current = ordinal;
                if is_timed_sample(rec) {
                    ordinal += 1;
                    if rec.t_us > t_us {
                        found = Some(self.hit_from(rec, current));
                        return false;
                    }
                }
                true
            })?;
            return Ok(found);
        }

        let mut idx = self.floor_checkpoint(t_us);
        loop {
            let mut ordinal = self.checkpoints[idx].frame_ordinal;
            let mut last = None;
            self.scan_from(idx, |rec| {
                if rec.t_us >= t_us {
                    return false;
                }
                if is_timed_sample(rec) {
                    last = Some(self.hit_from(rec, ordinal));
                    ordinal += 1;
                }
                true
            })?;
            if last.is_some() {
                return Ok(last);
            }
            if idx == 0 {
                return Ok(None);
            }
            idx -= 1;
        }
    }

    fn build(
        source: Source,
        format: LogFormat,
        map: Option<&SignalMap>,
        timeout: TimeoutFactor,
        control: Option<&IndexControl>,
    ) -> Result<Self> {
        let byte_len = source.byte_len()?;
        if let Some(control) = control {
            control.set_total(byte_len);
        }
        let scan_source = source.clone();
        let built = if format == LogFormat::DecodedCsv {
            scan_decoded(&scan_source, control)?
        } else {
            scan_framed(&scan_source, format, map, timeout, control)?
        };
        if built.frame_count == 0 {
            return Err(Error::msg(
                "log has no frames. Signal Loom needs at least one sample row.",
            ));
        }
        Ok(Self {
            source,
            format,
            body: format,
            checkpoints: built.checkpoints,
            snapshots: built.snapshots,
            frame_count: built.frame_count,
            event_count: built.event_count,
            events_truncated: built.events_truncated,
            t_start_us: built.t_start_us,
            t_end_us: built.t_end_us,
            byte_len,
            events: built.events,
            skipped: built.skipped,
            warnings: built.warnings,
            signals: built.signals,
            name_index: built.name_index,
            msg_index: built.msg_index,
            message_names: built.message_names,
            seen_ids: built.seen_ids,
        })
    }

    fn floor_checkpoint(&self, t_us: u64) -> usize {
        let mut lo = 0usize;
        let mut hi = self.checkpoints.len();
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if self.checkpoints[mid].t_us <= t_us {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    fn snapshot(&self, idx: usize) -> Vec<Option<f64>> {
        let mut held = self.snapshots.get(idx).cloned().unwrap_or_default();
        held.resize(self.signals.len(), None);
        held
    }

    /// Replay from checkpoint `idx`. Times pass through the same `order_time`
    /// the build used, seeded with the checkpoint's time, so a replay sees the
    /// timestamps the index stored.
    fn scan_from(&self, idx: usize, mut visit: impl FnMut(&Rec) -> bool) -> Result<()> {
        let checkpoint = &self.checkpoints[idx];
        self.source.with_reader(|reader| {
            let mut scanner =
                Scanner::resume_at(reader, self.body, checkpoint.offset, checkpoint.t_us)?;
            let mut last = Some(checkpoint.t_us);
            while let Some(mut rec) = scanner.next_rec()? {
                match order_time(&mut last, rec.t_us) {
                    TimeOrder::InOrder(t_us) | TimeOrder::Clamped(t_us) => rec.t_us = t_us,
                    TimeOrder::Skipped => continue,
                }
                if !visit(&rec) {
                    break;
                }
            }
            Ok(())
        })
    }

    fn touch(&self, rec: &Rec, held: &mut Vec<Option<f64>>, mut on_update: impl FnMut(usize, f64)) {
        if held.len() < self.signals.len() {
            held.resize(self.signals.len(), None);
        }
        match &rec.kind {
            RecKind::Frame {
                id,
                dlc,
                data,
                channel,
                ..
            } => {
                if let Some(indices) = self.msg_index.get(id) {
                    decode_frame(
                        &self.signals,
                        indices,
                        *channel,
                        *dlc,
                        data,
                        |idx, value| {
                            held[idx] = Some(value);
                            on_update(idx, value);
                        },
                    );
                }
            }
            RecKind::Sample { name, value, .. } => {
                if let Some(&idx) = self.name_index.get(name) {
                    held[idx] = Some(*value);
                    on_update(idx, *value);
                }
            }
            RecKind::Event { .. } => {}
        }
    }

    pub fn bus_load(&self, t0_us: u64, t1_us: u64) -> Result<crate::dto::BusLoad> {
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let mut frames = 0u64;
        let mut bits = 0u64;
        self.scan_from(self.floor_checkpoint(t0), |rec| {
            if rec.t_us > t1 {
                return false;
            }
            if rec.t_us >= t0 {
                if let RecKind::Frame { dlc, .. } = rec.kind {
                    frames += 1;
                    bits += 47 + u64::from(dlc.min(8)) * 8;
                }
            }
            true
        })?;
        let dt = ((t1.saturating_sub(t0)) as f64 / 1_000_000.0).max(1.0e-6);
        Ok(crate::dto::BusLoad {
            frames,
            rate: frames as f64 / dt,
            load: (bits as f64 / dt) / 500_000.0,
        })
    }

    /// Every sample of `names` in the window, each series led by the value held
    /// at `t0`. Unlike `query`, nothing is bucketed, so statistics and exports
    /// see each sample. A window over `MAX_WINDOW_SAMPLES` is refused, not cut.
    pub fn samples(&self, names: &[String], t0_us: u64, t1_us: u64) -> Result<Vec<Series>> {
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let mut wanted = Vec::with_capacity(names.len());
        let mut series = Vec::with_capacity(names.len());
        for name in names {
            let idx = self
                .name_index
                .get(name)
                .copied()
                .ok_or_else(|| Error::msg(format!("no signal named {name}")))?;
            wanted.push(idx);
            series.push(Series {
                name: name.clone(),
                unit: self.signals[idx].unit.clone(),
                points: Vec::new(),
            });
        }
        if self.checkpoints.is_empty() {
            return Ok(series);
        }
        let start = self.floor_checkpoint(t0);
        let mut held = self.snapshot(start);
        let mut seeded = false;
        let mut total = 0usize;
        let seed = |held: &[Option<f64>], series: &mut [Series]| {
            for (out, idx) in series.iter_mut().zip(&wanted) {
                if let Some(value) = held.get(*idx).copied().flatten() {
                    out.points.push((t0, value));
                }
            }
        };
        self.scan_from(start, |rec| {
            if rec.t_us > t1 {
                return false;
            }
            if !seeded && rec.t_us >= t0 {
                seed(&held, &mut series);
                seeded = true;
            }
            self.touch(rec, &mut held, |signal, value| {
                if rec.t_us < t0 {
                    return;
                }
                if let Some(pos) = wanted.iter().position(|&idx| idx == signal) {
                    let points = &mut series[pos].points;
                    if points.last().is_some_and(|(t, _)| *t == rec.t_us) {
                        points.pop();
                    }
                    points.push((rec.t_us, value));
                    total += 1;
                }
            });
            total <= MAX_WINDOW_SAMPLES
        })?;
        if total > MAX_WINDOW_SAMPLES {
            return Err(Error::msg(format!(
                "that window holds more than {MAX_WINDOW_SAMPLES} samples. Narrow it and try again"
            )));
        }
        if !seeded {
            seed(&held, &mut series);
        }
        Ok(series)
    }

    pub fn stats(&self, name: &str, t0_us: u64, t1_us: u64) -> Result<crate::dto::WindowStats> {
        let idx = self
            .name_index
            .get(name)
            .copied()
            .ok_or_else(|| Error::msg(format!("no signal named {name}")))?;
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let mut count = 0u64;
        let mut sum = 0.0f64;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let mut first = None;
        let mut last = None;
        let mut held = self.snapshot(self.floor_checkpoint(t0));
        self.scan_from(self.floor_checkpoint(t0), |rec| {
            if rec.t_us > t1 {
                return false;
            }
            self.touch(rec, &mut held, |signal, value| {
                if signal == idx && rec.t_us >= t0 && rec.t_us <= t1 {
                    count += 1;
                    sum += value;
                    min = min.min(value);
                    max = max.max(value);
                    if first.is_none() {
                        first = Some(value);
                    }
                    last = Some(value);
                }
            });
            true
        })?;
        if count == 0 {
            return Err(Error::msg(format!("no samples of {name} in that window")));
        }
        Ok(crate::dto::WindowStats {
            count,
            min,
            max,
            avg: sum / count as f64,
            first: first.unwrap_or(0.0),
            last: last.unwrap_or(0.0),
        })
    }

    pub fn export_csv(&self, names: &[String], t0_us: u64, t1_us: u64) -> Result<Export> {
        self.export_csv_capped(names, t0_us, t1_us, EXPORT_ROW_CAP)
    }

    fn export_csv_capped(
        &self,
        names: &[String],
        t0_us: u64,
        t1_us: u64,
        cap: usize,
    ) -> Result<Export> {
        if names.is_empty() {
            return Err(Error::msg("export needs at least one signal"));
        }
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let mut indexes = Vec::new();
        for name in names {
            let idx = self
                .name_index
                .get(name)
                .copied()
                .ok_or_else(|| Error::msg(format!("no signal named {name}")))?;
            indexes.push(idx);
        }
        let mut out = csv_header(names.iter().map(String::as_str));
        let mut rows = 0usize;
        let mut truncated = false;
        let mut held = self.snapshot(self.floor_checkpoint(t0));
        let mut dirty = false;
        self.scan_from(self.floor_checkpoint(t0), |rec| {
            if rec.t_us > t1 {
                return false;
            }
            dirty = false;
            self.touch(rec, &mut held, |signal, _value| {
                if indexes.contains(&signal) && rec.t_us >= t0 {
                    dirty = true;
                }
            });
            if dirty && rec.t_us >= t0 {
                if rows == cap {
                    truncated = true;
                    return false;
                }
                out.push_str(&rec.t_us.to_string());
                for idx in &indexes {
                    out.push(',');
                    if let Some(value) = held.get(*idx).copied().flatten() {
                        out.push_str(&format!("{value:.6}"));
                    }
                }
                out.push('\n');
                rows += 1;
            }
            true
        })?;
        if rows == 0 {
            return Err(Error::msg("that window has no samples to export"));
        }
        if truncated {
            out.push_str(&truncation_note(cap));
        }
        Ok(Export {
            text: out,
            rows,
            truncated,
        })
    }

    pub fn export_slog(&self, t0_us: u64, t1_us: u64) -> Result<Export> {
        self.export_slog_capped(t0_us, t1_us, EXPORT_ROW_CAP)
    }

    fn export_slog_capped(&self, t0_us: u64, t1_us: u64, cap: usize) -> Result<Export> {
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let mut out = String::from(
            "SLOGv1\n# Trimmed by Signal Loom. Synthetic or captured, this is only the selected window.\n",
        );
        let mut rows = 0usize;
        let mut truncated = false;
        self.scan_from(self.floor_checkpoint(t0), |rec| {
            if rec.t_us > t1 {
                return false;
            }
            if rec.t_us < t0 {
                return true;
            }
            let line = match &rec.kind {
                RecKind::Frame { id, dlc, data, .. } => {
                    format!("F {} {id:X} {}\n", rec.t_us, hex_payload(data, *dlc))
                }
                RecKind::Event { label } => format!("E {} {label}\n", rec.t_us),
                RecKind::Sample { .. } => return true,
            };
            if rows == cap {
                truncated = true;
                return false;
            }
            out.push_str(&line);
            rows += 1;
            true
        })?;
        if rows == 0 {
            return Err(Error::msg("that window has no frames to export"));
        }
        if truncated {
            out.push_str(&truncation_note(cap));
        }
        Ok(Export {
            text: out,
            rows,
            truncated,
        })
    }

    /// Whether `name` is a decoded signal of this log.
    pub fn has_signal(&self, name: &str) -> bool {
        self.name_index.contains_key(name)
    }

    /// Times where `name` first satisfies `op` against `level`, until it stops
    /// doing so. At most 200 hits.
    pub fn crossings(&self, name: &str, op: TriggerOp, level: f64) -> Result<Vec<(u64, String)>> {
        let idx = self
            .name_index
            .get(name)
            .copied()
            .ok_or_else(|| Error::msg(format!("no signal named {name}")))?;
        let label = format!("Trigger {name} {} {level}", op.symbol());
        let mut events = Vec::new();
        let mut armed = true;
        let mut held = self.snapshot(0);
        self.scan_from(0, |rec| {
            self.touch(rec, &mut held, |signal, value| {
                if signal != idx {
                    return;
                }
                let hot = op.holds(value, level);
                if hot && armed {
                    events.push((rec.t_us, label.clone()));
                    armed = false;
                } else if !hot {
                    armed = true;
                }
            });
            events.len() < 200
        })?;
        Ok(events)
    }

    fn hit_from(&self, rec: &Rec, ordinal: u64) -> FrameHit {
        match &rec.kind {
            RecKind::Frame {
                id,
                dlc,
                data,
                extended,
                ..
            } => FrameHit {
                t_us: rec.t_us,
                ordinal,
                message_id: Some(*id),
                message_name: self
                    .message_names
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| format!("0x{id:X}")),
                extended: *extended,
                dlc: *dlc,
                data_hex: hex_payload(data, *dlc),
            },
            RecKind::Sample { name, .. } => FrameHit {
                t_us: rec.t_us,
                ordinal,
                message_id: None,
                message_name: name.clone(),
                extended: false,
                dlc: 0,
                data_hex: String::new(),
            },
            RecKind::Event { label } => FrameHit {
                t_us: rec.t_us,
                ordinal,
                message_id: None,
                message_name: label.clone(),
                extended: false,
                dlc: 0,
                data_hex: String::new(),
            },
        }
    }
}

/// Decode one frame's signals for a message. The mux switch and the selected
/// branch compare raw values, and a frame too short for a signal leaves it held.
fn decode_frame(
    signals: &[SignalMeta],
    indices: &[usize],
    channel: u8,
    dlc: u8,
    data: &FrameData,
    mut emit: impl FnMut(usize, f64),
) {
    let carried = |signal: &SignalMeta| {
        let on_channel = signal.channel == 0 || channel == 0 || signal.channel == channel;
        let spec = signal
            .spec
            .filter(|spec| spec.bytes_needed() <= usize::from(dlc));
        spec.filter(|_| on_channel)
    };
    let switch = indices.iter().find_map(|&idx| {
        let signal = &signals[idx];
        signal
            .mux_switch
            .then(|| carried(signal))
            .flatten()
            .and_then(|spec| spec.switch_value(data))
    });
    for &idx in indices {
        let signal = &signals[idx];
        if signal
            .mux_value
            .is_some_and(|expected| switch != Some(expected))
        {
            continue;
        }
        if let Some(spec) = carried(signal) {
            emit(idx, spec.decode(data));
        }
    }
}

impl SignalMeta {
    fn info(&self) -> SignalInfo {
        SignalInfo {
            name: self.name.clone(),
            unit: self.unit.clone(),
            message_name: self.message_name.clone(),
            message_id: self.message_id,
            min: self.min,
            max: self.max,
            step: self
                .spec
                .map(|spec| spec.factor.abs())
                .filter(|step| *step > 0.0),
            from_map: self.spec.is_some(),
        }
    }

    fn note(&mut self, value: f64) {
        self.min = Some(self.min.map(|min| min.min(value)).unwrap_or(value));
        self.max = Some(self.max.map(|max| max.max(value)).unwrap_or(value));
    }
}

fn sniff_path(path: &Path) -> Result<LogFormat> {
    let mut file = File::open(path).map_err(|err| Error::read(path, err))?;
    let mut head = [0u8; 4096];
    let n = file.read(&mut head).map_err(|err| Error::read(path, err))?;
    sniff(&head[..n])
}

fn scan_framed(
    source: &Source,
    format: LogFormat,
    map: Option<&SignalMap>,
    timeout: TimeoutFactor,
    control: Option<&IndexControl>,
) -> Result<Built> {
    let mut signals = Vec::new();
    let mut name_index = HashMap::new();
    let mut msg_index: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut message_names = HashMap::new();
    if let Some(map) = map {
        for mapped in &map.signals {
            let idx = signals.len();
            name_index.insert(mapped.name.clone(), idx);
            msg_index.entry(mapped.message_id).or_default().push(idx);
            message_names
                .entry(mapped.message_id)
                .or_insert_with(|| mapped.message_name.clone());
            signals.push(SignalMeta {
                name: mapped.name.clone(),
                unit: mapped.unit.clone(),
                message_name: mapped.message_name.clone(),
                message_id: Some(mapped.message_id),
                spec: Some(mapped.spec),
                min: None,
                max: None,
                channel: mapped.channel,
                mux_switch: mapped.mux_switch,
                mux_value: mapped.mux_value,
                table: mapped.table.clone(),
                integrity: Integrity::classify(&mapped.name, &mapped.spec),
            });
        }
    }

    let mut built = empty_built(signals, name_index, msg_index, message_names);
    let mut held = vec![None; built.signals.len()];
    let mut last_t: Option<u64> = None;
    let mut stride = CHECKPOINT_EVERY;
    let mut pending: Vec<(usize, f64)> = Vec::new();
    let cycles: HashMap<u32, u64> = map
        .map(|map| {
            map.messages
                .iter()
                .filter_map(|message| message.cycle_us.map(|cycle| (message.id, cycle)))
                .collect()
        })
        .unwrap_or_default();
    let timeout_factor = timeout.get();
    let mut last_seen: HashMap<u32, u64> = HashMap::new();
    let mut last_counter: HashMap<usize, u64> = HashMap::new();
    let checksums = source.with_reader(|reader| probe_checksums(&mut built, reader, format))?;
    let mut pulses = 0u64;
    source.with_reader(|reader| {
        let mut scanner = Scanner::open(reader, format)?;
        while let Some(mut rec) = scanner.next_rec()? {
            pulses += 1;
            if pulses.is_multiple_of(64) {
                pulse(
                    control,
                    scanner.position(),
                    built.frame_count,
                    built.skipped.saturating_add(scanner.skipped()),
                )?;
            }
            let Some(t_us) = accept_time(&mut built, &mut last_t, rec.t_us) else {
                continue;
            };
            rec.t_us = t_us;
            match &rec.kind {
                RecKind::Frame {
                    id,
                    dlc,
                    data,
                    channel,
                    ..
                } => {
                    if rec.starts_container || built.frame_count.is_multiple_of(stride) {
                        push_checkpoint(&mut built, &held, rec.t_us, rec.offset, &mut stride);
                    }
                    pending.clear();
                    if let Some(indices) = built.msg_index.get(id) {
                        decode_frame(
                            &built.signals,
                            indices,
                            *channel,
                            *dlc,
                            data,
                            |idx, value| pending.push((idx, value)),
                        );
                    }
                    for (idx, value) in pending.iter().copied() {
                        held[idx] = Some(value);
                        built.signals[idx].note(value);
                    }
                    note_integrity(
                        &mut built,
                        &cycles,
                        timeout_factor,
                        &mut last_seen,
                        &mut last_counter,
                        &checksums,
                        IntegrityFrame {
                            t_us: rec.t_us,
                            id: *id,
                            dlc: *dlc,
                            data,
                            decoded: &pending,
                        },
                    );
                    built.seen_ids.insert(*id);
                    note_domain(&mut built, rec.t_us);
                    built.frame_count += 1;
                }
                RecKind::Event { label } => note_event(&mut built, rec.t_us, label),
                RecKind::Sample { .. } => {}
            }
        }
        pulse(
            control,
            scanner.position(),
            built.frame_count,
            built.skipped.saturating_add(scanner.skipped()),
        )?;
        absorb_scanner(&mut built, scanner.skipped(), &scanner.notes());
        Ok(())
    })?;
    pad_snapshots(&mut built);
    Ok(built)
}

fn pulse(control: Option<&IndexControl>, bytes_done: u64, frames: u64, skipped: u64) -> Result<()> {
    match control {
        Some(control) => control.observe(bytes_done, frames, skipped),
        None => Ok(()),
    }
}

fn scan_decoded(source: &Source, control: Option<&IndexControl>) -> Result<Built> {
    let mut built = empty_built(Vec::new(), HashMap::new(), HashMap::new(), HashMap::new());
    let mut held: Vec<Option<f64>> = Vec::new();
    let mut last_t: Option<u64> = None;
    let mut stride = CHECKPOINT_EVERY;
    let mut pulses = 0u64;
    source.with_reader(|reader| {
        let mut scanner = Scanner::open(reader, LogFormat::DecodedCsv)?;
        while let Some(mut rec) = scanner.next_rec()? {
            pulses += 1;
            if pulses.is_multiple_of(64) {
                pulse(
                    control,
                    scanner.position(),
                    built.frame_count,
                    built.skipped.saturating_add(scanner.skipped()),
                )?;
            }
            let Some(t_us) = accept_time(&mut built, &mut last_t, rec.t_us) else {
                continue;
            };
            rec.t_us = t_us;
            if let RecKind::Sample { name, value, unit } = &rec.kind {
                let idx = ensure_signal(&mut built, name, unit);
                if held.len() < built.signals.len() {
                    held.resize(built.signals.len(), None);
                }
                if built.frame_count.is_multiple_of(stride) {
                    push_checkpoint(&mut built, &held, rec.t_us, rec.offset, &mut stride);
                }
                held[idx] = Some(*value);
                built.signals[idx].note(*value);
                note_domain(&mut built, rec.t_us);
                built.frame_count += 1;
            }
        }
        pulse(
            control,
            scanner.position(),
            built.frame_count,
            built.skipped.saturating_add(scanner.skipped()),
        )?;
        absorb_scanner(&mut built, scanner.skipped(), &scanner.notes());
        Ok(())
    })?;
    pad_snapshots(&mut built);
    Ok(built)
}

fn empty_built(
    signals: Vec<SignalMeta>,
    name_index: HashMap<String, usize>,
    msg_index: HashMap<u32, Vec<usize>>,
    message_names: HashMap<u32, String>,
) -> Built {
    Built {
        checkpoints: Vec::new(),
        snapshots: Vec::new(),
        frame_count: 0,
        event_count: 0,
        events_truncated: false,
        t_start_us: 0,
        t_end_us: 0,
        events: Vec::new(),
        skipped: 0,
        reordered: 0,
        warnings: Vec::new(),
        signals,
        name_index,
        msg_index,
        message_names,
        seen_ids: HashSet::new(),
    }
}

fn ensure_signal(built: &mut Built, name: &str, unit: &str) -> usize {
    if let Some(&idx) = built.name_index.get(name) {
        if built.signals[idx].unit.is_empty() && !unit.is_empty() {
            built.signals[idx].unit = unit.to_string();
        }
        return idx;
    }
    let idx = built.signals.len();
    built.name_index.insert(name.to_string(), idx);
    built.signals.push(SignalMeta {
        name: name.to_string(),
        unit: unit.to_string(),
        message_name: "decoded".into(),
        message_id: None,
        spec: None,
        min: None,
        max: None,
        channel: 0,
        mux_switch: false,
        mux_value: None,
        table: Vec::new(),
        integrity: Integrity::None,
    });
    for snap in &mut built.snapshots {
        snap.push(None);
    }
    idx
}

fn push_checkpoint(
    built: &mut Built,
    held: &[Option<f64>],
    t_us: u64,
    offset: u64,
    stride: &mut u64,
) {
    if built.checkpoints.len() >= MAX_CHECKPOINTS {
        let checkpoints = built.checkpoints.iter().step_by(2).cloned().collect();
        let snapshots = built.snapshots.iter().step_by(2).cloned().collect();
        built.checkpoints = checkpoints;
        built.snapshots = snapshots;
        *stride = stride.saturating_mul(2).max(CHECKPOINT_EVERY);
    }
    built.checkpoints.push(Checkpoint {
        t_us,
        offset,
        frame_ordinal: built.frame_count,
    });
    let mut snap = held.to_vec();
    snap.resize(built.signals.len(), None);
    built.snapshots.push(snap);
}

fn note_domain(built: &mut Built, t_us: u64) {
    if built.frame_count == 0 && built.event_count == 0 {
        built.t_start_us = t_us;
        built.t_end_us = t_us;
        return;
    }
    built.t_start_us = built.t_start_us.min(t_us);
    built.t_end_us = built.t_end_us.max(t_us);
}

struct IntegrityFrame<'a> {
    t_us: u64,
    id: u32,
    dlc: u8,
    data: &'a [u8],
    /// Signals decoded from this frame.
    decoded: &'a [(usize, f64)],
}

fn note_integrity(
    built: &mut Built,
    cycles: &HashMap<u32, u64>,
    timeout_factor: f64,
    last_seen: &mut HashMap<u32, u64>,
    last_counter: &mut HashMap<usize, u64>,
    checksums: &HashMap<usize, ChecksumAlgo>,
    frame: IntegrityFrame<'_>,
) {
    if let Some(cycle) = cycles.get(&frame.id).copied() {
        if let Some(prev) = last_seen.get(&frame.id).copied() {
            let gap = frame.t_us.saturating_sub(prev);
            if cycle > 0 && gap as f64 > cycle as f64 * timeout_factor {
                let name = built
                    .message_names
                    .get(&frame.id)
                    .cloned()
                    .unwrap_or_else(|| format!("{:03X}", frame.id));
                note_event(built, frame.t_us, &format!("Timeout {name}"));
            }
        }
        last_seen.insert(frame.id, frame.t_us);
    }
    let Some(indices) = built.msg_index.get(&frame.id) else {
        return;
    };
    // Checksums first, as an ECU checks them: a frame that fails its checksum
    // is rejected, so its counter neither raises an event nor moves the reference.
    let width = (frame.dlc as usize).min(frame.data.len());
    let mut labels: Vec<String> = Vec::new();
    for &idx in indices {
        let signal = &built.signals[idx];
        let Integrity::Checksum { byte } = signal.integrity else {
            continue;
        };
        if byte >= width {
            continue;
        }
        if let Some(algo) = checksums.get(&idx) {
            if algo.compute_iter(covered_bytes(frame.data, width, byte)) != frame.data[byte] {
                labels.push(format!("Checksum {}", signal.name));
            }
        }
    }
    if labels.is_empty() {
        for &(idx, _) in frame.decoded {
            let signal = &built.signals[idx];
            let (Integrity::Counter { bits }, Some(spec)) = (signal.integrity, signal.spec) else {
                continue;
            };
            let mask = if bits >= 64 {
                u64::MAX
            } else {
                (1u64 << bits) - 1
            };
            let raw = spec.raw(frame.data);
            if let Some(prev) = last_counter.get(&idx).copied() {
                if raw != prev.wrapping_add(1) & mask {
                    labels.push(format!("Counter {}", signal.name));
                }
            }
            last_counter.insert(idx, raw);
        }
    }
    for label in labels {
        note_event(built, frame.t_us, &label);
    }
}

/// 8-bit checksum schemes a probe can recognise. Each covers every payload
/// byte except the checksum byte itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChecksumAlgo {
    Xor,
    Sum,
    /// SAE J1850: polynomial 0x1D, init 0xFF, final XOR 0xFF.
    CrcJ1850,
    /// AUTOSAR CRC8H2F: polynomial 0x2F, init 0xFF, final XOR 0xFF.
    Crc8H2F,
}

impl ChecksumAlgo {
    /// XOR first: it is what a mostly-good short log falls back to.
    const ALL: [ChecksumAlgo; 4] = [Self::Xor, Self::Sum, Self::CrcJ1850, Self::Crc8H2F];

    #[cfg(test)]
    pub(crate) fn compute(self, bytes: &[u8]) -> u8 {
        self.compute_iter(bytes.iter().copied())
    }

    fn compute_iter(self, bytes: impl Iterator<Item = u8>) -> u8 {
        match self {
            Self::Xor => bytes.fold(0, |acc, byte| acc ^ byte),
            Self::Sum => bytes.fold(0u8, |acc, byte| acc.wrapping_add(byte)),
            Self::CrcJ1850 => crc8(bytes, 0x1D),
            Self::Crc8H2F => crc8(bytes, 0x2F),
        }
    }
}

fn crc8(bytes: impl Iterator<Item = u8>, poly: u8) -> u8 {
    let mut crc = 0xFFu8;
    for byte in bytes {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ poly
            } else {
                crc << 1
            };
        }
    }
    crc ^ 0xFF
}

/// Frames a checksum is watched for before its scheme is judged.
const CHECKSUM_PROBE: usize = 16;
/// The pre-pass stops here even if a checksum message never shows up.
const CHECKSUM_PROBE_RECORDS: u64 = 200_000;

fn covered_bytes(
    data: &[u8],
    width: usize,
    checksum_byte: usize,
) -> impl Iterator<Item = u8> + Clone + '_ {
    data[..width]
        .iter()
        .enumerate()
        .filter(move |&(i, _)| i != checksum_byte)
        .map(|(_, &byte)| byte)
}

/// A DBC names checksum signals but not their scheme. Read the start of the
/// log once, before the build, and settle each one: a scheme that matches 90%
/// of its first frames is used; otherwise XOR stays if it matched at least
/// half, as before; otherwise the signal is not checked, with a note. Settling
/// first means the build checks every frame, from the first, the same way.
fn probe_checksums(
    built: &mut Built,
    reader: &mut dyn ReadSeek,
    format: LogFormat,
) -> Result<HashMap<usize, ChecksumAlgo>> {
    let mut seen: HashMap<usize, Vec<u8>> = built
        .signals
        .iter()
        .enumerate()
        .filter(|(_, signal)| matches!(signal.integrity, Integrity::Checksum { .. }))
        .map(|(idx, _)| (idx, Vec::new()))
        .collect();
    if seen.is_empty() {
        return Ok(HashMap::new());
    }
    let mut scanner = Scanner::open(reader, format)?;
    let mut records = 0u64;
    while let Some(rec) = scanner.next_rec()? {
        records += 1;
        if records > CHECKSUM_PROBE_RECORDS {
            break;
        }
        let RecKind::Frame { id, dlc, data, .. } = &rec.kind else {
            continue;
        };
        let Some(indices) = built.msg_index.get(id) else {
            continue;
        };
        let width = usize::from(*dlc).min(data.len());
        for idx in indices {
            let Some(masks) = seen
                .get_mut(idx)
                .filter(|masks| masks.len() < CHECKSUM_PROBE)
            else {
                continue;
            };
            let Integrity::Checksum { byte } = built.signals[*idx].integrity else {
                continue;
            };
            if byte >= width {
                continue;
            }
            let covered = covered_bytes(data, width, byte);
            let mask = ChecksumAlgo::ALL
                .iter()
                .enumerate()
                .filter(|(_, algo)| algo.compute_iter(covered.clone()) == data[byte])
                .fold(0u8, |mask, (bit, _)| mask | 1 << bit);
            masks.push(mask);
        }
        if seen.values().all(|masks| masks.len() >= CHECKSUM_PROBE) {
            break;
        }
    }
    let mut settled = HashMap::new();
    let mut probed: Vec<(usize, Vec<u8>)> = seen
        .into_iter()
        .filter(|(_, masks)| !masks.is_empty())
        .collect();
    probed.sort_unstable_by_key(|(idx, _)| *idx);
    for (idx, masks) in probed {
        let hits = |bit: usize| masks.iter().filter(|mask| *mask & (1 << bit) != 0).count();
        let (best, best_hits) = (0..ChecksumAlgo::ALL.len())
            .map(|bit| (bit, hits(bit)))
            .max_by_key(|&(bit, count)| (count, std::cmp::Reverse(bit)))
            .unwrap_or((0, 0));
        let chosen = if best_hits * 10 >= masks.len() * 9 {
            Some(best)
        } else if hits(0) * 2 >= masks.len() {
            Some(0)
        } else {
            None
        };
        match chosen {
            Some(bit) => {
                settled.insert(idx, ChecksumAlgo::ALL[bit]);
            }
            None => {
                let name = built.signals[idx].name.clone();
                note_warn(
                    built,
                    format!(
                        "{name} is not an XOR, byte sum, SAE J1850 or CRC-8H2F checksum of the other bytes, so it is not checked"
                    ),
                );
            }
        }
    }
    Ok(settled)
}

fn note_event(built: &mut Built, t_us: u64, label: &str) {
    if built.frame_count == 0 && built.event_count == 0 {
        built.t_start_us = t_us;
        built.t_end_us = t_us;
    } else {
        built.t_start_us = built.t_start_us.min(t_us);
        built.t_end_us = built.t_end_us.max(t_us);
    }
    built.event_count += 1;
    if built.events.len() < MAX_EVENTS {
        built.events.push((t_us, label.to_string()));
    } else {
        built.events_truncated = true;
    }
}

/// Real captures interleave Tx and Rx lines a few milliseconds out of order.
const REORDER_TOLERANCE_US: u64 = 50_000;

enum TimeOrder {
    InOrder(u64),
    /// A small step back: kept, at the previous time.
    Clamped(u64),
    /// A jump back past the tolerance: a broken log, so the record is dropped.
    Skipped,
}

fn order_time(last: &mut Option<u64>, t_us: u64) -> TimeOrder {
    match *last {
        Some(prev) if t_us < prev => {
            if prev - t_us <= REORDER_TOLERANCE_US {
                TimeOrder::Clamped(prev)
            } else {
                TimeOrder::Skipped
            }
        }
        _ => {
            *last = Some(t_us);
            TimeOrder::InOrder(t_us)
        }
    }
}

fn accept_time(built: &mut Built, last: &mut Option<u64>, t_us: u64) -> Option<u64> {
    let prev = *last;
    match order_time(last, t_us) {
        TimeOrder::InOrder(t_us) => Some(t_us),
        TimeOrder::Clamped(t_us) => {
            built.reordered += 1;
            Some(t_us)
        }
        TimeOrder::Skipped => {
            built.skipped += 1;
            note_warn(
                built,
                format!(
                    "skipped backwards timestamp at {t_us} µs (previous {} µs)",
                    prev.unwrap_or(0)
                ),
            );
            None
        }
    }
}

fn note_warn(built: &mut Built, message: String) {
    if built.warnings.len() < MAX_WARNINGS && !built.warnings.iter().any(|have| have == &message) {
        built.warnings.push(message);
    }
}

fn absorb_scanner(built: &mut Built, skipped: u64, warnings: &[String]) {
    built.skipped += skipped;
    if built.reordered > 0 {
        let reordered = built.reordered;
        note_warn(
            built,
            format!(
                "{reordered} records were up to {} ms out of order and were kept at the previous timestamp",
                REORDER_TOLERANCE_US / 1000
            ),
        );
    }
    for warning in warnings {
        note_warn(built, warning.clone());
    }
}

fn pad_snapshots(built: &mut Built) {
    let width = built.signals.len();
    for snap in &mut built.snapshots {
        snap.resize(width, None);
    }
}

fn is_timed_sample(rec: &Rec) -> bool {
    matches!(rec.kind, RecKind::Frame { .. } | RecKind::Sample { .. })
}

fn ordered_range(t0: u64, t1: u64) -> (u64, u64) {
    if t0 <= t1 {
        (t0, t1)
    } else {
        (t1, t0)
    }
}

fn capture_lead(held: &[Option<f64>], wanted: &[usize], lead: &mut [Option<f64>]) {
    for (slot, &signal_i) in wanted.iter().enumerate() {
        lead[slot] = held.get(signal_i).copied().flatten();
    }
}

#[derive(Clone)]
struct Bucket {
    min_t: u64,
    min_v: f64,
    max_t: u64,
    max_v: f64,
}

impl Bucket {
    fn emit(&self, out: &mut Vec<(u64, f64)>) {
        if self.min_t == self.max_t {
            out.push((self.min_t, self.min_v));
        } else if self.min_t < self.max_t {
            out.push((self.min_t, self.min_v));
            out.push((self.max_t, self.max_v));
        } else {
            out.push((self.max_t, self.max_v));
            out.push((self.min_t, self.min_v));
        }
    }
}

/// Buckets for one signal. Each bucket emits up to two points and a lead point
/// takes one more, so with a lead the budget loses a slot before it is halved.
fn bucket_count(max_points: usize, has_lead: bool) -> usize {
    let budget = if has_lead { max_points - 1 } else { max_points };
    (budget / 2).max(1)
}

fn push_bucket(buckets: &mut [Option<Bucket>], t0: u64, t1: u64, t: u64, value: f64) {
    let bucket_count = buckets.len();
    let span = t1.saturating_sub(t0).max(1);
    let mut index = ((u128::from(t.saturating_sub(t0)) * u128::from(bucket_count as u64))
        / u128::from(span)) as usize;
    if index >= bucket_count {
        index = bucket_count - 1;
    }
    match &mut buckets[index] {
        None => {
            buckets[index] = Some(Bucket {
                min_t: t,
                min_v: value,
                max_t: t,
                max_v: value,
            });
        }
        Some(bucket) => {
            if value < bucket.min_v || ((value - bucket.min_v).abs() < 1e-15 && t < bucket.min_t) {
                bucket.min_v = value;
                bucket.min_t = t;
            }
            if value > bucket.max_v || ((value - bucket.max_v).abs() < 1e-15 && t > bucket.max_t) {
                bucket.max_v = value;
                bucket.max_t = t;
            }
        }
    }
}

impl IndexedLog {
    /// An upper bound on the frames a replay of `[t0, t1]` reads, from the
    /// checkpoint index alone: a replay starts at the checkpoint at or before
    /// `t0` and stops at the first frame after `t1`, which comes no later than
    /// the next checkpoint.
    pub(crate) fn records_in_window(&self, t0_us: u64, t1_us: u64) -> u64 {
        if self.checkpoints.is_empty() {
            return 0;
        }
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let start = self.checkpoints[self.floor_checkpoint(t0)].frame_ordinal;
        let end = match self.checkpoints.get(self.floor_checkpoint(t1) + 1) {
            Some(next) => next.frame_ordinal,
            None => self.frame_count,
        };
        end.saturating_sub(start)
    }
}

/// Reduce a time-sorted series to at most `max_points` with the min/max
/// buckets and lead point `query` gives a physical series. A first point at the
/// window start takes the lead's place, whether it is the value held there or
/// a sample taken exactly at the start. Points must
/// lie in the window with strictly rising times, as `samples` and
/// `Compiled::eval_series` produce them.
pub(crate) fn decimate_points(
    points: Vec<(u64, f64)>,
    t0_us: u64,
    t1_us: u64,
    max_points: usize,
) -> Vec<(u64, f64)> {
    let (t0, t1) = ordered_range(t0_us, t1_us);
    let max_points = max_points.clamp(2, MAX_QUERY_POINTS);
    let lead = points.first().filter(|(t, _)| *t == t0).copied();
    let mut buckets = vec![None; bucket_count(max_points, lead.is_some())];
    for &(t, value) in &points[usize::from(lead.is_some())..] {
        push_bucket(&mut buckets, t0, t1, t, value);
    }
    let mut out = Vec::new();
    for bucket in buckets.iter().flatten() {
        bucket.emit(&mut out);
    }
    if let Some(lead) = lead {
        let excess = (out.len() + 1).saturating_sub(max_points);
        out.drain(..excess);
        out.insert(0, lead);
    }
    out.truncate(max_points);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Speed is `i` km/h at `i * 1000` µs for i in 0..=1000, a dense rising ramp.
    fn ramp_log() -> IndexedLog {
        let mut text = String::from("t_us,signal,value,unit\n");
        for i in 0..=1000u64 {
            text.push_str(&format!("{},Speed,{},km/h\n", i * 1000, i));
        }
        IndexedLog::open_bytes(text.into_bytes(), None).unwrap()
    }

    fn speed_points(log: &IndexedLog, t0_us: u64, max_points: usize) -> Vec<(u64, f64)> {
        let series = log
            .query(&QueryWindow {
                t0_us,
                t1_us: 1_000_000,
                signals: vec!["Speed".into()],
                max_points,
            })
            .unwrap();
        series[0].points.clone()
    }

    #[test]
    fn dense_window_keeps_last_sample_for_even_budgets() {
        let log = ramp_log();
        for max_points in (2..=64).step_by(2) {
            let points = speed_points(&log, 300_500, max_points);
            assert!(points.len() <= max_points, "{max_points}: {points:?}");
            assert_eq!(points.first(), Some(&(300_500, 300.0)), "{max_points}");
            assert_eq!(points.last(), Some(&(1_000_000, 1000.0)), "{max_points}");
        }
    }

    #[test]
    fn points_never_exceed_max_points() {
        let log = ramp_log();
        for max_points in 0..=40 {
            for t0_us in [0, 300_500] {
                let points = speed_points(&log, t0_us, max_points);
                assert!(
                    points.len() <= max_points.max(2),
                    "{max_points} at {t0_us}: {points:?}"
                );
                assert_eq!(points.last(), Some(&(1_000_000, 1000.0)));
            }
        }
    }

    #[test]
    fn signal_without_samples_in_window_keeps_its_lead_point() {
        let text = "t_us,signal,value,unit\n0,Speed,7,km/h\n0,RPM,800,rpm\n2000,Speed,9,km/h\n5000,Oil,30,kPa\n";
        let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), None).unwrap();
        let series = log
            .query(&QueryWindow {
                t0_us: 1000,
                t1_us: 3000,
                signals: vec!["Speed".into(), "RPM".into(), "Oil".into()],
                max_points: 10,
            })
            .unwrap();
        let points = |name: &str| {
            series
                .iter()
                .find(|s| s.name == name)
                .unwrap()
                .points
                .clone()
        };
        assert_eq!(points("Speed"), vec![(1000, 7.0), (2000, 9.0)]);
        assert_eq!(points("RPM"), vec![(1000, 800.0)]);
        assert_eq!(points("Oil"), Vec::<(u64, f64)>::new());
    }

    fn bytes_hex(data: &[u8]) -> String {
        data.iter().map(|byte| format!("{byte:02X}")).collect()
    }

    fn events_of(dbc: &str, frames: &[Vec<u8>]) -> (Vec<String>, Vec<String>) {
        let map = crate::dbc::parse(dbc).unwrap();
        let mut text = String::new();
        for (i, frame) in frames.iter().enumerate() {
            text.push_str(&format!("F {} 100 {}\n", i * 10_000, bytes_hex(frame)));
        }
        let log = IndexedLog::open_bytes(text.into_bytes(), Some(&map)).unwrap();
        let events = log
            .events()
            .iter()
            .map(|(_, label)| label.clone())
            .collect();
        (events, log.warnings().to_vec())
    }

    #[test]
    fn a_scaled_counter_is_checked_on_its_raw_bits() {
        let dbc = "BO_ 256 Msg: 1 ECU\n SG_ Counter : 0|8@1+ (0.5,10) [0|255] \"\" X\n";
        let steady: Vec<Vec<u8>> = (0..20u8).map(|i| vec![i]).collect();
        assert_eq!(events_of(dbc, &steady).0, Vec::<String>::new());
        let skipping: Vec<Vec<u8>> = [0u8, 1, 2, 4, 5, 6].iter().map(|&i| vec![i]).collect();
        assert_eq!(events_of(dbc, &skipping).0, vec!["Counter Counter"]);
    }

    #[test]
    fn a_wide_counter_wraps_at_its_own_width() {
        let dbc = "BO_ 256 Msg: 3 ECU\n SG_ Counter : 0|20@1+ (1,0) [0|1048575] \"\" X\n";
        let frame = |value: u32| value.to_le_bytes()[..3].to_vec();
        let wrap: Vec<Vec<u8>> = [0xFFFFE, 0xFFFFF, 0, 1].map(frame).to_vec();
        assert_eq!(events_of(dbc, &wrap).0, Vec::<String>::new());
        let skip: Vec<Vec<u8>> = [0xFFFFE, 0xFFFFF, 1, 2].map(frame).to_vec();
        assert_eq!(events_of(dbc, &skip).0, vec!["Counter Counter"]);
    }

    #[test]
    fn a_64_bit_counter_wraps_without_overflow() {
        let dbc = "BO_ 256 Msg: 8 ECU\n SG_ Counter : 0|64@1+ (1,0) [0|0] \"\" X\n";
        let frame = |value: u64| value.to_le_bytes().to_vec();
        let wrap: Vec<Vec<u8>> = [u64::MAX - 1, u64::MAX, 0, 1].map(frame).to_vec();
        assert_eq!(events_of(dbc, &wrap).0, Vec::<String>::new());
        let skip: Vec<Vec<u8>> = [u64::MAX, 1].map(frame).to_vec();
        assert_eq!(events_of(dbc, &skip).0, vec!["Counter Counter"]);
    }

    fn xor_frames(corrupt: Option<usize>) -> Vec<Vec<u8>> {
        (0..20u8)
            .map(|i| {
                let body = [i, 0x21, i.wrapping_mul(3)];
                let sum = body.iter().fold(0, |acc, byte| acc ^ byte);
                let sum = if corrupt == Some(usize::from(i)) {
                    sum ^ 0x55
                } else {
                    sum
                };
                vec![sum, body[0], body[1], body[2]]
            })
            .collect()
    }

    #[test]
    fn a_motorola_checksum_in_its_own_byte_is_checked() {
        let dbc = "BO_ 256 Msg: 4 ECU\n SG_ Checksum : 7|8@0+ (1,0) [0|255] \"\" X\n";
        assert_eq!(events_of(dbc, &xor_frames(None)).0, Vec::<String>::new());
        assert_eq!(
            events_of(dbc, &xor_frames(Some(7))).0,
            vec!["Checksum Checksum"]
        );
    }

    #[test]
    fn an_eight_bit_checksum_that_straddles_bytes_is_not_checked() {
        for dbc in [
            "BO_ 256 Msg: 4 ECU\n SG_ Checksum : 4|8@1+ (1,0) [0|255] \"\" X\n",
            "BO_ 256 Msg: 4 ECU\n SG_ Checksum : 3|8@0+ (1,0) [0|255] \"\" X\n",
        ] {
            let (events, warnings) = events_of(dbc, &xor_frames(Some(7)));
            assert_eq!(events, Vec::<String>::new(), "{dbc}");
            assert_eq!(warnings, Vec::<String>::new(), "{dbc}");
        }
    }

    #[test]
    fn a_window_replay_is_bounded_by_the_checkpoints_around_it() {
        let log = ramp_log();
        assert_eq!(log.frame_count(), 1001);
        assert_eq!(log.records_in_window(0, 100_000), 256);
        assert_eq!(log.records_in_window(300_000, 600_000), 512);
        assert_eq!(log.records_in_window(600_000, 300_000), 512);
        assert_eq!(log.records_in_window(0, 1_000_000), 1001);
        assert_eq!(log.records_in_window(900_000, 2_000_000), 233);
    }

    #[test]
    fn a_signal_name_with_a_comma_is_quoted_in_the_csv_header() {
        let map = SignalMap::parse(
            r#"{"name":"dash","version":1,"messages":[{"id":"0x1A0","name":"Dash",
            "signals":[{"name":"Speed, km/h","startBit":0,"bitLength":16,"factor":0.25,"unit":"km/h"}]}]}"#,
        )
        .unwrap();
        let log = IndexedLog::open_bytes(
            b"SLOGv1\nF 0 1A0 800C881378640000\nF 10000 1A0 800C881378640000\n".to_vec(),
            Some(&map),
        )
        .unwrap();
        let export = log
            .export_csv(&["Speed, km/h".to_string()], 0, 20_000)
            .unwrap();
        assert_eq!(export.text.lines().next(), Some("t_us,\"Speed, km/h\""));
        assert!(!export.truncated);
    }

    #[test]
    fn a_csv_export_cut_at_the_row_cap_says_so() {
        let log = ramp_log();
        let names = ["Speed".to_string()];
        let whole = log.export_csv_capped(&names, 0, 1_000_000, 1001).unwrap();
        assert_eq!((whole.rows, whole.truncated), (1001, false));
        assert!(!whole.text.contains("Truncated"));
        let cut = log.export_csv_capped(&names, 0, 1_000_000, 3).unwrap();
        assert_eq!((cut.rows, cut.truncated), (3, true));
        assert_eq!(
            cut.text,
            "t_us,Speed\n0,0.000000\n1000,1.000000\n2000,2.000000\n# Truncated by Signal Loom at 3 rows. Narrow the window to export the rest.\n"
        );
    }

    #[test]
    fn a_slog_export_cut_at_the_row_cap_says_so() {
        let text = b"SLOGv1\nF 0 1A0 01\nF 1000 1A0 02\nF 2000 1A0 03\n".to_vec();
        let log = IndexedLog::open_bytes(text, None).unwrap();
        let whole = log.export_slog_capped(0, 2_000, 3).unwrap();
        assert_eq!((whole.rows, whole.truncated), (3, false));
        let cut = log.export_slog_capped(0, 2_000, 2).unwrap();
        assert_eq!((cut.rows, cut.truncated), (2, true));
        assert_eq!(cut.text.lines().count(), 5);
        assert_eq!(
            cut.text.lines().last(),
            Some("# Truncated by Signal Loom at 2 rows. Narrow the window to export the rest.")
        );
    }
}
