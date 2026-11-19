use super::build::{note_event, Built};
use super::checksum::{covered_bytes, ChecksumAlgo};
use super::signal::SignalMeta;
use crate::decode::{DecodeSpec, Endian};
use crate::map::{SignalMap, TimeoutFactor};
use std::collections::HashMap;

/// Checksum or counter. A timeout is not one of these: the judge recomputes
/// silence from the frames, so a timeout stays a label and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IntegrityKind {
    Checksum,
    Counter,
}

/// A checksum or counter event, kept beside its label. A later judge asks, for
/// one signal (its message id and its name), which of these happened and when.
/// This is not part of the labelled event list the UI reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IntegrityEvent {
    pub(crate) t_us: u64,
    pub(crate) kind: IntegrityKind,
    pub(crate) message_id: u32,
    /// The signal's name on this index, the same name the label carries.
    /// A repeated DBC name is stored as `Name@<id>`, which is how the index
    /// knows the signal.
    pub(crate) signal: String,
}

/// What a signal does for frame integrity, decided once from its name and layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Integrity {
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
    pub(super) fn classify(name: &str, spec: &DecodeSpec) -> Self {
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

pub(super) struct IntegrityFrame<'a> {
    pub(super) t_us: u64,
    pub(super) id: u32,
    pub(super) channel: u8,
    pub(super) dlc: u8,
    pub(super) data: &'a [u8],
    /// Signals decoded from this frame.
    pub(super) decoded: &'a [(usize, f64)],
}

/// What the build remembers between frames to raise timeout, checksum and
/// counter events.
pub(super) struct IntegrityWatch {
    cycles: HashMap<u32, u64>,
    timeout_factor: f64,
    checksums: HashMap<usize, ChecksumAlgo>,
    /// Previous frame time of one message on one channel. The cycle time stays
    /// keyed by id: two channels of the same message share it.
    last_seen: HashMap<(u8, u32), u64>,
    last_counter: HashMap<usize, u64>,
}

