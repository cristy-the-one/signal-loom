use serde::{Serialize, Serializer};
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
    /// An I/O failure on something already open, with what was being done.
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("line {line}: {message}")]
    Parse { line: u64, message: String },
    #[error("byte {offset}: {message}")]
    Binary { offset: u64, message: String },
    /// Something the caller named does not exist: a log, a signal, a channel.
    #[error("{0}")]
    NotFound(String),
    /// Input the caller can fix: a bad expression, trigger, project or file.
    #[error("{0}")]
    Invalid(String),
    /// The caller asked a running job to stop.
    #[error("{0}")]
    Cancelled(String),
    /// A fault in the engine itself: a poisoned lock, a worker panic.
    #[error("{0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// What went wrong, in terms an adapter can act on without reading the message.
/// Serialized in snake case, so `NotFound` crosses the boundary as `"not_found"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotFound,
    Invalid,
    Parse,
    Binary,
    Cancelled,
    Io,
    Internal,
}

/// An error as both transports send it: `{"kind": "...", "message": "..."}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorBody {
    pub kind: ErrorKind,
    pub message: String,
}

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

    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::Cancelled(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    /// The kind an adapter branches on. A read of a file that is not there is
    /// `NotFound`; every other failed read or write is `Io`.
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Read { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                ErrorKind::NotFound
            }
            Self::Read { .. } | Self::Write { .. } | Self::Io { .. } => ErrorKind::Io,
            Self::Parse { .. } => ErrorKind::Parse,
            Self::Binary { .. } => ErrorKind::Binary,
            Self::NotFound(_) => ErrorKind::NotFound,
            Self::Invalid(_) => ErrorKind::Invalid,
            Self::Cancelled(_) => ErrorKind::Cancelled,
            Self::Internal(_) => ErrorKind::Internal,
        }
    }

    pub fn body(&self) -> ErrorBody {
        ErrorBody {
            kind: self.kind(),
            message: self.to_string(),
        }
    }
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.body().serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn each_error_maps_to_its_kind() {
        let denied = || io::Error::from(io::ErrorKind::PermissionDenied);
        let cases = [
            (Error::read("a.log", denied()), ErrorKind::Io),
            (
                Error::read("a.log", io::Error::from(io::ErrorKind::NotFound)),
                ErrorKind::NotFound,
            ),
            (Error::write("a.csv", denied()), ErrorKind::Io),
            (
                Error::write("a.csv", io::Error::from(io::ErrorKind::NotFound)),
                ErrorKind::Io,
            ),
            (Error::io("could not seek log", denied()), ErrorKind::Io),
            (
                Error::Parse {
                    line: 3,
                    message: "bad".into(),
                },
                ErrorKind::Parse,
            ),
            (
                Error::Binary {
                    offset: 9,
                    message: "cut".into(),
                },
                ErrorKind::Binary,
            ),
            (Error::not_found("no log is open"), ErrorKind::NotFound),
            (Error::invalid("bad number"), ErrorKind::Invalid),
            (Error::cancelled("indexing cancelled"), ErrorKind::Cancelled),
            (Error::internal("poisoned"), ErrorKind::Internal),
        ];
        for (err, kind) in cases {
            assert_eq!(err.kind(), kind, "{err}");
        }
    }

    #[test]
    fn an_error_serializes_as_kind_and_message() {
        let err = Error::io("could not seek log", io::Error::other("disk gone"));
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            serde_json::json!({ "kind": "io", "message": "could not seek log: disk gone" })
        );
        assert_eq!(
            serde_json::to_value(Error::not_found("no log is open")).unwrap(),
            serde_json::json!({ "kind": "not_found", "message": "no log is open" })
        );
    }
}
