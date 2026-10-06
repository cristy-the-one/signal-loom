use crate::decode::DecodeSpec;
use crate::error::{Error, Result};
use crate::map::SignalMap;
use crate::scan::{hex_payload, sniff, LogFormat, ReadSeek, Rec, RecKind, Scanner};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CHECKPOINT_EVERY: u64 = 256;
const MAX_EVENTS: usize = 5_000;
const MAX_QUERY_POINTS: usize = 8_000;

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
}

struct Checkpoint {
    t_us: u64,
    offset: u64,
    frame_ordinal: u64,
}

/// Sparse time index. Sample payloads stay in the file; a query seeks to the
/// nearest checkpoint and downsamples that window.
pub struct IndexedLog {
    source: Source,
    format: LogFormat,
    checkpoints: Vec<Checkpoint>,
    snapshots: Vec<Vec<Option<f64>>>,
    frame_count: u64,
    event_count: u64,
    events_truncated: bool,
    t_start_us: u64,
    t_end_us: u64,
    byte_len: u64,
    events: Vec<(u64, String)>,
    signals: Vec<SignalMeta>,
    name_index: HashMap<String, usize>,
    msg_index: HashMap<u32, Vec<usize>>,
    message_names: HashMap<u32, String>,
}

#[derive(Debug, Clone)]
pub struct SignalInfo {
    pub name: String,
    pub unit: String,
    pub message_name: String,
    pub message_id: Option<u32>,
    pub min: Option<f64>,
    pub max: Option<f64>,
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
}

#[derive(Debug, Clone)]
pub struct FrameHit {
    pub t_us: u64,
    pub ordinal: u64,
    pub message_id: Option<u32>,
    pub message_name: String,
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
    signals: Vec<SignalMeta>,
    name_index: HashMap<String, usize>,
    msg_index: HashMap<u32, Vec<usize>>,
    message_names: HashMap<u32, String>,
}

impl IndexedLog {
    pub fn open_path(path: &Path, map: Option<&SignalMap>) -> Result<Self> {
        let format = sniff_path(path)?;
        Self::build(Source::Path(path.to_path_buf()), format, map)
    }

    pub fn open_bytes(bytes: Vec<u8>, map: Option<&SignalMap>) -> Result<Self> {
        let format = sniff(&bytes)?;
        Self::build(Source::Memory(Arc::new(bytes)), format, map)
    }

