//! AArch64 exception entry and return (EL1 only).

use super::arch_state::Aarch64ArchState;

// Exception vector offsets (from VBAR_EL1)
pub const SYNC_EL1_SP0: u64 = 0x000; // Synchronous, from EL1, using SP_EL0
pub const IRQ_EL1_SP0: u64 = 0x080;
pub const FIQ_EL1_SP0: u64 = 0x100;
pub const SERROR_EL1_SP0: u64 = 0x180;
pub const SYNC_EL1_SP1: u64 = 0x200; // Synchronous, from EL1, using SP_EL1
pub const IRQ_EL1_SP1: u64 = 0x280;
pub const FIQ_EL1_SP1: u64 = 0x300;
pub const SERROR_EL1_SP1: u64 = 0x380;
pub const SYNC_EL0_64: u64 = 0x400; // Synchronous, from EL0 (AArch64)
pub const IRQ_EL0_64: u64 = 0x480;
pub const FIQ_EL0_64: u64 = 0x500;
pub const SERROR_EL0_64: u64 = 0x580;

// ESR_EL1 exception class (EC) values, shifted to [31:26]
pub const EC_SVC_A64: u32 = 0x15 << 26; // SVC from AArch64
pub const EC_DATA_ABORT_EL1: u32 = 0x25 << 26; // Data abort from EL1
pub const EC_DATA_ABORT_EL0: u32 = 0x24 << 26; // Data abort from EL0
pub const EC_INSN_ABORT_EL1: u32 = 0x21 << 26; // Instruction abort from EL1
pub const EC_INSN_ABORT_EL0: u32 = 0x20 << 26; // Instruction abort from EL0
pub const EC_UNKNOWN: u32 = 0x00 << 26;

/// Enter an exception at EL1.
///
/// Saves current PSTATE to SPSR_EL1, PC to ELR_EL1, sets ESR/FAR,
/// masks DAIF, and jumps to VBAR_EL1 + vector_offset.
pub fn exception_entry(
    a: &mut Aarch64ArchState,
    vector_offset: u64,
    syndrome: u32,
    far: u64,
) {
    // Save PSTATE to SPSR_EL1.
    // SPSR_EL1 format: [31:28] = NZCV, [9:6] = DAIF, [4] = M[4], [3:0] = M[3:0]
    // M[3:0] encodes exception level and SP selection:
    //   EL0t  = 0b0000 (EL0 with SP_EL0)
    //   EL1t  = 0b0100 (EL1 with SP_EL0)
    //   EL1h  = 0b0101 (EL1 with SP_EL1)
    let mode = if a.current_el == 0 {
        0b0000 // EL0t
    } else if a.spsel {
        0b0101 // EL1h
    } else {
        0b0100 // EL1t
    };
    let pstate = a.nzcv | (a.daif << 6) | mode;
    a.spsr_el1 = pstate;

    // Save return address.
    a.elr_el1 = a.pc;

    // Set syndrome and fault address.
    a.esr_el1 = syndrome;
    a.far_el1 = far;

    // Mask all DAIF (D, A, I, F).
    a.daif = 0xF;

    // Switch to EL1 with SP_EL1.
    a.current_el = 1;
    a.spsel = true;

    // Jump to exception vector.
    a.pc = a.vbar_el1.wrapping_add(vector_offset);
}

/// Return from exception (ERET instruction).
///
/// Restores PSTATE from SPSR_EL1 and PC from ELR_EL1.
pub fn exception_return(a: &mut Aarch64ArchState) {
    let spsr = a.spsr_el1;

    // Restore NZCV.
    a.nzcv = spsr & 0xF000_0000;

    // Restore DAIF.
    a.daif = (spsr >> 6) & 0xF;

    // Restore exception level and SP selection from M field.
    let mode = spsr & 0x1F;
    match mode & 0xF {
        0b0000 => {
            // EL0t
            a.current_el = 0;
            a.spsel = false;
        }
        0b0100 => {
            // EL1t (EL1 using SP_EL0)
            a.current_el = 1;
            a.spsel = false;
        }
        0b0101 => {
            // EL1h (EL1 using SP_EL1)
            a.current_el = 1;
            a.spsel = true;
        }
        _ => {
            // Default to EL1h for unknown modes.
            a.current_el = 1;
            a.spsel = true;
        }
    }

    // Restore PC from ELR_EL1.
    a.pc = a.elr_el1;
}
