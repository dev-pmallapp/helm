//! `helm-memory` — memory building blocks for the current runtime and the
//! longer-term unified memory model.
//!
//! # Key types
//! - [`FlatMem`]      — sparse RAM backend with a page-table fast path
//! - [`HelmAddressSpace`] — the current authoritative physical-memory surface
//!   for runtime RAM + MMIO dispatch
//! - [`SharedDmaPort`] — shared DMA adapter over [`HelmAddressSpace`]
//! - `MemoryRegion` / `MemoryMap` / `FlatView` — experimental region-tree
//!   surface, available only with the `experimental-memmap` cargo feature

#![allow(missing_docs)]

mod address_space;
mod dma;
mod flat_mem;

#[cfg(feature = "experimental-memmap")]
use helm_core::{AccessType, MemFault, MemInterface};
#[cfg(feature = "experimental-memmap")]
use helm_core::{MemoryMap as MemoryMapTrait, MemoryMapRange, MemoryMapRangeKind};

pub use address_space::HelmAddressSpace;
pub use dma::SharedDmaPort;
pub use flat_mem::FlatMem;

// ── MemoryRegion ──────────────────────────────────────────────────────────────

/// A node in the experimental QEMU-style memory region tree.
///
/// This model is not the live runtime memory surface today. `HelmAddressSpace`
/// remains authoritative for current RAM/MMIO behavior while this tree model
/// still lacks complete alias/container/remap semantics.
#[cfg(feature = "experimental-memmap")]
pub enum MemoryRegion {
    /// Read-write DRAM.
    Ram { data: Vec<u8> },
    /// Read-only ROM.
    Rom { data: Vec<u8> },
    /// MMIO — forwards reads/writes to an external device callback.
    /// `size` must match the device's `region_size()`.
    Mmio {
        size: u64,
        read: Box<dyn Fn(u64, usize) -> u64 + Send>,
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

#[cfg(feature = "experimental-memmap")]
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

    /// Return the coarse-grained kind for this region.
    pub fn kind(&self) -> MemoryMapRangeKind {
        match self {
            Self::Ram { .. } => MemoryMapRangeKind::Ram,
            Self::Rom { .. } => MemoryMapRangeKind::Rom,
            Self::Mmio { .. } => MemoryMapRangeKind::Mmio,
            Self::Alias { .. } => MemoryMapRangeKind::Alias,
            Self::Container { .. } => MemoryMapRangeKind::Container,
            Self::Reserved { .. } => MemoryMapRangeKind::Reserved,
        }
    }
}

// ── FlatView ──────────────────────────────────────────────────────────────────

/// One contiguous, non-overlapping guest-physical address range.
#[cfg(feature = "experimental-memmap")]
pub struct FlatRange {
    /// Guest-physical start address.
    pub base: u64,
    /// Length in bytes.
    pub size: u64,
    /// Index into `MemoryMap::regions` (Phase 1 detail — simplified here).
    pub region_idx: usize,
}

/// Sorted, non-overlapping list of `FlatRange`s covering the full GPA space.
#[cfg(feature = "experimental-memmap")]
pub type FlatView = Vec<FlatRange>;

// ── MemoryMap ─────────────────────────────────────────────────────────────────

/// Experimental root memory map — owns all regions and the cached FlatView.
///
/// This is intentionally not presented as the active runtime answer for
/// physical memory. It is a partial region-tree implementation retained for
/// future convergence work once alias/container/remap behavior is complete.
#[cfg(feature = "experimental-memmap")]
pub struct MemoryMap {
    regions: Vec<(u64, MemoryRegion)>, // (base, region)
    flat: Option<FlatView>,
}

#[cfg(feature = "experimental-memmap")]
impl Default for MemoryMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "experimental-memmap")]
impl MemoryMap {
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            flat: None,
        }
    }

    /// Register a top-level region at the given guest-physical base address.
    /// Invalidates the cached FlatView.
    pub fn add_region(&mut self, base: u64, region: MemoryRegion) {
        self.flat = None;
        self.regions.push((base, region));
    }

    /// Remove and return the top-level region whose base exactly matches `base`.
    pub fn remove_region(&mut self, base: u64) -> Option<MemoryRegion> {
        let idx = self
            .regions
            .iter()
            .position(|(region_base, _)| *region_base == base)?;
        self.flat = None;
        Some(self.regions.remove(idx).1)
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
            .map(|(idx, (base, r))| FlatRange {
                base: *base,
                size: r.size(),
                region_idx: idx,
            })
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

#[cfg(feature = "experimental-memmap")]
impl MemoryMapTrait for MemoryMap {
    type Region = MemoryRegion;

    fn add_region(&mut self, base: u64, region: Self::Region) {
        Self::add_region(self, base, region);
    }

    fn remove_region(&mut self, base: u64) -> Option<Self::Region> {
        Self::remove_region(self, base)
    }

    fn mapped_ranges(&mut self) -> Vec<MemoryMapRange> {
        let kinds: Vec<MemoryMapRangeKind> = self
            .regions
            .iter()
            .map(|(_, region)| region.kind())
            .collect();
        self.flat_view()
            .iter()
            .map(|range| MemoryMapRange {
                base: range.base,
                size: range.size,
                kind: kinds[range.region_idx],
            })
            .collect()
    }
}

#[cfg(feature = "experimental-memmap")]
impl MemInterface for MemoryMap {
    fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
        let (idx, offset) = self.resolve(addr).ok_or(MemFault::AccessFault { addr })?;
        match &self.regions[idx].1 {
            MemoryRegion::Ram { data } | MemoryRegion::Rom { data } => {
                let end = offset as usize + size;
                if end > data.len() {
                    return Err(MemFault::AccessFault { addr });
                }
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
                if end > data.len() {
                    return Err(MemFault::AccessFault { addr });
                }
                let bytes = val.to_le_bytes();
                data[offset as usize..end].copy_from_slice(&bytes[..size]);
                Ok(())
            }
            MemoryRegion::Rom { .. } => Err(MemFault::ReadOnly { addr }),
            MemoryRegion::Mmio { write, .. } => {
                (write)(offset, size, val);
                Ok(())
            }
            MemoryRegion::Reserved { .. } => Err(MemFault::AccessFault { addr }),
            MemoryRegion::Alias { .. } | MemoryRegion::Container { .. } => {
                Err(MemFault::AccessFault { addr })
            }
        }
    }
}

#[cfg(all(test, feature = "experimental-memmap"))]
mod tests {
    use super::*;

    #[test]
    fn memory_map_trait_object_supports_region_management_and_lookup() {
        let mut map = MemoryMap::new();
        let view: &mut dyn MemoryMapTrait<Region = MemoryRegion> = &mut map;

        view.add_region(
            0x1000,
            MemoryRegion::Ram {
                data: vec![0u8; 0x100],
            },
        );
        view.add_region(0x2000, MemoryRegion::Reserved { size: 0x80 });

        assert!(view.contains(0x1000));
        assert_eq!(
            view.lookup_range(0x2001),
            Some(MemoryMapRange {
                base: 0x2000,
                size: 0x80,
                kind: MemoryMapRangeKind::Reserved,
            })
        );

        let removed = view.remove_region(0x1000);
        assert!(matches!(removed, Some(MemoryRegion::Ram { .. })));
        assert!(!view.contains(0x1000));
    }
}
