//! VirtIO split-ring virtqueue descriptor ring processor.
//!
//! Implements the split-ring virtqueue layout defined in VirtIO spec §2.7.
//! The queue consists of three guest-memory regions:
//!
//! - **Descriptor table** (`desc_addr`): array of `VirtqDesc`, each pointing
//!   to a buffer with flags and a chain-next link.
//! - **Available ring** (`driver_addr`): written by the driver; lists
//!   descriptor chain heads it has produced.
//! - **Used ring** (`device_addr`): written by the device; lists completed
//!   chain heads with byte counts.
//!
//! # Usage
//!
//! Call [`VirtQueue::pop_chain`] to obtain the next available descriptor
//! chain. Walk the chain with [`DescChain::next_desc`]. When done,
//! call [`VirtQueue::push_used`] to return the chain head to the used ring.
//!
//! # Simulator note
//!
//! Because the simulator has direct access to guest physical memory via
//! [`helm_core::ByteMem`], this implementation reads/writes descriptor and ring
//! structures directly without DMA abstraction. Reads are intentionally
//! mutable because the active runtime memory surface may attach side effects
//! even to read operations. This is correct for functional simulation (FE/SE).
//! Cycle-accurate (Accurate) timing would need to account for DMA latency
//! through the [`crate::transport`]'s `transact()` path.

use helm_core::ByteMem;
use helm_devices::BlockBackend;

// ── On-wire descriptor layout (VirtIO spec §2.7.5) ──────────────────────────

/// Raw virtqueue descriptor as it appears in guest memory.
///
/// Packed into 16 bytes: addr (8), len (4), flags (2), next (2).
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtqDesc {
    /// Guest physical address of the buffer.
    pub addr: u64,
    /// Length of the buffer in bytes.
    pub len: u32,
    /// Descriptor flags (see `VIRTQ_DESC_F_*` constants).
    pub flags: u16,
    /// Index of the next descriptor in the chain (if `VIRTQ_DESC_F_NEXT`).
    pub next: u16,
}

/// Descriptor flag: this descriptor continues via the `next` field.
pub const VIRTQ_DESC_F_NEXT: u16 = 1;
/// Descriptor flag: buffer is write-only (device writes, driver reads).
pub const VIRTQ_DESC_F_WRITE: u16 = 2;
/// Descriptor flag: the buffer contains a list of indirect descriptors.
pub const VIRTQ_DESC_F_INDIRECT: u16 = 4;

impl VirtqDesc {
    /// Return `true` if this descriptor has a next descriptor in the chain.
    #[inline]
    pub fn has_next(&self) -> bool {
        self.flags & VIRTQ_DESC_F_NEXT != 0
    }

    /// Return `true` if this is a write-only (device→driver) buffer.
    #[inline]
    pub fn is_write(&self) -> bool {
        self.flags & VIRTQ_DESC_F_WRITE != 0
    }

    /// Return `true` if this descriptor points to an indirect descriptor list.
    #[inline]
    pub fn is_indirect(&self) -> bool {
        self.flags & VIRTQ_DESC_F_INDIRECT != 0
    }
}

// ── Available ring (VirtIO spec §2.7.6) ─────────────────────────────────────

/// Interrupt suppression flag in the available ring `flags` field.
pub const VIRTQ_AVAIL_F_NO_INTERRUPT: u16 = 1;

// ── Used ring (VirtIO spec §2.7.8) ──────────────────────────────────────────

/// Notification suppression flag in the used ring `flags` field.
pub const VIRTQ_USED_F_NO_NOTIFY: u16 = 1;

// ── ByteMem helpers ─────────────────────────────────────────────────────────

fn read_u16_le(mem: &mut dyn ByteMem, gpa: u64) -> u16 {
    mem.read_le_u64(gpa, 2).unwrap() as u16
}

fn read_u32_le(mem: &mut dyn ByteMem, gpa: u64) -> u32 {
    mem.read_le_u64(gpa, 4).unwrap() as u32
}

fn read_u64_le(mem: &mut dyn ByteMem, gpa: u64) -> u64 {
    mem.read_le_u64(gpa, 8).unwrap()
}

fn write_u16_le(mem: &mut dyn ByteMem, gpa: u64, val: u16) {
    mem.write_le_u64(gpa, 2, u64::from(val)).unwrap();
}

fn write_u32_le(mem: &mut dyn ByteMem, gpa: u64, val: u32) {
    mem.write_le_u64(gpa, 4, u64::from(val)).unwrap();
}

