//! Log readers. `Scanner` picks the reader for a format: lines of text with a
//! parser per format, SLB1 binary, or BLF containers. Each reader keeps only
//! the state its own family needs.

mod asc;
mod binary;
mod candump;
mod container;
mod csv;
pub(crate) mod id;
mod slog;
mod source;
mod text;

use crate::error::{Error, Result};
use binary::Slb1Reader;
use container::BlfReader;
use std::io::{Read, Seek};
use text::TextReader;

/// On-disk / in-memory log family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Slog,
    Slbin,
    CanCsv,
    DecodedCsv,
    Asc,
    Candump,
    Blf,
}

impl LogFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Slog => "SLOGv1",
            Self::Slbin => "SLB1",
            Self::CanCsv => "CAN CSV",
            Self::DecodedCsv => "Decoded CSV",
            Self::Asc => "Vector ASC",
            Self::Candump => "candump",
            Self::Blf => "BLF",
        }
    }
}

/// Classic CAN is 8 bytes. CAN FD frames keep up to 64.
pub const MAX_CLASSIC_DATA: usize = 8;
pub const MAX_DATA: usize = 64;
pub type FrameData = [u8; MAX_DATA];

#[derive(Debug, Clone)]
pub enum RecKind {
    Frame {
        id: u32,
        /// 29-bit identifier. The extended flag is separate so a DBC id still matches.
        extended: bool,
        /// 0 when the log did not name a channel.
        channel: u8,
        dlc: u8,
        data: FrameData,
    },
    Event {
        label: String,
    },
    Sample {
        name: String,
        value: f64,
        unit: String,
    },
}

#[derive(Debug, Clone)]
pub struct Rec {
    /// Byte offset of this record. Checkpoints resume from here.
    /// For a compressed BLF container this is the container, not the inner object.
    pub offset: u64,
    pub t_us: u64,
    pub kind: RecKind,
    /// True for the first record yielded from a BLF container. Checkpoints land here
    /// so a resume can re-read that container from the start.
    pub starts_container: bool,
}

impl Rec {
    /// A record not yet placed in a file: offset 0, not a container start. The
    /// reader that produced it calls `placed`.
    pub fn new(t_us: u64, kind: RecKind) -> Self {
        Self {
            offset: 0,
            t_us,
            kind,
            starts_container: false,
        }
    }

    /// A frame. `extended` is the caller's call: from the format when it says,
    /// else `id::implied_extended`.
    pub fn frame(
        t_us: u64,
        id: u32,
        extended: bool,
        channel: u8,
        dlc: u8,
        data: FrameData,
    ) -> Self {
        Self::new(
            t_us,
            RecKind::Frame {
                id,
                extended,
                channel,
                dlc,
                data,
            },
        )
    }

    pub fn event(t_us: u64, label: String) -> Self {
        Self::new(t_us, RecKind::Event { label })
    }

    /// Set where the record sits in the file.
    pub fn placed(self, offset: u64, starts_container: bool) -> Self {
        Self {
            offset,
            starts_container,
            ..self
        }
    }
}

pub fn sniff(head: &[u8]) -> Result<LogFormat> {
    if head.starts_with(b"SLB1") {
        return Ok(LogFormat::Slbin);
    }
    if head.starts_with(crate::blf::FILE_MAGIC) {
        return Ok(LogFormat::Blf);
    }
    let text = String::from_utf8_lossy(head);
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        if line == "SLOGv1" {
            return Ok(LogFormat::Slog);
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("date ")
            || lower.starts_with("base hex")
            || lower.starts_with("base dec")
            || lower.starts_with("timestamps ")
        {
            return Ok(LogFormat::Asc);
        }
        if looks_like_candump(line) {
            return Ok(LogFormat::Candump);
        }
        let mut parts = line.split_whitespace();
        if let Some(tag) = parts.next() {
            if matches!(tag, "F" | "E" | "X")
                && parts.next().is_some_and(|t| t.parse::<u64>().is_ok())
            {
                return Ok(LogFormat::Slog);
            }
        }
        if line.contains(',') {
            return csv_format(line);
        }
        return Err(Error::msg(
            "unrecognized log. Expected SLOGv1, SLB1, CSV, Vector ASC, BLF, or candump.",
        ));
    }
    Err(Error::msg("log is empty"))
}

