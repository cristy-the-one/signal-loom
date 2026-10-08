//! candump logs: `(seconds) can0 1A0#1122`, `(seconds) 1A0#1122`, `can0 1A0 [8] 11 22 …`.

use super::id::{implied_extended, mask_29, parse_id, IdBase};
use super::source::peek_prefix;
use super::text::{
    check_payload_len, parse_payload, read_hex_bytes, seconds_to_us, LineParser, LineResult,
    TextReader, Tokens,
};
use super::{ReadSeek, Rec, MAX_DATA};
use crate::error::{Error, Result};
use std::io::SeekFrom;

/// A first stamp above this many microseconds is an epoch clock, rebased to zero.
const EPOCH_FLOOR_US: u64 = 10_000_000_000;
/// First backward read when looking for the clock before a checkpoint.
const CLOCK_SCAN_STEP: u64 = 4096;
/// An id written with this many hex digits is a 29-bit id.
const EXTENDED_ID_DIGITS: usize = 8;

struct Candump {
    /// Subtract this from timestamps. Epoch logs start at zero.
    origin_us: u64,
    /// The clock a line without a timestamp takes.
    last_us: u64,
    spans: Vec<(usize, usize)>,
}

/// A candump reader that continues at `offset`, with the clock a read from the
/// top would hold there.
pub(super) fn reader(reader: &mut dyn ReadSeek, offset: u64) -> Result<TextReader<'_>> {
    let origin_us = peek_origin(reader)?;
    let last_us = clock_before(reader, offset, origin_us)?;
    let parser = Candump {
        origin_us,
        last_us,
        spans: Vec::new(),
    };
    TextReader::open(reader, Box::new(parser), offset)
}

impl LineParser for Candump {
    fn parse(&mut self, line: &str) -> LineResult {
        if contains_ignore_case(line, "ERRORFRAME") {
            if let Some(t_us) = candump_time(line, self.origin_us) {
                self.last_us = t_us;
            }
            return Ok(Some(Rec::event(self.last_us, "Error frame".into())));
        }
        let (t_us, text) = if let Some(rest) = line.strip_prefix('(') {
            let (num, after) = rest
                .split_once(')')
                .ok_or("candump timestamp is missing a closing parenthesis")?;
            let stamp = seconds_to_us(num.trim())?;
            self.last_us = stamp.saturating_sub(self.origin_us);
            (self.last_us, after.trim())
        } else {
            // A line with no clock takes the time of the record before it.
            (self.last_us, line)
        };
        let mut spans = std::mem::take(&mut self.spans);
        let result = frame(&Tokens::split(text, &mut spans), t_us);
        self.spans = spans;
        result
    }
}

fn frame(tokens: &Tokens, t_us: u64) -> LineResult {
    let hash = tokens.iter().enumerate().find_map(|(index, token)| {
        token
            .split_once('#')
            .map(|(id, payload)| (index, id, payload))
    });
    if let Some((index, id_tok, payload)) = hash {
        let channel = index
            .checked_sub(1)
            .and_then(|iface| tokens.get(iface))
            .map_or(0, iface_channel);
        return hash_frame(t_us, id_tok, payload, channel);
    }
    match (tokens.get(0), tokens.get(1), tokens.get(2)) {
        (Some(iface), Some(id), Some(len)) if len.starts_with('[') => {
            bracket_frame(t_us, iface, id, len, &tokens.from(3))
        }
        (Some(id), Some(len), _) if len.starts_with('[') => {
            bracket_frame(t_us, "", id, len, &tokens.from(2))
        }
        _ => Err("candump line was not recognized".into()),
    }
}

/// `can0 1A0 [8] 11 22 …`
fn bracket_frame(
    t_us: u64,
    iface: &str,
    id_tok: &str,
    len_tok: &str,
    bytes: &Tokens,
) -> LineResult {
    let (id, extended) = candump_id(id_tok)?;
    if let Ok(want) = len_tok.trim_matches(['[', ']']).parse::<usize>() {
        check_payload_len(bytes.len(), want)?;
    }
    let (data, n) = read_hex_bytes(bytes, MAX_DATA)?;
    Ok(Some(Rec::frame(
        t_us,
        id,
        extended,
        iface_channel(iface),
        n,
        data,
    )))
}

