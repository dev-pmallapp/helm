//! Virtqueue — the core data structure for VirtIO device communication.
//!
//! Implements both **split** (spec 2.7) and **packed** (spec 2.8) layouts.
//! The guest sets up descriptor tables in guest memory; the device reads
//! descriptors via the address space. For simulation purposes, we maintain
//! the queue state in host memory and synchronize through the transport.

use serde::{Deserialize, Serialize};

use crate::virtio::features::{VRING_DESC_F_INDIRECT, VRING_DESC_F_NEXT, VRING_DESC_F_WRITE};

// ── Split virtqueue descriptor ──────────────────────────────────────────────

/// A single descriptor in the split virtqueue descriptor table (spec 2.7.5).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[repr(C)]
pub struct VringDesc {
    /// Physical (guest) address of the buffer.
    pub addr: u64,
    /// Length of the buffer in bytes.
    pub len: u32,
    /// Descriptor flags (NEXT, WRITE, INDIRECT).
    pub flags: u16,
    /// Index of the next descriptor if NEXT flag is set.
    pub next: u16,
}

impl VringDesc {
    pub fn has_next(&self) -> bool {
        self.flags & VRING_DESC_F_NEXT != 0
    }

    pub fn is_write(&self) -> bool {
        self.flags & VRING_DESC_F_WRITE != 0
    }

    pub fn is_indirect(&self) -> bool {
        self.flags & VRING_DESC_F_INDIRECT != 0
    }
}

/// An element in the used ring (spec 2.7.8).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[repr(C)]
pub struct VringUsedElem {
    /// Index of the head descriptor in the chain.
    pub id: u32,
    /// Total bytes written by the device.
    pub len: u32,
}

// ── Split virtqueue ─────────────────────────────────────────────────────────

/// A split virtqueue (spec 2.7).
///
/// For simulation, descriptor tables and rings are stored in host memory.
/// A real implementation would read/write through the guest address space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitVirtqueue {
    /// Queue size (number of descriptors, must be power of 2).
    pub size: u16,
    /// Whether the queue has been set up and is ready.
    pub ready: bool,

    // Descriptor table
    pub desc_table: Vec<VringDesc>,

    // Available ring
    pub avail_flags: u16,
    pub avail_idx: u16,
    pub avail_ring: Vec<u16>,

    // Used ring
    pub used_flags: u16,
    pub used_idx: u16,
    pub used_ring: Vec<VringUsedElem>,

    /// Last available index seen by the device.
    pub last_avail_idx: u16,

    /// Guest physical addresses (set by driver via MMIO).
    pub desc_addr: u64,
    pub avail_addr: u64,
    pub used_addr: u64,

    /// Notification suppression (EVENT_IDX).
    pub avail_event: u16,
    pub used_event: u16,
}

impl SplitVirtqueue {
    pub fn new(size: u16) -> Self {
        let sz = size as usize;
        Self {
            size,
            ready: false,
            desc_table: vec![VringDesc::default(); sz],
            avail_flags: 0,
            avail_idx: 0,
            avail_ring: vec![0; sz],
            used_flags: 0,
            used_idx: 0,
            used_ring: vec![VringUsedElem::default(); sz],
            last_avail_idx: 0,
            desc_addr: 0,
            avail_addr: 0,
            used_addr: 0,
            avail_event: 0,
            used_event: 0,
        }
    }

    /// Check if the queue has new available buffers.
    pub fn has_available(&self) -> bool {
        self.avail_idx != self.last_avail_idx
    }

    /// Number of available buffers pending.
    pub fn num_available(&self) -> u16 {
        self.avail_idx.wrapping_sub(self.last_avail_idx)
    }

    /// Pop the next available descriptor chain head index.
    pub fn pop_avail(&mut self) -> Option<u16> {
        if !self.has_available() {
            return None;
        }
        let idx = self.last_avail_idx % self.size;
        let desc_idx = self.avail_ring[idx as usize];
        self.last_avail_idx = self.last_avail_idx.wrapping_add(1);
        Some(desc_idx)
    }

    /// Push a used buffer notification.
    pub fn push_used(&mut self, id: u16, len: u32) {
        let idx = self.used_idx % self.size;
        self.used_ring[idx as usize] = VringUsedElem { id: id as u32, len };
        self.used_idx = self.used_idx.wrapping_add(1);
    }

    /// Make a descriptor available (for test/simulation use).
    pub fn push_avail(&mut self, desc_idx: u16) {
        let idx = self.avail_idx % self.size;
        self.avail_ring[idx as usize] = desc_idx;
        self.avail_idx = self.avail_idx.wrapping_add(1);
    }

    /// Set up a simple descriptor (for test/simulation).
    pub fn set_desc(&mut self, idx: u16, addr: u64, len: u32, flags: u16, next: u16) {
        if (idx as usize) < self.desc_table.len() {
            self.desc_table[idx as usize] = VringDesc {
                addr,
                len,
                flags,
                next,
            };
        }
    }

    /// Walk a descriptor chain starting at `head`, collecting all descriptors.
    pub fn walk_chain(&self, head: u16) -> Vec<VringDesc> {
        let mut chain = Vec::new();
        let mut idx = head;
        loop {
            let desc = self.desc_table[idx as usize % self.desc_table.len()];
            chain.push(desc);
            if desc.has_next() {
                idx = desc.next;
            } else {
                break;
            }
        }
        chain
    }

