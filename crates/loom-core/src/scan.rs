use crate::error::{Error, Result};
use crate::map::parse_can_id;
use std::io::{Read, Seek, SeekFrom};

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

pub fn sniff(head: &[u8]) -> Result<LogFormat> {
    if head.starts_with(b"SLB1") {
        return Ok(LogFormat::Slbin);
    }
    if head.starts_with(b"LOGG") {
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

fn csv_format(header: &str) -> Result<LogFormat> {
    let cols = split_csv(header);
    let cols: Vec<String> = cols.iter().map(|c| c.to_ascii_lowercase()).collect();
    if cols.len() >= 3 && matches!(cols[0].as_str(), "t_us" | "time_us" | "timestamp_us" | "t") {
        if matches!(cols[1].as_str(), "signal" | "name") {
            return Ok(LogFormat::DecodedCsv);
        }
        if matches!(cols[1].as_str(), "id" | "can_id" | "arb_id") {
            return Ok(LogFormat::CanCsv);
        }
    }
    Err(Error::msg(
        "CSV header must be t_us,id,data or t_us,signal,value (microseconds)",
    ))
}

pub struct Scanner<'a> {
    reader: &'a mut dyn ReadSeek,
    format: LogFormat,
    pos: u64,
    line_no: u64,
    /// CSV body mode skips nothing; header mode consumes the first CSV header.
    header_pending: bool,
    /// Vector ASC ids are hex unless the preamble says `base dec`.
    asc_hex: bool,
    /// Subtract this from candump timestamps. Epoch logs start at zero.
    time_origin_us: u64,
    /// Running clock for `timestamps relative`.
    last_stamp_us: u64,
    asc_relative: bool,
    skipped: u64,
    warnings: Vec<String>,
    buf: Vec<u8>,
    buf_at: usize,
    buf_len: usize,
    blf_buf: Vec<u8>,
    blf_at: usize,
    blf_container: u64,
    blf_header_done: bool,
    blf_fresh: bool,
}

pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

impl<'a> Scanner<'a> {
    pub fn open(reader: &'a mut dyn ReadSeek, format: LogFormat) -> Result<Self> {
        reader
            .seek(SeekFrom::Start(0))
            .map_err(|err| Error::msg(format!("could not rewind log: {err}")))?;
        let (asc_hex, asc_relative) = if format == LogFormat::Asc {
            peek_asc_mode(reader)?
        } else {
            (true, false)
        };
        let time_origin_us = if format == LogFormat::Candump {
            peek_candump_origin(reader)?
        } else {
            0
        };
        reader
            .seek(SeekFrom::Start(0))
            .map_err(|err| Error::msg(format!("could not rewind log: {err}")))?;
        let header_pending = matches!(format, LogFormat::CanCsv | LogFormat::DecodedCsv);
        let mut scanner = Self {
            reader,
            format,
            pos: 0,
            line_no: 0,
            header_pending,
            asc_hex,
            time_origin_us,
            last_stamp_us: 0,
            asc_relative,
            skipped: 0,
            warnings: Vec::new(),
            buf: vec![0; 64 * 1024],
            buf_at: 0,
            buf_len: 0,
            blf_buf: Vec::new(),
            blf_at: 0,
            blf_container: 0,
            blf_header_done: false,
            blf_fresh: false,
        };
        if format == LogFormat::Slbin {
            scanner.consume_binary_header()?;
        }
        if format == LogFormat::Blf {
            scanner.prepare_blf(0)?;
        }
        Ok(scanner)
    }

    /// Continue at a record boundary recorded in the checkpoint index.
    pub fn resume(reader: &'a mut dyn ReadSeek, format: LogFormat, offset: u64) -> Result<Self> {
        let (asc_hex, asc_relative) = if format == LogFormat::Asc {
            peek_asc_mode(reader)?
        } else {
            (true, false)
        };
        let time_origin_us = if format == LogFormat::Candump {
            peek_candump_origin(reader)?
        } else {
            0
        };
        let start = if asc_relative { 0 } else { offset };
        reader
            .seek(SeekFrom::Start(start))
            .map_err(|err| Error::msg(format!("could not seek log: {err}")))?;
        let mut scanner = Self {
            reader,
            format,
            pos: start,
            line_no: 0,
            header_pending: false,
            asc_hex,
            time_origin_us,
            last_stamp_us: 0,
            asc_relative,
            skipped: 0,
            warnings: Vec::new(),
            buf: vec![0; 64 * 1024],
            buf_at: 0,
            buf_len: 0,
            blf_buf: Vec::new(),
            blf_at: 0,
            blf_container: 0,
            blf_header_done: offset > 0,
            blf_fresh: false,
        };
        if format == LogFormat::Blf && offset == 0 {
            scanner.prepare_blf(0)?;
        }
        if asc_relative {
            scanner.skip_until(offset)?;
        }
        Ok(scanner)
    }

    pub fn skipped(&self) -> u64 {
        self.skipped
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn position(&self) -> u64 {
        self.pos
    }

    fn skip_until(&mut self, offset: u64) -> Result<()> {
        while self.pos < offset {
            if self.next_text()?.is_none() {
                break;
            }
        }
        Ok(())
    }

    pub fn next_rec(&mut self) -> Result<Option<Rec>> {
        match self.format {
            LogFormat::Slbin => self.next_binary(),
            LogFormat::Blf => self.next_blf(),
            LogFormat::Slog
            | LogFormat::CanCsv
            | LogFormat::DecodedCsv
            | LogFormat::Asc
            | LogFormat::Candump => self.next_text(),
        }
    }

    fn note_skip(&mut self, message: impl Into<String>) {
        self.skipped += 1;
        if self.warnings.len() < 32 {
            let message = message.into();
            let line = self.line_no;
            let text = if line > 0 && !message.starts_with("line ") && !message.starts_with("byte ")
            {
                format!("line {line}: {message}")
            } else {
                message
            };
            if !self.warnings.iter().any(|have| have == &text) {
                self.warnings.push(text);
            }
        }
    }

    fn consume_binary_header(&mut self) -> Result<()> {
        let mut magic = [0u8; 4];
        self.read_exact_bin(&mut magic)?;
        if &magic != b"SLB1" {
            return Err(Error::Binary {
                offset: 0,
                message: "missing SLB1 magic".into(),
            });
        }
        let mut rest = [0u8; 12];
        self.read_exact_bin(&mut rest)?;
        let version = u16::from_le_bytes([rest[0], rest[1]]);
        if version != 1 {
            return Err(Error::Binary {
                offset: 4,
                message: format!("SLB1 version {version} is not supported"),
            });
        }
        Ok(())
    }

    fn next_binary(&mut self) -> Result<Option<Rec>> {
        let start = self.pos;
        let mut tag = [0u8; 1];
        if let Err(err) = self.read_exact_bin(&mut tag) {
            if self.pos == start {
                return Ok(None);
            }
            self.note_skip(err.to_string());
            return Ok(None);
        }
        let t_us = match self.read_u64() {
            Ok(value) => value,
            Err(err) => {
                self.note_skip(err.to_string());
                return Ok(None);
            }
        };
        match tag[0] {
            1 => {
                let id = match self.read_u32() {
                    Ok(value) => value,
                    Err(err) => {
                        self.note_skip(err.to_string());
                        return Ok(None);
                    }
                };
                let mut dlc_buf = [0u8; 1];
                if let Err(err) = self.read_exact_bin(&mut dlc_buf) {
                    self.note_skip(err.to_string());
                    return Ok(None);
                }
                let mut data = [0u8; 8];
                if let Err(err) = self.read_exact_bin(&mut data) {
                    self.note_skip(err.to_string());
                    return Ok(None);
                }
                let dlc = dlc_buf[0].min(8);
                let mut wide = [0u8; MAX_DATA];
                wide[..8].copy_from_slice(&data);
                Ok(Some(Rec {
                    offset: start,
                    t_us,
                    starts_container: false,
                    kind: RecKind::Frame {
                        id,
                        extended: id > 0x7FF,
                        channel: 0,
                        dlc,
                        data: wide,
                    },
                }))
            }
            2 => {
                let len = match self.read_u16() {
                    Ok(value) => value as usize,
                    Err(err) => {
                        self.note_skip(err.to_string());
                        return Ok(None);
                    }
                };
                if len > 4_000 {
                    self.note_skip(format!("byte {start}: event label is too long"));
                    return Ok(None);
                }
                let mut buf = vec![0u8; len];
                if len > 0 {
                    if let Err(err) = self.read_exact_bin(&mut buf) {
                        self.note_skip(err.to_string());
                        return Ok(None);
                    }
                }
                let Ok(label) = String::from_utf8(buf) else {
                    self.note_skip(format!("byte {start}: event label is not utf-8"));
                    return Ok(None);
                };
                Ok(Some(Rec {
                    offset: start,
                    t_us,
                    starts_container: false,
                    kind: RecKind::Event { label },
                }))
            }
            other => {
                self.note_skip(format!(
                    "byte {start}: unknown SLB1 tag {other}; stopped, the rest of the file was not indexed"
                ));
                Ok(None)
            }
        }
    }

    fn next_text(&mut self) -> Result<Option<Rec>> {
        loop {
            let start = self.pos;
            let line = match self.read_line() {
                Ok(Some(line)) => line,
                Ok(None) => return Ok(None),
                Err(err) => {
                    self.note_skip(err.to_string());
                    continue;
                }
            };
            let trimmed = line.trim().trim_start_matches('\u{feff}');
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed == "SLOGv1" || (self.format == LogFormat::Asc && is_asc_preamble(trimmed)) {
                continue;
            }
            if self.header_pending {
                self.header_pending = false;
                csv_format(trimmed)?;
                continue;
            }
            match self.parse_line(trimmed, start) {
                Ok(mut rec) => {
                    rec.offset = start;
                    rec.starts_container = false;
                    return Ok(Some(rec));
                }
                Err(err) => {
                    self.note_skip(err.to_string());
                }
            }
        }
    }

    fn parse_line(&mut self, line: &str, start: u64) -> Result<Rec> {
        match self.format {
            LogFormat::Slog => self.parse_slog(line),
            LogFormat::CanCsv => self.parse_can_csv(line),
            LogFormat::DecodedCsv => self.parse_decoded_csv(line),
            LogFormat::Asc => self.parse_asc(line),
            LogFormat::Candump => self.parse_candump(line, start),
            LogFormat::Slbin | LogFormat::Blf => unreachable!(),
        }
    }

    fn parse_slog(&self, line: &str) -> Result<Rec> {
        let mut parts = line.split_whitespace();
        let tag = parts.next().unwrap_or("");
        let t_us = self.parse_time(parts.next())?;
        match tag {
            "F" => {
                let id_tok = parts.next().unwrap_or("");
                let id = parse_hex_id(id_tok).map_err(|message| Error::Parse {
                    line: self.line_no,
                    message,
                })?;
                let data_tok = parts.next().unwrap_or("");
                if parts.next().is_some() {
                    return self.bad("frame record has extra columns");
                }
                let (data, dlc) = parse_payload(data_tok).map_err(|message| Error::Parse {
                    line: self.line_no,
                    message,
                })?;
                Ok(Rec {
                    offset: 0,
                    t_us,
                    starts_container: false,
                    kind: RecKind::Frame {
                        id,
                        extended: id > 0x7FF,
                        channel: 0,
                        dlc,
                        data,
                    },
                })
            }
            "E" => {
                let label = parts.collect::<Vec<_>>().join(" ");
                if label.is_empty() {
                    return self.bad("event is missing a label");
                }
                if label.len() > 400 {
                    return self.bad("event label is too long");
                }
                Ok(Rec {
                    offset: 0,
                    t_us,
                    starts_container: false,
                    kind: RecKind::Event { label },
                })
            }
            "X" => {
                if parts.next().is_some() {
                    return self.bad("error frame has extra columns");
                }
                Ok(Rec {
                    offset: 0,
                    t_us,
                    starts_container: false,
                    kind: RecKind::Event {
                        label: "Error frame".to_string(),
                    },
                })
            }
            _ => self.bad("expected an F frame, E event, or X error frame"),
        }
    }

    fn parse_asc(&mut self, line: &str) -> Result<Rec> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            return self.bad("ASC line is too short");
        }
        let stamp = seconds_to_us(parts[0]).map_err(|message| self.err_msg(&message))?;
        let t_us = if self.asc_relative {
            self.last_stamp_us = self.last_stamp_us.saturating_add(stamp);
            self.last_stamp_us
        } else {
            stamp
        };
        if parts
            .iter()
            .any(|part| part.eq_ignore_ascii_case("ErrorFrame"))
        {
            return Ok(event_rec(t_us, "Error frame".into()));
        }
        if parts.iter().any(|part| part.eq_ignore_ascii_case("CANFD")) {
            return self.parse_asc_fd(&parts, t_us);
        }
        let channel = parts.get(1).and_then(|tok| tok.parse().ok()).unwrap_or(0);
        let raw_id = parts.get(2).copied().unwrap_or("");
        let extended = raw_id.ends_with('x') || raw_id.ends_with('X');
        let id_tok = raw_id.trim_end_matches(['x', 'X']);
        let id = if self.asc_hex {
            u32::from_str_radix(id_tok, 16)
        } else {
            id_tok.parse::<u32>()
        }
        .map_err(|_| self.err_msg(&format!("bad ASC id {raw_id}")))?
            & 0x1FFF_FFFF;
        let Some(marker) = parts.iter().position(|part| *part == "d" || *part == "D") else {
            return self.bad("ASC frame is missing the data marker");
        };
        let dlc_tok = parts.get(marker + 1).copied().unwrap_or("0");
        let dlc: usize = dlc_tok
            .parse()
            .map_err(|_| self.err_msg("ASC dlc is not a number"))?;
        let available = parts.get(marker + 2..).unwrap_or(&[]);
        let (data, n) = read_hex_bytes(available, dlc).map_err(|message| self.err_msg(&message))?;
        Ok(frame_rec(t_us, id, extended, channel, n, data))
    }

    fn parse_asc_fd(&self, parts: &[&str], t_us: u64) -> Result<Rec> {
        let fd = parts
            .iter()
            .position(|part| part.eq_ignore_ascii_case("CANFD"))
            .unwrap_or(0);
        let channel = parts
            .get(fd + 1)
            .and_then(|tok| tok.parse().ok())
            .unwrap_or(0);
        let raw_id = parts.get(fd + 3).copied().unwrap_or("");
        let extended = raw_id.ends_with('x') || raw_id.ends_with('X');
        let id_tok = raw_id.trim_end_matches(['x', 'X']);
        let id = u32::from_str_radix(id_tok, 16)
            .map_err(|_| self.err_msg(&format!("bad CAN FD id {raw_id}")))?
            & 0x1FFF_FFFF;
        let Some(marker) = parts.iter().position(|part| *part == "d" || *part == "D") else {
            return self.bad("CAN FD line is missing the data marker");
        };
        let len_tok = parts.get(marker + 2).copied().unwrap_or("0");
        let len: usize = len_tok
            .parse()
            .map_err(|_| self.err_msg("CAN FD length is not a number"))?;
        let available = parts.get(marker + 3..).unwrap_or(&[]);
        let (data, n) = read_hex_bytes(available, len).map_err(|message| self.err_msg(&message))?;
        Ok(frame_rec(t_us, id, extended, channel, n, data))
    }

    fn parse_candump(&mut self, line: &str, start: u64) -> Result<Rec> {
        if line.to_ascii_uppercase().contains("ERRORFRAME") {
            return Ok(event_rec(
                candump_time(line, start, self.time_origin_us).unwrap_or(start),
                "Error frame".into(),
            ));
        }
        if let Some(rest) = line.trim().strip_prefix('(') {
            let Some((num, after)) = rest.split_once(')') else {
                return self.bad("candump timestamp is missing a closing parenthesis");
            };
            let stamp = seconds_to_us(num.trim()).map_err(|message| self.err_msg(&message))?;
            let t_us = stamp.saturating_sub(self.time_origin_us);
            self.last_stamp_us = t_us;
            return self.candump_rest(t_us, after.trim());
        }
        // A line with no clock keeps moving forward. A byte offset is only used
        // when it does not step behind a timestamp already seen in this file.
        let t_us = if start >= self.last_stamp_us {
            start
        } else {
            self.last_stamp_us.saturating_add(1)
        };
        self.last_stamp_us = t_us;
        self.candump_rest(t_us, line)
    }

    fn candump_rest(&self, t_us: u64, text: &str) -> Result<Rec> {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if let Some(index) = parts.iter().position(|part| part.contains('#')) {
            let channel = if index == 0 {
                0
            } else {
                iface_channel(parts[index - 1])
            };
            return self.hash_frame(t_us, parts[index], channel);
        }
        if parts.len() >= 3 && parts[2].starts_with('[') {
            return self.bracket_frame(t_us, parts[0], parts[1], &parts[3..]);
        }
        if parts.len() >= 2 && parts[1].starts_with('[') {
            return self.bracket_frame(t_us, "", parts[0], &parts[2..]);
        }
        self.bad("candump line was not recognized")
    }

    fn bracket_frame(&self, t_us: u64, iface: &str, id_tok: &str, bytes: &[&str]) -> Result<Rec> {
        let id = candump_id(id_tok).map_err(|message| self.err_msg(&message))?;
        let (data, n) =
            read_hex_bytes(bytes, MAX_DATA).map_err(|message| self.err_msg(&message))?;
        Ok(frame_rec(
            t_us,
            id,
            id > 0x7FF,
            iface_channel(iface),
            n,
            data,
        ))
    }

    fn hash_frame(&self, t_us: u64, spec: &str, channel: u8) -> Result<Rec> {
        let Some((id_tok, rest)) = spec.split_once('#') else {
            return self.bad("candump frame is missing a payload");
        };
        let fd = rest.starts_with('#');
        let mut data_tok = rest.trim_start_matches('#');
        if fd && data_tok.len() % 2 == 1 {
            let Some(payload) = data_tok.get(1..) else {
                return self.bad("candump FD flags are not a hex digit");
            };
            data_tok = payload;
        }
        let id = candump_id(id_tok).map_err(|message| self.err_msg(&message))?;
        if data_tok.is_empty() || data_tok.starts_with('R') || data_tok.starts_with('r') {
            return Ok(frame_rec(t_us, id, id > 0x7FF, channel, 0, [0; MAX_DATA]));
        }
        let (data, dlc) = parse_payload(data_tok).map_err(|message| self.err_msg(&message))?;
        Ok(frame_rec(t_us, id, id > 0x7FF, channel, dlc, data))
    }

    fn err_msg(&self, message: &str) -> Error {
        Error::Parse {
            line: self.line_no,
            message: message.to_string(),
        }
    }

    fn parse_can_csv(&self, line: &str) -> Result<Rec> {
        let cols = split_csv(line);
        if cols.len() < 3 {
            return self.bad("CAN CSV rows need t_us,id,data");
        }
        let t_us = self.parse_time(Some(cols[0].trim()))?;
        let id = parse_can_id(cols[1]).map_err(|message| Error::Parse {
            line: self.line_no,
            message,
        })?;
        let (data, dlc) = parse_payload(cols[2]).map_err(|message| Error::Parse {
            line: self.line_no,
            message,
        })?;
        Ok(frame_rec(t_us, id, id > 0x7FF, 0, dlc, data))
    }

    fn parse_decoded_csv(&self, line: &str) -> Result<Rec> {
        let cols = split_csv(line);
        if cols.len() < 3 {
            return self.bad("decoded CSV rows need t_us,signal,value");
        }
        let t_us = self.parse_time(Some(cols[0].trim()))?;
        let name = cols[1].trim();
        if name.is_empty() {
            return self.bad("signal name is empty");
        }
        let value: f64 = cols[2].trim().parse().map_err(|_| Error::Parse {
            line: self.line_no,
            message: format!("value '{}' is not a number", cols[2].trim()),
        })?;
        if !value.is_finite() {
            return self.bad("value must be finite");
        }
        let unit = cols
            .get(3)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        Ok(Rec {
            offset: 0,
            t_us,
            starts_container: false,
            kind: RecKind::Sample {
                name: name.to_string(),
                value,
                unit,
            },
        })
    }

    fn parse_time(&self, token: Option<&str>) -> Result<u64> {
        let Some(token) = token else {
            return self.bad("missing timestamp");
        };
        token.parse::<u64>().map_err(|_| Error::Parse {
            line: self.line_no,
            message: format!("timestamp '{token}' is not an integer microsecond count"),
        })
    }

    fn bad<T>(&self, message: &str) -> Result<T> {
        Err(Error::Parse {
            line: self.line_no,
            message: message.to_string(),
        })
    }

    fn read_line(&mut self) -> Result<Option<String>> {
        let mut buf = Vec::new();
        let mut too_long = false;
        loop {
            let Some(byte) = self.pull()? else {
                if buf.is_empty() && !too_long {
                    return Ok(None);
                }
                break;
            };
            if byte == b'\n' {
                break;
            }
            if byte == b'\r' {
                continue;
            }
            if buf.len() >= 1_000_000 {
                too_long = true;
                continue;
            }
            buf.push(byte);
        }
        self.line_no += 1;
        if too_long {
            return Err(Error::Parse {
                line: self.line_no,
                message: "line is longer than 1MB".into(),
            });
        }
        match String::from_utf8(buf) {
            Ok(text) => Ok(Some(text)),
            Err(_) => Err(Error::Parse {
                line: self.line_no,
                message: "line is not utf-8".into(),
            }),
        }
    }

    fn pull(&mut self) -> Result<Option<u8>> {
        if self.buf_at >= self.buf_len {
            self.buf_len = self
                .reader
                .read(&mut self.buf)
                .map_err(|err| Error::msg(format!("read failed: {err}")))?;
            self.buf_at = 0;
            if self.buf_len == 0 {
                return Ok(None);
            }
        }
        let byte = self.buf[self.buf_at];
        self.buf_at += 1;
        self.pos += 1;
        Ok(Some(byte))
    }

    fn read_exact_bin(&mut self, buf: &mut [u8]) -> Result<()> {
        let start = self.pos;
        let mut off = 0;
        while off < buf.len() {
            if self.buf_at >= self.buf_len {
                let n = self
                    .reader
                    .read(&mut buf[off..])
                    .map_err(|err| Error::msg(format!("binary read failed: {err}")))?;
                if n == 0 {
                    return Err(Error::Binary {
                        offset: start,
                        message: "truncated record".into(),
                    });
                }
                off += n;
                self.pos += n as u64;
                continue;
            }
            buf[off] = self.buf[self.buf_at];
            self.buf_at += 1;
            self.pos += 1;
            off += 1;
        }
        Ok(())
    }

    fn prepare_blf(&mut self, offset: u64) -> Result<()> {
        if offset > 0 {
            self.blf_header_done = true;
            return Ok(());
        }
        let mut magic = [0u8; 4];
        self.read_exact_bin(&mut magic)?;
        if &magic != b"LOGG" {
            return Err(Error::msg("BLF is missing the LOGG header"));
        }
        let mut size_bytes = [0u8; 4];
        self.read_exact_bin(&mut size_bytes)?;
        let header = u32::from_le_bytes(size_bytes) as usize;
        if !(8..=1_048_576).contains(&header) {
            return Err(Error::msg("BLF header size is not usable"));
        }
        if header > 8 {
            let mut rest = vec![0u8; header - 8];
            self.read_exact_bin(&mut rest)?;
        }
        self.blf_header_done = true;
        Ok(())
    }

    fn next_blf(&mut self) -> Result<Option<Rec>> {
        if !self.blf_header_done {
            match self.prepare_blf(0) {
                Ok(()) => {}
                Err(err) => {
                    self.note_skip(err.to_string());
                    return Ok(None);
                }
            }
        }
        loop {
            if let Some(mut rec) =
                crate::blf::next_inner(&self.blf_buf, &mut self.blf_at, self.blf_container)
            {
                if self.blf_fresh {
                    rec.starts_container = true;
                    self.blf_fresh = false;
                }
                return Ok(Some(rec));
            }
            self.blf_buf.clear();
            self.blf_at = 0;
            let Some((offset, object, obj_type)) = self.read_blf_object()? else {
                return Ok(None);
            };
            if crate::blf::is_container(obj_type) {
                match crate::blf::inflate_container(&object) {
                    Ok(data) => {
                        self.blf_buf = data;
                        self.blf_container = offset;
                        self.blf_fresh = true;
                    }
                    Err(err) => self.note_skip(format!("byte {offset}: {err}")),
                }
                continue;
            }
            if crate::blf::is_frame_object(obj_type) {
                if let Some(mut rec) = crate::blf::decode_object(&object) {
                    rec.offset = offset;
                    rec.starts_container = true;
                    return Ok(Some(rec));
                }
            }
        }
    }

    fn read_blf_object(&mut self) -> Result<Option<(u64, Vec<u8>, u32)>> {
        let start = self.pos;
        let mut head = [0u8; 16];
        if let Err(err) = self.read_exact_bin(&mut head) {
            if err.to_string().contains("truncated") {
                return Ok(None);
            }
            return Err(err);
        }
        if &head[0..4] != b"LOBJ" {
            self.note_skip(format!(
                "byte {start}: BLF object is not LOBJ; stopped indexing the rest of the file"
            ));
            return Ok(None);
        }
        let obj_size = u32::from_le_bytes([head[8], head[9], head[10], head[11]]) as usize;
        let obj_type = u32::from_le_bytes([head[12], head[13], head[14], head[15]]);
        if !(16..=16 * 1024 * 1024).contains(&obj_size) {
            self.note_skip(format!(
                "byte {start}: BLF object size {obj_size} is not usable"
            ));
            return Ok(None);
        }
        let mut object = vec![0u8; obj_size];
        object[..16].copy_from_slice(&head);
        if obj_size > 16 {
            if let Err(err) = self.read_exact_bin(&mut object[16..]) {
                self.note_skip(format!("byte {start}: {err}"));
                return Ok(None);
            }
        }
        let pad = (4 - (obj_size % 4)) % 4;
        if pad > 0 {
            let mut junk = [0u8; 3];
            let _ = self.read_exact_bin(&mut junk[..pad]);
        }
        Ok(Some((start, object, obj_type)))
    }

    fn read_u16(&mut self) -> Result<u16> {
        let mut buf = [0u8; 2];
        self.read_exact_bin(&mut buf)?;
        Ok(u16::from_le_bytes(buf))
    }

    fn read_u32(&mut self) -> Result<u32> {
        let mut buf = [0u8; 4];
        self.read_exact_bin(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    fn read_u64(&mut self) -> Result<u64> {
        let mut buf = [0u8; 8];
        self.read_exact_bin(&mut buf)?;
        Ok(u64::from_le_bytes(buf))
    }
}

fn parse_hex_id(text: &str) -> std::result::Result<u32, String> {
    let text = text.trim();
    let hex = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    if hex.is_empty() {
        return Err("missing CAN id".into());
    }
    u32::from_str_radix(hex, 16).map_err(|_| format!("bad hex CAN id '{text}'"))
}

pub fn parse_payload(text: &str) -> std::result::Result<(FrameData, u8), String> {
    let hex: String = text
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && *c != '_')
        .collect();
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("bad payload hex '{hex}'"));
    }
    if hex.len() > MAX_DATA * 2 || !hex.len().is_multiple_of(2) {
        return Err("payload must be an even number of hex digits, at most 64 bytes".into());
    }
    let dlc = (hex.len() / 2) as u8;
    let mut data = [0u8; MAX_DATA];
    for i in 0..dlc as usize {
        data[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| format!("bad payload hex '{}'", &hex[i * 2..i * 2 + 2]))?;
    }
    Ok((data, dlc))
}

