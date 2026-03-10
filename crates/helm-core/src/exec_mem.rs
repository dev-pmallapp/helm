//! ExecMem trait — byte-oriented memory interface for instruction execution.
//!
//! This trait lives in helm-core so it can be implemented by both
//! helm-memory (for AddressSpace) and helm-isa (for TraitMemBridge).

use crate::types::Addr;
use crate::HelmResult;

/// Byte-oriented memory interface used by ISA executors.
///
/// This is a lower-level interface than `MemoryAccess` — it operates on
/// raw byte slices, matching what the hardware does. `MemoryAccess` is
/// the higher-level trait for the generic session (scalar u64 values).
///
/// Methods `read_phys` and `host_ptr_for_pa` have defaults for SE mode
/// (no MMU). FS mode overrides them.
pub trait ExecMem {
    /// Read `buf.len()` bytes from `addr`.
    fn read_bytes(&mut self, addr: Addr, buf: &mut [u8]) -> HelmResult<()>;

    /// Write `data` bytes to `addr`.
    fn write_bytes(&mut self, addr: Addr, data: &[u8]) -> HelmResult<()>;

    /// Read from a physical address (for MMU page table walks).
    /// Default: no-op (correct when MMU is off).
    fn read_phys(&self, _addr: Addr, _buf: &mut [u8]) -> HelmResult<()> {
        Ok(())
    }

    /// Get a host pointer for a physical page (for TLB fast path).
    /// Default: None (no fast path).
    fn host_ptr_for_pa(&self, _pa: u64) -> Option<*mut u8> {
        None
    }
}