fn looks_like_candump(line: &str) -> bool {
    let line = line.trim();
    if line.starts_with('(') && line.contains(")#") {
        return false;
    }
    if line.starts_with('(') && line.contains('#') {
        return true;
    }
    if line.contains("ERRORFRAME") && line.split_whitespace().count() >= 2 {
        return true;
    }
    let parts: Vec<&str> = line.split_whitespace().collect();
    parts.len() >= 3 && parts[2].starts_with('[') && parts[2].ends_with(']')
}

/// Which CSV a header line describes.
fn csv_format(header: &str) -> Result<LogFormat> {
    let mut cols = header.split(',').map(str::trim);
    if let (Some(time), Some(kind), Some(_)) = (cols.next(), cols.next(), cols.next()) {
        let is_one_of =
            |col: &str, names: &[&str]| names.iter().any(|n| col.eq_ignore_ascii_case(n));
        if is_one_of(time, &["t_us", "time_us", "timestamp_us", "t"]) {
            if is_one_of(kind, &["signal", "name"]) {
                return Ok(LogFormat::DecodedCsv);
            }
            if is_one_of(kind, &["id", "can_id", "arb_id"]) {
                return Ok(LogFormat::CanCsv);
            }
        }
    }
    Err(Error::msg(
        "CSV header must be t_us,id,data or t_us,signal,value (microseconds)",
    ))
}

pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// One reader per log family. A reader skips what it cannot parse and counts it.
trait RecordReader {
    fn next_rec(&mut self) -> Result<Option<Rec>>;
    /// Bytes consumed so far.
    fn position(&self) -> u64;
    fn skipped(&self) -> u64;
    /// Warnings, plus any summary of records left out on purpose.
    fn notes(&self) -> Vec<String>;
}

pub struct Scanner<'a> {
    reader: Box<dyn RecordReader + 'a>,
}

impl<'a> Scanner<'a> {
    pub fn open(reader: &'a mut dyn ReadSeek, format: LogFormat) -> Result<Self> {
        Self::start(reader, format, 0, None)
    }

    /// Continue at a record boundary recorded in the checkpoint index. A
    /// relative-time ASC log is read from the top to rebuild its running clock;
    /// `resume_at` skips that.
    #[cfg(test)]
    pub fn resume(reader: &'a mut dyn ReadSeek, format: LogFormat, offset: u64) -> Result<Self> {
        Self::start(reader, format, offset, None)
    }

    /// Like `resume`, given `at_us`, the time of the record at `offset`. A
    /// relative-time ASC log picks up its running clock from it.
    pub fn resume_at(
        reader: &'a mut dyn ReadSeek,
        format: LogFormat,
        offset: u64,
        at_us: u64,
    ) -> Result<Self> {
        Self::start(reader, format, offset, Some(at_us))
    }

    fn start(
        reader: &'a mut dyn ReadSeek,
        format: LogFormat,
        offset: u64,
        at_us: Option<u64>,
    ) -> Result<Self> {
        let reader: Box<dyn RecordReader + 'a> = match format {
            LogFormat::Slbin => Box::new(Slb1Reader::open(reader, offset)?),
            LogFormat::Blf => Box::new(BlfReader::open(reader, offset)?),
            LogFormat::Slog => Box::new(TextReader::open(reader, Box::new(slog::Slog), offset)?),
            LogFormat::CanCsv => {
                let parser = csv::CanCsv::new(offset == 0);
                Box::new(TextReader::open(reader, Box::new(parser), offset)?)
            }
            LogFormat::DecodedCsv => {
                let parser = csv::DecodedCsv::new(offset == 0);
                Box::new(TextReader::open(reader, Box::new(parser), offset)?)
            }
            LogFormat::Asc => Box::new(asc::reader(reader, offset, at_us)?),
            LogFormat::Candump => Box::new(candump::reader(reader, offset)?),
        };
        Ok(Self { reader })
    }

    pub fn next_rec(&mut self) -> Result<Option<Rec>> {
        self.reader.next_rec()
    }

    pub fn skipped(&self) -> u64 {
        self.reader.skipped()
    }

    /// Warnings, plus one summary line for tool-level messages left out.
    pub fn notes(&self) -> Vec<String> {
        self.reader.notes()
    }

    pub fn position(&self) -> u64 {
        self.reader.position()
    }
}

