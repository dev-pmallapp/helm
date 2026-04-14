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
// Slot 38: LDP/STP stash scratch (ldst.rs)
/// Slot for `*mut Aarch64ArchState` pointer (system register helper access).
pub const REG_JIT_ARCH_STATE: usize = 39;
// Slot 40: spare
// Slots 41–43: Lazy NZCV (REG_FLAG_OP / REG_FLAG_LHS / REG_FLAG_RHS)
/// Slot for `JitSeTlb` base pointer (SE-mode inline TLB fast path).
/// Stores `tlb.entries.as_ptr() as u64`. Populated by `run_jit` on entry.
pub const REG_JIT_SE_TLB: usize = 44;
/// Scratch slot used to preserve pinned guest registers across inline JIT fast paths.
pub const REG_JIT_TMP0: usize = 45;
/// Slot for `jit_mem_read` function pointer (stencil backend).
pub const REG_JIT_MEM_READ: usize = 46;
/// Slot for `jit_mem_write` function pointer (stencil backend).
pub const REG_JIT_MEM_WRITE: usize = 47;

/// Base slot for V0 (128-bit SIMD registers). Each Vn occupies 2 u64 slots:
/// V0 = slots 48-49, V1 = slots 50-51, ..., V31 = slots 110-111.
pub const REG_V_BASE: usize = 48;

/// Total number of 64-bit slots in the flat array (48 GPR/system + 64 SIMD).
pub const REG_COUNT: usize = 112;

/// Byte offset of Vn's low 64-bit half in the flat array.
#[inline]
pub const fn vreg_offset_lo(vn: usize) -> i32 {
    ((REG_V_BASE + vn * 2) * 8) as i32
}

/// Byte offset of Vn's high 64-bit half in the flat array.
#[inline]
pub const fn vreg_offset_hi(vn: usize) -> i32 {
    ((REG_V_BASE + vn * 2 + 1) * 8) as i32
}

/// Byte offset of a register slot (for use in dynasm `[rdi + off]` operands).
#[inline]
pub const fn reg_offset(slot: usize) -> i32 {
    (slot * 8) as i32
}

// ── Register Pinning (RMSB) ─────────────────────────────────────────────────
//
// The 10 most-used guest registers are pinned to callee-saved x86-64 registers
// across the entire compiled block lifetime. This eliminates `[rdi + N*8]`
// memory traffic for the hot register set.
//
// Slot 38 is reserved for LDP/STP stash (ldst.rs uses it as scratch storage
// across two emit_mem_read calls). Slots 41–43 are reserved for lazy NZCV.
// Slots 44–45 are spare. Slots 46–47 are mem-helper function pointers.
//
// x86-64 register assignment (callee-saved per System V AMD64 ABI):
//   rbx, rbp, r12, r13, r14, r15 are callee-saved.
//   r8–r11 are caller-saved — JIT block callers (run_jit) must save/restore them.

/// An x86-64 host register used to pin a guest register for the block lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostReg {
    /// x86-64 r8  (caller-saved; JIT block owns it across the block)
    R8,
    /// x86-64 r9
    R9,
    /// x86-64 r10
    R10,
    /// x86-64 r11
    R11,
    /// x86-64 r12 (callee-saved)
    R12,
    /// x86-64 r13 (callee-saved)
    R13,
    /// x86-64 r14 (callee-saved)
    R14,
    /// x86-64 r15 (callee-saved)
    R15,
    /// x86-64 rbx (callee-saved)
    Rbx,
    /// x86-64 rbp (callee-saved)
    Rbp,
}