fn write_u64_le(mem: &mut dyn ByteMem, gpa: u64, val: u64) {
    mem.write_le_u64(gpa, 8, val).unwrap();
}

// ── VirtQueue ────────────────────────────────────────────────────────────────

/// A single split-ring virtqueue.
///
/// Tracks the last-seen `avail_idx` (shadow index) so that repeated calls
/// to [`pop_chain`](Self::pop_chain) drain the available ring without
/// re-processing already-seen entries.
pub struct VirtQueue {
    /// Number of descriptors; must be a power of two.
    pub size: u16,
    /// Guest physical address of the descriptor table.
    pub desc_addr: u64,
    /// Guest physical address of the available ring.
    pub driver_addr: u64,
    /// Guest physical address of the used ring.
    pub device_addr: u64,
    /// Shadow index: last avail_idx the device has consumed.
    last_avail_idx: u16,
    /// Index into the used ring where the next used element is written.
    used_idx: u16,
}

impl VirtQueue {
    /// Create a new virtqueue with the given configuration.
    ///
    /// `size` must match what the guest driver wrote to `QueueNum`.
    pub fn new(size: u16, desc_addr: u64, driver_addr: u64, device_addr: u64) -> Self {
        Self {
            size,
            desc_addr,
            driver_addr,
            device_addr,
            last_avail_idx: 0,
            used_idx: 0,
        }
    }

    /// Create a queue with caller-supplied shadow progress counters.
    pub fn new_with_progress(
        size: u16,
        desc_addr: u64,
        driver_addr: u64,
        device_addr: u64,
        last_avail_idx: u16,
        used_idx: u16,
    ) -> Self {
        Self {
            size,
            desc_addr,
            driver_addr,
            device_addr,
            last_avail_idx,
            used_idx,
        }
    }

    /// Return `true` if the available ring has at least one new entry.
    pub fn has_available(&self, mem: &mut dyn ByteMem) -> bool {
        // avail ring layout: flags(2), idx(2), ring[size](2 each), ...
        let avail_idx = read_u16_le(mem, self.driver_addr + 2);
        avail_idx != self.last_avail_idx
    }

    /// Pop the next descriptor chain head from the available ring.
    ///
    /// Returns the head descriptor index, or `None` if the ring is empty.
    pub fn pop_chain(&mut self, mem: &mut dyn ByteMem) -> Option<u16> {
        let avail_idx = read_u16_le(mem, self.driver_addr + 2);
        if avail_idx == self.last_avail_idx {
            return None;
        }
        // ring entries start at offset 4 in the avail ring
        let slot = (self.last_avail_idx % self.size) as u64;
        let desc_head = read_u16_le(mem, self.driver_addr + 4 + slot * 2);
        self.last_avail_idx = self.last_avail_idx.wrapping_add(1);
        Some(desc_head)
    }

    /// Read a descriptor from the descriptor table.
    ///
    /// `idx` must be less than `self.size`.
    pub fn read_desc(&self, mem: &mut dyn ByteMem, idx: u16) -> VirtqDesc {
        debug_assert!(
            (idx as usize) < self.size as usize,
            "descriptor index out of range"
        );
        let base = self.desc_addr + (idx as u64) * 16;
        VirtqDesc {
            addr: read_u64_le(mem, base),
            len: read_u32_le(mem, base + 8),
            flags: read_u16_le(mem, base + 12),
            next: read_u16_le(mem, base + 14),
        }
    }

    /// Write a descriptor into the descriptor table.
    pub fn write_desc(&self, mem: &mut dyn ByteMem, idx: u16, desc: &VirtqDesc) {
        let base = self.desc_addr + (idx as u64) * 16;
        write_u64_le(mem, base, desc.addr);
        write_u32_le(mem, base + 8, desc.len);
        write_u16_le(mem, base + 12, desc.flags);
        write_u16_le(mem, base + 14, desc.next);
    }

