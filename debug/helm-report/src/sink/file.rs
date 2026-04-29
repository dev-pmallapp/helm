// src/sink/file.rs -- FileSink: synchronous buffered file sink.
//
// Dual-impl per `docs/design/helm-report/HLD.md` § 13. With `report`
// off, `FileSink::open()` returns `Ok(Self)` (a ZST) WITHOUT touching
// the filesystem -- perf builds neither create files nor reserve a
// `BufWriter`.

#[cfg(feature = "report")]
pub use live::FileSink;
#[cfg(not(feature = "report"))]
pub use noop::FileSink;

#[cfg(feature = "report")]
mod live {
    use crate::sink::Sink;
    use std::fs::File;
    use std::io::{self, BufWriter, Write};
    use std::path::Path;
    use std::sync::Mutex;

    /// Synchronous buffered file sink.
    ///
    /// Uses an 8 KB `BufWriter`. The `Mutex` makes `write()` safe to
    /// call from multiple threads, though concurrent writes from
    /// unrelated reports are not expected in practice.
    ///
    /// Prefer `AsyncFileSink` when the caller cannot afford to block
    /// on I/O.
    pub struct FileSink {
        inner: Mutex<BufWriter<File>>,
        path: String,
    }

    impl FileSink {
        pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
            let path = path.as_ref();
            let file = File::create(path)?;
            Ok(FileSink {
                inner: Mutex::new(BufWriter::with_capacity(8 * 1024, file)),
                path: path.to_string_lossy().into_owned(),
            })
        }
    }

    impl Sink for FileSink {
        fn write(&self, data: &[u8]) -> io::Result<()> {
            let mut guard = self.inner.lock().unwrap();
            guard.write_all(data)
        }

        fn flush(&self) -> io::Result<()> {
            self.inner.lock().unwrap().flush()
        }

        fn name(&self) -> &str {
            &self.path
        }
    }

    impl Drop for FileSink {
        fn drop(&mut self) {
            // Best-effort flush; ignore error on drop.
            let _ = self.inner.lock().unwrap().flush();
        }
    }
}

#[cfg(not(feature = "report"))]
mod noop {
    use crate::sink::Sink;
    use std::io;
    use std::path::Path;

    /// ZST shell when `report` is off. `open()` succeeds without
    /// touching the filesystem; `write()` and `flush()` are no-ops.
    pub struct FileSink;

    impl FileSink {
        #[inline(always)]
        pub fn open(_path: impl AsRef<Path>) -> io::Result<Self> {
            Ok(FileSink)
        }
    }

    impl Sink for FileSink {
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
    use tempfile::NamedTempFile;

    #[test]
    fn filesink_write_and_read_back() {
        let tmp = NamedTempFile::new().unwrap();
        let sink = FileSink::open(tmp.path()).unwrap();

        sink.write(b"hello, helm-report\n").unwrap();
        sink.write(b"second line\n").unwrap();
        sink.flush().unwrap();
        drop(sink);

        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(contents, "hello, helm-report\nsecond line\n");
    }

    #[test]
    fn filesink_flush_on_drop() {
        let tmp = NamedTempFile::new().unwrap();
        {
            let sink = FileSink::open(tmp.path()).unwrap();
            sink.write(b"flushed-on-drop\n").unwrap();
            // Drop without explicit flush.
        }
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(contents.contains("flushed-on-drop"));
    }

    #[test]
    fn filesink_name_is_path() {
        let tmp = NamedTempFile::new().unwrap();
        let path_str = tmp.path().to_string_lossy().into_owned();
        let sink = FileSink::open(tmp.path()).unwrap();
        assert_eq!(sink.name(), path_str);
    }

    #[test]
    fn filesink_open_nonexistent_dir_fails() {
        let result = FileSink::open("/nonexistent/path/that/cannot/exist/file.txt");
        assert!(result.is_err());
    }
}