/// Static default binding: 10 guest register slots pinned to host registers.
///
/// On entry to a compiled block the prologue loads these from `[rdi + slot*8]`.
/// On exit the epilogue writes them back. Between those two points, memory
/// traffic to the flat array is eliminated for these slots.
pub static DEFAULT_BINDING: [(usize, HostReg); 10] = [
    (REG_X0, HostReg::R8),       // X0  — return value / arg 0
    (REG_X0 + 1, HostReg::R9),   // X1  — arg 1
    (REG_X0 + 2, HostReg::R10),  // X2  — arg 2
    (REG_X0 + 3, HostReg::R11),  // X3  — arg 3
    (REG_X0 + 4, HostReg::Rbx),  // X4  — arg 4 (callee-saved; C calls preserve)
    (REG_X0 + 19, HostReg::R12), // X19 — callee-saved: loop counter
    (REG_X0 + 20, HostReg::R13), // X20 — callee-saved: loop base
    (REG_X0 + 30, HostReg::R14), // X30 — link register (callee-saved)
    (REG_SP, HostReg::R15),      // SP  — guest stack pointer (callee-saved)
    (REG_NZCV, HostReg::Rbp),    // NZCV — flags (callee-saved)
];

/// Look up which host register holds a given guest slot.
///
/// Returns `None` if the slot is spilled (lives in `[rdi + slot*8]`).
#[inline]
pub fn pinned_host_reg(slot: usize) -> Option<HostReg> {
    for (s, r) in &DEFAULT_BINDING {
        if *s == slot {
            return Some(*r);
        }
    }
    None
}

/// Returns true if the given guest slot is pinned in a host register.
#[inline]
pub fn is_pinned(slot: usize) -> bool {
    pinned_host_reg(slot).is_some()
}

/// Copy architectural state into the flat JIT register array, skipping pinned slots.
///
/// Used at JIT→interpreter boundaries when pinned registers are already live in
/// host regs (they will be written back by the block epilogue). PC is always
/// written because the branch emitters keep it up-to-date in the flat array.
pub fn arch_to_flat_nonpinned(a64: &Aarch64ArchState, flat: &mut [u64; REG_COUNT]) {
    for i in 0..31 {
        if !is_pinned(i) {
            flat[REG_X0 + i] = a64.x[i];
        }
    }
    // SP and NZCV are pinned — skip.
    //
    // The pinned SP slot must still represent the architecturally visible SP,
    // which is banked when EL1+ uses SPSel=1.
    flat[REG_SP] = a64.current_sp();
    // PC is never pinned; branch emitters write it directly to the flat array.
    flat[REG_PC] = a64.pc;
    flat[REG_XZR] = 0;
    flat[REG_DAIF] = u64::from(a64.daif);
    flat[REG_CURRENT_EL] = u64::from(a64.current_el);
    flat[REG_SPSEL] = u64::from(a64.spsel);
}

// ── Lazy NZCV (deferred flag computation) ───────────────────────────────────
//
// Instead of computing and storing NZCV after every flag-setting instruction,
// we store the opcode and operands. The flags are materialised only when a
// branch instruction actually needs to read them.
//
// Slots 41–43 are used for FlagOp storage.
// (Slot 38 is already used as a scratch stash for LDP/STP in ldst.rs.)

/// Slot for the deferred flag operation code.
pub const REG_FLAG_OP: usize = 41;
/// Slot for the left-hand operand of the deferred flag operation.
pub const REG_FLAG_LHS: usize = 42;
/// Slot for the right-hand operand of the deferred flag operation.
pub const REG_FLAG_RHS: usize = 43;

/// Which arithmetic/logical operation produced the flags to be deferred.
///
/// The JIT emitter stores this alongside the raw operands so that the
/// materialise routine can reconstruct NZCV without re-executing the instruction.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlagOp {
    /// No pending deferred flags (NZCV is already up-to-date in rbp).
    None = 0,
    /// 64-bit addition: ADDS/CMN.
    Add64 = 1,
    /// 64-bit subtraction: SUBS/CMP.
    Sub64 = 2,
    /// 64-bit logical AND: ANDS/TST.
    And64 = 3,
    /// 32-bit addition.
    Add32 = 4,
    /// 32-bit subtraction.
    Sub32 = 5,
    /// 32-bit logical AND.
    And32 = 6,
}

