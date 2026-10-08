//! Vector BLF files: a `LOGG` header, then `LOBJ` objects. Log containers are
//! inflated one at a time and their frames handed out in order.

use super::source::{Skips, Source};
use super::{ReadSeek, Rec, RecordReader};
use crate::blf;
use crate::error::{Error, Result};

/// The file header is its signature, its own size, and fields we do not read.
const FILE_HEADER_MIN: usize = 8;
const FILE_HEADER_MAX: usize = 1_048_576;
/// Objects are padded to a multiple of this many bytes.
const OBJECT_ALIGN: usize = 4;

pub(super) struct BlfReader<'a> {
    src: Source<'a>,
    skips: Skips,
    /// The object last read from the file.
    object: Vec<u8>,
    /// The inflated container being walked, and how far into it.
    inflated: Vec<u8>,
    at: usize,
    /// Offset of that container in the file. Checkpoints resume from it.
    container: u64,
    /// The next frame is the first of its container.
    fresh: bool,
}

impl<'a> BlfReader<'a> {
    /// Continue at `offset`, an object boundary. At 0 the file header is read and checked.
    pub(super) fn open(reader: &'a mut dyn ReadSeek, offset: u64) -> Result<Self> {
        let mut this = Self {
            src: Source::at(reader, offset)?,
            skips: Skips::default(),
            object: Vec::new(),
            inflated: Vec::new(),
            at: 0,
            container: 0,
            fresh: false,
        };
        if offset == 0 {
            this.read_file_header()?;
        }
        Ok(this)
    }

    fn read_file_header(&mut self) -> Result<()> {
        if &self.src.read_array::<4>()? != blf::FILE_MAGIC {
            return Err(Error::invalid("BLF is missing the LOGG header"));
        }
        let size = u32::from_le_bytes(self.src.read_array()?) as usize;
        if !(FILE_HEADER_MIN..=FILE_HEADER_MAX).contains(&size) {
            return Err(Error::invalid("BLF header size is not usable"));
        }
        self.object.resize(size - FILE_HEADER_MIN, 0);
        self.src.read_exact(&mut self.object)
    }

    /// Read the next object from the file into `self.object`: its offset and type.
    /// `None` at the end of the file, or where the rest cannot be walked.
    fn read_object(&mut self) -> Result<Option<(u64, u32)>> {
        let start = self.src.pos();
        let head = match self.src.read_array::<{ blf::OBJECT_HEAD_LEN }>() {
            Ok(head) => head,
            Err(Error::Binary { .. }) => return Ok(None),
            Err(err) => return Err(err),
        };
        if &head[..4] != blf::OBJECT_MAGIC {
            self.skips.note(format!(
                "byte {start}: BLF object is not LOBJ; stopped indexing the rest of the file"
            ));
            return Ok(None);
        }
        let (size, obj_type) = blf::object_size_and_type(&head);
        if !(blf::OBJECT_HEAD_LEN..=blf::MAX_OBJECT_LEN).contains(&size) {
            self.skips.note(format!(
                "byte {start}: BLF object size {size} is not usable"
            ));
            return Ok(None);
        }
        self.object.resize(size, 0);
        self.object[..blf::OBJECT_HEAD_LEN].copy_from_slice(&head);
        if let Err(err) = self
            .src
            .read_exact(&mut self.object[blf::OBJECT_HEAD_LEN..])
        {
            if !matches!(err, Error::Binary { .. }) {
                return Err(err);
            }
            self.skips.note(format!("byte {start}: {err}"));
            return Ok(None);
        }
        let pad = (OBJECT_ALIGN - size % OBJECT_ALIGN) % OBJECT_ALIGN;
        let mut padding = [0u8; OBJECT_ALIGN];
        if let Err(err) = self.src.read_exact(&mut padding[..pad]) {
            if !matches!(err, Error::Binary { .. }) {
                return Err(err);
            }
        }
        Ok(Some((start, obj_type)))
    }
}

impl RecordReader for BlfReader<'_> {
    fn next_rec(&mut self) -> Result<Option<Rec>> {
        loop {
            match blf::next_inner_checked(&self.inflated, &mut self.at, self.container) {
                Ok(Some(rec)) => {
                    let offset = rec.offset;
                    let rec = rec.placed(offset, self.fresh);
                    self.fresh = false;
                    return Ok(Some(rec));
                }
                Ok(None) => {}
                Err(message) => {
                    self.skips
                        .note(format!("byte {}: {message}", self.container));
                    if self.at < self.inflated.len() {
                        continue;
                    }
                }
            }
            self.inflated.clear();
            self.at = 0;
            let Some((offset, obj_type)) = self.read_object()? else {
                return Ok(None);
            };
            if blf::is_container(obj_type) {
                match blf::inflate_container(&self.object, &mut self.inflated) {
                    Ok(()) => {
                        self.container = offset;
                        self.fresh = true;
                    }
                    Err(err) => {
                        self.inflated.clear();
                        self.skips.note(format!("byte {offset}: {err}"));
                    }
                }
                continue;
            }
            match blf::decode_object(&self.object) {
                Ok(Some(rec)) => return Ok(Some(rec.placed(offset, true))),
                Ok(None) => {}
                Err(message) => self.skips.note(format!("byte {offset}: {message}")),
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
        self.skips.warnings().to_vec()
    }
}
