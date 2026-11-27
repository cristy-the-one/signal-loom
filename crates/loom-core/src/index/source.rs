use crate::error::{Error, Result};
use crate::scan::{sniff, LogFormat, ReadSeek};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
pub(super) enum Source {
    Path(PathBuf),
    Memory(Arc<Vec<u8>>),
}

impl Source {
    pub(super) fn byte_len(&self) -> Result<u64> {
        match self {
            Self::Path(path) => std::fs::metadata(path)
                .map(|meta| meta.len())
                .map_err(|err| Error::read(path, err)),
            Self::Memory(bytes) => Ok(bytes.len() as u64),
        }
    }

    /// SHA-256 of the whole recording, lowercase hex, read in 64 KiB chunks.
    pub(super) fn sha256_hex(&self) -> Result<String> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        match self {
            Self::Memory(bytes) => hasher.update(bytes.as_slice()),
            Self::Path(path) => {
                let mut file = File::open(path).map_err(|err| Error::read(path, err))?;
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = file.read(&mut buf).map_err(|err| Error::read(path, err))?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buf[..n]);
                }
            }
        }
        Ok(hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }

    pub(super) fn with_reader<T>(
        &self,
        body: impl FnOnce(&mut dyn ReadSeek) -> Result<T>,
    ) -> Result<T> {
        match self {
            Self::Path(path) => {
                let mut file = File::open(path).map_err(|err| Error::read(path, err))?;
                body(&mut file)
            }
            Self::Memory(bytes) => {
                let mut cursor = Cursor::new(bytes.as_slice());
                body(&mut cursor)
            }
        }
    }
}

pub(super) fn sniff_path(path: &Path) -> Result<LogFormat> {
    let mut file = File::open(path).map_err(|err| Error::read(path, err))?;
    let mut head = [0u8; 4096];
    let n = file.read(&mut head).map_err(|err| Error::read(path, err))?;
    sniff(&head[..n])
}