/// Copy architectural state into the flat JIT register array.
pub fn arch_to_flat(a64: &Aarch64ArchState) -> [u64; REG_COUNT] {
    let mut flat = [0u64; REG_COUNT];
    // X0–X30
    for i in 0..31 {
        flat[REG_X0 + i] = a64.x[i];
    }
    flat[REG_SP] = a64.current_sp();
    flat[REG_PC] = a64.pc;
    flat[REG_NZCV] = u64::from(a64.nzcv);
    flat[REG_XZR] = 0; // sentinel — always zero
    flat[REG_DAIF] = u64::from(a64.daif);
    flat[REG_CURRENT_EL] = u64::from(a64.current_el);
    flat[REG_SPSEL] = u64::from(a64.spsel);
    // V0-V31 (128-bit each -> 2 u64 slots)
    for i in 0..32 {
        flat[REG_V_BASE + i * 2] = a64.v[i] as u64;
        flat[REG_V_BASE + i * 2 + 1] = (a64.v[i] >> 64) as u64;
    }
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
    if a64.current_el >= 1 && a64.spsel {
        match a64.current_el {
            1 => a64.sp_el1 = regs[REG_SP],
            2 => a64.sp_el2 = regs[REG_SP],
            3 => a64.sp_el3 = regs[REG_SP],
            _ => a64.sp = regs[REG_SP],
        }
    } else {
        a64.sp = regs[REG_SP];
    }
    a64.pc = regs[REG_PC];
    a64.nzcv = regs[REG_NZCV] as u32;
    // Re-zero the XZR sentinel in case JIT code accidentally wrote to it.
    regs[REG_XZR] = 0;
    // V0-V31 (128-bit each <- 2 u64 slots)
    for i in 0..32 {
        a64.v[i] = regs[REG_V_BASE + i * 2] as u128
            | ((regs[REG_V_BASE + i * 2 + 1] as u128) << 64);
    }
}

// ── RISC-V64 register sync ──────────────────────────────────────────────────

// RISC-V64 flat register layout constants — available when stencil backend compiled.
/// Total number of 64-bit slots in the RISC-V flat array.
pub const REG_COUNT_RV64: usize = 40;
/// PC slot index in the RISC-V flat array.
pub const REG_PC_RV64: usize = 32;

/// Copy RISC-V integer registers + PC into the flat JIT register array.
///
/// `iregs` is a `[u64; 32]` array and `pc` is the program counter.
/// The flat layout: slots 0–31 = x0–x31, slot 32 = PC.
pub fn arch_to_flat_rv64(iregs: &[u64; 32], pc: u64) -> [u64; REG_COUNT_RV64] {
    let mut flat = [0u64; REG_COUNT_RV64];
    for i in 0..32 {
        flat[i] = iregs[i];
    }
    flat[REG_PC_RV64] = pc;
    flat[0] = 0; // x0 hardwired zero
    flat
}

/// Write the flat JIT register array back into RISC-V integer registers + PC.
///
/// Writes x1–x31 and PC; x0 is re-zeroed in both the flat array and `iregs`.
pub fn flat_to_arch_rv64(regs: &mut [u64; REG_COUNT_RV64], iregs: &mut [u64; 32], pc: &mut u64) {
    for i in 1..32 {
        iregs[i] = regs[i];
    }
    iregs[0] = 0; // x0 hardwired zero
    *pc = regs[REG_PC_RV64];
    regs[0] = 0; // re-zero x0 in flat array
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
    fn round_trip_uses_current_banked_sp() {
        let mut a64 = Aarch64ArchState::default();
        a64.current_el = 1;
        a64.spsel = true;
        a64.sp = 0x1111;
        a64.sp_el1 = 0x2222;

        let mut flat = arch_to_flat(&a64);
        assert_eq!(flat[REG_SP], 0x2222);

        flat[REG_SP] = 0x3333;
        flat_to_arch(&mut flat, &mut a64);

        assert_eq!(a64.sp, 0x1111);
        assert_eq!(a64.sp_el1, 0x3333);
    }

    #[test]
    fn reg_offset_is_8_aligned() {
        assert_eq!(reg_offset(0), 0);
        assert_eq!(reg_offset(1), 8);
        assert_eq!(reg_offset(REG_PC), 256);
        assert_eq!(reg_offset(REG_NZCV), 264);
    }
}