fn frame_rec(t_us: u64, id: u32, extended: bool, channel: u8, dlc: u8, data: FrameData) -> Rec {
    Rec {
        offset: 0,
        t_us,
        starts_container: false,
        kind: RecKind::Frame {
            id,
            extended,
            channel,
            dlc,
            data,
        },
    }
}

fn event_rec(t_us: u64, label: String) -> Rec {
    Rec {
        offset: 0,
        t_us,
        starts_container: false,
        kind: RecKind::Event { label },
    }
}

fn read_hex_bytes(tokens: &[&str], want: usize) -> std::result::Result<(FrameData, u8), String> {
    let mut data = [0u8; MAX_DATA];
    let n = want.min(MAX_DATA).min(tokens.len());
    for (i, tok) in tokens.iter().take(n).enumerate() {
        data[i] = u8::from_str_radix(tok, 16).map_err(|_| format!("bad data byte {tok}"))?;
    }
    Ok((data, n as u8))
}

fn iface_channel(iface: &str) -> u8 {
    let digits: String = iface
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    digits.parse().unwrap_or(0)
}

fn split_csv(line: &str) -> Vec<&str> {
    line.split(',').map(|c| c.trim()).collect()
}

fn is_asc_preamble(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("date ")
        || lower.starts_with("base ")
        || lower.starts_with("timestamps ")
        || lower.starts_with("internal ")
        || lower.starts_with("begin trigger")
        || lower.starts_with("end trigger")
        || lower.starts_with("//")
}

