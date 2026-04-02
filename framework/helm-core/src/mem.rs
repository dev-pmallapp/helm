//! Memory access types, faults, and the `MemInterface` trait.

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
/// Phase 0: implemented by [`helm_memory::FlatMem`] (a sparse RAM backend).
/// Phase 1+: implemented by [`helm_memory::MemoryMap`].
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
