//! VirtIO block device backend (device type 2, VirtIO spec §5.2).
//!
//! Implements a storage device backed by a [`BlockBackend`]. The driver
//! submits I/O requests via a single virtqueue (queue 0). Each request is
//! a three-segment descriptor chain:
//!
//! ```text
//!   [0] header (read-only, 16 bytes): type, reserved, sector
//!   [1] data   (type-dependent)      : buffer to read into or write from
//!   [2] status (write-only, 1 byte)  : device writes VIRTIO_BLK_S_OK / _IOERR
//! ```
//!
//! # Limitations
//!
//! - Single queue only (multi-queue behind `VIRTIO_BLK_F_MQ` is not implemented).
//! - No discard / write-zeroes / secure-erase commands.
//! - Synchronous I/O only; latency is not modelled.

use helm_devices::BlockBackend;

use crate::proto::features::{
    VIRTIO_BLK_F_BLK_SIZE, VIRTIO_BLK_F_RO, VIRTIO_BLK_F_SIZE_MAX, VIRTIO_DEVICE_BLK,
    VIRTIO_F_VERSION_1,
};
use crate::proto::virtqueue::VirtQueue;
use crate::{VirtioBackend, VirtioPendingEvents};

// ── Block request header (VirtIO spec §5.2.6) ────────────────────────────────

/// Request type: read (device→driver).
const VIRTIO_BLK_T_IN: u32 = 0;
/// Request type: write (driver→device).
const VIRTIO_BLK_T_OUT: u32 = 1;
/// Request type: flush (sync storage).
const VIRTIO_BLK_T_FLUSH: u32 = 4;
/// Request type: get device ID (string).
const VIRTIO_BLK_T_GET_ID: u32 = 8;

/// Status: request completed successfully.
pub const VIRTIO_BLK_S_OK: u8 = 0;
/// Status: I/O error.
pub const VIRTIO_BLK_S_IOERR: u8 = 1;
/// Status: unsupported request type.
pub const VIRTIO_BLK_S_UNSUPP: u8 = 2;

/// Sector size (always 512 bytes per spec §5.2.4).
const SECTOR_SIZE: u64 = 512;
/// Device ID string returned by GET_ID (20 bytes including NUL padding).
const DEVICE_ID_BYTES: &[u8; 20] = b"helm-virtio-blk\0\0\0\0\0";

// ── Config space layout (VirtIO spec §5.2.4) ─────────────────────────────────

/// VirtIO block device config space.
///
/// All fields are LE. The driver reads these via the transport's config
/// registers (offset 0x100+ in MMIO space).
#[derive(Debug, Default, Clone)]
struct BlkConfig {
    /// Total number of 512-byte sectors.
    capacity: u64,
    /// Suggested maximum transfer size in bytes (if `VIRTIO_BLK_F_SIZE_MAX`).
    size_max: u32,
    /// Unused alignment padding.
    _pad: u32,
}

// ── VirtioBlk ────────────────────────────────────────────────────────────────

/// VirtIO block device backend.
///
/// Wraps a [`BlockBackend`] and implements the VirtIO block protocol.
/// Pass a boxed instance to [`VirtioMmioTransport::new`](crate::proto::transport::VirtioMmioTransport::new).
///
/// # Example
///
/// ```rust,ignore
/// use helm_hw_virtio::devices::VirtioBlk;
/// use helm_hw_virtio::virtqueue::RamBlockBackend;
/// use helm_hw_virtio::transport::VirtioMmioTransport;
///
/// let disk = RamBlockBackend::zeroed(1024 * 1024); // 1 MiB
/// let blk  = VirtioBlk::new(Box::new(disk), false);
/// let transport = VirtioMmioTransport::new(Box::new(blk));
/// ```
pub struct VirtioBlk {
    backend: Box<dyn BlockBackend>,
    read_only: bool,
    config: BlkConfig,
    /// Pending I/O from the last `queue_notify`. In this simple model,
    /// we defer processing to [`Self::drain_pending`]; the transport
    /// calls `queue_notify` but cannot provide guest memory at that moment.
    /// Callers that have access to guest memory should call
    /// [`VirtioBlk::process_queue`] directly.
    _notify_pending: bool,
}

impl VirtioBlk {
    /// Create a new block device backed by `backend`.
    ///
    /// If `read_only` is `true`, write requests return `VIRTIO_BLK_S_IOERR`.
    pub fn new(backend: Box<dyn BlockBackend>, read_only: bool) -> Self {
        let capacity_bytes = backend.capacity();
        let sectors = capacity_bytes / SECTOR_SIZE;
        let config = BlkConfig {
            capacity: sectors,
            size_max: 65536,
            _pad: 0,
        };
        Self {
            backend,
            read_only,
            config,
            _notify_pending: false,
        }
    }

    fn request_in_bounds(&self, offset: u64, len: u64) -> bool {
        offset
            .checked_add(len)
            .is_some_and(|end| end <= self.backend.capacity())
    }

