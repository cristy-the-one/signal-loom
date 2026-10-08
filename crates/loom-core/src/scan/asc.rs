//! Vector ASC text: classic CAN, CAN FD, error frames, hex or decimal ids,
//! absolute or relative time, and stamps in microseconds.

use super::id::{mask_29, parse_id, IdBase};
use super::source::{peek_prefix, read_retrying};
use super::text::{
    check_payload_len, read_hex_bytes, seconds_to_us, LineParser, LineResult, TextReader, Tokens,
};
use super::{ReadSeek, Rec};
use crate::error::{Error, Result};
use std::io::SeekFrom;

/// Fewest stamps in the preamble that can show the time column is microseconds.
const MICROS_MIN_STAMPS: usize = 8;
/// A microsecond log has a stamp above this many "seconds" within its first lines.
const MICROS_LARGE_STAMP: f64 = 1000.0;
/// Bytes read at a checkpoint to find the stamp of the line it points at.
const STAMP_PEEK: usize = 256;

struct AscMode {
    hex: bool,
    relative: bool,
    micros: bool,
}

impl Default for AscMode {
    fn default() -> Self {
        Self {
            hex: true,
            relative: false,
            micros: false,
        }
    }
}

impl AscMode {
    fn id_base(&self) -> IdBase {
        if self.hex {
            IdBase::Hex
        } else {
            IdBase::Decimal
        }
    }

    /// The time column of one line, in microseconds.
    fn stamp_us(&self, token: &str) -> std::result::Result<u64, String> {
        if self.micros {
            whole_us(token)
        } else {
            seconds_to_us(token)
        }
    }
}

struct AscParser {
    mode: AscMode,
    /// Running clock for `timestamps relative`.
    clock_us: u64,
    /// Tool-level `Node.Message` lines left out of the frame stream.
    symbolic: u64,
    spans: Vec<(usize, usize)>,
}

/// An ASC reader that continues at `offset`. `at_us` is the time of the record
/// there, when known. A relative-time log needs it to resume without reading
/// from the top.
pub(super) fn reader(
    reader: &mut dyn ReadSeek,
    offset: u64,
    at_us: Option<u64>,
) -> Result<TextReader<'_>> {
    let mode = peek_asc_mode(reader)?;
    let mut clock_us = 0;
    let mut replay = false;
    if mode.relative && offset > 0 {
        let clock = match at_us {
            Some(at_us) => clock_before(reader, offset, at_us, &mode)?,
            None => None,
        };
        match clock {
            Some(clock) => clock_us = clock,
            None => replay = true,
        }
    }
    let parser = AscParser {
        mode,
        clock_us,
        symbolic: 0,
        spans: Vec::new(),
    };
    let mut text = TextReader::open(reader, Box::new(parser), if replay { 0 } else { offset })?;
    if replay {
        text.skip_until(offset)?;
    }
    Ok(text)
}

impl LineParser for AscParser {
    fn parse(&mut self, line: &str) -> LineResult {
        if is_preamble(line) {
            return Ok(None);
        }
        let mut spans = std::mem::take(&mut self.spans);
        let result = self.parse_fields(&Tokens::split(line, &mut spans));
        self.spans = spans;
        result
    }

    fn warning(&self) -> Option<String> {
        self.mode.micros.then(|| {
            "ASC timestamps are whole numbers in microseconds, not seconds; read them as microseconds"
                .to_string()
        })
    }

    fn summary(&self) -> Option<String> {
        (self.symbolic > 0).then(|| {
            format!(
                "{} symbolic Node.Message lines were left out: they are tool-level messages, not CAN frames with an id",
                self.symbolic
            )
        })
    }
}

