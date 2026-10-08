//! Vector BLF reader.
//!
//! Objects are pulled one container at a time. A zlib container is inflated
//! into a bounded buffer and discarded after its frames are indexed, so a
//! multi-gigabyte log is not copied into a second in-memory trace.
//! Classic CAN, CAN FD (first 64 bytes), and error frames are accepted.
//! A truncated or unknown object is skipped by the scanner.

use crate::error::{Error, Result};
use crate::scan::id::split_flagged;
use crate::scan::{FrameData, Rec, MAX_CLASSIC_DATA, MAX_DATA};
use flate2::read::ZlibDecoder;
use std::io::Read;

pub const FILE_MAGIC: &[u8; 4] = b"LOGG";
pub const OBJECT_MAGIC: &[u8; 4] = b"LOBJ";
/// Signature, header size, header version, object size, object type.
pub const OBJECT_HEAD_LEN: usize = 16;
pub const MAX_OBJECT_LEN: usize = 16 * 1024 * 1024;
pub const CONTAINER_CAP: usize = 8 * 1024 * 1024;
/// A log container's body starts with a header of this size, then the objects.
const CONTAINER_HEADER_LEN: usize = 16;
const INFLATE_CHUNK: usize = 64 * 1024;

const HEADER_SIZE_AT: usize = 4;
const HEADER_VERSION_AT: usize = 6;
const OBJECT_SIZE_AT: usize = 8;
const OBJECT_TYPE_AT: usize = 12;
const FLAGS_AT: usize = 16;
const TIMESTAMP_AT: usize = 24;
/// Shortest object per header version: the header through the timestamp (v1)
/// and the extra fields of v2.
const V1_MIN_LEN: usize = 32;
const V2_MIN_LEN: usize = 40;

const CAN_MESSAGE: u32 = 1;
const LOG_CONTAINER: u32 = 10;
const CAN_ERROR_EXT: u32 = 73;
const CAN_MESSAGE2: u32 = 86;
const CAN_FD_MESSAGE: u32 = 100;
const CAN_FD_MESSAGE_64: u32 = 101;
const TIME_TEN_US: u32 = 1;

/// Classic CAN payload: channel at 0, DLC at 3, raw id at 4, data at 8.
const CLASSIC_MIN_LEN: usize = 16;
const CLASSIC_DLC_AT: usize = 3;
const CLASSIC_ID_AT: usize = 4;
const CLASSIC_DATA_AT: usize = 8;
/// CAN FD payload: channel (u16) at 0, raw id at 4, valid length at 14, data at 20.
const FD_MIN_LEN: usize = 20;
const FD_ID_AT: usize = 4;
const FD_VALID_AT: usize = 14;
const FD_DATA_AT: usize = 20;
/// CAN FD 64 payload: channel at 0, valid length at 2, raw id at 4, data at 40.
const FD64_MIN_LEN: usize = 40;
const FD64_VALID_AT: usize = 2;
const FD64_ID_AT: usize = 4;
const FD64_DATA_AT: usize = 40;

pub fn is_container(object_type: u32) -> bool {
    object_type == LOG_CONTAINER
}

fn is_frame_object(object_type: u32) -> bool {
    matches!(
        object_type,
        CAN_MESSAGE | CAN_MESSAGE2 | CAN_ERROR_EXT | CAN_FD_MESSAGE | CAN_FD_MESSAGE_64
    )
}

/// The object size and type from the first bytes of an object whose signature
/// has been checked.
pub fn object_size_and_type(head: &[u8; OBJECT_HEAD_LEN]) -> (usize, u32) {
    let word = |at: usize| u32::from_le_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]);
    (word(OBJECT_SIZE_AT) as usize, word(OBJECT_TYPE_AT))
}

/// Inflate one log container into `out`, which is cleared first. The result is
/// capped. `out` is left empty on an error.
pub fn inflate_container(object: &[u8], out: &mut Vec<u8>) -> Result<()> {
    out.clear();
    let result = inflate_into(object, out);
    if result.is_err() {
        out.clear();
    }
    result
}

fn inflate_into(object: &[u8], out: &mut Vec<u8>) -> Result<()> {
    if object.len() < OBJECT_HEAD_LEN || &object[0..4] != OBJECT_MAGIC {
        return Err(Error::invalid("BLF container is missing LOBJ"));
    }
    let header_size = u16::from_le_bytes(copy2(object, HEADER_SIZE_AT)?) as usize;
    if header_size > object.len() {
        return Err(Error::invalid(
            "BLF container header is larger than the object",
        ));
    }
    let body = &object[header_size..];
    if body.len() < CONTAINER_HEADER_LEN {
        return Err(Error::invalid("BLF log container is truncated"));
    }
    let method = u16::from_le_bytes(copy2(body, 0)?);
    let payload = &body[CONTAINER_HEADER_LEN..];
    match method {
        0 => {
            if payload.len() > CONTAINER_CAP {
                return Err(Error::invalid("BLF container is larger than 8MB"));
            }
            out.extend_from_slice(payload);
            Ok(())
        }
        2 => {
            let mut decoder = ZlibDecoder::new(payload);
            let mut chunk = [0u8; INFLATE_CHUNK];
            loop {
                let n = decoder
                    .read(&mut chunk)
                    .map_err(|err| Error::invalid(format!("BLF zlib container: {err}")))?;
                if n == 0 {
                    break;
                }
                if out.len() + n > CONTAINER_CAP {
                    return Err(Error::invalid("BLF container is larger than 8MB"));
                }
                out.extend_from_slice(&chunk[..n]);
            }
            Ok(())
        }
        other => Err(Error::invalid(format!(
            "BLF compression method {other} is not supported"
        ))),
    }
}

