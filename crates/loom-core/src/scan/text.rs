//! Line-oriented logs: a reader that cuts lines and a parser per format.

use super::source::{Line, Skips, Source};
use super::{FrameData, ReadSeek, Rec, RecordReader, MAX_DATA};
use crate::error::{Error, Result};

/// Why a line gave no record.
pub(super) enum Fault {
    /// The line is malformed. It is counted and the scan goes on.
    Skip(String),
    /// The log cannot be read as this format.
    Abort(Error),
}

impl From<String> for Fault {
    fn from(message: String) -> Self {
        Self::Skip(message)
    }
}

impl From<&str> for Fault {
    fn from(message: &str) -> Self {
        Self::Skip(message.to_string())
    }
}

/// `Ok(None)` is a line that carries no record: a preamble, a header, a marker.
pub(super) type LineResult = std::result::Result<Option<Rec>, Fault>;

/// One text format. It sees each trimmed line that is not blank or a `#` comment,
/// and keeps only the state its own format needs.
pub(super) trait LineParser {
    fn parse(&mut self, line: &str) -> LineResult;

    /// A note to show before any skipped-line warning.
    fn warning(&self) -> Option<String> {
        None
    }

    /// A note about lines left out on purpose, shown after the scan.
    fn summary(&self) -> Option<String> {
        None
    }
}

pub(super) struct TextReader<'a> {
    src: Source<'a>,
    line: Vec<u8>,
    line_no: u64,
    parser: Box<dyn LineParser>,
    skips: Skips,
}

impl<'a> TextReader<'a> {
    pub(super) fn open(
        reader: &'a mut dyn ReadSeek,
        parser: Box<dyn LineParser>,
        start: u64,
    ) -> Result<Self> {
        let skips = Skips::with_warnings(parser.warning().into_iter().collect());
        Ok(Self {
            src: Source::at(reader, start)?,
            line: Vec::new(),
            line_no: 0,
            parser,
            skips,
        })
    }

    /// Read and drop records until the position reaches `offset`.
    pub(super) fn skip_until(&mut self, offset: u64) -> Result<()> {
        while self.src.pos() < offset {
            if self.next_rec()?.is_none() {
                break;
            }
        }
        Ok(())
    }

    fn note_skip(&mut self, message: &str) {
        self.skips.note(format!("line {}: {message}", self.line_no));
    }
}

impl RecordReader for TextReader<'_> {
    fn next_rec(&mut self) -> Result<Option<Rec>> {
        loop {
            let start = self.src.pos();
            match self.src.read_line(&mut self.line)? {
                Line::End => return Ok(None),
                Line::TooLong => {
                    self.line_no += 1;
                    self.note_skip("line is longer than 1MB");
                    continue;
                }
                Line::Text => self.line_no += 1,
            }
            let Ok(text) = std::str::from_utf8(&self.line) else {
                self.note_skip("line is not utf-8");
                continue;
            };
            let line = text.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with('#') || line == "SLOGv1" {
                continue;
            }
            match self.parser.parse(line) {
                Ok(Some(rec)) => return Ok(Some(rec.placed(start, false))),
                Ok(None) => {}
                Err(Fault::Skip(message)) => self.note_skip(&message),
                Err(Fault::Abort(err)) => return Err(err),
            }
        }
    }

    fn position(&self) -> u64 {
        self.src.pos()
    }

    fn skipped(&self) -> u64 {
        self.skips.count()
    }

    fn notes(&self) -> Vec<String> {
        let mut notes = self.skips.warnings().to_vec();
        notes.extend(self.parser.summary());
        notes
    }
}

/// The whitespace-separated fields of one line, found once. `spans` is the
/// caller's buffer, reused from line to line.
pub(super) struct Tokens<'a> {
    line: &'a str,
    spans: &'a [(usize, usize)],
}

impl<'a> Tokens<'a> {
    pub(super) fn split(line: &'a str, spans: &'a mut Vec<(usize, usize)>) -> Self {
        spans.clear();
        let base = line.as_ptr() as usize;
        spans.extend(line.split_whitespace().map(|token| {
            let start = token.as_ptr() as usize - base;
            (start, start + token.len())
        }));
        Self { line, spans }
    }

    pub(super) fn len(&self) -> usize {
        self.spans.len()
    }

    pub(super) fn get(&self, index: usize) -> Option<&'a str> {
        self.spans
            .get(index)
            .map(|&(start, end)| &self.line[start..end])
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.spans
            .iter()
            .map(|&(start, end)| &self.line[start..end])
    }

    /// The fields from `start` on.
    pub(super) fn from(&self, start: usize) -> Tokens<'a> {
        Tokens {
            line: self.line,
            spans: self.spans.get(start..).unwrap_or(&[]),
        }
    }
}

/// Decimal seconds with a fraction, as microseconds.
pub(super) fn seconds_to_us(text: &str) -> std::result::Result<u64, String> {
    let secs: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("bad timestamp {text}"))?;
    if !secs.is_finite() || secs < 0.0 {
        return Err(format!("bad timestamp {text}"));
    }
    Ok((secs * 1_000_000.0).round() as u64)
}

