//! Vector BLF reader.
//!
//! Objects are pulled one container at a time. A zlib container is inflated
//! into a bounded buffer and discarded after its frames are indexed, so a
//! multi-gigabyte log is not copied into a second in-memory trace.
//! Classic CAN, CAN FD (first 64 bytes), and error frames are accepted.
//! A truncated or unknown object is skipped by the scanner.

use crate::error::{Error, Result};
use crate::scan::{FrameData, Rec, RecKind, MAX_DATA};
use flate2::read::ZlibDecoder;
use std::io::Read;

const CAN_MESSAGE: u32 = 1;
const LOG_CONTAINER: u32 = 10;
const CAN_ERROR_EXT: u32 = 73;
const CAN_MESSAGE2: u32 = 86;
const CAN_FD_MESSAGE: u32 = 100;
const CAN_FD_MESSAGE_64: u32 = 101;
const TIME_TEN_US: u32 = 1;
pub const CONTAINER_CAP: usize = 8 * 1024 * 1024;

pub fn is_container(object_type: u32) -> bool {
    object_type == LOG_CONTAINER
}

pub fn is_frame_object(object_type: u32) -> bool {
    matches!(
        object_type,
        CAN_MESSAGE | CAN_MESSAGE2 | CAN_ERROR_EXT | CAN_FD_MESSAGE | CAN_FD_MESSAGE_64
    )
}

/// Inflate one log container. The returned buffer is capped.
pub fn inflate_container(object: &[u8]) -> Result<Vec<u8>> {
    if object.len() < 16 || &object[0..4] != b"LOBJ" {
        return Err(Error::msg("BLF container is missing LOBJ"));
    }
    let header_size = u16::from_le_bytes(copy2(object, 4)?) as usize;
    if header_size > object.len() {
        return Err(Error::msg("BLF container header is larger than the object"));
    }
    let body = &object[header_size..];
    if body.len() < 16 {
        return Err(Error::msg("BLF log container is truncated"));
    }
    let method = u16::from_le_bytes(copy2(body, 0)?);
    let payload = &body[16..];
    match method {
        0 => {
            if payload.len() > CONTAINER_CAP {
                return Err(Error::msg("BLF container is larger than 8MB"));
            }
            Ok(payload.to_vec())
        }
        2 => {
            let mut decoder = ZlibDecoder::new(payload);
            let mut out = Vec::new();
            let mut chunk = [0u8; 64 * 1024];
            loop {
                let n = decoder
                    .read(&mut chunk)
                    .map_err(|err| Error::msg(format!("BLF zlib container: {err}")))?;
                if n == 0 {
                    break;
                }
                if out.len() + n > CONTAINER_CAP {
                    return Err(Error::msg("BLF container is larger than 8MB"));
                }
                out.extend_from_slice(&chunk[..n]);
            }
            Ok(out)
        }
        other => Err(Error::msg(format!(
            "BLF compression method {other} is not supported"
        ))),
    }
}

/// Next decodable object inside an inflated container. `at` advances past it.
pub fn next_inner(data: &[u8], at: &mut usize, container_off: u64) -> Option<Rec> {
    loop {
        let rest = data.get(*at..)?;
        let rel = rest.windows(4).position(|word| word == b"LOBJ")?;
        let cursor = *at + rel;
        if cursor + 16 > data.len() {
            *at = data.len();
            return None;
        }
        let size_bytes: [u8; 4] = data.get(cursor + 8..cursor + 12)?.try_into().ok()?;
        let obj_size = u32::from_le_bytes(size_bytes) as usize;
        if obj_size < 16 || cursor + obj_size > data.len() {
            *at = data.len();
            return None;
        }
        let object = &data[cursor..cursor + obj_size];
        *at = cursor + obj_size;
        if let Some(mut rec) = decode_object(object) {
            rec.offset = container_off;
            return Some(rec);
        }
    }
}

