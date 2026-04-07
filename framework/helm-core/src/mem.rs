//! Memory access types, faults, and memory access traits.

use thiserror::Error;

/// The kind of memory access being performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    /// Instruction fetch.
    Fetch,
    /// Normal data load.
    Load,
    /// Normal data store.
    Store,
    /// Atomic read-modify-write (LR/SC, AMO).
    Atomic,
}

/// A memory fault returned from `MemInterface` operations.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum MemFault {
    /// Access could not be serviced by the memory subsystem.
    #[error("access fault at {addr:#x}")]
    AccessFault {
        /// Faulting guest address.
        addr: u64,
    },

    /// Access was not aligned for the requested width.
    #[error("alignment fault at {addr:#x} (size={size})")]
    AlignmentFault {
        /// Faulting guest address.
        addr: u64,
        /// Requested access width in bytes.
        size: usize,
    },

    /// Page translation or mapping failed for the requested address.
    #[error("page fault at {addr:#x} (iss={iss:#x})")]
    PageFault {
        /// Faulting guest address.
        addr: u64,
        /// `AArch64` ESR ISS field encoding (DFSC[5:0] + `WnR`[6] etc.).
        /// Callers should use this when constructing `ESR_EL1` for exception injection.
        /// For load faults: DFSC only. For store faults: caller ORs in `WnR` (bit 6).
        iss: u32,
    },

    /// Write attempted to modify a read-only mapping.
    #[error("write to read-only region at {addr:#x}")]
    ReadOnly {
        /// Faulting guest address.
        addr: u64,
    },
}

/// The memory subsystem interface presented to the execution engine.
///
/// Today this is implemented by:
/// - [`helm_memory::FlatMem`] for sparse RAM-only paths
/// - [`helm_memory::HelmAddressSpace`] for the live runtime physical-memory
///   surface (RAM + MMIO)
/// - [`helm_memory::MemoryMap`] only as an experimental region-tree model
///
/// `size` is in bytes: 1, 2, 4, or 8. Values are always returned/stored as
/// little-endian `u64` regardless of host endianness.
pub trait MemInterface: Send {
    /// Read a little-endian value from guest memory.
    fn read(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault>;
    /// Write a little-endian value to guest memory.
    fn write(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault>;

    /// Convenience: fetch a 32-bit instruction word.
    fn fetch32(&mut self, addr: u64) -> Result<u32, MemFault> {
        self.read(addr, 4, AccessType::Fetch)
            .map(|v| match u32::try_from(v) {
                Ok(word) => word,
                Err(_) => unreachable!("4-byte fetch must fit in u32"),
            })
    }

    /// Convenience: fetch a 16-bit compressed instruction.
    fn fetch16(&mut self, addr: u64) -> Result<u16, MemFault> {
        self.read(addr, 2, AccessType::Fetch)
            .map(|v| match u16::try_from(v) {
                Ok(word) => word,
                Err(_) => unreachable!("2-byte fetch must fit in u16"),
            })
    }
}

/// Shared byte-oriented guest-memory contract.
///
/// This sits above scalar [`MemInterface`] reads/writes and below device- or
/// ISA-specific protocols such as VirtIO descriptor walking or IOMMU table
/// walks.
pub trait ByteMem: Send {
    /// Read `buf.len()` bytes from guest memory at `addr`.
    fn read_bytes(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), MemFault>;

    /// Write `data` bytes to guest memory at `addr`.
    fn write_bytes(&mut self, addr: u64, data: &[u8]) -> Result<(), MemFault>;

    /// Read up to 8 little-endian bytes and pack them into a `u64`.
    fn read_le_u64(&mut self, addr: u64, size: usize) -> Result<u64, MemFault> {
        debug_assert!(size <= 8);
        let mut buf = [0u8; 8];
        self.read_bytes(addr, &mut buf[..size])?;
        Ok(u64::from_le_bytes(buf))
    }

    /// Write the low `size` little-endian bytes of `value`.
    fn write_le_u64(&mut self, addr: u64, size: usize, value: u64) -> Result<(), MemFault> {
        debug_assert!(size <= 8);
        self.write_bytes(addr, &value.to_le_bytes()[..size])
    }
}

/// Coarse-grained classification for one mapped memory range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryMapRangeKind {
    /// Read-write RAM backing.
    Ram,
    /// Read-only ROM backing.
    Rom,
    /// Memory-mapped I/O device window.
    Mmio,
    /// Alias window into another region.
    Alias,
    /// Structural container node.
    Container,
    /// Reserved / faulting address range.
    Reserved,
}

/// One visible range in a memory-map view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryMapRange {
    /// Guest-physical base address.
    pub base: u64,
    /// Size in bytes.
    pub size: u64,
    /// Semantic classification for the range.
    pub kind: MemoryMapRangeKind,
}