    /// Reset the queue to initial state.
    pub fn reset(&mut self) {
        self.ready = false;
        self.avail_idx = 0;
        self.avail_flags = 0;
        self.used_idx = 0;
        self.used_flags = 0;
        self.last_avail_idx = 0;
        self.desc_addr = 0;
        self.avail_addr = 0;
        self.used_addr = 0;
        for d in &mut self.desc_table {
            *d = VringDesc::default();
        }
        for r in &mut self.avail_ring {
            *r = 0;
        }
        for r in &mut self.used_ring {
            *r = VringUsedElem::default();
        }
    }
}

// ── Packed virtqueue ────────────────────────────────────────────────────────

/// A single descriptor in the packed virtqueue (spec 2.8).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[repr(C)]
pub struct VringPackedDesc {
    /// Buffer address.
    pub addr: u64,
    /// Buffer length.
    pub len: u32,
    /// Buffer ID.
    pub id: u16,
    /// Flags (includes avail/used wrap counters).
    pub flags: u16,
}

/// A packed virtqueue (spec 2.8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackedVirtqueue {
    pub size: u16,
    pub ready: bool,
    pub desc_table: Vec<VringPackedDesc>,

    /// Device-side wrap counter.
    pub device_wrap_counter: bool,
    /// Device-side next index to process.
    pub device_next_off: u16,

    /// Driver-side wrap counter.
    pub driver_wrap_counter: bool,
    /// Driver-side next index.
    pub driver_next_off: u16,

    /// Guest physical address of the descriptor ring.
    pub desc_addr: u64,

    /// Event suppression structures.
    pub driver_event_addr: u64,
    pub device_event_addr: u64,
}

impl PackedVirtqueue {
    pub fn new(size: u16) -> Self {
        Self {
            size,
            ready: false,
            desc_table: vec![VringPackedDesc::default(); size as usize],
            device_wrap_counter: true,
            device_next_off: 0,
            driver_wrap_counter: true,
            driver_next_off: 0,
            desc_addr: 0,
            driver_event_addr: 0,
            device_event_addr: 0,
        }
    }

    /// Check if a descriptor is available (avail flag matches device wrap counter).
    pub fn desc_is_avail(&self, idx: u16) -> bool {
        let desc = &self.desc_table[idx as usize % self.desc_table.len()];
        let avail = desc.flags & crate::virtio::features::VRING_PACKED_DESC_F_AVAIL != 0;
        let used = desc.flags & crate::virtio::features::VRING_PACKED_DESC_F_USED != 0;
        avail != used && avail == self.device_wrap_counter
    }

    /// Pop the next available descriptor. Returns its index.
    pub fn pop_avail(&mut self) -> Option<u16> {
        if !self.desc_is_avail(self.device_next_off) {
            return None;
        }
        let idx = self.device_next_off;
        self.device_next_off += 1;
        if self.device_next_off >= self.size {
            self.device_next_off = 0;
            self.device_wrap_counter = !self.device_wrap_counter;
        }
        Some(idx)
    }

    /// Mark a descriptor as used.
    pub fn mark_used(&mut self, idx: u16, len: u32) {
        let table_len = self.desc_table.len();
        let desc = &mut self.desc_table[idx as usize % table_len];
        desc.len = len;
        // Set both avail and used flags to match device_wrap_counter
        let flags = desc.flags
            & !(crate::virtio::features::VRING_PACKED_DESC_F_AVAIL
                | crate::virtio::features::VRING_PACKED_DESC_F_USED);
        desc.flags = if self.device_wrap_counter {
            flags
                | crate::virtio::features::VRING_PACKED_DESC_F_AVAIL
                | crate::virtio::features::VRING_PACKED_DESC_F_USED
        } else {
            flags
        };
    }

    /// Reset to initial state.
    pub fn reset(&mut self) {
        self.ready = false;
        self.device_wrap_counter = true;
        self.device_next_off = 0;
        self.driver_wrap_counter = true;
        self.driver_next_off = 0;
        for d in &mut self.desc_table {
            *d = VringPackedDesc::default();
        }
    }
}

// ── Virtqueue enum (split or packed) ────────────────────────────────────────

/// A virtqueue — either split or packed layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Virtqueue {
    Split(SplitVirtqueue),
    Packed(PackedVirtqueue),
}

impl Virtqueue {
    pub fn new_split(size: u16) -> Self {
        Self::Split(SplitVirtqueue::new(size))
    }

    pub fn new_packed(size: u16) -> Self {
        Self::Packed(PackedVirtqueue::new(size))
    }

    pub fn size(&self) -> u16 {
        match self {
            Self::Split(q) => q.size,
            Self::Packed(q) => q.size,
        }
    }

    pub fn ready(&self) -> bool {
        match self {
            Self::Split(q) => q.ready,
            Self::Packed(q) => q.ready,
        }
    }

    pub fn set_ready(&mut self, ready: bool) {
        match self {
            Self::Split(q) => q.ready = ready,
            Self::Packed(q) => q.ready = ready,
        }
    }

    pub fn reset(&mut self) {
        match self {
            Self::Split(q) => q.reset(),
            Self::Packed(q) => q.reset(),
        }
    }

    pub fn as_split(&self) -> Option<&SplitVirtqueue> {
        match self {
            Self::Split(q) => Some(q),
            _ => None,
        }
    }

    pub fn as_split_mut(&mut self) -> Option<&mut SplitVirtqueue> {
        match self {
            Self::Split(q) => Some(q),
            _ => None,
        }
    }

    pub fn as_packed(&self) -> Option<&PackedVirtqueue> {
        match self {
            Self::Packed(q) => Some(q),
            _ => None,
        }
    }

    pub fn as_packed_mut(&mut self) -> Option<&mut PackedVirtqueue> {
        match self {
            Self::Packed(q) => Some(q),
            _ => None,
        }
    }
}