impl IntegrityWatch {
    pub(super) fn new(
        map: Option<&SignalMap>,
        timeout: TimeoutFactor,
        checksums: HashMap<usize, ChecksumAlgo>,
    ) -> Self {
        let cycles = map
            .map(|map| {
                map.messages
                    .iter()
                    .filter_map(|message| message.cycle_us.map(|cycle| (message.id, cycle)))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            cycles,
            timeout_factor: timeout.get(),
            checksums,
            last_seen: HashMap::new(),
            last_counter: HashMap::new(),
        }
    }

    pub(super) fn note(&mut self, built: &mut Built, frame: IntegrityFrame<'_>) {
        if let Some(cycle) = self.cycles.get(&frame.id).copied() {
            let key = (frame.channel, frame.id);
            if let Some(prev) = self.last_seen.get(&key).copied() {
                let gap = frame.t_us.saturating_sub(prev);
                if cycle > 0 && gap as f64 > cycle as f64 * self.timeout_factor {
                    let name = built
                        .message_names
                        .get(&frame.id)
                        .cloned()
                        .unwrap_or_else(|| format!("{:03X}", frame.id));
                    note_event(built, frame.t_us, &format!("Timeout {name}"));
                }
            }
            self.last_seen.insert(key, frame.t_us);
        }
        let Some(indices) = built.msg_index.get(&frame.id) else {
            return;
        };
        // Checksums first, as an ECU checks them: a frame that fails its checksum
        // is rejected, so its counter neither raises an event nor moves the reference.
        let width = (frame.dlc as usize).min(frame.data.len());
        let mut raised: Vec<IntegrityEvent> = Vec::new();
        for &idx in indices {
            let signal = &built.signals[idx];
            // Same channel rule as `decode_frame`: a channel-0 signal or frame
            // meets every channel, and otherwise the channels must match.
            let on_channel =
                signal.channel == 0 || frame.channel == 0 || signal.channel == frame.channel;
            if !on_channel {
                continue;
            }
            let Integrity::Checksum { byte } = signal.integrity else {
                continue;
            };
            if byte >= width {
                continue;
            }
            if let Some(algo) = self.checksums.get(&idx) {
                if algo.compute_iter(covered_bytes(frame.data, width, byte)) != frame.data[byte] {
                    raised.push(integrity_event(&frame, IntegrityKind::Checksum, signal));
                }
            }
        }
        if raised.is_empty() {
            for &(idx, _) in frame.decoded {
                let signal = &built.signals[idx];
                let (Integrity::Counter { bits }, Some(spec)) = (signal.integrity, signal.spec)
                else {
                    continue;
                };
                let mask = if bits >= 64 {
                    u64::MAX
                } else {
                    (1u64 << bits) - 1
                };
                let raw = spec.raw(frame.data);
                if let Some(prev) = self.last_counter.get(&idx).copied() {
                    if raw != prev.wrapping_add(1) & mask {
                        raised.push(integrity_event(&frame, IntegrityKind::Counter, signal));
                    }
                }
                self.last_counter.insert(idx, raw);
            }
        }
        for event in raised {
            let label = match event.kind {
                IntegrityKind::Checksum => format!("Checksum {}", event.signal),
                IntegrityKind::Counter => format!("Counter {}", event.signal),
            };
            note_integrity(built, &label, event);
        }
    }

    /// Called once after the last record. A timeout is raised when a later
    /// frame arrives; a message that stops and never returns has no such
    /// frame. The gap is measured to `t_end_us`, with the same comparison as
    /// `note`. The stamp is the one the judge uses for an ongoing silence:
    /// the last frame plus the threshold, rounded up to a whole microsecond
    /// (`index/evaluate.rs`, `Problems`: `from = last + ceil(threshold)`).
    pub(super) fn finish(&mut self, built: &mut Built, t_end_us: u64) {
        let mut raised: Vec<(u64, u8, u32, String)> = Vec::new();
        for (&(channel, id), &last) in &self.last_seen {
            let Some(cycle) = self.cycles.get(&id).copied() else {
                continue;
            };
            if cycle == 0 {
                continue;
            }
            let threshold = cycle as f64 * self.timeout_factor;
            if t_end_us.saturating_sub(last) as f64 > threshold {
                let name = built
                    .message_names
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| format!("{:03X}", id));
                let t_us = last.saturating_add(threshold.ceil() as u64);
                raised.push((t_us, channel, id, format!("Timeout {name}")));
            }
        }
        if raised.is_empty() {
            return;
        }
        // Time order, then channel and id, so two silences at one instant
        // do not follow `HashMap`'s iteration. `note_event` appends and keeps
        // the cap: past it the event is counted, `events_truncated` is set,
        // and the label is not stored.
        raised.sort_by_key(|event| (event.0, event.1, event.2));
        for (t_us, _, _, label) in &raised {
            note_event(built, *t_us, label);
        }
        // The new stamps sit before the end of the log, ahead of labels that
        // were recorded later. A stable sort keeps equal times in the order
        // they already had.
        built.events.sort_by_key(|event| event.0);
    }
}

fn integrity_event(
    frame: &IntegrityFrame<'_>,
    kind: IntegrityKind,
    signal: &SignalMeta,
) -> IntegrityEvent {
    IntegrityEvent {
        t_us: frame.t_us,
        kind,
        message_id: signal.message_id.unwrap_or(frame.id),
        signal: signal.name.clone(),
    }
}

/// Store the label every event has, and, beside it, the kind a judge looks up.
/// Past the label cap there is no event to put this beside, so it is not stored.
fn note_integrity(built: &mut Built, label: &str, event: IntegrityEvent) {
    let before = built.events.len();
    note_event(built, event.t_us, label);
    if built.events.len() > before {
        built.integrity_events.push(event);
    }
}