/// Planned memory-map trait boundary that was missing from the live code.
///
/// This trait materializes the intended abstraction over a tree- or
/// view-backed physical address space without forcing callers to bind directly
/// to one concrete `helm-memory` implementation.
pub trait MemoryMap: MemInterface + ByteMem {
    /// Concrete region type accepted by this map implementation.
    type Region;

    /// Insert one region at `base`.
    fn add_region(&mut self, base: u64, region: Self::Region);

    /// Remove and return the region whose base exactly matches `base`.
    fn remove_region(&mut self, base: u64) -> Option<Self::Region>;

    /// Return the current visible range view.
    ///
    /// This is intentionally a cold-path allocation-friendly API so callers can
    /// inspect the map without depending on one concrete flat-view type.
    fn mapped_ranges(&mut self) -> Vec<MemoryMapRange>;

    /// Look up the visible range that contains `addr`.
    fn lookup_range(&mut self, addr: u64) -> Option<MemoryMapRange> {
        self.mapped_ranges().into_iter().find(|range| {
            let end = u128::from(range.base) + u128::from(range.size);
            u128::from(addr) >= u128::from(range.base) && u128::from(addr) < end
        })
    }

    /// Returns `true` if any visible range contains `addr`.
    fn contains(&mut self, addr: u64) -> bool {
        self.lookup_range(addr).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockMemoryMap {
        ranges: Vec<MemoryMapRange>,
    }

    impl MemInterface for MockMemoryMap {
        fn read(&mut self, _addr: u64, _size: usize, _ty: AccessType) -> Result<u64, MemFault> {
            Ok(0)
        }

        fn write(
            &mut self,
            _addr: u64,
            _size: usize,
            _val: u64,
            _ty: AccessType,
        ) -> Result<(), MemFault> {
            Ok(())
        }
    }

    impl MemoryMap for MockMemoryMap {
        type Region = MemoryMapRange;

        fn add_region(&mut self, _base: u64, region: Self::Region) {
            self.ranges.push(region);
        }

        fn remove_region(&mut self, base: u64) -> Option<Self::Region> {
            let idx = self.ranges.iter().position(|range| range.base == base)?;
            Some(self.ranges.remove(idx))
        }

        fn mapped_ranges(&mut self) -> Vec<MemoryMapRange> {
            self.ranges.clone()
        }
    }

    #[test]
    fn memory_map_lookup_range_finds_containing_entry() {
        let mut map = MockMemoryMap {
            ranges: vec![
                MemoryMapRange {
                    base: 0x1000,
                    size: 0x100,
                    kind: MemoryMapRangeKind::Ram,
                },
                MemoryMapRange {
                    base: 0x2000,
                    size: 0x80,
                    kind: MemoryMapRangeKind::Mmio,
                },
            ],
        };

        assert_eq!(
            map.lookup_range(0x107F),
            Some(MemoryMapRange {
                base: 0x1000,
                size: 0x100,
                kind: MemoryMapRangeKind::Ram,
            })
        );
        assert_eq!(map.lookup_range(0x2080), None);
    }

    #[test]
    fn memory_map_contains_reflects_lookup() {
        let mut map = MockMemoryMap {
            ranges: vec![MemoryMapRange {
                base: 0x4000,
                size: 0x10,
                kind: MemoryMapRangeKind::Reserved,
            }],
        };

        assert!(map.contains(0x400F));
        assert!(!map.contains(0x4010));
    }
}

impl<T: MemInterface + ?Sized> ByteMem for T {
    fn read_bytes(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), MemFault> {
        for (offset, byte) in buf.iter_mut().enumerate() {
            *byte = self.read(addr + offset as u64, 1, AccessType::Load)? as u8;
        }
        Ok(())
    }

    fn write_bytes(&mut self, addr: u64, data: &[u8]) -> Result<(), MemFault> {
        for (offset, byte) in data.iter().enumerate() {
            self.write(addr + offset as u64, 1, u64::from(*byte), AccessType::Store)?;
        }
        Ok(())
    }
}