    /// Push a completed chain to the used ring.
    ///
    /// `head` is the descriptor index returned by [`pop_chain`](Self::pop_chain).
    /// `bytes_written` is the number of bytes the device wrote into write-only
    /// buffers (0 for read-only operations like block writes).
    ///
    /// Returns `true` if the driver has not suppressed interrupts.
    pub fn push_used(&mut self, mem: &mut dyn ByteMem, head: u16, bytes_written: u32) -> bool {
        // used ring layout: flags(2), idx(2), ring[size](8 each: id(4)+len(4)), avail_event(2)
        let slot = (self.used_idx % self.size) as u64;
        let entry_addr = self.device_addr + 4 + slot * 8;
        write_u32_le(mem, entry_addr, head as u32);
        write_u32_le(mem, entry_addr + 4, bytes_written);

        // Advance used idx (visible to driver after the write above)
        self.used_idx = self.used_idx.wrapping_add(1);
        write_u16_le(mem, self.device_addr + 2, self.used_idx);

        // Check driver's VIRTQ_AVAIL_F_NO_INTERRUPT flag
        let avail_flags = read_u16_le(mem, self.driver_addr);
        (avail_flags & VIRTQ_AVAIL_F_NO_INTERRUPT) == 0
    }

    /// Walk a descriptor chain starting at `head`, collecting all segments.
    ///
    /// Returns a `Vec` of `(addr, len, is_write)` tuples, in chain order.
    /// Stops after following at most `self.size` descriptors to prevent loops.
    pub fn collect_chain(&self, mem: &mut dyn ByteMem, head: u16) -> Vec<(u64, u32, bool)> {
        let mut segments = Vec::new();
        let mut idx = head;
        let max = self.size as usize;
        for _ in 0..max {
            let desc = self.read_desc(mem, idx);
            let is_write = (desc.flags & VIRTQ_DESC_F_WRITE) != 0;
            segments.push((desc.addr, desc.len, is_write));
            if (desc.flags & VIRTQ_DESC_F_NEXT) == 0 {
                break;
            }
            idx = desc.next;
        }
        segments
    }
}

// ── FileBlockBackend ─────────────────────────────────────────────────────────

/// A [`BlockBackend`] backed by a `Vec<u8>` in host memory.
///
/// Suitable for small images in tests and early bring-up. For large disk
/// images, use a file-backed implementation instead.
pub struct RamBlockBackend {
    data: Vec<u8>,
}

impl RamBlockBackend {
    /// Create a new RAM-backed block device pre-filled with `data`.
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Create a zeroed block device of `capacity` bytes.
    pub fn zeroed(capacity: usize) -> Self {
        Self {
            data: vec![0u8; capacity],
        }
    }
}

impl BlockBackend for RamBlockBackend {
    fn read_block(&mut self, offset: u64, buf: &mut [u8]) {
        let start = offset as usize;
        let end = start + buf.len();
        if end <= self.data.len() {
            buf.copy_from_slice(&self.data[start..end]);
        } else {
            // Partial or out-of-range: zero-fill
            let valid = self.data.len().saturating_sub(start);
            buf[..valid].copy_from_slice(&self.data[start..start + valid]);
            buf[valid..].fill(0);
        }
    }

    fn write_block(&mut self, offset: u64, buf: &[u8]) {
        let start = offset as usize;
        let end = start + buf.len();
        if end <= self.data.len() {
            self.data[start..end].copy_from_slice(buf);
        }
    }

    fn capacity(&self) -> u64 {
        self.data.len() as u64
    }
}

impl RamBlockBackend {
    /// Read `count` 512-byte sectors starting at `sector` into a `Vec<u8>`.
    ///
    /// Returns `None` if the range is out of bounds.
    pub fn read_sectors(&mut self, sector: u64, count: usize) -> Option<Vec<u8>> {
        let byte_off = sector * 512;
        let byte_len = count * 512;
        if byte_off as usize + byte_len > self.data.len() {
            return None;
        }
        let mut buf = vec![0u8; byte_len];
        self.read_block(byte_off, &mut buf);
        Some(buf)
    }

    /// Write a byte slice to sector `sector`.
    ///
    /// `data` must be a multiple of 512 bytes. Returns `false` if out of bounds.
    pub fn write_sectors(&mut self, sector: u64, data: &[u8]) -> bool {
        let byte_off = sector * 512;
        if byte_off as usize + data.len() > self.data.len() {
            return false;
        }
        self.write_block(byte_off, data);
        true
    }
}

impl VirtQueue {
    /// Reset queue state: clear shadow indices and ready flag tracking.
    ///
    /// Called when the transport receives a device reset (Status written to 0).
    /// Addresses are cleared by the transport; the queue itself resets counters.
    pub fn reset(&mut self) {
        self.last_avail_idx = 0;
        self.used_idx = 0;
    }

