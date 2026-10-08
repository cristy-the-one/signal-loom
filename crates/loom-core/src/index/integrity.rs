use super::build::{note_event, Built};
use super::checksum::{covered_bytes, ChecksumAlgo};
use crate::decode::{DecodeSpec, Endian};
use crate::map::{SignalMap, TimeoutFactor};
use std::collections::HashMap;

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
    last_seen: HashMap<u32, u64>,
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
            if let Some(prev) = self.last_seen.get(&frame.id).copied() {
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
            self.last_seen.insert(frame.id, frame.t_us);
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
            if let Some(algo) = self.checksums.get(&idx) {
                if algo.compute_iter(covered_bytes(frame.data, width, byte)) != frame.data[byte] {
                    labels.push(format!("Checksum {}", signal.name));
                }
            }
        }
        if labels.is_empty() {
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
                        labels.push(format!("Counter {}", signal.name));
                    }
                }
                self.last_counter.insert(idx, raw);
            }
        }
        for label in labels {
            note_event(built, frame.t_us, &label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::IndexedLog;

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
}
