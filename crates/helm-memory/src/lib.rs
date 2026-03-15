//! `helm-memory` — unified memory subsystem: region tree, FlatView, MMIO, TLB/cache.
//!
//! # Phase 0 note
//! Phase 0 uses a plain `FlatMem` (in `helm-engine`) for simplicity.
//! This crate is wired in at Phase 1 when `MemoryMap` replaces `FlatMem`.
//!
//! # Key types
//! - [`MemoryRegion`] — the recursive region tree (RAM, ROM, MMIO, Alias, Container)
//! - [`MemoryMap`]    — root container + cached [`FlatView`] + `MemInterface` impl
//! - [`FlatRange`]    — one contiguous segment in the flattened view

use helm_core::{AccessType, MemFault, MemInterface};

// ── MemoryRegion ──────────────────────────────────────────────────────────────

/// A node in the QEMU-style memory region tree.
///
/// The tree is built in Python config (Phase 2+) or Rust tests (Phase 1).
/// `MemoryMap` flattens it into a sorted `Vec<FlatRange>` on first access.
pub enum MemoryRegion {
    /// Read-write DRAM.
    Ram { data: Vec<u8> },
    /// Read-only ROM.
    Rom { data: Vec<u8> },
    /// MMIO — forwards reads/writes to an external device callback.
    /// `size` must match the device's `region_size()`.
    Mmio {
        size: u64,
        read:  Box<dyn Fn(u64, usize) -> u64 + Send>,
        write: Box<dyn Fn(u64, usize, u64) + Send>,
    },
    /// Alias into another region at a different base address.
    Alias {
        target_base: u64,
        offset: u64,
        size: u64,
    },
    /// Container: a region composed of sub-regions.
    Container {
        size: u64,
        // (local_offset, sub_region)
        children: Vec<(u64, MemoryRegion)>,
    },
    /// Hole / reserved range — all accesses fault.
    Reserved { size: u64 },
}

impl MemoryRegion {
    /// Return the size of this region in bytes.
    pub fn size(&self) -> u64 {
        match self {
            Self::Ram { data } | Self::Rom { data } => data.len() as u64,
            Self::Mmio { size, .. }
            | Self::Alias { size, .. }
            | Self::Container { size, .. }
            | Self::Reserved { size } => *size,
        }
    }
}

// ── FlatView ──────────────────────────────────────────────────────────────────

/// One contiguous, non-overlapping guest-physical address range.
pub struct FlatRange {
    /// Guest-physical start address.
    pub base: u64,
    /// Length in bytes.
    pub size: u64,
    /// Index into `MemoryMap::regions` (Phase 1 detail — simplified here).
    pub region_idx: usize,
}

/// Sorted, non-overlapping list of `FlatRange`s covering the full GPA space.
pub type FlatView = Vec<FlatRange>;

// ── MemoryMap ─────────────────────────────────────────────────────────────────

/// The root memory map — owns all regions and the cached FlatView.
///
/// `elaborate()` must be called before first access to build the FlatView.
pub struct MemoryMap {
    regions: Vec<(u64, MemoryRegion)>, // (base, region)
    flat:    Option<FlatView>,
}

impl Default for MemoryMap {
    fn default() -> Self { Self::new() }
}

impl MemoryMap {
    pub fn new() -> Self { Self { regions: Vec::new(), flat: None } }

    /// Register a top-level region at the given guest-physical base address.
    /// Invalidates the cached FlatView.
    pub fn add_region(&mut self, base: u64, region: MemoryRegion) {
        self.flat = None;
        self.regions.push((base, region));
    }

    /// Build (or return cached) the FlatView.
    pub fn flat_view(&mut self) -> &FlatView {
        if self.flat.is_none() {
            self.flat = Some(self.build_flat_view());
        }
        self.flat.as_ref().unwrap()
    }

    fn build_flat_view(&self) -> FlatView {
        // TODO(phase-1): recursive flattening with alias resolution.
        // Flat view is sorted by base address; overlaps use last-added wins.
        let mut ranges: FlatView = self
            .regions
            .iter()
            .enumerate()
            .map(|(idx, (base, r))| FlatRange { base: *base, size: r.size(), region_idx: idx })
            .collect();
        ranges.sort_unstable_by_key(|r| r.base);
        ranges
    }

    /// Resolve a guest-physical address to a `(region_idx, offset_within_region)`.
    fn resolve(&mut self, addr: u64) -> Option<(usize, u64)> {
        let flat = self.flat_view();
        let idx = flat.partition_point(|r| r.base + r.size <= addr);
        if idx < flat.len() && flat[idx].base <= addr {
            let r = &flat[idx];
            Some((r.region_idx, addr - r.base))
        } else {
            None
        }
    }
}

impl MemInterface for MemoryMap {
    fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
        let (idx, offset) = self.resolve(addr).ok_or(MemFault::AccessFault { addr })?;
        match &self.regions[idx].1 {
            MemoryRegion::Ram { data } | MemoryRegion::Rom { data } => {
                let end = offset as usize + size;
                if end > data.len() { return Err(MemFault::AccessFault { addr }); }
                let mut buf = [0u8; 8];
                buf[..size].copy_from_slice(&data[offset as usize..end]);
                Ok(u64::from_le_bytes(buf))
            }
            MemoryRegion::Mmio { read, .. } => Ok((read)(offset, size)),
            MemoryRegion::Reserved { .. } => Err(MemFault::AccessFault { addr }),
            MemoryRegion::Alias { .. } | MemoryRegion::Container { .. } => {
                // TODO(phase-1): alias/container resolution
                Err(MemFault::AccessFault { addr })
            }
        }
    }

    fn write(&mut self, addr: u64, size: usize, val: u64, _ty: AccessType) -> Result<(), MemFault> {
        let (idx, offset) = self.resolve(addr).ok_or(MemFault::AccessFault { addr })?;
        match &mut self.regions[idx].1 {
            MemoryRegion::Ram { data } => {
                let end = offset as usize + size;
                if end > data.len() { return Err(MemFault::AccessFault { addr }); }
                let bytes = val.to_le_bytes();
                data[offset as usize..end].copy_from_slice(&bytes[..size]);
                Ok(())
            }
            MemoryRegion::Rom { .. } => Err(MemFault::ReadOnly { addr }),
            MemoryRegion::Mmio { write, .. } => { (write)(offset, size, val); Ok(()) }
            MemoryRegion::Reserved { .. } => Err(MemFault::AccessFault { addr }),
            MemoryRegion::Alias { .. } | MemoryRegion::Container { .. } => {
                Err(MemFault::AccessFault { addr })
            }
        }
    }
}