/// Next decodable object inside an inflated container. `at` advances past it.
/// A container whose objects run out mid-object is an error that says where,
/// so the caller can report the dropped bytes. `at` moves to the end of the
/// container in that case. A CAN object that cannot be decoded is an error too,
/// with `at` just past it, so the caller can count it and go on.
pub fn next_inner_checked(
    data: &[u8],
    at: &mut usize,
    container_off: u64,
) -> std::result::Result<Option<Rec>, String> {
    loop {
        let Some(rest) = data.get(*at..) else {
            return Ok(None);
        };
        let Some(rel) = rest.windows(4).position(|word| word == OBJECT_MAGIC) else {
            return Ok(None);
        };
        let cursor = *at + rel;
        if cursor + OBJECT_HEAD_LEN > data.len() {
            *at = data.len();
            return Err(format!(
                "object at container offset {cursor} is cut short; the rest of the container was dropped"
            ));
        }
        let obj_size = u32::from_le_bytes(
            copy4(data, cursor + OBJECT_SIZE_AT).map_err(|err| err.to_string())?,
        ) as usize;
        if obj_size < OBJECT_HEAD_LEN || cursor + obj_size > data.len() {
            *at = data.len();
            return Err(format!(
                "object at container offset {cursor} declares size {obj_size}, which is not usable; the rest of the container was dropped"
            ));
        }
        let object = &data[cursor..cursor + obj_size];
        *at = cursor + obj_size;
        match decode_object(object) {
            Ok(Some(rec)) => return Ok(Some(rec.placed(container_off, false))),
            Ok(None) => {}
            Err(message) => {
                return Err(format!("object at container offset {cursor}: {message}"));
            }
        }
    }
}

/// The record in one object. `Ok(None)` is an object that is not a CAN frame or
/// error frame, which is left out without comment. `Err` is a CAN object that
/// cannot be decoded.
pub fn decode_object(object: &[u8]) -> std::result::Result<Option<Rec>, String> {
    if object.len() < OBJECT_HEAD_LEN || &object[0..4] != OBJECT_MAGIC {
        return Err("BLF object has no LOBJ header".to_string());
    }
    let obj_type =
        u32::from_le_bytes(copy4(object, OBJECT_TYPE_AT).map_err(|err| err.to_string())?);
    if !is_frame_object(obj_type) {
        return Ok(None);
    }
    let header_version =
        u16::from_le_bytes(copy2(object, HEADER_VERSION_AT).map_err(|err| err.to_string())?);
    if !matches!(header_version, 1 | 2) {
        return Err(format!(
            "BLF frame object has header version {header_version}, which is not supported"
        ));
    }
    decode_frame_object(object, obj_type, header_version)
        .map(Some)
        .ok_or_else(|| "BLF frame object is too short to decode".to_string())
}

fn decode_frame_object(object: &[u8], obj_type: u32, header_version: u16) -> Option<Rec> {
    let min_len = if header_version == 1 {
        V1_MIN_LEN
    } else {
        V2_MIN_LEN
    };
    if object.len() < min_len {
        return None;
    }
    let header_size = u16::from_le_bytes(copy2(object, HEADER_SIZE_AT).ok()?) as usize;
    let flags = u32::from_le_bytes(copy4(object, FLAGS_AT).ok()?);
    let timestamp = u64::from_le_bytes(copy8(object, TIMESTAMP_AT).ok()?);
    let payload = object.get(header_size..)?;
    let t_us = if flags == TIME_TEN_US {
        timestamp.saturating_mul(10)
    } else {
        timestamp / 1_000
    };
    match obj_type {
        CAN_MESSAGE | CAN_MESSAGE2 => decode_classic(t_us, payload),
        CAN_ERROR_EXT => Some(Rec::event(t_us, "Error frame".into())),
        CAN_FD_MESSAGE => {
            if payload.len() < FD_MIN_LEN {
                return None;
            }
            let channel = u8::try_from(u16::from_le_bytes(copy2(payload, 0).ok()?)).ok()?;
            let raw_id = u32::from_le_bytes(copy4(payload, FD_ID_AT).ok()?);
            let valid = payload[FD_VALID_AT] as usize;
            let (id, extended) = split_flagged(raw_id);
            Some(frame_from_slice(
                t_us,
                id,
                extended,
                channel,
                &payload[FD_DATA_AT..],
                valid,
            ))
        }
        CAN_FD_MESSAGE_64 => {
            if payload.len() < FD64_MIN_LEN {
                return None;
            }
            let channel = payload[0];
            let valid = payload[FD64_VALID_AT] as usize;
            let raw_id = u32::from_le_bytes(copy4(payload, FD64_ID_AT).ok()?);
            let (id, extended) = split_flagged(raw_id);
            Some(frame_from_slice(
                t_us,
                id,
                extended,
                channel,
                &payload[FD64_DATA_AT..],
                valid,
            ))
        }
        _ => None,
    }
}