impl AscParser {
    fn parse_fields(&mut self, tokens: &Tokens) -> LineResult {
        if is_marker(tokens) {
            return Ok(None);
        }
        if is_symbolic(tokens) {
            self.symbolic += 1;
            return Ok(None);
        }
        let (Some(stamp_tok), Some(channel_tok), Some(id_tok)) =
            (tokens.get(0), tokens.get(1), tokens.get(2))
        else {
            return Err("ASC line is too short".into());
        };
        let stamp = self.mode.stamp_us(stamp_tok)?;
        let t_us = if self.mode.relative {
            self.clock_us = self.clock_us.saturating_add(stamp);
            self.clock_us
        } else {
            stamp
        };
        if tokens
            .iter()
            .any(|token| token.eq_ignore_ascii_case("ErrorFrame"))
        {
            return Ok(Some(Rec::event(t_us, "Error frame".into())));
        }
        if let Some(fd) = tokens
            .iter()
            .position(|token| token.eq_ignore_ascii_case("CANFD"))
        {
            return fd_frame(tokens, fd, t_us);
        }
        let channel = channel_tok
            .parse::<u8>()
            .map_err(|_| "ASC channel is not a number")?;
        let (id, extended) = flagged_id(id_tok, self.mode.id_base())
            .ok_or_else(|| format!("bad ASC id {id_tok}"))?;
        let marker = data_marker(tokens).ok_or("ASC frame is missing the data marker")?;
        let dlc_tok = tokens
            .get(marker + 1)
            .ok_or("ASC frame is missing its DLC")?;
        let dlc = asc_dlc_len(dlc_tok).ok_or("ASC dlc is not a number")?;
        let payload = tokens.from(marker + 2);
        check_payload_len(payload.len(), dlc)?;
        let (data, n) = read_hex_bytes(&payload, dlc)?;
        Ok(Some(Rec::frame(t_us, id, extended, channel, n, data)))
    }
}

/// `<t> CANFD <ch> <dir> <id> <brs> <esi> d <dlc code> <len> <bytes…>`, `fd` being
/// the index of `CANFD`. Ids are always hex.
fn fd_frame(tokens: &Tokens, fd: usize, t_us: u64) -> LineResult {
    let channel = tokens
        .get(fd + 1)
        .ok_or("CAN FD line is missing its channel")?
        .parse::<u8>()
        .map_err(|_| "ASC channel is not a number")?;
    let raw_id = tokens.get(fd + 3).unwrap_or("");
    let (id, extended) =
        flagged_id(raw_id, IdBase::Hex).ok_or_else(|| format!("bad CAN FD id {raw_id}"))?;
    let marker = data_marker(tokens).ok_or("CAN FD line is missing the data marker")?;
    let len: usize = tokens
        .get(marker + 2)
        .ok_or("CAN FD line is missing its length")?
        .parse()
        .map_err(|_| "CAN FD length is not a number")?;
    let payload = tokens.from(marker + 3);
    check_payload_len(payload.len(), len)?;
    let (data, n) = read_hex_bytes(&payload, len)?;
    Ok(Some(Rec::frame(t_us, id, extended, channel, n, data)))
}

/// An id with an `x` suffix is 29-bit, and the suffix is the only thing that says so.
fn flagged_id(raw: &str, base: IdBase) -> Option<(u32, bool)> {
    let id = parse_id(raw.trim_end_matches(['x', 'X']), base)?;
    Some((mask_29(id), raw.ends_with(['x', 'X'])))
}

fn data_marker(tokens: &Tokens) -> Option<usize> {
    tokens.iter().position(|token| token == "d" || token == "D")
}

fn starts_with_ci(text: &str, prefix: &str) -> bool {
    text.as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
}

fn is_preamble(line: &str) -> bool {
    [
        "date ",
        "base ",
        "timestamps ",
        "internal ",
        "begin trigger",
        "end trigger",
        "//",
    ]
    .iter()
    .any(|prefix| starts_with_ci(line, prefix))
}

/// `0.000000 Start of measurement` and similar markers carry no frame.
fn is_marker(tokens: &Tokens) -> bool {
    let word = |index, want: &str| {
        tokens
            .get(index)
            .is_some_and(|token| token.eq_ignore_ascii_case(want))
    };
    word(1, "start") && word(2, "of") && word(3, "measurement")
}

