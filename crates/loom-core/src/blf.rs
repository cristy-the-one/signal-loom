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
/// A container whose objects run out mid-object is an error that says where,
/// so the caller can report the dropped bytes. `at` moves
/// to the end of the container in that case.
pub fn next_inner_checked(
    data: &[u8],
    at: &mut usize,
    container_off: u64,
) -> std::result::Result<Option<Rec>, String> {
    loop {
        let Some(rest) = data.get(*at..) else {
            return Ok(None);
        };
        let Some(rel) = rest.windows(4).position(|word| word == b"LOBJ") else {
            return Ok(None);
        };
        let cursor = *at + rel;
        if cursor + 16 > data.len() {
            *at = data.len();
            return Err(format!(
                "object at container offset {cursor} is cut short; the rest of the container was dropped"
            ));
        }
        let obj_size =
            u32::from_le_bytes(copy4(data, cursor + 8).map_err(|err| err.to_string())?) as usize;
        if obj_size < 16 || cursor + obj_size > data.len() {
            *at = data.len();
            return Err(format!(
                "object at container offset {cursor} declares size {obj_size}, which is not usable; the rest of the container was dropped"
            ));
        }
        let object = &data[cursor..cursor + obj_size];
        *at = cursor + obj_size;
        if let Some(mut rec) = decode_object(object) {
            rec.offset = container_off;
            return Ok(Some(rec));
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
            if payload.len() < 20 {
                return None;
            }
            let channel = u8::try_from(u16::from_le_bytes(copy2(payload, 0).ok()?)).ok()?;
            let raw_id = u32::from_le_bytes(copy4(payload, 4).ok()?);
            let valid = payload[14] as usize;
            Some(frame_from_slice(
                t_us,
                raw_id & 0x1FFF_FFFF,
                raw_id & 0x8000_0000 != 0,
                channel,
                &payload[20..],
                valid,
            ))
        }
        CAN_FD_MESSAGE_64 => {
            if payload.len() < 40 {
                return None;
            }
            let channel = payload[0];
            let valid = payload[2] as usize;
            let raw_id = u32::from_le_bytes(copy4(payload, 4).ok()?);
            Some(frame_from_slice(
                t_us,
                raw_id & 0x1FFF_FFFF,
                raw_id & 0x8000_0000 != 0,
                channel,
                &payload[40..],
                valid,
            ))
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
    use super::{next_inner_checked, CAN_MESSAGE, LOG_CONTAINER};
    use crate::index::IndexedLog;
    use crate::scan::{LogFormat, RecKind, Scanner};

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

    #[test]
    fn fd_frame_keeps_its_channel_and_extended_flag() {
        let mut payload = vec![0u8; 84];
        payload[0..2].copy_from_slice(&2u16.to_le_bytes());
        payload[3] = 8;
        payload[4..8].copy_from_slice(&0x8123_4567u32.to_le_bytes());
        payload[14] = 8;
        payload[20..28].copy_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
        let frame = read_one_frame(&blf_file(&[lobj(100, &payload)]));
        assert_eq!(
            frame,
            (
                0x0123_4567,
                true,
                2,
                8,
                vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]
            )
        );
    }

    #[test]
    fn fd_64_frame_keeps_its_channel_and_extended_flag() {
        let mut payload = vec![0u8; 48];
        payload[0] = 2;
        payload[2] = 8;
        payload[4..8].copy_from_slice(&0x8123_4567u32.to_le_bytes());
        payload[40..48].copy_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
        let frame = read_one_frame(&blf_file(&[lobj(101, &payload)]));
        assert_eq!(
            frame,
            (
                0x0123_4567,
                true,
                2,
                8,
                vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]
            )
        );
    }

    #[test]
    fn container_cut_short_is_reported_with_its_offset() {
        let mut payload = vec![0u8; 16];
        payload[0] = 1;
        payload[3] = 8;
        payload[4..8].copy_from_slice(&0x1A0u32.to_le_bytes());
        payload[8..16].copy_from_slice(&[0x80, 0x0C, 0x88, 0x13, 0x78, 0x64, 0x00, 0x00]);
        let mut container = lobj(CAN_MESSAGE, &payload);
        container.extend_from_slice(b"LOBJ");
        container.extend_from_slice(&[0u8; 8]);
        let mut at = 0;
        let first = next_inner_checked(&container, &mut at, 0).unwrap().unwrap();
        assert!(matches!(first.kind, RecKind::Frame { id: 0x1A0, .. }));
        let err = next_inner_checked(&container, &mut at, 0).unwrap_err();
        assert_eq!(
            err,
            "object at container offset 48 is cut short; the rest of the container was dropped"
        );
        assert_eq!(at, container.len());
    }

    #[test]
    fn container_with_a_cut_short_object_is_counted_as_skipped() {
        let mut payload = vec![0u8; 16];
        payload[0] = 1;
        payload[3] = 8;
        payload[4..8].copy_from_slice(&0x1A0u32.to_le_bytes());
        payload[8..16].copy_from_slice(&[0x80, 0x0C, 0x88, 0x13, 0x78, 0x64, 0x00, 0x00]);
        // A container body starts with a 16-byte header. Method 0 is uncompressed.
        let mut body = vec![0u8; 16];
        body.extend_from_slice(&lobj(CAN_MESSAGE, &payload));
        body.extend_from_slice(b"LOBJ");
        body.extend_from_slice(&[0u8; 8]);
        let bytes = blf_file(&[lobj(LOG_CONTAINER, &body)]);
        let log = IndexedLog::open_bytes(bytes, None).unwrap();
        assert_eq!(log.format().label(), "BLF");
        assert_eq!(log.frame_count(), 1);
        assert_eq!(log.skipped(), 1);
        assert!(log.warnings().contains(
            &"byte 144: object at container offset 48 is cut short; the rest of the container was dropped"
                .to_string()
        ));
    }

    fn lobj(obj_type: u32, payload: &[u8]) -> Vec<u8> {
        let size = 32 + payload.len();
        let mut out = Vec::with_capacity(size);
        out.extend_from_slice(b"LOBJ");
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&obj_type.to_le_bytes());
        out.extend_from_slice(&[0u8; 16]);
        out.extend_from_slice(payload);
        out
    }

    fn blf_file(objects: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"LOGG".to_vec();
        out.extend_from_slice(&144u32.to_le_bytes());
        out.resize(144, 0);
        for object in objects {
            out.extend_from_slice(object);
        }
        out
    }

    fn read_one_frame(bytes: &[u8]) -> (u32, bool, u8, u8, Vec<u8>) {
        let mut cursor = std::io::Cursor::new(bytes.to_vec());
        let mut scanner = Scanner::open(&mut cursor, LogFormat::Blf).unwrap();
        let rec = scanner.next_rec().unwrap().unwrap();
        let RecKind::Frame {
            id,
            extended,
            channel,
            dlc,
            data,
        } = rec.kind
        else {
            panic!("expected a frame");
        };
        (id, extended, channel, dlc, data[..dlc as usize].to_vec())
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name)
    }
}