    pub fn open_shared(bytes: Arc<Vec<u8>>, map: Option<&SignalMap>) -> Result<Self> {
        let format = sniff(bytes.as_slice())?;
        Self::build(Source::Memory(bytes), format, map)
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
        self.scan_from(self.checkpoints[idx].offset, |rec| {
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
        self.scan_from(self.checkpoints[idx].offset, |rec| {
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
            self.scan_from(self.checkpoints[idx].offset, |rec| {
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
            self.scan_from(self.checkpoints[idx].offset, |rec| {
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

    fn build(source: Source, format: LogFormat, map: Option<&SignalMap>) -> Result<Self> {
        let byte_len = source.byte_len()?;
        let scan_source = source.clone();
        let built = if format == LogFormat::DecodedCsv {
            scan_decoded(&scan_source)?
        } else {
            scan_framed(&scan_source, format, map)?
        };
        if built.frame_count == 0 {
            return Err(Error::msg(
                "log has no frames. Signal Loom needs at least one sample row.",
            ));
        }
        Ok(Self {
            source,
            format,
            checkpoints: built.checkpoints,
            snapshots: built.snapshots,
            frame_count: built.frame_count,
            event_count: built.event_count,
            events_truncated: built.events_truncated,
            t_start_us: built.t_start_us,
            t_end_us: built.t_end_us,
            byte_len,
            events: built.events,
            signals: built.signals,
            name_index: built.name_index,
            msg_index: built.msg_index,
            message_names: built.message_names,
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

    fn scan_from(&self, offset: u64, mut visit: impl FnMut(&Rec) -> bool) -> Result<()> {
        self.source.with_reader(|reader| {
            let mut scanner = Scanner::resume(reader, self.format, offset)?;
            while let Some(rec) = scanner.next_rec()? {
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
            RecKind::Frame { id, data, .. } => {
                if let Some(indices) = self.msg_index.get(id) {
                    for &idx in indices {
                        if let Some(spec) = self.signals[idx].spec {
                            let value = spec.decode(data);
                            held[idx] = Some(value);
                            on_update(idx, value);
                        }
                    }
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

    fn hit_from(&self, rec: &Rec, ordinal: u64) -> FrameHit {
        match &rec.kind {
            RecKind::Frame { id, dlc, data } => FrameHit {
                t_us: rec.t_us,
                ordinal,
                message_id: Some(*id),
                message_name: self
                    .message_names
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| format!("0x{id:X}")),
                dlc: *dlc,
                data_hex: hex_payload(data, *dlc),
            },
            RecKind::Sample { name, .. } => FrameHit {
                t_us: rec.t_us,
                ordinal,
                message_id: None,
                message_name: name.clone(),
                dlc: 0,
                data_hex: String::new(),
            },
            RecKind::Event { label } => FrameHit {
                t_us: rec.t_us,
                ordinal,
                message_id: None,
                message_name: label.clone(),
                dlc: 0,
                data_hex: String::new(),
            },
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

fn scan_framed(source: &Source, format: LogFormat, map: Option<&SignalMap>) -> Result<Built> {
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
            });
        }
    }

    let mut built = empty_built(signals, name_index, msg_index, message_names);
    let mut held = vec![None; built.signals.len()];
    let mut last_t: Option<u64> = None;
    let mut pending: Vec<(usize, f64)> = Vec::new();
    source.with_reader(|reader| {
        let mut scanner = Scanner::open(reader, format)?;
        while let Some(rec) = scanner.next_rec()? {
            note_time(&mut last_t, rec.t_us)?;
            match &rec.kind {
                RecKind::Frame { id, data, .. } => {
                    push_checkpoint(&mut built, &held, rec.t_us, rec.offset);
                    pending.clear();
                    if let Some(indices) = built.msg_index.get(id) {
                        for &idx in indices {
                            if let Some(spec) = built.signals[idx].spec {
                                pending.push((idx, spec.decode(data)));
                            }
                        }
                    }
                    for (idx, value) in pending.iter().copied() {
                        held[idx] = Some(value);
                        built.signals[idx].note(value);
                    }
                    note_domain(&mut built, rec.t_us);
                    built.frame_count += 1;
                }
                RecKind::Event { label } => note_event(&mut built, rec.t_us, label),
                RecKind::Sample { .. } => {}
            }
        }
        Ok(())
    })?;
    pad_snapshots(&mut built);
    Ok(built)
}

fn scan_decoded(source: &Source) -> Result<Built> {
    let mut built = empty_built(Vec::new(), HashMap::new(), HashMap::new(), HashMap::new());
    let mut held: Vec<Option<f64>> = Vec::new();
    let mut last_t: Option<u64> = None;
    source.with_reader(|reader| {
        let mut scanner = Scanner::open(reader, LogFormat::DecodedCsv)?;
        while let Some(rec) = scanner.next_rec()? {
            note_time(&mut last_t, rec.t_us)?;
            if let RecKind::Sample { name, value, unit } = &rec.kind {
                let idx = ensure_signal(&mut built, name, unit);
                if held.len() < built.signals.len() {
                    held.resize(built.signals.len(), None);
                }
                push_checkpoint(&mut built, &held, rec.t_us, rec.offset);
                held[idx] = Some(*value);
                built.signals[idx].note(*value);
                note_domain(&mut built, rec.t_us);
                built.frame_count += 1;
            }
        }
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
        signals,
        name_index,
        msg_index,
        message_names,
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
    });
    for snap in &mut built.snapshots {
        snap.push(None);
    }
    idx
}

fn push_checkpoint(built: &mut Built, held: &[Option<f64>], t_us: u64, offset: u64) {
    if !built.frame_count.is_multiple_of(CHECKPOINT_EVERY) {
        return;
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

fn note_time(last: &mut Option<u64>, t_us: u64) -> Result<()> {
    if let Some(prev) = *last {
        if t_us < prev {
            return Err(Error::msg(format!(
                "timestamps go backwards at {t_us} µs (previous {prev} µs). Logs must be time-sorted."
            )));
        }
    }
    *last = Some(t_us);
    Ok(())
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