/// `<t> <ch> Node.Message Tx d <len> …`: a message a tool logged by name after
/// reassembling it (a whole UDS transfer, say). It has no CAN id, and its
/// CAN frames are usually logged on their own lines.
fn is_symbolic(tokens: &Tokens) -> bool {
    let (Some(channel), Some(name), Some(direction), Some(marker)) =
        (tokens.get(1), tokens.get(2), tokens.get(3), tokens.get(4))
    else {
        return false;
    };
    let name = name.trim_end_matches(['x', 'X']);
    channel.chars().all(|c| c.is_ascii_digit())
        && !name.is_empty()
        && !name.chars().all(|c| c.is_ascii_hexdigit())
        && (direction.eq_ignore_ascii_case("rx") || direction.eq_ignore_ascii_case("tx"))
        && (marker == "d" || marker == "D")
}

/// A decimal byte count, or a single hex digit as a CAN FD DLC code.
fn asc_dlc_len(token: &str) -> Option<usize> {
    if token.chars().all(|c| c.is_ascii_digit()) {
        return token.parse().ok();
    }
    let code = u8::from_str_radix(token, 16)
        .ok()
        .filter(|_| token.len() == 1)?;
    Some(match code {
        0..=8 => usize::from(code),
        9 => 12,
        10 => 16,
        11 => 20,
        12 => 24,
        13 => 32,
        14 => 48,
        _ => 64,
    })
}

fn whole_us(text: &str) -> std::result::Result<u64, String> {
    let value: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("bad timestamp {text}"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("bad timestamp {text}"));
    }
    Ok(value.round() as u64)
}

/// Read the preamble, and decide whether the time column is microseconds.
/// Vector writes seconds with six decimals. A log whose stamps are all whole
/// numbers, with one above 1000 in the first lines, is microseconds: a real
/// seconds log would need its frames on exact seconds for over 16 minutes.
fn peek_asc_mode(reader: &mut dyn ReadSeek) -> Result<AscMode> {
    let text = peek_prefix(reader)?;
    let mut mode = AscMode::default();
    let mut stamps = 0usize;
    let mut whole = true;
    let mut large = false;
    for raw in text.lines() {
        let lower = raw.trim().to_ascii_lowercase();
        if lower.starts_with("base dec") {
            mode.hex = false;
        } else if lower.starts_with("base hex") {
            mode.hex = true;
        }
        if lower.contains("timestamps relative") {
            mode.relative = true;
        } else if lower.contains("timestamps absolute") {
            mode.relative = false;
        }
        let Some(first) = lower.split_whitespace().next() else {
            continue;
        };
        let Ok(value) = first.parse::<f64>() else {
            continue;
        };
        if !first.contains('.') || !value.is_finite() {
            continue;
        }
        stamps += 1;
        whole &= value.fract() == 0.0;
        large |= value > MICROS_LARGE_STAMP;
    }
    mode.micros = stamps >= MICROS_MIN_STAMPS && whole && large;
    Ok(mode)
}

/// The running clock of a relative-time log just before the record at `offset`,
/// whose own time is `at_us`: that time less the line's own step. `None` when
/// the line cannot be read, and the caller reads from the top instead.
fn clock_before(
    reader: &mut dyn ReadSeek,
    offset: u64,
    at_us: u64,
    mode: &AscMode,
) -> Result<Option<u64>> {
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|err| Error::msg(format!("could not seek log: {err}")))?;
    let mut head = [0u8; STAMP_PEEK];
    let n = read_retrying(reader, &mut head)
        .map_err(|err| Error::msg(format!("read failed: {err}")))?;
    let text = String::from_utf8_lossy(&head[..n]);
    // A second field proves the first was not cut off by the end of the peek.
    let mut fields = text.split_whitespace();
    let (Some(stamp), Some(_)) = (fields.next(), fields.next()) else {
        return Ok(None);
    };
    Ok(mode
        .stamp_us(stamp)
        .ok()
        .and_then(|step| at_us.checked_sub(step)))
}

