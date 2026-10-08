use super::integrity::Integrity;
use super::IndexedLog;
use crate::decode::DecodeSpec;
use crate::error::{Error, Result};
use crate::scan::FrameData;

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

/// Decode one frame's signals for a message. The mux switch and the selected
/// branch compare raw values, and a frame too short for a signal leaves it held.
pub(super) fn decode_frame(
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