fn decode_classic(t_us: u64, payload: &[u8]) -> Option<Rec> {
    if payload.len() < CLASSIC_MIN_LEN {
        return None;
    }
    let dlc = payload[CLASSIC_DLC_AT].min(MAX_CLASSIC_DATA as u8);
    let raw_id = u32::from_le_bytes(copy4(payload, CLASSIC_ID_AT).ok()?);
    let (id, extended) = split_flagged(raw_id);
    let channel = payload[0];
    Some(frame_from_slice(
        t_us,
        id,
        extended,
        channel,
        &payload[CLASSIC_DATA_AT..],
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
    Rec::frame(t_us, id, extended, channel, n as u8, data)
}

fn copy2(bytes: &[u8], at: usize) -> Result<[u8; 2]> {
    bytes
        .get(at..at + 2)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| Error::invalid("truncated BLF field"))
}

fn copy4(bytes: &[u8], at: usize) -> Result<[u8; 4]> {
    bytes
        .get(at..at + 4)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| Error::invalid("truncated BLF field"))
}

fn copy8(bytes: &[u8], at: usize) -> Result<[u8; 8]> {
    bytes
        .get(at..at + 8)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| Error::invalid("truncated BLF field"))
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

    fn good_frame_payload() -> Vec<u8> {
        let mut payload = vec![0u8; 16];
        payload[0] = 1;
        payload[3] = 8;
        payload[4..8].copy_from_slice(&0x1A0u32.to_le_bytes());
        payload[8..16].copy_from_slice(&[0x80, 0x0C, 0x88, 0x13, 0x78, 0x64, 0x00, 0x00]);
        payload
    }

    #[test]
    fn a_top_level_frame_object_too_short_to_decode_is_counted_as_skipped() {
        let bytes = blf_file(&[
            lobj(CAN_MESSAGE, &good_frame_payload()),
            lobj(CAN_MESSAGE, &[0u8; 4]),
            lobj(CAN_MESSAGE, &good_frame_payload()),
        ]);
        let log = IndexedLog::open_bytes(bytes, None).unwrap();
        assert_eq!(log.frame_count(), 2);
        assert_eq!(log.skipped(), 1);
        assert!(log
            .warnings()
            .contains(&"byte 192: BLF frame object is too short to decode".to_string()));
    }

    #[test]
    fn a_frame_object_too_short_inside_a_container_is_counted_and_the_rest_is_kept() {
        let mut body = vec![0u8; 16];
        body.extend_from_slice(&lobj(CAN_MESSAGE, &good_frame_payload()));
        body.extend_from_slice(&lobj(CAN_MESSAGE, &[0u8; 4]));
        body.extend_from_slice(&lobj(CAN_MESSAGE, &good_frame_payload()));
        let bytes = blf_file(&[lobj(LOG_CONTAINER, &body)]);
        let log = IndexedLog::open_bytes(bytes, None).unwrap();
        assert_eq!(log.frame_count(), 2);
        assert_eq!(log.skipped(), 1);
        assert!(log.warnings().contains(
            &"byte 144: object at container offset 48: BLF frame object is too short to decode"
                .to_string()
        ));
    }

    #[test]
    fn a_frame_object_with_an_unknown_header_version_is_counted_as_skipped() {
        let mut odd = lobj(CAN_MESSAGE, &good_frame_payload());
        odd[6..8].copy_from_slice(&3u16.to_le_bytes());
        let bytes = blf_file(&[odd, lobj(CAN_MESSAGE, &good_frame_payload())]);
        let log = IndexedLog::open_bytes(bytes, None).unwrap();
        assert_eq!(log.frame_count(), 1);
        assert_eq!(log.skipped(), 1);
        assert!(log.warnings().contains(
            &"byte 144: BLF frame object has header version 3, which is not supported".to_string()
        ));
    }

    #[test]
    fn objects_that_are_not_can_frames_are_left_out_without_a_count() {
        let mut body = vec![0u8; 16];
        body.extend_from_slice(&lobj(65, &[0u8; 8]));
        body.extend_from_slice(&lobj(CAN_MESSAGE, &good_frame_payload()));
        let bytes = blf_file(&[lobj(65, &[0u8; 4]), lobj(LOG_CONTAINER, &body)]);
        let log = IndexedLog::open_bytes(bytes, None).unwrap();
        assert_eq!(log.frame_count(), 1);
        assert_eq!(log.skipped(), 0);
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