#[cfg(test)]
mod tests {
    use crate::scan::{LogFormat, Scanner};
    use std::io::{Cursor, Read, Seek, SeekFrom};

    fn stamps(scanner: &mut Scanner) -> Vec<u64> {
        std::iter::from_fn(|| scanner.next_rec().unwrap())
            .map(|rec| rec.t_us)
            .collect()
    }

    #[test]
    fn a_line_without_a_dlc_token_is_skipped_and_counted() {
        let text = "\
base hex timestamps absolute
0.000000 1 1A0 Rx d 1 01
0.001000 1 1A0 Rx d
0.002000 CANFD 1 Rx 1A0 0 0 d 15
0.003000 1 1A0 Rx d 0
";
        let mut reader = Cursor::new(text.as_bytes().to_vec());
        let mut scanner = Scanner::open(&mut reader, LogFormat::Asc).unwrap();
        assert_eq!(stamps(&mut scanner), vec![0, 3_000]);
        assert_eq!(scanner.skipped(), 2);
        assert_eq!(
            scanner.notes(),
            vec![
                "line 3: ASC frame is missing its DLC".to_string(),
                "line 4: CAN FD line is missing its length".to_string(),
            ]
        );
    }

    /// Counts the bytes read from the log.
    struct Counting {
        inner: Cursor<Vec<u8>>,
        read: usize,
    }

    impl Read for Counting {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.read += n;
            Ok(n)
        }
    }

    impl Seek for Counting {
        fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(to)
        }
    }

    /// A relative-time log of `count` frames, with each frame's offset and time.
    fn relative_log(count: usize) -> (Vec<u8>, Vec<u64>, Vec<u64>) {
        let mut text = String::from("base hex timestamps relative\n");
        let (mut offsets, mut times) = (Vec::new(), Vec::new());
        let mut clock = 0;
        for i in 0..count {
            offsets.push(text.len() as u64);
            let step = 1_000 + (i as u64 % 7);
            clock += step;
            times.push(clock);
            text += &format!("0.{step:06} 1 1A0 Rx d 1 {:02X}\n", i % 256);
        }
        (text.into_bytes(), offsets, times)
    }

    #[test]
    fn relative_resume_carries_the_clock_instead_of_reading_from_the_top() {
        let (bytes, offsets, times) = relative_log(20_000);
        let total = bytes.len();
        let mut reader = Counting {
            inner: Cursor::new(bytes),
            read: 0,
        };
        let at = 15_000;
        {
            let mut scanner =
                Scanner::resume_at(&mut reader, LogFormat::Asc, offsets[at], times[at]).unwrap();
            let first = scanner.next_rec().unwrap().unwrap();
            assert_eq!((first.offset, first.t_us), (offsets[at], times[at]));
            assert_eq!(scanner.next_rec().unwrap().unwrap().t_us, times[at + 1]);
        }
        assert!(
            reader.read < total / 4,
            "read {} of {total} bytes",
            reader.read
        );
    }

    #[test]
    fn relative_resume_without_a_usable_time_rebuilds_the_clock_from_the_top() {
        let (bytes, offsets, times) = relative_log(300);
        let at = 200;
        let mut reader = Cursor::new(bytes.clone());
        let mut scanner = Scanner::resume(&mut reader, LogFormat::Asc, offsets[at]).unwrap();
        assert_eq!(scanner.next_rec().unwrap().unwrap().t_us, times[at]);
        // A time smaller than the line's own step cannot be the clock at that line.
        let mut reader = Cursor::new(bytes);
        let mut scanner = Scanner::resume_at(&mut reader, LogFormat::Asc, offsets[at], 0).unwrap();
        assert_eq!(scanner.next_rec().unwrap().unwrap().t_us, times[at]);
    }
}
