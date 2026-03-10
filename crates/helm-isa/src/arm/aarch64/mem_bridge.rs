//! Memory bridge — abstracts over AddressSpace and dyn MemoryAccess.
//!
//! The AArch64 executor uses three memory operations:
//!   1. `read_bytes` / `write_bytes` — data load/store (SE + FS)
//!   2. `read_phys` — physical-address reads for MMU page table walks (FS only)
//!   3. `host_ptr_for_pa` — raw host pointer for TLB fast path (FS only)
//!
//! `ExecMem` unifies these behind a single trait, implemented for both
//! `AddressSpace` (legacy path) and `TraitMemBridge` (new trait path).

use helm_core::types::Addr;
use helm_core::HelmResult;

/// Byte-oriented memory interface used by the AArch64 executor.
///
/// Methods 2 and 3 have defaults that work for SE mode (no MMU).
/// FS mode overrides them via the AddressSpace implementation.
pub trait ExecMem {
    /// Read `buf.len()` bytes from `addr`.
    fn read_bytes(&mut self, addr: Addr, buf: &mut [u8]) -> HelmResult<()>;

    /// Write `data` bytes to `addr`.
    fn write_bytes(&mut self, addr: Addr, data: &[u8]) -> HelmResult<()>;

    /// Read from a physical address (for MMU page table walks).
    /// Default delegates to `read_bytes` (correct when MMU is off).
    fn read_phys(&self, _addr: Addr, _buf: &mut [u8]) -> HelmResult<()> {
        Ok(()) // SE mode: no page table walks
    }

    /// Get a host pointer for a physical page (for TLB fast path).
    /// Default returns None (no fast path available).
    fn host_ptr_for_pa(&self, _pa: u64) -> Option<*mut u8> {
        None
    }
}

// ── AddressSpace implementation (legacy FS + SE path) ───────────────

impl ExecMem for helm_memory::address_space::AddressSpace {
    #[inline]
    fn read_bytes(&mut self, addr: Addr, buf: &mut [u8]) -> HelmResult<()> {
        self.read(addr, buf)
    }

    #[inline]
    fn write_bytes(&mut self, addr: Addr, data: &[u8]) -> HelmResult<()> {
        self.write(addr, data)
    }

    fn read_phys(&self, addr: Addr, buf: &mut [u8]) -> HelmResult<()> {
        helm_memory::address_space::AddressSpace::read_phys(
            // read_phys takes &self not &mut self
            self, addr, buf,
        )
    }

    fn host_ptr_for_pa(&self, pa: u64) -> Option<*mut u8> {
        helm_memory::address_space::AddressSpace::host_ptr_for_pa(self, pa)
    }
}

// ── TraitMemBridge (new trait-based path) ───────────────────────────

/// Wraps `&mut dyn MemoryAccess` to provide `ExecMem` for the generic session.
pub struct TraitMemBridge<'a>(pub &'a mut dyn helm_core::mem::MemoryAccess);

impl ExecMem for TraitMemBridge<'_> {
    #[inline]
    fn read_bytes(&mut self, addr: Addr, buf: &mut [u8]) -> HelmResult<()> {
        self.0.fetch(addr, buf).map_err(|f| helm_core::HelmError::Memory {
            addr: f.addr,
            reason: format!("{:?}", f.kind),
        })
    }

    #[inline]
    fn write_bytes(&mut self, addr: Addr, data: &[u8]) -> HelmResult<()> {
        if data.len() <= 8 {
            let mut val_bytes = [0u8; 8];
            val_bytes[..data.len()].copy_from_slice(data);
            let val = u64::from_le_bytes(val_bytes);
            self.0.write(addr, data.len(), val).map_err(|f| {
                helm_core::HelmError::Memory {
                    addr: f.addr,
                    reason: format!("{:?}", f.kind),
                }
            })
        } else {
            for (i, chunk) in data.chunks(8).enumerate() {
                let mut val_bytes = [0u8; 8];
                val_bytes[..chunk.len()].copy_from_slice(chunk);
                let val = u64::from_le_bytes(val_bytes);
                self.0
                    .write(addr + (i * 8) as u64, chunk.len(), val)
                    .map_err(|f| helm_core::HelmError::Memory {
                        addr: f.addr,
                        reason: format!("{:?}", f.kind),
                    })?;
            }
            Ok(())
        }
    }

    // SE mode: no page table walks or host pointers needed.
    // Defaults from trait are correct.
}
