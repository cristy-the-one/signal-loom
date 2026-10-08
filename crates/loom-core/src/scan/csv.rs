//! CAN CSV (`t_us,id,data`) and decoded CSV (`t_us,signal,value[,unit]`).

use super::id::implied_extended;
use super::text::{parse_payload, parse_time, Fault, LineParser, LineResult};
use super::{csv_format, LogFormat, Rec, RecKind};
use crate::error::Error;
use crate::map::parse_can_id;

/// The header line, until it has been read and checked against the format.
struct Header {
    expect: LogFormat,
    pending: bool,
}

impl Header {
    /// True when `line` was the header and holds no record.
    fn consume(&mut self, line: &str) -> Result<bool, Fault> {
        if !self.pending {
            return Ok(false);
        }
        self.pending = false;
        let found = csv_format(line).map_err(Fault::Abort)?;
        if found != self.expect {
            return Err(Fault::Abort(Error::msg(format!(
                "CSV header is a {} header, but the log is being read as {}",
                found.label(),
                self.expect.label()
            ))));
        }
        Ok(true)
    }
}

pub(super) struct CanCsv(Header);

impl CanCsv {
    /// `header_pending` is false when reading resumes past the header.
    pub(super) fn new(header_pending: bool) -> Self {
        Self(Header {
            expect: LogFormat::CanCsv,
            pending: header_pending,
        })
    }
}

impl LineParser for CanCsv {
    fn parse(&mut self, line: &str) -> LineResult {
        if self.0.consume(line)? {
            return Ok(None);
        }
        let mut cols = line.split(',').map(str::trim);
        let (Some(time), Some(id), Some(data)) = (cols.next(), cols.next(), cols.next()) else {
            return Err("CAN CSV rows need t_us,id,data".into());
        };
        let t_us = parse_time(Some(time))?;
        let id = parse_can_id(id)?;
        let (data, dlc) = parse_payload(data)?;
        Ok(Some(Rec::frame(
            t_us,
            id,
            implied_extended(id),
            0,
            dlc,
            data,
        )))
    }
}

pub(super) struct DecodedCsv(Header);

impl DecodedCsv {
    pub(super) fn new(header_pending: bool) -> Self {
        Self(Header {
            expect: LogFormat::DecodedCsv,
            pending: header_pending,
        })
    }
}

impl LineParser for DecodedCsv {
    fn parse(&mut self, line: &str) -> LineResult {
        if self.0.consume(line)? {
            return Ok(None);
        }
        let mut cols = line.split(',').map(str::trim);
        let (Some(time), Some(name), Some(value)) = (cols.next(), cols.next(), cols.next()) else {
            return Err("decoded CSV rows need t_us,signal,value".into());
        };
        let t_us = parse_time(Some(time))?;
        if name.is_empty() {
            return Err("signal name is empty".into());
        }
        let value: f64 = value
            .parse()
            .map_err(|_| format!("value '{value}' is not a number"))?;
        if !value.is_finite() {
            return Err("value must be finite".into());
        }
        let unit = cols.next().unwrap_or_default().to_string();
        Ok(Some(Rec::new(
            t_us,
            RecKind::Sample {
                name: name.to_string(),
                value,
                unit,
            },
        )))
    }
}

#[cfg(test)]
mod tests {
    use crate::scan::{LogFormat, RecKind, Scanner};
    use std::io::Cursor;

    #[test]
    fn a_header_for_the_other_csv_is_refused() {
        let mut reader = Cursor::new(b"t_us,signal,value\n0,Speed,1\n".to_vec());
        let mut scanner = Scanner::open(&mut reader, LogFormat::CanCsv).unwrap();
        let err = scanner
            .next_rec()
            .expect_err("a decoded header is not a CAN CSV header");
        assert_eq!(
            err.to_string(),
            "CSV header is a Decoded CSV header, but the log is being read as CAN CSV"
        );
    }

    #[test]
    fn rows_parse_and_resume_skips_the_header_check() {
        let text = "t_us,id,data\n0,416,0102\n1000,0x1A0,03\n";
        let mut reader = Cursor::new(text.as_bytes().to_vec());
        let mut scanner = Scanner::open(&mut reader, LogFormat::CanCsv).unwrap();
        let first = scanner.next_rec().unwrap().unwrap();
        assert!(matches!(
            first.kind,
            RecKind::Frame {
                id: 416,
                dlc: 2,
                ..
            }
        ));
        let second_at = scanner.position();
        let mut reader = Cursor::new(text.as_bytes().to_vec());
        let mut resumed = Scanner::resume(&mut reader, LogFormat::CanCsv, second_at).unwrap();
        let second = resumed.next_rec().unwrap().unwrap();
        assert_eq!((second.offset, second.t_us), (second_at, 1_000));
        assert!(matches!(
            second.kind,
            RecKind::Frame {
                id: 0x1A0,
                dlc: 1,
                ..
            }
        ));
    }

    #[test]
    fn decoded_rows_keep_an_optional_unit() {
        let text = "t_us,signal,value,unit\n0,Speed,12.5,km/h\n1000,Gear,3\n";
        let mut reader = Cursor::new(text.as_bytes().to_vec());
        let mut scanner = Scanner::open(&mut reader, LogFormat::DecodedCsv).unwrap();
        let units: Vec<String> = std::iter::from_fn(|| scanner.next_rec().unwrap())
            .filter_map(|rec| match rec.kind {
                RecKind::Sample { unit, .. } => Some(unit),
                _ => None,
            })
            .collect();
        assert_eq!(units, vec!["km/h".to_string(), String::new()]);
    }
}