impl super::IndexedLog {
    /// Checksum and counter events, in the order they were recorded.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the judge reads this; no other loom-core caller yet"
        )
    )]
    pub(crate) fn integrity_events(&self) -> &[IntegrityEvent] {
        &self.integrity_events
    }

    /// Checksum and counter events for the signal `signal` on `message_id`,
    /// in the order they were recorded.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the judge reads this; no other loom-core caller yet"
        )
    )]
    pub(crate) fn integrity_events_for<'a>(
        &'a self,
        message_id: u32,
        signal: &'a str,
    ) -> impl Iterator<Item = &'a IntegrityEvent> + 'a {
        self.integrity_events
            .iter()
            .filter(move |event| event.message_id == message_id && event.signal == signal)
    }
}

#[cfg(test)]
mod tests {
    use super::super::IndexedLog;
    use super::IntegrityKind;

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

    /// Each kind carries its message id and signal, and a lookup by that pair
    /// returns it. The labels stay the text they have always been. A timeout
    /// on the same log is still only that label.
    #[test]
    fn checksum_and_counter_events_record_kind_message_and_signal() {
        let dbc = r#"
BO_ 256 Body: 8 ECU
 SG_ FastCounter : 0|4@1+ (1,0) [0|15] "" ECU
 SG_ FastChecksum : 56|8@1+ (1,0) [0|255] "" ECU
BO_ 512 Other: 1 ECU
 SG_ OtherCounter : 0|8@1+ (1,0) [0|255] "" ECU
BA_ "GenMsgCycleTime" BO_ 256 10;
"#;
        let map = crate::dbc::parse(dbc).unwrap();
        let body = |counter: u8, corrupt: bool| {
            let mut data = [0u8; 8];
            data[0] = counter;
            data[7] = data[..7].iter().fold(0, |acc, byte| acc ^ byte);
            if corrupt {
                data[7] ^= 0xFF;
            }
            bytes_hex(&data)
        };
        let text = format!(
            "F 0 100 {}\nF 1000 200 00\nF 11000 200 01\nF 80000 100 {}\nF 81000 200 03\nF 90000 100 {}\n",
            body(1, false),
            body(4, false),
            body(9, true)
        );
        let log = IndexedLog::open_bytes(text.into_bytes(), Some(&map)).unwrap();

        let labels: Vec<(u64, &str)> = log
            .events()
            .iter()
            .map(|(t_us, label)| (*t_us, label.as_str()))
            .collect();
        assert_eq!(
            labels,
            vec![
                (80_000, "Timeout Body"),
                (80_000, "Counter FastCounter"),
                (81_000, "Counter OtherCounter"),
                (90_000, "Checksum FastChecksum"),
            ]
        );

        let stored = log.integrity_events();
        assert_eq!(stored.len(), 3, "{stored:?}");
        assert_eq!(
            stored
                .iter()
                .map(|event| (
                    event.t_us,
                    event.kind,
                    event.message_id,
                    event.signal.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![
                (80_000, IntegrityKind::Counter, 0x100, "FastCounter"),
                (81_000, IntegrityKind::Counter, 0x200, "OtherCounter"),
                (90_000, IntegrityKind::Checksum, 0x100, "FastChecksum"),
            ]
        );

        let of = |message_id, signal: &str| {
            log.integrity_events_for(message_id, signal)
                .map(|event| (event.t_us, event.kind))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            of(0x100, "FastCounter"),
            vec![(80_000, IntegrityKind::Counter)]
        );
        assert_eq!(
            of(0x100, "FastChecksum"),
            vec![(90_000, IntegrityKind::Checksum)]
        );
        assert_eq!(
            of(0x200, "OtherCounter"),
            vec![(81_000, IntegrityKind::Counter)]
        );
        assert_eq!(
            of(0x100, "OtherCounter"),
            Vec::<(u64, IntegrityKind)>::new()
        );
        assert_eq!(
            of(0x200, "FastChecksum"),
            Vec::<(u64, IntegrityKind)>::new()
        );
        assert_eq!(of(0x100, "Body"), Vec::<(u64, IntegrityKind)>::new());
    }
}
