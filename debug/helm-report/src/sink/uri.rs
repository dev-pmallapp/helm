// src/sink/uri.rs -- sink_from_uri(): construct a Sink from a URI string.

use super::{AsyncFileSink, FileSink, NullSink, Sink, StderrSink, TcpSink};
use crate::error::SinkError;

/// Construct a `Box<dyn Sink>` from a URI string.
///
/// Supported URI schemes:
///
/// | URI                        | Sink             |
/// |----------------------------|------------------|
/// | `stderr:`                  | `StderrSink`     |
/// | `null:`                    | `NullSink`       |
/// | `file:/absolute/path`      | `AsyncFileSink`  |
/// | `file+sync:/absolute/path` | `FileSink`       |
/// | `tcp:host:port`            | `TcpSink`        |
///
/// Returns `Err(SinkError::InvalidUri)` for unrecognised or malformed URIs.
/// Returns `Err(SinkError::Io)` if the sink cannot be opened.
pub fn sink_from_uri(uri: &str) -> Result<Box<dyn Sink>, SinkError> {
    if uri == "stderr:" {
        return Ok(Box::new(StderrSink));
    }
    if uri == "null:" {
        return Ok(Box::new(NullSink));
    }
    if let Some(path) = uri.strip_prefix("file+sync:") {
        let sink = FileSink::open(path).map_err(SinkError::Io)?;
        return Ok(Box::new(sink));
    }
    if let Some(path) = uri.strip_prefix("file:") {
        let (sink, _handle) = AsyncFileSink::open(path).map_err(SinkError::Io)?;
        return Ok(Box::new(sink));
    }
    if let Some(addr) = uri.strip_prefix("tcp:") {
        // Validate: must contain at least one colon separating host and port.
        if !addr.contains(':') {
            return Err(SinkError::InvalidUri(format!(
                "tcp: URI must be tcp:host:port, got {uri:?}"
            )));
        }
        let sink = TcpSink::connect(addr).map_err(SinkError::Io)?;
        return Ok(Box::new(sink));
    }
    Err(SinkError::InvalidUri(format!(
        "unrecognised URI scheme in {uri:?}"
    )))
}

#[cfg(all(test, feature = "report"))]
mod tests {
    use super::*;

    #[test]
    fn uri_stderr_colon() {
        let sink = sink_from_uri("stderr:").unwrap();
        assert_eq!(sink.name(), "stderr");
    }

    #[test]
    fn uri_null_colon() {
        let sink = sink_from_uri("null:").unwrap();
        assert_eq!(sink.name(), "null");
    }

    #[test]
    fn uri_file_sync() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let uri = format!("file+sync:{}", tmp.path().display());
        let sink = sink_from_uri(&uri).unwrap();
        assert!(sink.name().contains('/'));
    }

    #[test]
    fn uri_file_async() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let uri = format!("file:{}", tmp.path().display());
        let sink = sink_from_uri(&uri).unwrap();
        assert!(sink.name().contains('/'));
    }

    #[test]
    fn uri_tcp_malformed_no_port() {
        let result = sink_from_uri("tcp:hostname");
        assert!(
            matches!(result, Err(crate::error::SinkError::InvalidUri(_))),
            "expected InvalidUri"
        );
    }

    #[test]
    fn uri_unknown_scheme() {
        let result = sink_from_uri("ftp:somehost");
        assert!(
            matches!(result, Err(crate::error::SinkError::InvalidUri(_))),
            "expected InvalidUri for unknown scheme"
        );
    }

    #[test]
    fn uri_empty_string() {
        let result = sink_from_uri("");
        assert!(result.is_err());
    }
}
