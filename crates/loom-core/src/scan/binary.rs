//! SLB1 binary: a 16-byte header, then tagged records.

use super::id::implied_extended;
use super::source::{Skips, Source};
use super::{ReadSeek, Rec, RecKind, RecordReader, MAX_CLASSIC_DATA, MAX_DATA};
use crate::error::{Error, Result};

const MAGIC: &[u8; 4] = b"SLB1";
const VERSION: u16 = 1;
/// Header bytes after the magic: version, reserved, reserved.
const HEADER_REST_LEN: usize = 12;
const TAG_FRAME: u8 = 1;
const TAG_EVENT: u8 = 2;
/// Longest event label a binary record may carry.
const MAX_LABEL: usize = 4_000;

pub(super) struct Slb1Reader<'a> {
    src: Source<'a>,
    skips: Skips,
}

impl<'a> Slb1Reader<'a> {
    /// Continue at `offset`, a record boundary. At 0 the header is read and checked.
    pub(super) fn open(reader: &'a mut dyn ReadSeek, offset: u64) -> Result<Self> {
        let mut this = Self {
            src: Source::at(reader, offset)?,
            skips: Skips::default(),
        };
        if offset == 0 {
            this.read_header()?;
        }
        Ok(this)
    }

    fn read_header(&mut self) -> Result<()> {
        if &self.src.read_array::<4>()? != MAGIC {
            return Err(Error::Binary {
                offset: 0,
                message: "missing SLB1 magic".into(),
            });
        }
        let rest = self.src.read_array::<HEADER_REST_LEN>()?;
        let version = u16::from_le_bytes([rest[0], rest[1]]);
        if version != VERSION {
            return Err(Error::Binary {
                offset: 4,
                message: format!("SLB1 version {version} is not supported"),
            });
        }
        Ok(())
    }

    fn read_record(&mut self, start: u64, tag: u8) -> Result<Option<Rec>> {
        let t_us = u64::from_le_bytes(self.src.read_array()?);
        match tag {
            TAG_FRAME => {
                let id = u32::from_le_bytes(self.src.read_array()?);
                let [dlc] = self.src.read_array::<1>()?;
                let stored = self.src.read_array::<MAX_CLASSIC_DATA>()?;
                let mut data = [0u8; MAX_DATA];
                data[..MAX_CLASSIC_DATA].copy_from_slice(&stored);
                let dlc = dlc.min(MAX_CLASSIC_DATA as u8);
                let rec = Rec::frame(t_us, id, implied_extended(id), 0, dlc, data);
                Ok(Some(rec.placed(start, false)))
            }
            TAG_EVENT => {
                let len = usize::from(u16::from_le_bytes(self.src.read_array()?));
                if len > MAX_LABEL {
                    self.skips
                        .note(format!("byte {start}: event label is too long"));
                    return Ok(None);
                }
                let mut label = vec![0u8; len];
                self.src.read_exact(&mut label)?;
                let Ok(label) = String::from_utf8(label) else {
                    self.skips
                        .note(format!("byte {start}: event label is not utf-8"));
                    return Ok(None);
                };
                Ok(Some(
                    Rec::new(t_us, RecKind::Event { label }).placed(start, false),
                ))
            }
            other => {
                self.skips.note(format!(
                    "byte {start}: unknown SLB1 tag {other}; stopped, the rest of the file was not indexed"
                ));
                Ok(None)
            }
        }
    }

    /// A record cut short by the end of the file is counted as skipped. Any other
    /// read failure is returned, so the index does not end as if the log were complete.
    fn fault(&mut self, err: Error) -> Result<Option<Rec>> {
        if !matches!(err, Error::Binary { .. }) {
            return Err(err);
        }
        self.skips.note(err.to_string());
        Ok(None)
    }
}

impl RecordReader for Slb1Reader<'_> {
    fn next_rec(&mut self) -> Result<Option<Rec>> {
        let start = self.src.pos();
        let [tag] = match self.src.read_array::<1>() {
            Ok(tag) => tag,
            Err(err) if self.src.pos() == start && matches!(err, Error::Binary { .. }) => {
                return Ok(None);
            }
            Err(err) => return self.fault(err),
        };
        match self.read_record(start, tag) {
            Err(err) => self.fault(err),
            done => done,
        }
    }

    fn position(&self) -> u64 {
        self.src.pos()
    }

    fn skipped(&self) -> u64 {
        self.skips.count()
    }

    fn notes(&self) -> Vec<String> {
        self.skips.warnings().to_vec()
    }
}
