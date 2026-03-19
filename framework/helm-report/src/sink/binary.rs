// src/sink/binary.rs -- BinaryTraceSink<T>: typed binary trace file with background drain.

use bytemuck::{Pod, Zeroable};
use std::fs::OpenOptions;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    mpsc::{self, SyncSender},
    Arc,
};
use std::thread::{self, JoinHandle};
use super::Sink;

pub const HELM_TRACE_MAGIC: u32 = 0x484D_4C54; // 'HLMT' LE
pub const HELM_TRACE_VERSION: u32 = 1;

/// 80-byte file header written at offset 0.
/// `record_count` is zero at open time; written with the final count on close.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct TraceFileHeader {
    pub magic: u32,          //  0
    pub version: u32,        //  4
    pub record_size: u32,    //  8
    pub record_count: u32,   // 12  -- patched on close
    pub type_name: [u8; 64], // 16  -- null-padded ASCII
}
// total: 80 bytes

// Compile-time size assertion.
const _: () = assert!(std::mem::size_of::<TraceFileHeader>() == 80);

impl TraceFileHeader {
    pub fn new<T>(type_name: &str) -> Self {
        let mut hdr = TraceFileHeader::zeroed();
        hdr.magic = HELM_TRACE_MAGIC;
        hdr.version = HELM_TRACE_VERSION;
        hdr.record_size = std::mem::size_of::<T>() as u32;
        let bytes = type_name.as_bytes();
        let len = bytes.len().min(63);
        hdr.type_name[..len].copy_from_slice(&bytes[..len]);
        hdr
    }
}

enum BinaryMsg<T: Pod + Send + 'static> {
    Records(Vec<T>),
    Stop,
}

/// Typed binary trace sink. Drains records to a binary file in a background thread.
///
/// # Type constraints
/// `T: Copy + Send + Pod + 'static`
/// - `Copy`: drain loop copies records out of the channel message.
/// - `Pod`: safe byte cast via `bytemuck::cast_slice` (no `unsafe` in drain loop).
/// - `Send`: records are moved across thread boundary.
pub struct BinaryTraceSink<T: Copy + Send + Pod + 'static> {
    tx: SyncSender<BinaryMsg<T>>,
    record_count: Arc<AtomicU32>,
    name: String,
}

impl<T: Copy + Send + Pod + 'static> BinaryTraceSink<T> {
    const CHANNEL_BOUND: usize = 512;
    const DRAIN_INTERVAL_MS: u64 = 10;

    /// Open `path`, write the header, and spawn the drain thread.
    pub fn open(
        path: impl AsRef<Path>,
        type_name: &str,
    ) -> io::Result<(Self, JoinHandle<()>)> {
        let path = path.as_ref();
        let name = path.to_string_lossy().into_owned();

        // Write header (record_count = 0; will be patched on Stop).
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        let hdr = TraceFileHeader::new::<T>(type_name);
        file.write_all(bytemuck::bytes_of(&hdr))?;

        let (tx, rx) = mpsc::sync_channel::<BinaryMsg<T>>(Self::CHANNEL_BOUND);
        let record_count = Arc::new(AtomicU32::new(0));
        let rc_clone = Arc::clone(&record_count);
        let path_clone = path.to_path_buf();

        let handle = thread::Builder::new()
            .name(format!("helm-report-binary:{name}"))
            .spawn(move || {
                let mut writer = BufWriter::with_capacity(256 * 1024, file);
                let mut count: u32 = 0;

                loop {
                    match rx.recv_timeout(std::time::Duration::from_millis(
                        Self::DRAIN_INTERVAL_MS,
                    )) {
                        Ok(BinaryMsg::Records(recs)) => {
                            let bytes = bytemuck::cast_slice(&recs);
                            let _ = writer.write_all(bytes);
                            count += recs.len() as u32;
                        }
                        Ok(BinaryMsg::Stop)
                        | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            let _ = writer.flush();
                            // Patch record_count at header offset 12.
                            if let Ok(mut raw) =
                                OpenOptions::new().write(true).open(&path_clone)
                            {
                                let _ = raw.seek(SeekFrom::Start(12));
                                let _ = raw.write_all(&count.to_le_bytes());
                            }
                            rc_clone.store(count, Ordering::Relaxed);
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let _ = writer.flush();
                        }
                    }
                }
            })
            .expect("failed to spawn binary drain thread");

        Ok((
            BinaryTraceSink {
                tx,
                record_count,
                name,
            },
            handle,
        ))
    }

    /// Push a slice of records to the drain queue.
    pub fn push_records(&self, records: &[T]) -> io::Result<()> {
        self.tx
            .send(BinaryMsg::Records(records.to_vec()))
            .map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "binary drain stopped")
            })
    }

    /// Return the number of records flushed so far (approximate; updated on Stop).
    pub fn record_count(&self) -> u32 {
        self.record_count.load(Ordering::Relaxed)
    }
}

