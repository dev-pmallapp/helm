// src/sink/async_file.rs -- AsyncFileSink: background drain thread file sink.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};
use super::Sink;

/// Message sent from the sink to the drain thread.
enum DrainMsg {
    Write(Vec<u8>),
    Flush,
    Stop,
}

/// Asynchronous file sink. Writes are queued to a background drain thread.
///
/// The `SyncSender` with bound 1024 provides bounded memory: if the drain
/// thread falls behind by more than 1024 messages the `write()` call will
/// block -- which is the correct back-pressure behavior.
///
/// Call `join()` on the returned handle to wait for the drain thread to
/// finish flushing. If the sink is dropped without calling `join()`, the
/// drain thread is stopped and remaining queued messages are written before
/// the file is closed.
pub struct AsyncFileSink {
    tx: SyncSender<DrainMsg>,
    name: String,
}

impl AsyncFileSink {
    const CHANNEL_BOUND: usize = 1024;
    const DRAIN_TIMEOUT_MS: u64 = 100;

    /// Open `path` and spawn a drain thread.
    ///
    /// Returns `(sink, join_handle)`. The caller may store the handle and call
    /// `join_handle.join()` at simulation exit to ensure the file is fully written.
    pub fn open(path: impl AsRef<Path>) -> io::Result<(Self, JoinHandle<()>)> {
        let path = path.as_ref();
        let file = File::create(path)?;
        let name = path.to_string_lossy().into_owned();
        let (tx, rx) = mpsc::sync_channel::<DrainMsg>(Self::CHANNEL_BOUND);
        let mut writer = BufWriter::with_capacity(64 * 1024, file);

        let handle = thread::Builder::new()
            .name(format!("helm-report-drain:{name}"))
            .spawn(move || {
                loop {
                    match rx.recv_timeout(std::time::Duration::from_millis(
                        Self::DRAIN_TIMEOUT_MS,
                    )) {
                        Ok(DrainMsg::Write(data)) => {
                            let _ = writer.write_all(&data);
                        }
                        Ok(DrainMsg::Flush) => {
                            let _ = writer.flush();
                        }
                        Ok(DrainMsg::Stop) => {
                            let _ = writer.flush();
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let _ = writer.flush();
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            let _ = writer.flush();
                            break;
                        }
                    }
                }
            })
            .expect("failed to spawn drain thread");

        Ok((AsyncFileSink { tx, name }, handle))
    }
}

impl Sink for AsyncFileSink {
    fn write(&self, data: &[u8]) -> io::Result<()> {
        self.tx
            .send(DrainMsg::Write(data.to_vec()))
            .map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "drain thread stopped")
            })
    }

    fn flush(&self) -> io::Result<()> {
        self.tx.send(DrainMsg::Flush).map_err(|_| {
            io::Error::new(io::ErrorKind::BrokenPipe, "drain thread stopped")
        })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for AsyncFileSink {
    fn drop(&mut self) {
        let _ = self.tx.send(DrainMsg::Stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::Sink;
    use tempfile::NamedTempFile;

    #[test]
    fn async_filesink_delivers_all_writes() {
        let tmp = NamedTempFile::new().unwrap();
        let (sink, handle) = AsyncFileSink::open(tmp.path()).unwrap();

        for i in 0..100u32 {
            sink.write(format!("line {i}\n").as_bytes()).unwrap();
        }
        drop(sink);
        handle.join().expect("drain thread panicked");

        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(contents.contains("line 0\n"));
        assert!(contents.contains("line 99\n"));
        assert_eq!(contents.lines().count(), 100);
    }

    #[test]
    fn async_filesink_join_on_drop() {
        let tmp = NamedTempFile::new().unwrap();
        let (sink, handle) = AsyncFileSink::open(tmp.path()).unwrap();
        sink.write(b"only-write\n").unwrap();
        drop(sink);
        handle.join().unwrap();

        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(contents, "only-write\n");
    }

    #[test]
    fn async_filesink_name_is_path() {
        let tmp = NamedTempFile::new().unwrap();
        let path_str = tmp.path().to_string_lossy().into_owned();
        let (sink, handle) = AsyncFileSink::open(tmp.path()).unwrap();
        assert_eq!(sink.name(), path_str);
        drop(sink);
        handle.join().unwrap();
    }
}
