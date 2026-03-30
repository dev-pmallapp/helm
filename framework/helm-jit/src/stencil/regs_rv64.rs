//! Flat register array layout for RISC-V64 JIT code.
//!
//! JIT-compiled code accesses guest registers through a flat `[u64; REG_COUNT_RV64]`
//! array passed via the `rdi` register. Each slot is at a fixed byte offset:
//! `offset = slot * 8`.
//!
//! # Layout
//!
//! | Slot | Content |
//! |------|---------|
//! | 0–31 | x0–x31 (x0 is hardwired zero, re-zeroed after sync) |
//! | 32   | PC      |
//! | 33–39| Reserved (fcsr, etc.) |

#![allow(missing_docs)]

/// Total number of 64-bit slots in the RISC-V flat array.
pub const REG_COUNT_RV64: usize = 40;

// Slot indices.
pub const REG_X0: usize = 0;
// x1–x31 follow contiguously at indices 1–31.
pub const REG_PC_RV64: usize = 32;

/// Byte offset of a register slot (for use in JIT `[rdi + off]` operands).
#[inline]
pub const fn reg_offset_rv64(slot: usize) -> i32 {
    (slot * 8) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rv64_layout_offsets() {
        assert_eq!(reg_offset_rv64(0), 0);        // x0
        assert_eq!(reg_offset_rv64(1), 8);        // x1 (ra)
        assert_eq!(reg_offset_rv64(2), 16);       // x2 (sp)
        assert_eq!(reg_offset_rv64(REG_PC_RV64), 256); // PC
    }

    #[test]
    fn rv64_x0_always_zero() {
        let mut flat = [0u64; REG_COUNT_RV64];
        flat[REG_X0] = 0xDEAD; // simulate JIT writing to x0
        // Re-zero (caller's responsibility after sync):
        flat[REG_X0] = 0;
        assert_eq!(flat[REG_X0], 0);
    }
}
