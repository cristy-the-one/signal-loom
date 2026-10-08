use super::build::{note_warn, Built};
use super::integrity::Integrity;
use crate::error::Result;
use crate::scan::{LogFormat, ReadSeek, RecKind, Scanner};
use std::collections::HashMap;

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

    pub(super) fn compute_iter(self, bytes: impl Iterator<Item = u8>) -> u8 {
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

pub(super) fn covered_bytes(
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
pub(super) fn probe_checksums(
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