impl<T: Copy + Send + Pod + 'static> Sink for BinaryTraceSink<T> {
    fn write(&self, data: &[u8]) -> io::Result<()> {
        let record_size = std::mem::size_of::<T>();
        if data.len() % record_size != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "data length not a multiple of record size",
            ));
        }
        let records: &[T] = bytemuck::cast_slice(data);
        self.push_records(records)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl<T: Copy + Send + Pod + 'static> Drop for BinaryTraceSink<T> {
    fn drop(&mut self) {
        let _ = self.tx.send(BinaryMsg::Stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::{Pod, Zeroable};
    use tempfile::NamedTempFile;

    /// 32-byte test record that matches BranchRecord layout.
    #[repr(C)]
    #[derive(Copy, Clone, Pod, Zeroable, Debug, PartialEq)]
    struct TestRecord {
        pc: u64,
        target: u64,
        insn_count: u64,
        flags: u8,
        _pad: [u8; 7],
    }

    #[test]
    fn binary_trace_sink_header_correct() {
        let tmp = NamedTempFile::new().unwrap();
        let (sink, handle) =
            BinaryTraceSink::<TestRecord>::open(tmp.path(), "TestRecord").unwrap();
        drop(sink);
        handle.join().unwrap();

        let data = std::fs::read(tmp.path()).unwrap();
        assert!(data.len() >= 80, "file too short for header");

        let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let rec_sz = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let rec_cnt = u32::from_le_bytes(data[12..16].try_into().unwrap());
        let type_name =
            std::str::from_utf8(&data[16..80]).unwrap().trim_end_matches('\0');

        assert_eq!(magic, 0x484D_4C54, "wrong magic");
        assert_eq!(version, 1, "wrong version");
        assert_eq!(rec_sz, 32, "wrong record size");
        assert_eq!(rec_cnt, 0, "no records written -- count should be 0");
        assert_eq!(type_name, "TestRecord");
    }

    #[test]
    fn binary_trace_sink_records_readable() {
        let tmp = NamedTempFile::new().unwrap();
        let (sink, handle) =
            BinaryTraceSink::<TestRecord>::open(tmp.path(), "TestRecord").unwrap();

        let records: Vec<TestRecord> = (0u64..10)
            .map(|i| TestRecord {
                pc: 0x4000 + i * 4,
                target: 0x5000,
                insn_count: i * 100,
                flags: if i % 2 == 0 { 1 } else { 0 },
                _pad: [0u8; 7],
            })
            .collect();

        sink.push_records(&records).unwrap();
        drop(sink);
        handle.join().unwrap();

        let data = std::fs::read(tmp.path()).unwrap();
        let hdr_size = 80usize;
        let rec_size = std::mem::size_of::<TestRecord>();

        let rec_count_in_hdr = u32::from_le_bytes(data[12..16].try_into().unwrap());
        assert_eq!(
            rec_count_in_hdr, 10,
            "header record_count should be 10 after close"
        );

        assert_eq!(data.len(), hdr_size + 10 * rec_size, "wrong file size");

        let records_bytes = &data[hdr_size..];
        let read_back: &[TestRecord] = bytemuck::cast_slice(records_bytes);
        assert_eq!(read_back.len(), 10);
        for (i, rec) in read_back.iter().enumerate() {
            assert_eq!(rec.pc, 0x4000 + i as u64 * 4);
            assert_eq!(rec.insn_count, i as u64 * 100);
        }
    }

    #[test]
    fn binary_trace_sink_header_size_is_80() {
        assert_eq!(std::mem::size_of::<TraceFileHeader>(), 80);
    }
}