/// `1A0#1122`, `1A0##1<flags><payload>` for CAN FD, `1A0#R` for a remote frame.
fn hash_frame(t_us: u64, id_tok: &str, payload: &str, channel: u8) -> LineResult {
    let fd = payload.starts_with('#');
    let mut data_tok = payload.trim_start_matches('#');
    if fd && data_tok.len() % 2 == 1 {
        data_tok = data_tok
            .get(1..)
            .ok_or("candump FD flags are not a hex digit")?;
    }
    let (id, extended) = candump_id(id_tok)?;
    if data_tok.is_empty() || data_tok.starts_with(['R', 'r']) {
        return Ok(Some(Rec::frame(
            t_us,
            id,
            extended,
            channel,
            0,
            [0; MAX_DATA],
        )));
    }
    let (data, dlc) = parse_payload(data_tok)?;
    Ok(Some(Rec::frame(t_us, id, extended, channel, dlc, data)))
}

/// The id, and whether it is 29-bit: eight digits say so, otherwise its value does.
fn candump_id(text: &str) -> std::result::Result<(u32, bool), String> {
    let digits = text.trim().trim_end_matches(['x', 'X']);
    let id = parse_id(digits, IdBase::Hex).ok_or_else(|| format!("bad candump id {digits}"))?;
    let id = mask_29(id);
    Ok((
        id,
        digits.len() == EXTENDED_ID_DIGITS || implied_extended(id),
    ))
}

/// The channel number an interface name ends in: `can1` is 1, `vcan` is 0.
fn iface_channel(iface: &str) -> u8 {
    let name = iface.trim_end_matches(|c: char| c.is_ascii_digit());
    iface[name.len()..].parse().unwrap_or(0)
}

fn contains_ignore_case(text: &str, needle: &str) -> bool {
    text.as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn candump_time(line: &str, origin: u64) -> Option<u64> {
    let rest = line.trim().strip_prefix('(')?;
    let (num, _) = rest.split_once(')')?;
    let stamp = seconds_to_us(num.trim()).ok()?;
    Some(stamp.saturating_sub(origin))
}

fn peek_origin(reader: &mut dyn ReadSeek) -> Result<u64> {
    let text = peek_prefix(reader)?;
    for raw in text.lines() {
        let Some(rest) = raw.trim().strip_prefix('(') else {
            continue;
        };
        let Some((num, _)) = rest.split_once(')') else {
            continue;
        };
        if let Ok(us) = seconds_to_us(num.trim()) {
            return Ok(if us > EPOCH_FLOOR_US { us } else { 0 });
        }
    }
    Ok(0)
}

/// The candump clock a build from byte 0 holds just before `offset`: the last
/// timestamp above it, or 0 when no line before has one. Reads backwards in
/// growing chunks, so a clockless run of any length costs a single pass.
fn clock_before(reader: &mut dyn ReadSeek, offset: u64, origin: u64) -> Result<u64> {
    let mut window: Vec<u8> = Vec::new();
    let mut lo = offset;
    let mut step = CLOCK_SCAN_STEP;
    while lo > 0 {
        let take = step.min(lo);
        lo -= take;
        let mut head = vec![0u8; take as usize];
        reader
            .seek(SeekFrom::Start(lo))
            .map_err(|err| Error::io("could not seek log", err))?;
        reader
            .read_exact(&mut head)
            .map_err(|err| Error::io("could not read log", err))?;
        head.extend_from_slice(&window);
        window = head;
        if let Some(t_us) = last_stamp(&window, lo == 0, origin) {
            return Ok(t_us);
        }
        step = step.saturating_mul(2);
    }
    Ok(0)
}

/// Last candump timestamp in `window`, the bytes just before an offset. Unless the
/// window starts at byte 0, its first line may be cut, so it is not used.
fn last_stamp(window: &[u8], from_start: bool, origin: u64) -> Option<u64> {
    let body = if from_start {
        window
    } else {
        let first_break = window.iter().position(|byte| *byte == b'\n')?;
        &window[first_break + 1..]
    };
    body.split(|byte| *byte == b'\n').rev().find_map(|raw| {
        let text = std::str::from_utf8(raw).ok()?;
        candump_time(text.trim_start_matches('\u{feff}'), origin)
    })
}
