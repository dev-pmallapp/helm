// src/error.rs -- SinkError and FormatError types for helm-report.

use std::io;

/// Errors returned by sink operations.
#[derive(Debug)]
pub enum SinkError {
    /// A single sink returned an I/O error.
    Io(io::Error),
    /// Multiple sinks returned errors; deliver() continues past each failure.
    MultipleErrors(Vec<io::Error>),
    /// URI string could not be parsed into a valid sink.
    InvalidUri(String),
}

impl std::fmt::Display for SinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SinkError::Io(e) => write!(f, "sink I/O error: {e}"),
            SinkError::MultipleErrors(es) => {
                write!(f, "{} sink error(s):", es.len())?;
                for e in es {
                    write!(f, "\n  - {e}")?;
                }
                Ok(())
            }
            SinkError::InvalidUri(uri) => write!(f, "invalid sink URI: {uri:?}"),
        }
    }
}

impl std::error::Error for SinkError {}

impl From<io::Error> for SinkError {
    fn from(e: io::Error) -> Self {
        SinkError::Io(e)
    }
}