    /// Read a byte range directly from the backing store.
    pub fn read_bytes(&mut self, offset: u64, len: usize) -> Vec<u8> {
        let mut buf = vec![0u8; len];
        self.backend.read_block(offset, &mut buf);
        buf
    }

    /// Process one descriptor chain head from the driver.
    ///
    /// `chain` is the collected segment list from
    /// [`VirtQueue::collect_chain`](crate::proto::virtqueue::VirtQueue::collect_chain).
    ///
    /// Returns the number of bytes written into write-only (device→driver)
    /// buffers, and the status byte to store in the status segment.
    pub fn handle_request(
        &mut self,
        chain: &[(u64, u32, bool)],
        mem: &mut impl FnMut(u64, u32, bool, &mut [u8]),
    ) -> (u32, u8) {
        // Chain must have at least: header + status
        if chain.len() < 2 {
            return (0, VIRTIO_BLK_S_IOERR);
        }

        // Read request header from the first (read-only) segment
        let (hdr_addr, hdr_len, hdr_write) = chain[0];
        if hdr_write || hdr_len < 16 {
            return (0, VIRTIO_BLK_S_IOERR);
        }

        let mut hdr = [0u8; 16];
        mem(hdr_addr, 16, false, &mut hdr);
        let req_type = u32::from_le_bytes(hdr[0..4].try_into().unwrap());
        let sector = u64::from_le_bytes(hdr[8..16].try_into().unwrap());

        // Status segment is the last descriptor (write-only)
        let status_seg = chain.last().unwrap();
        if !status_seg.2 {
            return (0, VIRTIO_BLK_S_IOERR);
        }
        let status_addr = status_seg.0;

        // Data segments: everything between header and status
        let data_segs = &chain[1..chain.len() - 1];

        let (status, bytes_written) = match req_type {
            VIRTIO_BLK_T_IN => {
                let mut bytes_written = 0u32;
                let mut byte_offset = sector * SECTOR_SIZE;
                let total_len = data_segs.iter().map(|seg| u64::from(seg.1)).sum::<u64>();
                if !self.request_in_bounds(byte_offset, total_len) {
                    (VIRTIO_BLK_S_IOERR, 1)
                } else {
                    let mut status = VIRTIO_BLK_S_OK;
                    for &(addr, len, is_write) in data_segs {
                        if !is_write {
                            log::warn!("virtio-blk: IN request has read-only data segment");
                            status = VIRTIO_BLK_S_IOERR;
                            bytes_written = 0;
                            break;
                        }
                        let mut buf = vec![0u8; len as usize];
                        self.backend.read_block(byte_offset, &mut buf);
                        mem(addr, len, true, &mut buf);
                        byte_offset += len as u64;
                        bytes_written += len;
                    }
                    (status, bytes_written + 1)
                }
            }
            VIRTIO_BLK_T_OUT => {
                if self.read_only {
                    log::warn!("virtio-blk: write to read-only device");
                    (VIRTIO_BLK_S_IOERR, 1)
                } else {
                    let mut byte_offset = sector * SECTOR_SIZE;
                    let total_len = data_segs.iter().map(|seg| u64::from(seg.1)).sum::<u64>();
                    if !self.request_in_bounds(byte_offset, total_len) {
                        (VIRTIO_BLK_S_IOERR, 1)
                    } else {
                        let mut status = VIRTIO_BLK_S_OK;
                        for &(addr, len, is_write) in data_segs {
                            if is_write {
                                log::warn!("virtio-blk: OUT request has write-only data segment");
                                status = VIRTIO_BLK_S_IOERR;
                                break;
                            }
                            let mut buf = vec![0u8; len as usize];
                            mem(addr, len, false, &mut buf);
                            self.backend.write_block(byte_offset, &buf);
                            byte_offset += len as u64;
                        }
                        (status, 1)
                    }
                }
            }
            VIRTIO_BLK_T_FLUSH => {
                // No-op in functional mode (no write cache)
                (VIRTIO_BLK_S_OK, 1)
            }
            VIRTIO_BLK_T_GET_ID => {
                let mut cursor = 0usize;
                for &(addr, len, is_write) in data_segs {
                    if !is_write || cursor >= DEVICE_ID_BYTES.len() {
                        continue;
                    }
                    let n = (len as usize).min(DEVICE_ID_BYTES.len() - cursor);
                    let mut buf = vec![0u8; n];
                    buf.copy_from_slice(&DEVICE_ID_BYTES[cursor..cursor + n]);
                    mem(addr, n as u32, true, &mut buf);
                    cursor += n;
                }
                (VIRTIO_BLK_S_OK, cursor as u32 + 1)
            }
            _ => (VIRTIO_BLK_S_UNSUPP, 1),
        };

        // Write status byte
        let mut status_buf = [status];
        mem(status_addr, 1, true, &mut status_buf);

        (bytes_written, status)
    }
}

impl VirtioBackend for VirtioBlk {
    fn device_type(&self) -> u32 {
        VIRTIO_DEVICE_BLK
    }