fn seconds_to_us(text: &str) -> std::result::Result<u64, String> {
    let secs: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("bad timestamp {text}"))?;
    if !secs.is_finite() || secs < 0.0 {
        return Err(format!("bad timestamp {text}"));
    }
    Ok((secs * 1_000_000.0).round() as u64)
}

fn candump_id(text: &str) -> std::result::Result<u32, String> {
    let text = text.trim().trim_end_matches(['x', 'X']);
    let id = u32::from_str_radix(text, 16).map_err(|_| format!("bad candump id {text}"))?;
    Ok(id & 0x1FFF_FFFF)
}

fn candump_time(line: &str, start: u64, origin: u64) -> Option<u64> {
    let rest = line.trim().strip_prefix('(')?;
    let (num, _) = rest.split_once(')')?;
    let stamp = seconds_to_us(num.trim()).ok()?;
    let _ = start;
    Some(stamp.saturating_sub(origin))
}

fn peek_prefix(reader: &mut dyn ReadSeek) -> Result<String> {
    let pos = reader
        .stream_position()
        .map_err(|err| Error::msg(format!("could not tell log position: {err}")))?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|err| Error::msg(format!("could not rewind log: {err}")))?;
    let mut buf = [0u8; 4096];
    let n = reader
        .read(&mut buf)
        .map_err(|err| Error::msg(format!("could not read log: {err}")))?;
    reader
        .seek(SeekFrom::Start(pos))
        .map_err(|err| Error::msg(format!("could not restore log position: {err}")))?;
    Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
}

fn peek_asc_mode(reader: &mut dyn ReadSeek) -> Result<(bool, bool)> {
    let text = peek_prefix(reader)?;
    let mut hex = true;
    let mut relative = false;
    for raw in text.lines() {
        let lower = raw.trim().to_ascii_lowercase();
        if lower.starts_with("base dec") {
            hex = false;
        } else if lower.starts_with("base hex") {
            hex = true;
        }
        if lower.contains("timestamps relative") {
            relative = true;
        } else if lower.contains("timestamps absolute") {
            relative = false;
        }
    }
    Ok((hex, relative))
}

fn peek_candump_origin(reader: &mut dyn ReadSeek) -> Result<u64> {
    let text = peek_prefix(reader)?;
    for raw in text.lines() {
        let Some(rest) = raw.trim().strip_prefix('(') else {
            continue;
        };
        let Some((num, _)) = rest.split_once(')') else {
            continue;
        };
        if let Ok(us) = seconds_to_us(num.trim()) {
            return Ok(if us > 10_000_000_000 { us } else { 0 });
        }
    }
    Ok(0)
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
