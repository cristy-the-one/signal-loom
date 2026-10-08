//! The byte stream under a reader, and the skipped-record counter every reader keeps.

use super::ReadSeek;
use crate::error::{Error, Result};
use std::io::{ErrorKind, SeekFrom};

const CHUNK: usize = 64 * 1024;
/// A text line past this many bytes is skipped.
const MAX_LINE: usize = 1_000_000;
/// Distinct warning texts kept per scan. The skipped count keeps going.
const MAX_WARNINGS: usize = 32;

/// What `Source::read_line` found.
pub(super) enum Line {
    End,
    Text,
    TooLong,
}

/// Position-tracking view of the log. Text is read through a chunk buffer;
/// binary reads go straight to the reader.
pub(super) struct Source<'a> {
    reader: &'a mut dyn ReadSeek,
    pos: u64,
    buf: Vec<u8>,
    at: usize,
    len: usize,
}

impl<'a> Source<'a> {
    pub(super) fn at(reader: &'a mut dyn ReadSeek, pos: u64) -> Result<Self> {
        reader
            .seek(SeekFrom::Start(pos))
            .map_err(|err| Error::msg(format!("could not seek log: {err}")))?;
        Ok(Self {
            reader,
            pos,
            buf: vec![0; CHUNK],
            at: 0,
            len: 0,
        })
    }

    pub(super) fn pos(&self) -> u64 {
        self.pos
    }

    /// The next line without its `\n`, with every `\r` dropped, in `line`.
    /// A line past `MAX_LINE` is consumed to its end and reported as `TooLong`.
    pub(super) fn read_line(&mut self, line: &mut Vec<u8>) -> Result<Line> {
        line.clear();
        let mut too_long = false;
        loop {
            if self.at >= self.len {
                self.len = read_retrying(self.reader, &mut self.buf)
                    .map_err(|err| Error::msg(format!("read failed: {err}")))?;
                self.at = 0;
                if self.len == 0 {
                    break;
                }
            }
            let chunk = &self.buf[self.at..self.len];
            let end = chunk.iter().position(|byte| *byte == b'\n');
            for byte in &chunk[..end.unwrap_or(chunk.len())] {
                if *byte == b'\r' {
                    continue;
                }
                if line.len() >= MAX_LINE {
                    too_long = true;
                    continue;
                }
                line.push(*byte);
            }
            let used = end.map_or(chunk.len(), |at| at + 1);
            self.at += used;
            self.pos += used as u64;
            if end.is_some() {
                return Ok(if too_long { Line::TooLong } else { Line::Text });
            }
        }
        if line.is_empty() && !too_long {
            return Ok(Line::End);
        }
        Ok(if too_long { Line::TooLong } else { Line::Text })
    }

    /// Fill `buf`, or fail with `Error::Binary` when the log ends first.
    pub(super) fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        let start = self.pos;
        let mut off = 0;
        while off < buf.len() {
            if self.at >= self.len {
                let n = read_retrying(self.reader, &mut buf[off..])
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
            buf[off] = self.buf[self.at];
            self.at += 1;
            self.pos += 1;
            off += 1;
        }
        Ok(())
    }

    pub(super) fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut bytes = [0u8; N];
        self.read_exact(&mut bytes)?;
        Ok(bytes)
    }
}

/// Records left out of a scan: how many, and the first distinct reasons.
#[derive(Default)]
pub(super) struct Skips {
    count: u64,
    warnings: Vec<String>,
}

impl Skips {
    pub(super) fn with_warnings(warnings: Vec<String>) -> Self {
        Self { count: 0, warnings }
    }

    pub(super) fn count(&self) -> u64 {
        self.count
    }

    pub(super) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub(super) fn note(&mut self, message: impl Into<String>) {
        self.count += 1;
        if self.warnings.len() < MAX_WARNINGS {
            let message = message.into();
            if !self.warnings.iter().any(|have| have == &message) {
                self.warnings.push(message);
            }
        }
    }
}

/// `read` again when a signal interrupted it. Any other failure is returned.
pub(super) fn read_retrying(reader: &mut dyn ReadSeek, buf: &mut [u8]) -> std::io::Result<usize> {
    loop {
        match reader.read(buf) {
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            other => return other,
        }
    }
}

/// Bytes read from the top of the log to decide a text format's mode.
const PEEK_LEN: usize = 4096;

/// The first `PEEK_LEN` bytes as text. The reader's position is put back.
pub(super) fn peek_prefix(reader: &mut dyn ReadSeek) -> Result<String> {
    let pos = reader
        .stream_position()
        .map_err(|err| Error::msg(format!("could not tell log position: {err}")))?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|err| Error::msg(format!("could not rewind log: {err}")))?;
    let mut buf = [0u8; PEEK_LEN];
    let n = reader
        .read(&mut buf)
        .map_err(|err| Error::msg(format!("could not read log: {err}")))?;
    reader
        .seek(SeekFrom::Start(pos))
        .map_err(|err| Error::msg(format!("could not restore log position: {err}")))?;
    Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
}
