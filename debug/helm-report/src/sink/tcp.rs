// src/sink/tcp.rs -- TcpSink: buffered TCP stream sink.
//
// Dual-impl per `docs/design/helm-report/HLD.md` § 13. The `noop`
// version of `connect()` returns `Err(ConnectionRefused)` so a perf
// build that somehow asks for a TCP sink fails fast (no socket, no
// background work) -- the URI dispatcher already returns
// `InvalidUri` in that case, so this only matters if a caller wired
// `TcpSink::connect()` directly.

#[cfg(feature = "report")]
pub use live::TcpSink;
#[cfg(not(feature = "report"))]
pub use noop::TcpSink;

#[cfg(feature = "report")]
mod live {
    use crate::sink::Sink;
    use std::io::{self, BufWriter, Write};
    use std::net::TcpStream;
    use std::sync::Mutex;

    /// Buffered TCP stream sink.
    ///
    /// Connects once at construction time. If the connection drops,
    /// writes return `Err(BrokenPipe)` -- the sink does NOT attempt
    /// to reconnect. The caller should create a new `TcpSink` if
    /// reconnection is desired.
    ///
    /// Buffer size: 4 KB. Flushed after every `flush()` call.
    pub struct TcpSink {
        inner: Mutex<BufWriter<TcpStream>>,
        addr: String,
    }

    impl TcpSink {
        pub fn connect(addr: &str) -> io::Result<Self> {
            let stream = TcpStream::connect(addr)?;
            Ok(TcpSink {
                inner: Mutex::new(BufWriter::with_capacity(4 * 1024, stream)),
                addr: addr.to_owned(),
            })
        }
    }

    impl Sink for TcpSink {
        fn write(&self, data: &[u8]) -> io::Result<()> {
            self.inner.lock().unwrap().write_all(data)
        }

        fn flush(&self) -> io::Result<()> {
            self.inner.lock().unwrap().flush()
        }

        fn name(&self) -> &str {
            &self.addr
        }
    }
}

#[cfg(not(feature = "report"))]
mod noop {
    use crate::sink::Sink;
    use std::io;

    /// ZST shell. `connect()` returns `Err(ConnectionRefused)` so a
    /// caller that bypassed the URI dispatcher fails fast instead of
    /// silently dropping data.
    pub struct TcpSink;

    impl TcpSink {
        #[inline(always)]
        pub fn connect(_addr: &str) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "TcpSink requires the 'report' feature",
            ))
        }
    }

    impl Sink for TcpSink {
        #[inline(always)]
        fn write(&self, _data: &[u8]) -> io::Result<()> {
            Ok(())
        }

        #[inline(always)]
        fn name(&self) -> &str {
            ""
        }
    }
}

#[cfg(all(test, feature = "report"))]
mod tests {
    use super::*;
    use crate::sink::Sink;
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    /// Spawn a single-connection TCP server on a random port.
    fn mock_tcp_server() -> (String, mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = Vec::new();
                let _ = stream.read_to_end(&mut buf);
                let _ = tx.send(buf);
            }
        });
        (addr, rx)
    }

    #[test]
    fn tcp_sink_connect_and_write() {
        let (addr, rx) = mock_tcp_server();
        let sink = TcpSink::connect(&addr).unwrap();
        sink.write(b"hello from TcpSink\n").unwrap();
        sink.flush().unwrap();
        drop(sink);

        let received = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert!(received.starts_with(b"hello from TcpSink"));
    }

    #[test]
    fn tcp_sink_connect_refused_returns_error() {
        let result = TcpSink::connect("127.0.0.1:1");
        assert!(result.is_err(), "expected connection refused");
    }

    #[test]
    fn tcp_sink_name_is_address() {
        let (addr, _rx) = mock_tcp_server();
        let sink = TcpSink::connect(&addr).unwrap();
        assert_eq!(sink.name(), addr);
    }
}