/// An integer microsecond count, as SLOGv1 and CAN CSV write time.
pub(super) fn parse_time(token: Option<&str>) -> std::result::Result<u64, String> {
    let Some(token) = token else {
        return Err("missing timestamp".into());
    };
    token
        .parse::<u64>()
        .map_err(|_| format!("timestamp '{token}' is not an integer microsecond count"))
}

/// Hex digits, with spaces and `_` ignored, as a payload of up to 64 bytes.
pub(super) fn parse_payload(text: &str) -> std::result::Result<(FrameData, u8), String> {
    let digits = || {
        text.chars()
            .filter(|c| !c.is_ascii_whitespace() && *c != '_')
    };
    if !digits().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "bad payload hex '{}'",
            digits().collect::<String>()
        ));
    }
    let count = digits().count();
    if count > MAX_DATA * 2 || count % 2 == 1 {
        return Err("payload must be an even number of hex digits, at most 64 bytes".into());
    }
    let dlc = count / 2;
    let mut data = [0u8; MAX_DATA];
    let mut nibbles = digits().filter_map(|c| c.to_digit(16));
    for slot in &mut data[..dlc] {
        if let (Some(high), Some(low)) = (nibbles.next(), nibbles.next()) {
            *slot = (high * 16 + low) as u8;
        }
    }
    Ok((data, dlc as u8))
}

/// A payload shorter than its declared length is skipped, not kept as a shorter frame.
pub(super) fn check_payload_len(available: usize, want: usize) -> std::result::Result<(), String> {
    if available < want.min(MAX_DATA) {
        return Err("payload is shorter than its DLC".into());
    }
    Ok(())
}

/// Up to `want` space-separated hex bytes.
pub(super) fn read_hex_bytes(
    tokens: &Tokens,
    want: usize,
) -> std::result::Result<(FrameData, u8), String> {
    let mut data = [0u8; MAX_DATA];
    let n = want.min(MAX_DATA).min(tokens.len());
    for (slot, token) in data.iter_mut().zip(tokens.iter().take(n)) {
        *slot = u8::from_str_radix(token, 16).map_err(|_| format!("bad data byte {token}"))?;
    }
    Ok((data, n as u8))
}

#[cfg(test)]
mod tests {
    use crate::scan::{LogFormat, RecKind, Scanner};
    use std::io::Cursor;

    type Scanned = (Vec<(u64, String)>, u64, Vec<String>);

    fn scan(format: LogFormat, bytes: Vec<u8>) -> Scanned {
        let len = bytes.len() as u64;
        let mut reader = Cursor::new(bytes);
        let mut scanner = Scanner::open(&mut reader, format).unwrap();
        let recs: Vec<(u64, String)> = std::iter::from_fn(|| scanner.next_rec().unwrap())
            .map(|rec| {
                let what = match rec.kind {
                    RecKind::Frame { id, .. } => format!("{id:X}"),
                    RecKind::Event { label } => label,
                    RecKind::Sample { name, .. } => name,
                };
                (rec.t_us, what)
            })
            .collect();
        assert_eq!(scanner.position(), len);
        (recs, scanner.skipped(), scanner.notes())
    }

    #[test]
    fn crlf_and_a_last_line_without_a_newline() {
        let text = b"SLOGv1\r\nF 0 1A0 01\r\n\r\nE 5 hello  world\r\nF 9 1A1 02";
        let (recs, skipped, _) = scan(LogFormat::Slog, text.to_vec());
        assert_eq!(
            recs,
            vec![
                (0, "1A0".to_string()),
                (5, "hello world".to_string()),
                (9, "1A1".to_string())
            ]
        );
        assert_eq!(skipped, 0);
    }

    #[test]
    fn lines_crossing_the_read_buffer_are_kept_whole() {
        let mut text = String::from("SLOGv1\n");
        for i in 0..2_000 {
            text += &format!("E {i} {}\n", "x".repeat(60 + i % 50));
        }
        let (recs, skipped, _) = scan(LogFormat::Slog, text.into_bytes());
        assert_eq!(recs.len(), 2_000);
        assert_eq!(recs[1_999], (1_999, "x".repeat(60 + 1_999 % 50)));
        assert_eq!(skipped, 0);
    }

    #[test]
    fn an_overlong_or_non_utf8_line_is_skipped_and_counted() {
        let mut text = b"F 0 1A0 01\nE 1 ".to_vec();
        text.extend(std::iter::repeat_n(b'x', 1_000_001));
        text.extend_from_slice(b"\nF 2 1A0 \xff\xfe\nF 3 1A0 03\n");
        let (recs, skipped, notes) = scan(LogFormat::Slog, text);
        assert_eq!(recs, vec![(0, "1A0".to_string()), (3, "1A0".to_string())]);
        assert_eq!(skipped, 2);
        assert_eq!(
            notes,
            vec![
                "line 2: line is longer than 1MB".to_string(),
                "line 3: line is not utf-8".to_string(),
            ]
        );
    }
}