pub fn decode_object(object: &[u8]) -> Option<Rec> {
    if object.len() < 16 || &object[0..4] != b"LOBJ" {
        return None;
    }
    let header_size = u16::from_le_bytes(copy2(object, 4).ok()?) as usize;
    let header_version = u16::from_le_bytes(copy2(object, 6).ok()?);
    let obj_type = u32::from_le_bytes(copy4(object, 12).ok()?);
    if header_size > object.len() {
        return None;
    }
    let (flags, timestamp) = match header_version {
        1 => {
            if object.len() < 32 {
                return None;
            }
            (
                u32::from_le_bytes(copy4(object, 16).ok()?),
                u64::from_le_bytes(copy8(object, 24).ok()?),
            )
        }
        2 => {
            if object.len() < 40 {
                return None;
            }
            (
                u32::from_le_bytes(copy4(object, 16).ok()?),
                u64::from_le_bytes(copy8(object, 24).ok()?),
            )
        }
        _ => return None,
    };
    let payload = object.get(header_size..)?;
    let t_us = if flags == TIME_TEN_US {
        timestamp.saturating_mul(10)
    } else {
        timestamp / 1_000
    };
    match obj_type {
        CAN_MESSAGE | CAN_MESSAGE2 => decode_classic(t_us, payload),
        CAN_ERROR_EXT => Some(Rec {
            offset: 0,
            t_us,
            starts_container: false,
            kind: RecKind::Event {
                label: "Error frame".into(),
            },
        }),
        CAN_FD_MESSAGE => {
            if payload.len() < 16 {
                return None;
            }
            let id = u32::from_le_bytes(copy4(payload, 4).ok()?) & 0x1FFF_FFFF;
            let valid = payload[15] as usize;
            let data_at = 21.min(payload.len());
            Some(frame_from(t_us, id, &payload[data_at..], valid))
        }
        CAN_FD_MESSAGE_64 => {
            if payload.len() < 40 {
                return None;
            }
            let valid = payload[2] as usize;
            let id = u32::from_le_bytes(copy4(payload, 4).ok()?) & 0x1FFF_FFFF;
            Some(frame_from(t_us, id, &payload[40..], valid))
        }
        _ => None,
    }
}

fn decode_classic(t_us: u64, payload: &[u8]) -> Option<Rec> {
    if payload.len() < 16 {
        return None;
    }
    let dlc = payload[3].min(8);
    let raw_id = u32::from_le_bytes(copy4(payload, 4).ok()?);
    let extended = raw_id & 0x8000_0000 != 0;
    let id = raw_id & 0x1FFF_FFFF;
    let channel = payload[0];
    Some(frame_from_slice(
        t_us,
        id,
        extended,
        channel,
        &payload[8..],
        dlc as usize,
    ))
}

fn frame_from(t_us: u64, id: u32, raw: &[u8], valid: usize) -> Rec {
    let extended = id > 0x7FF;
    frame_from_slice(t_us, id, extended, 0, raw, valid)
}

fn frame_from_slice(
    t_us: u64,
    id: u32,
    extended: bool,
    channel: u8,
    raw: &[u8],
    valid: usize,
) -> Rec {
    let n = valid.min(MAX_DATA).min(raw.len());
    let mut data: FrameData = [0; MAX_DATA];
    data[..n].copy_from_slice(&raw[..n]);
    Rec {
        offset: 0,
        t_us,
        starts_container: false,
        kind: RecKind::Frame {
            id,
            extended,
            channel,
            dlc: n as u8,
            data,
        },
    }
}

fn copy2(bytes: &[u8], at: usize) -> Result<[u8; 2]> {
    bytes
        .get(at..at + 2)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| Error::msg("truncated BLF field"))
}

fn copy4(bytes: &[u8], at: usize) -> Result<[u8; 4]> {
    bytes
        .get(at..at + 4)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| Error::msg("truncated BLF field"))
}

fn copy8(bytes: &[u8], at: usize) -> Result<[u8; 8]> {
    bytes
        .get(at..at + 8)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| Error::msg("truncated BLF field"))
}

#[cfg(test)]
mod tests {
    use crate::index::IndexedLog;

    #[test]
    fn reads_uncompressed_and_zlib_containers() {
        let raw = std::fs::read(fixture("tiny_raw.blf")).unwrap();
        let zlib = std::fs::read(fixture("tiny_zlib.blf")).unwrap();
        for bytes in [raw, zlib] {
            let log = IndexedLog::open_bytes(bytes, None).unwrap();
            assert_eq!(log.format().label(), "BLF");
            assert_eq!(log.frame_count(), 2);
            let values = log.step_frame(1, true).unwrap().unwrap();
            assert_eq!(values.message_id, Some(0x1A0));
            assert!(log.events().iter().any(|(_, label)| label == "Error frame"));
        }
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name)
    }
}