/// Build an SLB1 blob. Tests use this to hand frames to the indexer.
#[cfg(test)]
pub(crate) fn encode_slb1(records: &[Rec]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"SLB1");
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    for rec in records {
        match &rec.kind {
            RecKind::Frame { id, dlc, data, .. } => {
                out.push(1);
                out.extend_from_slice(&rec.t_us.to_le_bytes());
                out.extend_from_slice(&id.to_le_bytes());
                let n = (*dlc as usize).min(8);
                out.push(n as u8);
                let mut stored = [0u8; 8];
                stored[..n].copy_from_slice(&data[..n]);
                out.extend_from_slice(&stored);
            }
            RecKind::Event { label } => {
                out.push(2);
                out.extend_from_slice(&rec.t_us.to_le_bytes());
                let bytes = label.as_bytes();
                let len = u16::try_from(bytes.len()).unwrap_or(u16::MAX);
                let bytes = &bytes[..len as usize];
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(bytes);
            }
            RecKind::Sample { .. } => {}
        }
    }
    out
}

pub fn hex_payload(data: &[u8], dlc: u8) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let n = (dlc as usize).min(data.len());
    let mut out = String::with_capacity(n * 2);
    for byte in &data[..n] {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, ErrorKind, SeekFrom};

    /// Serves `inner` up to byte `fail_at`, then fails every read.
    struct FailingReader {
        inner: Cursor<Vec<u8>>,
        fail_at: u64,
    }

    impl Read for FailingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let pos = self.inner.position();
            if pos >= self.fail_at {
                return Err(std::io::Error::other("cable pulled"));
            }
            let room = (self.fail_at - pos).min(buf.len() as u64) as usize;
            self.inner.read(&mut buf[..room])
        }
    }

    impl Seek for FailingReader {
        fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(to)
        }
    }

    /// Fails the first `left` reads with `Interrupted`, then serves `inner`.
    struct Interrupted {
        inner: Cursor<Vec<u8>>,
        left: u32,
    }

    impl Read for Interrupted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.left > 0 {
                self.left -= 1;
                return Err(std::io::Error::from(ErrorKind::Interrupted));
            }
            self.inner.read(buf)
        }
    }

    impl Seek for Interrupted {
        fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(to)
        }
    }

    fn frame(t_us: u64, id: u32, bytes: &[u8]) -> Rec {
        let mut data = [0u8; MAX_DATA];
        data[..bytes.len()].copy_from_slice(bytes);
        Rec {
            offset: 0,
            t_us,
            starts_container: false,
            kind: RecKind::Frame {
                id,
                extended: false,
                channel: 0,
                dlc: bytes.len() as u8,
                data,
            },
        }
    }

    fn stamps(scanner: &mut Scanner) -> Vec<u64> {
        std::iter::from_fn(|| scanner.next_rec().unwrap())
            .map(|rec| rec.t_us)
            .collect()
    }

    #[test]
    fn persistent_text_read_error_ends_the_scan() {
        let text = b"SLOGv1\nF 0 1A0 0102\nF 1000 1A0 0304\n".to_vec();
        let mut reader = FailingReader {
            inner: Cursor::new(text),
            fail_at: 10,
        };
        let mut scanner = Scanner::open(&mut reader, LogFormat::Slog).unwrap();
        let err = scanner
            .next_rec()
            .expect_err("a failing read must end the scan with an error");
        assert_eq!(err.to_string(), "read failed: cable pulled");
    }

    #[test]
    fn interrupted_text_reads_are_retried() {
        let text = b"SLOGv1\nF 0 1A0 0102\n".to_vec();
        let mut reader = Interrupted {
            inner: Cursor::new(text),
            left: 2,
        };
        let mut scanner = Scanner::open(&mut reader, LogFormat::Slog).unwrap();
        let rec = scanner.next_rec().unwrap().unwrap();
        let RecKind::Frame { id, dlc, .. } = rec.kind else {
            panic!("expected a frame");
        };
        assert_eq!((id, dlc), (0x1A0, 2));
        assert!(scanner.next_rec().unwrap().is_none());
    }

    #[test]
    fn persistent_binary_read_error_is_returned() {
        let bytes = encode_slb1(&[frame(1_000, 0x1A0, &[1, 2]), frame(2_000, 0x1A0, &[3, 4])]);
        // 16 header bytes, then 22 per frame. The second frame is cut off mid-record.
        let mut reader = FailingReader {
            inner: Cursor::new(bytes),
            fail_at: 16 + 22 + 5,
        };
        let mut scanner = Scanner::open(&mut reader, LogFormat::Slbin).unwrap();
        assert_eq!(scanner.next_rec().unwrap().unwrap().t_us, 1_000);
        let err = scanner
            .next_rec()
            .expect_err("an I/O failure must not look like the end of the log");
        assert_eq!(err.to_string(), "binary read failed: cable pulled");
    }

    #[test]
    fn truncated_binary_record_is_counted_as_skipped() {
        let mut bytes = encode_slb1(&[frame(1_000, 0x1A0, &[1, 2]), frame(2_000, 0x1A0, &[3, 4])]);
        bytes.truncate(16 + 22 + 10);
        let mut reader = Cursor::new(bytes);
        let mut scanner = Scanner::open(&mut reader, LogFormat::Slbin).unwrap();
        assert_eq!(scanner.next_rec().unwrap().unwrap().t_us, 1_000);
        assert!(scanner.next_rec().unwrap().is_none());
        assert_eq!(scanner.skipped(), 1);
        assert_eq!(
            scanner.notes(),
            vec!["byte 47: truncated record".to_string()]
        );
    }

    #[test]
    fn clockless_candump_line_takes_the_previous_stamp() {
        let text = "(1.500000) can0 100#01\ncan0 100#02\n(2.000000) can0 100#03\n";
        let mut reader = Cursor::new(text.as_bytes().to_vec());
        let mut scanner = Scanner::open(&mut reader, LogFormat::Candump).unwrap();
        assert_eq!(stamps(&mut scanner), vec![1_500_000, 1_500_000, 2_000_000]);
    }

    #[test]
    fn candump_resume_recovers_the_clock_from_before_the_checkpoint() {
        // The clockless run is longer than one backward read chunk.
        let mut text = String::from("(1.000000) can0 100#01\n");
        text += &"100#00\n".repeat(700);
        text += "(2.000000) can0 100#02\n";
        text += &"100#00\n".repeat(700);
        let last = (text.len() - "100#00\n".len()) as u64;
        let mut reader = Cursor::new(text.into_bytes());
        let mut resumed = Scanner::resume(&mut reader, LogFormat::Candump, last).unwrap();
        let rec = resumed.next_rec().unwrap().unwrap();
        assert_eq!(rec.offset, last);
        assert_eq!(rec.t_us, 2_000_000);
    }

    #[test]
    fn asc_short_payload_and_bad_channel_are_skipped_and_counted() {
        let text = "\
base hex timestamps absolute
0.000000 1 1A0 Rx d 8 01 02 03 04 05 06 07 08
0.001000 1 1A0 Rx d 4 01 02
0.002000 x 1A0 Rx d 1 01
0.003000 1 1A0 Rx d 1 03
";
        let mut reader = Cursor::new(text.as_bytes().to_vec());
        let mut scanner = Scanner::open(&mut reader, LogFormat::Asc).unwrap();
        assert_eq!(stamps(&mut scanner), vec![0, 3_000]);
        assert_eq!(scanner.skipped(), 2);
        assert_eq!(
            scanner.notes(),
            vec![
                "line 3: payload is shorter than its DLC".to_string(),
                "line 4: ASC channel is not a number".to_string(),
            ]
        );
    }

    #[test]
    fn candump_short_bracket_payload_is_skipped_and_counted() {
        let text = "can0 100 [4] 01 02\ncan0 100 [2] 01 02\n";
        let mut reader = Cursor::new(text.as_bytes().to_vec());
        let mut scanner = Scanner::open(&mut reader, LogFormat::Candump).unwrap();
        assert_eq!(stamps(&mut scanner), vec![0]);
        assert_eq!(scanner.skipped(), 1);
        assert_eq!(
            scanner.notes(),
            vec!["line 1: payload is shorter than its DLC".to_string()]
        );
    }

    #[test]
    fn blf_object_body_read_failure_is_returned() {
        let mut bytes = b"LOGG".to_vec();
        bytes.extend_from_slice(&144u32.to_le_bytes());
        bytes.resize(144, 0);
        let mut object = b"LOBJ".to_vec();
        object.extend_from_slice(&32u16.to_le_bytes());
        object.extend_from_slice(&1u16.to_le_bytes());
        object.extend_from_slice(&48u32.to_le_bytes());
        object.extend_from_slice(&1u32.to_le_bytes());
        object.resize(48, 0);
        bytes.extend_from_slice(&object);
        // The object body starts at byte 160. Fail five bytes into it.
        let mut reader = FailingReader {
            inner: Cursor::new(bytes),
            fail_at: 165,
        };
        let mut scanner = Scanner::open(&mut reader, LogFormat::Blf).unwrap();
        let err = scanner
            .next_rec()
            .expect_err("an I/O failure in a BLF object must not be skipped");
        assert_eq!(err.to_string(), "binary read failed: cable pulled");
    }
}
