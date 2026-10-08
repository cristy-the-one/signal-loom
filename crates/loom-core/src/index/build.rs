use super::checkpoint::{pad_snapshots, push_checkpoint, Checkpoint, CHECKPOINT_EVERY};
use super::checksum::probe_checksums;
use super::control::{pulse, IndexControl};
use super::integrity::{Integrity, IntegrityFrame, IntegrityWatch};
use super::signal::{decode_frame, SignalMeta};
use super::source::Source;
use super::time::{accept_time, REORDER_TOLERANCE_US};
use super::IndexedLog;
use crate::error::{Error, Result};
use crate::map::{SignalMap, TimeoutFactor};
use crate::scan::{LogFormat, Rec, RecKind, Scanner};
use std::collections::{HashMap, HashSet};

const MAX_EVENTS: usize = 5_000;
const MAX_WARNINGS: usize = 32;

pub(super) struct Built {
    pub(super) checkpoints: Vec<Checkpoint>,
    pub(super) snapshots: Vec<Vec<Option<f64>>>,
    pub(super) frame_count: u64,
    pub(super) event_count: u64,
    pub(super) events_truncated: bool,
    pub(super) t_start_us: u64,
    pub(super) t_end_us: u64,
    pub(super) events: Vec<(u64, String)>,
    pub(super) skipped: u64,
    /// Records kept at the previous time after a small step back.
    pub(super) reordered: u64,
    pub(super) warnings: Vec<String>,
    pub(super) signals: Vec<SignalMeta>,
    pub(super) name_index: HashMap<String, usize>,
    pub(super) msg_index: HashMap<u32, Vec<usize>>,
    pub(super) message_names: HashMap<u32, String>,
    pub(super) seen_ids: HashSet<u32>,
}

impl Built {
    fn new(
        signals: Vec<SignalMeta>,
        name_index: HashMap<String, usize>,
        msg_index: HashMap<u32, Vec<usize>>,
        message_names: HashMap<u32, String>,
    ) -> Self {
        Self {
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

    /// The signals a map declares, before any record is read.
    fn from_map(map: Option<&SignalMap>) -> Self {
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
        Self::new(signals, name_index, msg_index, message_names)
    }
}

impl IndexedLog {
    pub(super) fn build(
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
        let built = if format == LogFormat::DecodedCsv {
            scan_decoded(&source, control)?
        } else {
            scan_framed(&source, format, map, timeout, control)?
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
}

/// What one build pass carries from record to record.
struct Pass {
    built: Built,
    held: Vec<Option<f64>>,
    last_t: Option<u64>,
    stride: u64,
}

impl Pass {
    fn checkpoint(&mut self, rec: &Rec) {
        push_checkpoint(
            &mut self.built,
            &self.held,
            rec.t_us,
            rec.offset,
            &mut self.stride,
        );
    }
}

/// Read every record of `source`, report progress, put each record's time in
/// order and hand it to `record`, then fold in what the scanner skipped and
/// noted. `record` decides what a record adds to the index.
fn drive(
    source: &Source,
    format: LogFormat,
    built: Built,
    control: Option<&IndexControl>,
    mut record: impl FnMut(&mut Pass, &Rec),
) -> Result<Built> {
    let mut pass = Pass {
        held: vec![None; built.signals.len()],
        built,
        last_t: None,
        stride: CHECKPOINT_EVERY,
    };
    let mut pulses = 0u64;
    source.with_reader(|reader| {
        let mut scanner = Scanner::open(reader, format)?;
        while let Some(mut rec) = scanner.next_rec()? {
            pulses += 1;
            if pulses.is_multiple_of(64) {
                pulse(
                    control,
                    scanner.position(),
                    pass.built.frame_count,
                    pass.built.skipped.saturating_add(scanner.skipped()),
                )?;
            }
            let Some(t_us) = accept_time(&mut pass.built, &mut pass.last_t, rec.t_us) else {
                continue;
            };
            rec.t_us = t_us;
            record(&mut pass, &rec);
        }
        pulse(
            control,
            scanner.position(),
            pass.built.frame_count,
            pass.built.skipped.saturating_add(scanner.skipped()),
        )?;
        absorb_scanner(&mut pass.built, scanner.skipped(), &scanner.notes());
        Ok(())
    })?;
    pad_snapshots(&mut pass.built);
    Ok(pass.built)
}

fn scan_framed(
    source: &Source,
    format: LogFormat,
    map: Option<&SignalMap>,
    timeout: TimeoutFactor,
    control: Option<&IndexControl>,
) -> Result<Built> {
    let mut built = Built::from_map(map);
    let checksums = source.with_reader(|reader| probe_checksums(&mut built, reader, format))?;
    let mut watch = IntegrityWatch::new(map, timeout, checksums);
    let mut pending: Vec<(usize, f64)> = Vec::new();
    drive(source, format, built, control, |pass, rec| {
        match &rec.kind {
            RecKind::Frame {
                id,
                dlc,
                data,
                channel,
                ..
            } => {
                if rec.starts_container || pass.built.frame_count.is_multiple_of(pass.stride) {
                    pass.checkpoint(rec);
                }
                pending.clear();
                if let Some(indices) = pass.built.msg_index.get(id) {
                    decode_frame(
                        &pass.built.signals,
                        indices,
                        *channel,
                        *dlc,
                        data,
                        |idx, value| pending.push((idx, value)),
                    );
                }
                for (idx, value) in pending.iter().copied() {
                    pass.held[idx] = Some(value);
                    pass.built.signals[idx].note(value);
                }
                watch.note(
                    &mut pass.built,
                    IntegrityFrame {
                        t_us: rec.t_us,
                        id: *id,
                        dlc: *dlc,
                        data,
                        decoded: &pending,
                    },
                );
                pass.built.seen_ids.insert(*id);
                note_domain(&mut pass.built, rec.t_us);
                pass.built.frame_count += 1;
            }
            RecKind::Event { label } => note_event(&mut pass.built, rec.t_us, label),
            RecKind::Sample { .. } => {}
        }
    })
}

fn scan_decoded(source: &Source, control: Option<&IndexControl>) -> Result<Built> {
    let built = Built::new(Vec::new(), HashMap::new(), HashMap::new(), HashMap::new());
    drive(
        source,
        LogFormat::DecodedCsv,
        built,
        control,
        |pass, rec| {
            if let RecKind::Sample { name, value, unit } = &rec.kind {
                let idx = ensure_signal(&mut pass.built, name, unit);
                if pass.held.len() < pass.built.signals.len() {
                    pass.held.resize(pass.built.signals.len(), None);
                }
                if pass.built.frame_count.is_multiple_of(pass.stride) {
                    pass.checkpoint(rec);
                }
                pass.held[idx] = Some(*value);
                pass.built.signals[idx].note(*value);
                note_domain(&mut pass.built, rec.t_us);
                pass.built.frame_count += 1;
            }
        },
    )
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

fn note_domain(built: &mut Built, t_us: u64) {
    if built.frame_count == 0 && built.event_count == 0 {
        built.t_start_us = t_us;
        built.t_end_us = t_us;
        return;
    }
    built.t_start_us = built.t_start_us.min(t_us);
    built.t_end_us = built.t_end_us.max(t_us);
}

pub(super) fn note_event(built: &mut Built, t_us: u64, label: &str) {
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

pub(super) fn note_warn(built: &mut Built, message: String) {
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
