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
}

impl LogFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Slog => "SLOGv1",
            Self::Slbin => "SLB1",
            Self::CanCsv => "CAN CSV",
            Self::DecodedCsv => "Decoded CSV",
        }
    }
}

#[derive(Debug, Clone)]
pub enum RecKind {
    Frame {
        id: u32,
        dlc: u8,
        data: [u8; 8],
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
    pub offset: u64,
    pub t_us: u64,
    pub kind: RecKind,
}

pub fn sniff(head: &[u8]) -> Result<LogFormat> {
    if head.starts_with(b"SLB1") {
        return Ok(LogFormat::Slbin);
    }
    let text = String::from_utf8_lossy(head);
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "SLOGv1" {
            return Ok(LogFormat::Slog);
        }
        let mut parts = line.split_whitespace();
        if let Some(tag) = parts.next() {
            if (tag == "F" || tag == "E") && parts.next().is_some_and(|t| t.parse::<u64>().is_ok())
            {
                return Ok(LogFormat::Slog);
            }
        }
        if line.contains(',') {
            return csv_format(line);
        }
        return Err(Error::msg(
            "unrecognized log. Expected SLOGv1 text, SLB1 binary, or a CSV with a t_us column.",
        ));
    }
    Err(Error::msg("log is empty"))
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
}

pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

impl<'a> Scanner<'a> {
    pub fn open(reader: &'a mut dyn ReadSeek, format: LogFormat) -> Result<Self> {
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
        };
        if format == LogFormat::Slbin {
            scanner.consume_binary_header()?;
        }
        Ok(scanner)
    }

    /// Continue at a record boundary recorded in the checkpoint index.
    pub fn resume(reader: &'a mut dyn ReadSeek, format: LogFormat, offset: u64) -> Result<Self> {
        reader
            .seek(SeekFrom::Start(offset))
            .map_err(|err| Error::msg(format!("could not seek log: {err}")))?;
        Ok(Self {
            reader,
            format,
            pos: offset,
            line_no: 0,
            header_pending: false,
        })
    }

    pub fn next_rec(&mut self) -> Result<Option<Rec>> {
        match self.format {
            LogFormat::Slbin => self.next_binary(),
            LogFormat::Slog | LogFormat::CanCsv | LogFormat::DecodedCsv => self.next_text(),
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
        match self.reader.read(&mut tag) {
            Ok(0) => return Ok(None),
            Ok(_) => self.pos += 1,
            Err(err) => return Err(Error::msg(format!("binary read failed: {err}"))),
        }
        let t_us = self.read_u64()?;
        match tag[0] {
            1 => {
                let id = self.read_u32()?;
                let mut dlc_buf = [0u8; 1];
                self.read_exact_bin(&mut dlc_buf)?;
                let mut data = [0u8; 8];
                self.read_exact_bin(&mut data)?;
                let dlc = dlc_buf[0].min(8);
                Ok(Some(Rec {
                    offset: start,
                    t_us,
                    kind: RecKind::Frame { id, dlc, data },
                }))
            }
            2 => {
                let len = self.read_u16()? as usize;
                if len > 4_000 {
                    return Err(Error::Binary {
                        offset: start,
                        message: "event label is too long".into(),
                    });
                }
                let mut buf = vec![0u8; len];
                if len > 0 {
                    self.read_exact_bin(&mut buf)?;
                }
                let label = String::from_utf8(buf).map_err(|_| Error::Binary {
                    offset: start,
                    message: "event label is not utf-8".into(),
                })?;
                Ok(Some(Rec {
                    offset: start,
                    t_us,
                    kind: RecKind::Event { label },
                }))
            }
            other => Err(Error::Binary {
                offset: start,
                message: format!("unknown record tag {other}"),
            }),
        }
    }

    fn next_text(&mut self) -> Result<Option<Rec>> {
        loop {
            let start = self.pos;
            let Some(line) = self.read_line()? else {
                return Ok(None);
            };
            let trimmed = line.trim().trim_start_matches('\u{feff}');
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed == "SLOGv1" {
                continue;
            }
            if self.header_pending {
                self.header_pending = false;
                csv_format(trimmed)?;
                continue;
            }
            let mut rec = self.parse_line(trimmed)?;
            rec.offset = start;
            return Ok(Some(rec));
        }
    }

    fn parse_line(&self, line: &str) -> Result<Rec> {
        match self.format {
            LogFormat::Slog => self.parse_slog(line),
            LogFormat::CanCsv => self.parse_can_csv(line),
            LogFormat::DecodedCsv => self.parse_decoded_csv(line),
            LogFormat::Slbin => unreachable!(),
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
                    kind: RecKind::Frame { id, dlc, data },
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
                    kind: RecKind::Event { label },
                })
            }
            _ => self.bad("expected an F frame or E event record"),
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
        Ok(Rec {
            offset: 0,
            t_us,
            kind: RecKind::Frame { id, dlc, data },
        })
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
        loop {
            let mut byte = [0u8; 1];
            let n = self
                .reader
                .read(&mut byte)
                .map_err(|err| Error::msg(format!("read failed: {err}")))?;
            if n == 0 {
                if buf.is_empty() {
                    return Ok(None);
                }
                break;
            }
            self.pos += 1;
            if byte[0] == b'\n' {
                break;
            }
            if buf.len() > 1_000_000 {
                return Err(Error::Parse {
                    line: self.line_no + 1,
                    message: "line is longer than 1MB".into(),
                });
            }
            if byte[0] != b'\r' {
                buf.push(byte[0]);
            }
        }
        self.line_no += 1;
        String::from_utf8(buf).map(Some).map_err(|_| Error::Parse {
            line: self.line_no,
            message: "line is not utf-8".into(),
        })
    }

    fn read_exact_bin(&mut self, buf: &mut [u8]) -> Result<()> {
        let start = self.pos;
        let mut off = 0;
        while off < buf.len() {
            let n = self
                .reader
                .read(&mut buf[off..])
                .map_err(|err| Error::msg(format!("binary read failed: {err}")))?;
            if n == 0 {
                return Err(Error::Binary {
                    offset: start,
                    message: "truncated SLB1 record".into(),
                });
            }
            off += n;
            self.pos += n as u64;
        }
        Ok(())
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

pub fn parse_payload(text: &str) -> std::result::Result<([u8; 8], u8), String> {
    let hex: String = text
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && *c != '_')
        .collect();
    if hex.len() > 16 || !hex.len().is_multiple_of(2) {
        return Err("payload must be an even number of hex digits, at most 8 bytes".into());
    }
    let dlc = (hex.len() / 2) as u8;
    let mut data = [0u8; 8];
    for i in 0..dlc as usize {
        data[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| format!("bad payload hex '{}'", &hex[i * 2..i * 2 + 2]))?;
    }
    Ok((data, dlc))
}

fn split_csv(line: &str) -> Vec<&str> {
    line.split(',').map(|c| c.trim()).collect()
}

/// Build an SLB1 blob. Tests use this to round-trip the binary reader.
#[cfg(test)]
pub fn encode_slb1(records: &[Rec]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"SLB1");
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    for rec in records {
        match &rec.kind {
            RecKind::Frame { id, dlc, data } => {
                out.push(1);
                out.extend_from_slice(&rec.t_us.to_le_bytes());
                out.extend_from_slice(&id.to_le_bytes());
                out.push(*dlc);
                out.extend_from_slice(data);
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
