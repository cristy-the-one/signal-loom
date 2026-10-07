use crate::decode::DecodeSpec;
use crate::error::{Error, Result};
use crate::map::SignalMap;
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
    pub fn open_path(path: &Path, map: Option<&SignalMap>) -> Result<Self> {
        Self::open_path_controlled(path, map, None)
    }

    pub fn open_path_controlled(
        path: &Path,
        map: Option<&SignalMap>,
        control: Option<&IndexControl>,
    ) -> Result<Self> {
        let format = sniff_path(path)?;
        Self::build(Source::Path(path.to_path_buf()), format, map, control)
    }

    pub fn open_bytes(bytes: Vec<u8>, map: Option<&SignalMap>) -> Result<Self> {
        let format = sniff(&bytes)?;
        Self::build(Source::Memory(Arc::new(bytes)), format, map, None)
    }

    pub fn open_shared(bytes: Arc<Vec<u8>>, map: Option<&SignalMap>) -> Result<Self> {
        let format = sniff(bytes.as_slice())?;
        Self::build(Source::Memory(bytes), format, map, None)
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
        let bucket_count = (max_points / 2).max(1);
        let mut buckets = vec![vec![None; bucket_count]; wanted.len()];
        let mut lead = vec![None; wanted.len()];
        let slot_of: HashMap<usize, usize> = wanted
            .iter()
            .enumerate()
            .map(|(slot, &signal)| (signal, slot))
            .collect();

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
                if let Some(&slot) = slot_of.get(&signal) {
                    push_bucket(&mut buckets[slot], bucket_count, t0, t1, rec.t_us, value);
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
                points.extend(bucket.emit());
            }
            if let Some(value) = lead[slot] {
                let replace = match points.first() {
                    None => true,
                    Some((t, existing)) => {
                        *t > t0 || (*t == t0 && (*existing - value).abs() > 1e-9)
                    }
                };
                if replace {
                    points.insert(0, (t0, value));
                }
            }
            if points.len() > max_points {
                points.truncate(max_points);
            }
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
            scan_framed(&scan_source, format, map, control)?
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
            let mut scanner = Scanner::resume(reader, self.body, checkpoint.offset)?;
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

    pub fn export_csv(&self, names: &[String], t0_us: u64, t1_us: u64) -> Result<String> {
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
        let mut out = String::from("t_us");
        for name in names {
            out.push(',');
            out.push_str(name);
        }
        out.push('\n');
        let mut rows = 0usize;
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
                if rows >= 500_000 {
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
        Ok(out)
    }

    pub fn export_slog(&self, t0_us: u64, t1_us: u64) -> Result<String> {
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let mut out = String::from(
            "SLOGv1\n# Trimmed by Signal Loom. Synthetic or captured, this is only the selected window.\n",
        );
        let mut rows = 0usize;
        self.scan_from(self.floor_checkpoint(t0), |rec| {
            if rec.t_us > t1 {
                return false;
            }
            if rec.t_us < t0 {
                return true;
            }
            if rows >= 500_000 {
                return false;
            }
            match &rec.kind {
                RecKind::Frame { id, dlc, data, .. } => {
                    out.push_str(&format!(
                        "F {} {id:X} {}\n",
                        rec.t_us,
                        hex_payload(data, *dlc)
                    ));
                    rows += 1;
                }
                RecKind::Event { label } => {
                    out.push_str(&format!("E {} {label}\n", rec.t_us));
                    rows += 1;
                }
                RecKind::Sample { .. } => {}
            }
            true
        })?;
        if rows == 0 {
            return Err(Error::msg("that window has no frames to export"));
        }
        Ok(out)
    }

    pub fn crossings(&self, name: &str, op: &str, level: f64) -> Result<Vec<(u64, String)>> {
        let idx = self
            .name_index
            .get(name)
            .copied()
            .ok_or_else(|| Error::msg(format!("no signal named {name}")))?;
        let pred = |value: f64| match op {
            ">" | "gt" => value > level,
            "<" | "lt" => value < level,
            ">=" | "ge" => value >= level,
            "<=" | "le" => value <= level,
            _ => false,
        };
        if !matches!(op, ">" | "<" | ">=" | "<=" | "gt" | "lt" | "ge" | "le") {
            return Err(Error::msg("trigger comparison must be >, <, >=, or <="));
        }
        let mut events = Vec::new();
        let mut armed = true;
        let mut held = self.snapshot(0);
        self.scan_from(0, |rec| {
            self.touch(rec, &mut held, |signal, value| {
                if signal != idx {
                    return;
                }
                let hot = pred(value);
                if hot && armed {
                    events.push((rec.t_us, format!("Trigger {name} {op} {level}")));
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
    let timeout_factor = map
        .map(|map| map.timeout_factor)
        .unwrap_or(crate::map::DEFAULT_TIMEOUT_FACTOR);
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
                            held: &held,
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
    held: &'a [Option<f64>],
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
    let Some(indices) = built.msg_index.get(&frame.id).cloned() else {
        return;
    };
    // Checksums first, as an ECU checks them: a frame that fails its checksum
    // is rejected, so its counter neither raises an event nor moves the reference.
    let mut corrupt = false;
    for &idx in &indices {
        let Some(spec) = built.signals[idx].spec else {
            continue;
        };
        let lname = built.signals[idx].name.to_ascii_lowercase();
        if lname.contains("checksum") && spec.bit_length == 8 {
            let byte = (spec.start_bit / 8) as usize;
            let width = (frame.dlc as usize).min(frame.data.len());
            if byte < width {
                if let Some(algo) = checksums.get(&idx) {
                    let covered = covered_bytes(frame.data, width, byte);
                    if algo.compute(&covered) != frame.data[byte] {
                        corrupt = true;
                        let name = built.signals[idx].name.clone();
                        note_event(built, frame.t_us, &format!("Checksum {name}"));
                    }
                }
            }
        }
    }
    if corrupt {
        return;
    }
    for &idx in &indices {
        let Some(spec) = built.signals[idx].spec else {
            continue;
        };
        let lname = built.signals[idx].name.to_ascii_lowercase();
        if lname.contains("counter") {
            if let Some(value) = frame.held.get(idx).copied().flatten() {
                let bits = u32::from(spec.bit_length.min(16));
                let modulus = 1u64 << bits;
                let raw = value.round().clamp(0.0, (modulus - 1) as f64) as u64;
                if let Some(prev) = last_counter.get(&idx).copied() {
                    let expect = (prev + 1) % modulus;
                    if raw != expect {
                        let name = built.signals[idx].name.clone();
                        note_event(built, frame.t_us, &format!("Counter {name}"));
                    }
                }
                last_counter.insert(idx, raw);
            }
        }
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

    pub(crate) fn compute(self, bytes: &[u8]) -> u8 {
        match self {
            Self::Xor => bytes.iter().fold(0, |acc, byte| acc ^ byte),
            Self::Sum => bytes.iter().fold(0u8, |acc, byte| acc.wrapping_add(*byte)),
            Self::CrcJ1850 => crc8(bytes, 0x1D),
            Self::Crc8H2F => crc8(bytes, 0x2F),
        }
    }
}

fn crc8(bytes: &[u8], poly: u8) -> u8 {
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

fn covered_bytes(data: &[u8], width: usize, checksum_byte: usize) -> Vec<u8> {
    (0..width)
        .filter(|&i| i != checksum_byte)
        .map(|i| data[i])
        .collect()
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
        .filter(|(_, signal)| {
            signal.name.to_ascii_lowercase().contains("checksum")
                && signal.spec.is_some_and(|spec| spec.bit_length == 8)
        })
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
            let Some(spec) = built.signals[*idx].spec else {
                continue;
            };
            let byte = usize::from(spec.start_bit / 8);
            if byte >= width {
                continue;
            }
            let covered = covered_bytes(data, width, byte);
            let mask = ChecksumAlgo::ALL
                .iter()
                .enumerate()
                .filter(|(_, algo)| algo.compute(&covered) == data[byte])
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
    fn emit(&self) -> Vec<(u64, f64)> {
        if self.min_t == self.max_t {
            vec![(self.min_t, self.min_v)]
        } else if self.min_t < self.max_t {
            vec![(self.min_t, self.min_v), (self.max_t, self.max_v)]
        } else {
            vec![(self.max_t, self.max_v), (self.min_t, self.min_v)]
        }
    }
}

fn push_bucket(
    buckets: &mut [Option<Bucket>],
    bucket_count: usize,
    t0: u64,
    t1: u64,
    t: u64,
    value: f64,
) {
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