    fn vendor_id(&self) -> u32 {
        crate::proto::features::VIRTIO_VENDOR_ID as u32
    }

    fn device_features(&self) -> u64 {
        let mut f = VIRTIO_F_VERSION_1 | VIRTIO_BLK_F_SIZE_MAX | VIRTIO_BLK_F_BLK_SIZE;
        if self.read_only {
            f |= VIRTIO_BLK_F_RO;
        }
        f
    }

    fn queue_max_size(&self, queue: usize) -> u32 {
        if queue == 0 {
            128
        } else {
            0
        }
    }

    fn queue_notify(&mut self, _queue: usize, _mem: Option<&mut dyn helm_core::ByteMem>) {
        self._notify_pending = true;
    }

    fn read_config(&self, offset: u32) -> u32 {
        match offset {
            0 => self.config.capacity as u32,
            4 => (self.config.capacity >> 32) as u32,
            8 => self.config.size_max,
            _ => 0,
        }
    }

    fn write_config(&mut self, _offset: u32, _val: u32) {
        // Config space is read-only for block devices
    }

    fn reset(&mut self) {
        self._notify_pending = false;
    }

    fn process_pending(
        &mut self,
        mem: &mut dyn helm_core::ByteMem,
        queues: &mut [VirtQueue],
    ) -> VirtioPendingEvents {
        if !self._notify_pending {
            return VirtioPendingEvents::default();
        }
        self._notify_pending = false;

        let Some(queue) = queues.get_mut(0) else {
            return VirtioPendingEvents::default();
        };

        let mut queue_irq = false;
        while let Some(head) = queue.pop_chain(mem) {
            let chain = queue.collect_chain(mem, head);
            let (bytes_written, _status) =
                self.handle_request(&chain, &mut |addr, len, is_write, buf| {
                    if is_write {
                        let _ = mem.write_bytes(addr, buf);
                    } else {
                        let _ = mem.read_bytes(addr, buf);
                    }
                    let _ = len;
                });
            queue_irq |= queue.push_used(mem, head, bytes_written);
        }

        VirtioPendingEvents {
            queue_irq,
            config_irq: false,
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::virtqueue::RamBlockBackend;

    #[test]
    fn blk_device_type() {
        let b = VirtioBlk::new(Box::new(RamBlockBackend::zeroed(512)), false);
        assert_eq!(b.device_type(), VIRTIO_DEVICE_BLK);
    }

    #[test]
    fn blk_config_capacity() {
        let b = VirtioBlk::new(Box::new(RamBlockBackend::zeroed(1024)), false);
        // 1024 bytes = 2 sectors
        assert_eq!(b.read_config(0), 2); // capacity low word
        assert_eq!(b.read_config(4), 0); // capacity high word
    }

    #[test]
    fn blk_read_only_feature() {
        let b = VirtioBlk::new(Box::new(RamBlockBackend::zeroed(512)), true);
        assert!(b.device_features() & VIRTIO_BLK_F_RO != 0);
    }

    #[test]
    fn blk_queue_max_size() {
        let b = VirtioBlk::new(Box::new(RamBlockBackend::zeroed(512)), false);
        assert_eq!(b.queue_max_size(0), 128);
        assert_eq!(b.queue_max_size(1), 0);
    }

    #[test]
    fn handle_read_request() {
        // Disk with known data
        let mut disk_data = vec![0u8; 512];
        disk_data[0] = 0xDE;
        disk_data[1] = 0xAD;

        let mut blk = VirtioBlk::new(Box::new(RamBlockBackend::new(disk_data)), false);

        // Host memory: header@0, data@64, status@128
        let mut host_mem = vec![0u8; 256];

        // Write request header: type=IN(0), reserved=0, sector=0
        host_mem[0..4].copy_from_slice(&0u32.to_le_bytes()); // type
        host_mem[4..8].copy_from_slice(&0u32.to_le_bytes()); // reserved
        host_mem[8..16].copy_from_slice(&0u64.to_le_bytes()); // sector 0

        let chain = vec![
            (0u64, 16u32, false), // header: read-only
            (64u64, 64u32, true), // data:   write-only (device writes)
            (128u64, 1u32, true), // status: write-only
        ];

        // Use a RefCell so the closure can borrow host_mem mutably through a shared ref.
        use std::cell::RefCell;
        let host_mem = RefCell::new(host_mem);
        let (written, status) = blk.handle_request(&chain, &mut |addr, len, is_write, buf| {
            let mut mem = host_mem.borrow_mut();
            let off = addr as usize;
            let n = len as usize;
            if is_write {
                mem[off..off + n].copy_from_slice(&buf[..n]);
            } else {
                buf[..n].copy_from_slice(&mem[off..off + n]);
            }
        });
        let host_mem = host_mem.into_inner();

        assert_eq!(status, VIRTIO_BLK_S_OK);
        assert!(written > 0);
        // Data segment should have first two bytes of disk sector 0
        assert_eq!(host_mem[64], 0xDE);
        assert_eq!(host_mem[65], 0xAD);
    }
}
