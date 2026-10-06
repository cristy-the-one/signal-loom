use std::path::PathBuf;

/// Recoverable failure from parsing, indexing, or project load.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("line {line}: {message}")]
    Parse { line: u64, message: String },
    #[error("byte {offset}: {message}")]
    Binary { offset: u64, message: String },
    #[error("{0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn read(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Read {
            path: path.into().display().to_string(),
            source,
        }
    }

    pub fn write(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Write {
            path: path.into().display().to_string(),
            source,
        }
    }

    pub fn msg(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}