    /// Return the queue shadow progress counters.
    pub fn progress(&self) -> (u16, u16) {
        (self.last_avail_idx, self.used_idx)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal in-process guest memory for testing.
    struct FlatMem(Vec<u8>);

    impl FlatMem {
        fn new(size: usize) -> Self {
            Self(vec![0u8; size])
        }
    }

    impl ByteMem for FlatMem {
        fn read_bytes(&mut self, gpa: u64, buf: &mut [u8]) -> Result<(), helm_core::MemFault> {
            let start = gpa as usize;
            buf.copy_from_slice(&self.0[start..start + buf.len()]);
            Ok(())
        }
        fn write_bytes(&mut self, gpa: u64, buf: &[u8]) -> Result<(), helm_core::MemFault> {
            let start = gpa as usize;
            self.0[start..start + buf.len()].copy_from_slice(buf);
            Ok(())
        }
    }

    fn make_queue(
        _mem: &mut FlatMem,
        desc_base: u64,
        avail_base: u64,
        used_base: u64,
        size: u16,
    ) -> VirtQueue {
        VirtQueue::new(size, desc_base, avail_base, used_base)
    }

    fn avail_push(mem: &mut FlatMem, avail_base: u64, size: u16, idx: u16, desc_head: u16) {
        // Read current avail idx
        let cur = u16::from_le_bytes(
            mem.0[avail_base as usize + 2..avail_base as usize + 4]
                .try_into()
                .unwrap(),
        );
        let slot = (cur % size) as usize;
        // Write desc head into ring slot
        let ring_off = avail_base as usize + 4 + slot * 2;
        mem.0[ring_off..ring_off + 2].copy_from_slice(&desc_head.to_le_bytes());
        // Advance avail idx
        let new_idx = cur.wrapping_add(1);
        mem.0[avail_base as usize + 2..avail_base as usize + 4]
            .copy_from_slice(&new_idx.to_le_bytes());
        let _ = idx; // suppress unused warning when called externally
    }

    fn write_desc(mem: &mut FlatMem, desc_base: u64, idx: u16, desc: VirtqDesc) {
        let base = desc_base as usize + idx as usize * 16;
        mem.0[base..base + 8].copy_from_slice(&desc.addr.to_le_bytes());
        mem.0[base + 8..base + 12].copy_from_slice(&desc.len.to_le_bytes());
        mem.0[base + 12..base + 14].copy_from_slice(&desc.flags.to_le_bytes());
        mem.0[base + 14..base + 16].copy_from_slice(&desc.next.to_le_bytes());
    }

    #[test]
    fn empty_queue_returns_none() {
        let mut mem = FlatMem::new(4096);
        let mut q = make_queue(&mut mem, 0, 512, 1024, 16);
        assert!(!q.has_available(&mut mem));
        assert!(q.pop_chain(&mut mem).is_none());
    }

    #[test]
    fn single_descriptor_chain() {
        let mut mem = FlatMem::new(8192);
        let desc_base: u64 = 0;
        let avail_base: u64 = 512;
        let used_base: u64 = 1024;
        let size: u16 = 16;

        // Write a read-only descriptor at index 0
        write_desc(
            &mut mem,
            desc_base,
            0,
            VirtqDesc {
                addr: 4096,
                len: 64,
                flags: 0, // read-only, no chaining
                next: 0,
            },
        );

        // Push descriptor 0 into available ring
        avail_push(&mut mem, avail_base, size, 0, 0);

        let mut q = make_queue(&mut mem, desc_base, avail_base, used_base, size);

        assert!(q.has_available(&mut mem));
        let head = q.pop_chain(&mut mem).unwrap();
        assert_eq!(head, 0);
        assert!(!q.has_available(&mut mem));

        let chain = q.collect_chain(&mut mem, head);
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0], (4096, 64, false));
    }

    #[test]
    fn push_used_advances_idx() {
        let mut mem = FlatMem::new(8192);
        let desc_base: u64 = 0;
        let avail_base: u64 = 512;
        let used_base: u64 = 1024;
        let size: u16 = 16;

        avail_push(&mut mem, avail_base, size, 0, 3);

        let mut q = make_queue(&mut mem, desc_base, avail_base, used_base, size);
        let head = q.pop_chain(&mut mem).unwrap();
        q.push_used(&mut mem, head, 0);

        // Used idx should now be 1
        let used_idx = u16::from_le_bytes(
            mem.0[used_base as usize + 2..used_base as usize + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(used_idx, 1);
    }

    #[test]
    fn ram_block_backend_read_write() {
        let mut b = RamBlockBackend::zeroed(512);
        let data = [0xABu8; 64];
        b.write_block(128, &data);
        let mut out = [0u8; 64];
        b.read_block(128, &mut out);
        assert_eq!(out, data);
        assert_eq!(b.capacity(), 512);
    }

    #[test]
    fn desc_flags_has_next() {
        let d = VirtqDesc {
            addr: 0x1000,
            len: 512,
            flags: VIRTQ_DESC_F_NEXT,
            next: 1,
        };
        assert!(d.has_next());
        assert!(!d.is_write());
        assert!(!d.is_indirect());
    }

    #[test]
    fn desc_flags_write() {
        let d = VirtqDesc {
            addr: 0,
            len: 0,
            flags: VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE,
            next: 2,
        };
        assert!(d.has_next());
        assert!(d.is_write());
    }

    #[test]
    fn desc_flags_indirect() {
        let d = VirtqDesc {
            addr: 0,
            len: 0,
            flags: VIRTQ_DESC_F_INDIRECT,
            next: 0,
        };
        assert!(d.is_indirect());
        assert!(!d.has_next());
    }

    #[test]
    fn queue_wrapping() {
        let mut mem = FlatMem::new(8192);
        let desc_base: u64 = 0;
        let avail_base: u64 = 512;
        let used_base: u64 = 1024;
        let size: u16 = 4;

        let mut q = make_queue(&mut mem, desc_base, avail_base, used_base, size);

        // Push 4, pop 4, push 2 more — tests index wrapping
        for i in 0..4u16 {
            avail_push(&mut mem, avail_base, size, i, i);
        }
        for i in 0..4u16 {
            assert_eq!(q.pop_chain(&mut mem), Some(i));
        }
        avail_push(&mut mem, avail_base, size, 4, 10);
        avail_push(&mut mem, avail_base, size, 5, 11);
        assert_eq!(q.pop_chain(&mut mem), Some(10));
        assert_eq!(q.pop_chain(&mut mem), Some(11));
        assert!(q.pop_chain(&mut mem).is_none());
    }

    #[test]
    fn queue_reset_clears_indices() {
        let mut mem = FlatMem::new(8192);
        let mut q = make_queue(&mut mem, 0, 512, 1024, 16);

        avail_push(&mut mem, 512, 16, 0, 0);
        let head = q.pop_chain(&mut mem).unwrap();
        q.push_used(&mut mem, head, 64);

        q.reset();
        assert_eq!(q.last_avail_idx, 0);
        assert_eq!(q.used_idx, 0);
    }

    #[test]
    fn three_segment_chain() {
        let mut mem = FlatMem::new(8192);
        let desc_base: u64 = 0;
        let avail_base: u64 = 512;
        let used_base: u64 = 1024;
        let size: u16 = 16;

        // desc[0] -> desc[1] -> desc[2]
        write_desc(
            &mut mem,
            desc_base,
            0,
            VirtqDesc {
                addr: 0x1000,
                len: 512,
                flags: VIRTQ_DESC_F_NEXT,
                next: 1,
            },
        );
        write_desc(
            &mut mem,
            desc_base,
            1,
            VirtqDesc {
                addr: 0x2000,
                len: 256,
                flags: VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE,
                next: 2,
            },
        );
        write_desc(
            &mut mem,
            desc_base,
            2,
            VirtqDesc {
                addr: 0x3000,
                len: 128,
                flags: VIRTQ_DESC_F_WRITE,
                next: 0,
            },
        );
        avail_push(&mut mem, avail_base, size, 0, 0);

        let mut q = make_queue(&mut mem, desc_base, avail_base, used_base, size);
        let head = q.pop_chain(&mut mem).unwrap();
        let chain = q.collect_chain(&mut mem, head);

        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0], (0x1000, 512, false)); // read-only
        assert_eq!(chain[1], (0x2000, 256, true)); // write-only
        assert_eq!(chain[2], (0x3000, 128, true)); // write-only

        // Check desc methods
        let d0 = q.read_desc(&mut mem, 0);
        assert!(d0.has_next());
        assert!(!d0.is_write());
        let d2 = q.read_desc(&mut mem, 2);
        assert!(!d2.has_next());
        assert!(d2.is_write());
    }

    #[test]
    fn ram_block_read_write_sectors() {
        let mut b = RamBlockBackend::zeroed(1024);
        assert!(b.write_sectors(0, &[0xAA; 512]));
        let data = b.read_sectors(0, 1).unwrap();
        assert_eq!(data.len(), 512);
        assert_eq!(data[0], 0xAA);

        // Out of bounds
        assert!(b.read_sectors(2, 1).is_none());
        assert!(!b.write_sectors(2, &[0u8; 512]));
    }
}
