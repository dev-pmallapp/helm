//! Abstract memory interface for instruction execution.
//!
//! Decouples ISA execute from AddressSpace/TLB/cache details.

use crate::types::Addr;

/// Memory fault descriptor.
#[derive(Debug)]
pub struct MemFault {
    pub addr: Addr,
    pub is_write: bool,
    pub kind: MemFaultKind,
}

/// Classification of memory faults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemFaultKind {
    Unmapped,
    Permission,
    Alignment,
    PageFault,
}

impl std::fmt::Display for MemFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MemFault({:?} at {:#x}, write={})",
            self.kind, self.addr, self.is_write
        )
    }
}

impl std::error::Error for MemFault {}

/// Abstract memory interface for instruction execution.
///
/// Scalar `read`/`write` handle values up to 8 bytes. `read_wide`/`write_wide`
/// handle 16/32/64-byte SIMD and AVX operands. `copy_bulk`/`fill_bulk` handle
/// x86 string ops and block transfers.
pub trait MemoryAccess: Send {
    fn read(&mut self, addr: Addr, size: usize) -> Result<u64, MemFault>;
    fn write(&mut self, addr: Addr, size: usize, val: u64) -> Result<(), MemFault>;
    fn fetch(&mut self, addr: Addr, buf: &mut [u8]) -> Result<(), MemFault>;

    /// Read 16/32/64-byte value (SSE/AVX/SVE). Default builds from scalar reads.
    fn read_wide(&mut self, addr: Addr, buf: &mut [u8]) -> Result<(), MemFault> {
        for (i, chunk) in buf.chunks_mut(8).enumerate() {
            let v = self.read(addr + (i * 8) as u64, chunk.len())?;
            chunk.copy_from_slice(&v.to_le_bytes()[..chunk.len()]);
        }
        Ok(())
    }

    /// Write 16/32/64-byte value (SSE/AVX/SVE). Default builds from scalar writes.
    fn write_wide(&mut self, addr: Addr, data: &[u8]) -> Result<(), MemFault> {
        for (i, chunk) in data.chunks(8).enumerate() {
            let mut buf = [0u8; 8];
            buf[..chunk.len()].copy_from_slice(chunk);
            self.write(
                addr + (i * 8) as u64,
                chunk.len(),
                u64::from_le_bytes(buf),
            )?;
        }
        Ok(())
    }

    /// Bulk copy (x86 REP MOVSB, block transfer). Default loops scalar reads/writes.
    fn copy_bulk(&mut self, dst: Addr, src: Addr, len: usize) -> Result<(), MemFault> {
        for i in 0..len {
            let v = self.read(src + i as u64, 1)?;
            self.write(dst + i as u64, 1, v)?;
        }
        Ok(())
    }

    /// Bulk fill (x86 REP STOSB). Default loops scalar writes.
    fn fill_bulk(&mut self, dst: Addr, val: u8, len: usize) -> Result<(), MemFault> {
        for i in 0..len {
            self.write(dst + i as u64, 1, val as u64)?;
        }
        Ok(())
    }

    /// Compare-and-exchange (CMPXCHG, LDXR/STXR, LR/SC).
    /// Returns `Ok(old_value)`. Writes `new` only if `*addr == expected`.
    fn compare_exchange(
        &mut self,
        addr: Addr,
        size: usize,
        expected: u64,
        new: u64,
    ) -> Result<u64, MemFault> {
        let old = self.read(addr, size)?;
        if old == expected {
            self.write(addr, size, new)?;
        }
        Ok(old)
    }
}
