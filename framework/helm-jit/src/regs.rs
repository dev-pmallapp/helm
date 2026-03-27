//! Flat register array layout for JIT code and arch-state synchronisation.
//!
//! JIT-compiled code accesses guest registers through a flat `[u64; REG_COUNT]`
//! array passed via the `rdi` register. Each slot is at a fixed byte offset:
//! `offset = slot * 8`.
//!
//! # Layout
//!
//! | Slot | Content |
//! |------|---------|
//! | 0–30 | X0–X30 |
//! | 31   | SP      |
//! | 32   | PC      |
//! | 33   | NZCV (packed u32 in low bits) |
//! | 34   | XZR sentinel (always 0, re-zeroed after writes) |
//! | 35–47| Reserved (DAIF, CurrentEL, SPSel, FPCR, FPSR, TPIDR, …) |

#![allow(missing_docs)]

use helm_arch::aarch64::arch_state::Aarch64ArchState;

// Slot indices into the flat register array.
pub const REG_X0: usize = 0;
// X1–X29 follow contiguously.
pub const REG_X30: usize = 30;
pub const REG_SP: usize = 31;
pub const REG_PC: usize = 32;
pub const REG_NZCV: usize = 33;
pub const REG_XZR: usize = 34;
pub const REG_DAIF: usize = 35;
pub const REG_CURRENT_EL: usize = 36;
pub const REG_SPSEL: usize = 37;

/// Total number of 64-bit slots in the flat array.
pub const REG_COUNT: usize = 48;

/// Byte offset of a register slot (for use in dynasm `[rdi + off]` operands).
#[inline]
pub const fn reg_offset(slot: usize) -> i32 {
    (slot * 8) as i32
}

/// Copy architectural state into the flat JIT register array.
pub fn arch_to_flat(a64: &Aarch64ArchState) -> [u64; REG_COUNT] {
    let mut flat = [0u64; REG_COUNT];
    // X0–X30
    for i in 0..31 {
        flat[REG_X0 + i] = a64.x[i];
    }
    flat[REG_SP] = a64.sp;
    flat[REG_PC] = a64.pc;
    flat[REG_NZCV] = u64::from(a64.nzcv);
    flat[REG_XZR] = 0; // sentinel — always zero
    flat[REG_DAIF] = u64::from(a64.daif);
    flat[REG_CURRENT_EL] = u64::from(a64.current_el);
    flat[REG_SPSEL] = u64::from(a64.spsel);
    flat
}

/// Write the flat JIT register array back into architectural state.
///
/// Only touches the fields that JIT code may have modified: X0–X30, SP, PC,
/// NZCV. The XZR sentinel is re-zeroed in the flat array after this call.
pub fn flat_to_arch(regs: &mut [u64; REG_COUNT], a64: &mut Aarch64ArchState) {
    for i in 0..31 {
        a64.x[i] = regs[REG_X0 + i];
    }
    a64.sp = regs[REG_SP];
    a64.pc = regs[REG_PC];
    a64.nzcv = regs[REG_NZCV] as u32;
    // Re-zero the XZR sentinel in case JIT code accidentally wrote to it.
    regs[REG_XZR] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_state() {
        let mut a64 = Aarch64ArchState::default();
        a64.x[0] = 0xDEAD_BEEF;
        a64.x[30] = 0x1234;
        a64.sp = 0x7FFF_FFFF_FFF0;
        a64.pc = 0x4000_0000;
        a64.nzcv = 0xA000_0000; // N=1, C=1

        let mut flat = arch_to_flat(&a64);
        assert_eq!(flat[REG_X0], 0xDEAD_BEEF);
        assert_eq!(flat[REG_X30], 0x1234);
        assert_eq!(flat[REG_SP], 0x7FFF_FFFF_FFF0);
        assert_eq!(flat[REG_PC], 0x4000_0000);
        assert_eq!(flat[REG_NZCV], 0xA000_0000);
        assert_eq!(flat[REG_XZR], 0);

        // Modify in flat array (simulating JIT execution)
        flat[REG_X0] = 42;
        flat[REG_PC] = 0x4000_0008;

        let mut a64_out = Aarch64ArchState::default();
        flat_to_arch(&mut flat, &mut a64_out);
        assert_eq!(a64_out.x[0], 42);
        assert_eq!(a64_out.pc, 0x4000_0008);
        assert_eq!(flat[REG_XZR], 0); // re-zeroed
    }

    #[test]
    fn reg_offset_is_8_aligned() {
        assert_eq!(reg_offset(0), 0);
        assert_eq!(reg_offset(1), 8);
        assert_eq!(reg_offset(REG_PC), 256);
        assert_eq!(reg_offset(REG_NZCV), 264);
    }
}
