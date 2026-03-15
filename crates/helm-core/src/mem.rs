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
    #[error("access fault at {addr:#x}")]
    AccessFault { addr: u64 },

    #[error("alignment fault at {addr:#x} (size={size})")]
    AlignmentFault { addr: u64, size: usize },

    #[error("page fault at {addr:#x}")]
    PageFault { addr: u64 },

    #[error("write to read-only region at {addr:#x}")]
    ReadOnly { addr: u64 },
}

/// The memory subsystem interface presented to the execution engine.
///
/// Phase 0: implemented by [`helm_engine::FlatMem`] (a `Vec<u8>` wrapper).
/// Phase 1+: implemented by [`helm_memory::MemoryMap`].
///
/// `size` is in bytes: 1, 2, 4, or 8. Values are always returned/stored as
/// little-endian `u64` regardless of host endianness.
pub trait MemInterface: Send {
    fn read(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault>;
    fn write(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault>;

    /// Convenience: fetch a 32-bit instruction word.
    fn fetch32(&mut self, addr: u64) -> Result<u32, MemFault> {
        self.read(addr, 4, AccessType::Fetch).map(|v| v as u32)
    }

    /// Convenience: fetch a 16-bit compressed instruction.
    fn fetch16(&mut self, addr: u64) -> Result<u16, MemFault> {
        self.read(addr, 2, AccessType::Fetch).map(|v| v as u16)
    }
}
