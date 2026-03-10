//! CPU architectural state trait.
//!
//! ISA crates implement this; engine crates provide the concrete struct.

use crate::types::{Addr, RegId};

/// Architectural state that an executor reads/writes.
///
/// The interface is intentionally minimal — read/write GPR, system reg, and
/// flags. ISA-specific details (x87 stack, segment descriptors, SVE vector
/// length, RISC-V CSR) live in the concrete implementation; the `Executor`
/// downcasts or uses ISA-specific extension traits when needed.
pub trait CpuState: Send {
    fn pc(&self) -> Addr;
    fn set_pc(&mut self, pc: Addr);
    fn gpr(&self, id: RegId) -> u64;
    fn set_gpr(&mut self, id: RegId, val: u64);
    fn sysreg(&self, enc: u32) -> u64;
    fn set_sysreg(&mut self, enc: u32, val: u64);

    /// Read processor status / flags register.
    /// ARM: PSTATE (NZCV + DAIF + EL + SPSel).
    /// x86: RFLAGS. RISC-V: mstatus/sstatus.
    fn flags(&self) -> u64;
    fn set_flags(&mut self, flags: u64);

    /// Current privilege level (ARM EL, x86 CPL, RISC-V priv mode).
    fn privilege_level(&self) -> u8;

    /// Read a SIMD/vector register (> 64 bits). Writes into `dst`.
    /// Returns number of bytes written. Default returns 0.
    fn gpr_wide(&self, _id: RegId, _dst: &mut [u8]) -> usize {
        0
    }

    /// Write a SIMD/vector register from a byte slice.
    fn set_gpr_wide(&mut self, _id: RegId, _src: &[u8]) {}
}
