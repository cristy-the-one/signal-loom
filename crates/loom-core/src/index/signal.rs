use super::integrity::Integrity;
use super::IndexedLog;
use crate::decode::DecodeSpec;
use crate::error::{Error, Result};
use crate::scan::{FrameData, Rec, RecKind};
use std::collections::HashMap;

/// Signals declared on one CAN message id.
#[derive(Default)]
pub(super) struct MessageSignals {
    pub(super) indices: Vec<usize>,
    pub(super) has_mux: bool,
}

/// A subset of signals to decode during a replay. Unrelated CAN messages are
/// skipped before inspecting any signal metadata.
pub(super) struct SignalFilter {
    wanted: Vec<usize>,
    by_msg: HashMap<u32, (bool, Vec<(usize, usize)>)>,
    by_sample: HashMap<usize, Vec<usize>>,
}

impl SignalFilter {
    pub(super) fn new(signals: &[SignalMeta], wanted: &[usize]) -> Self {
        let mut by_msg: HashMap<u32, (bool, Vec<(usize, usize)>)> = HashMap::new();
        let mut by_sample: HashMap<usize, Vec<usize>> = HashMap::new();
        for (slot, &idx) in wanted.iter().enumerate() {
            let signal = &signals[idx];
            if let Some(id) = signal.message_id {
                let entry = by_msg.entry(id).or_insert_with(|| (false, Vec::new()));
                entry.0 |= signal.mux_value.is_some();
                entry.1.push((idx, slot));
            } else {
                by_sample.entry(idx).or_default().push(slot);
            }
        }
        Self {
            wanted: wanted.to_vec(),
            by_msg,
            by_sample,
        }
    }

    pub(super) fn wanted(&self) -> &[usize] {
        &self.wanted
    }
}

#[derive(Clone)]
pub(super) struct SignalMeta {
    pub(super) name: String,
    pub(super) unit: String,
    pub(super) message_name: String,
    pub(super) message_id: Option<u32>,
    pub(super) spec: Option<DecodeSpec>,
    pub(super) min: Option<f64>,
    pub(super) max: Option<f64>,
    pub(super) channel: u8,
    pub(super) mux_switch: bool,
    pub(super) mux_value: Option<u32>,
    pub(super) table: Vec<(i64, String)>,
    pub(super) integrity: Integrity,
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

    pub(super) fn note(&mut self, value: f64) {
        self.min = Some(self.min.map(|min| min.min(value)).unwrap_or(value));
        self.max = Some(self.max.map(|max| max.max(value)).unwrap_or(value));
    }
}

impl IndexedLog {
    pub fn signals(&self) -> impl Iterator<Item = SignalInfo> + '_ {
        self.signals.iter().map(SignalMeta::info)
    }

    /// Whether `name` is a decoded signal of this log.
    pub fn has_signal(&self, name: &str) -> bool {
        self.resolve_signal(name).is_some()
    }

    /// The position of the signal called `name`, if the log has one.
    pub(super) fn resolve_signal(&self, name: &str) -> Option<usize> {
        self.name_index.get(name).copied()
    }

    /// `resolve_signal`, or the error a read of an unknown signal reports.
    pub(super) fn require_signal(&self, name: &str) -> Result<usize> {
        self.resolve_signal(name)
            .ok_or_else(|| Error::not_found(format!("no signal named {name}")))
    }
}

fn carried_spec(signal: &SignalMeta, channel: u8, dlc: u8) -> Option<DecodeSpec> {
    let on_channel = signal.channel == 0 || channel == 0 || signal.channel == channel;
    if !on_channel {
        return None;
    }
    signal
        .spec
        .filter(|spec| spec.bytes_needed() <= usize::from(dlc))
}

fn mux_switch_value(
    signals: &[SignalMeta],
    indices: &[usize],
    channel: u8,
    dlc: u8,
    data: &FrameData,
) -> Option<u32> {
    indices.iter().find_map(|&idx| {
        let signal = &signals[idx];
        signal
            .mux_switch
            .then(|| carried_spec(signal, channel, dlc))
            .flatten()
            .and_then(|spec| spec.switch_value(data))
    })
}

/// Decode one frame's signals for a message. The mux switch and the selected
/// branch compare raw values, and a frame too short for a signal leaves it held.
pub(super) fn decode_frame(
    signals: &[SignalMeta],
    group: &MessageSignals,
    channel: u8,
    dlc: u8,
    data: &FrameData,
    mut emit: impl FnMut(usize, f64),
) {
    let switch = if group.has_mux {
        mux_switch_value(signals, &group.indices, channel, dlc, data)
    } else {
        None
    };
    for &idx in &group.indices {
        let signal = &signals[idx];
        if signal
            .mux_value
            .is_some_and(|expected| switch != Some(expected))
        {
            continue;
        }
        if let Some(spec) = carried_spec(signal, channel, dlc) {
            emit(idx, spec.decode(data));
        }
    }
}

impl IndexedLog {
    /// Decode only the signals in `filter` from `rec`, reporting `(slot, value)`.
    pub(super) fn decode_wanted(
        &self,
        rec: &Rec,
        filter: &SignalFilter,
        mut emit: impl FnMut(usize, f64),
    ) {
        match &rec.kind {
            RecKind::Frame {
                id,
                dlc,
                data,
                channel,
                ..
            } => {
                let Some((needs_mux, entries)) = filter.by_msg.get(id) else {
                    return;
                };
                let switch = if *needs_mux {
                    self.msg_index.get(id).and_then(|group| {
                        mux_switch_value(&self.signals, &group.indices, *channel, *dlc, data)
                    })
                } else {
                    None
                };
                for &(idx, slot) in entries {
                    let signal = &self.signals[idx];
                    if signal
                        .mux_value
                        .is_some_and(|expected| switch != Some(expected))
                    {
                        continue;
                    }
                    if let Some(spec) = carried_spec(signal, *channel, *dlc) {
                        emit(slot, spec.decode(data));
                    }
                }
            }
            RecKind::Sample { name, value, .. } => {
                if let Some(idx) = self.resolve_signal(name) {
                    if let Some(slots) = filter.by_sample.get(&idx) {
                        for &slot in slots {
                            emit(slot, *value);
                        }
                    }
                }
            }
            RecKind::Event { .. } => {}
        }
    }

    /// Decode one signal from `rec`, skipping every other message and signal.
    pub(super) fn decode_one(&self, rec: &Rec, idx: usize) -> Option<f64> {
        let signal = &self.signals[idx];
        match &rec.kind {
            RecKind::Frame {
                id,
                dlc,
                data,
                channel,
                ..
            } => {
                if signal.message_id != Some(*id) {
                    return None;
                }
                let spec = carried_spec(signal, *channel, *dlc)?;
                if let Some(expected) = signal.mux_value {
                    let switch = self.msg_index.get(id).and_then(|group| {
                        mux_switch_value(&self.signals, &group.indices, *channel, *dlc, data)
                    });
                    if switch != Some(expected) {
                        return None;
                    }
                }
                Some(spec.decode(data))
            }
            RecKind::Sample { name, value, .. } => {
                (self.resolve_signal(name) == Some(idx)).then_some(*value)
            }
            RecKind::Event { .. } => None,
        }
    }
}
