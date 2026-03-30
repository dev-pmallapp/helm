//! AArch64 exception entry and return.

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
pub const EC_HVC_A64: u32 = 0x16 << 26; // HVC from AArch64
pub const EC_SMC_A64: u32 = 0x17 << 26; // SMC from AArch64
pub const EC_SYSREG_TRAP: u32 = 0x18 << 26; // trapped MSR/MRS
pub const EC_DATA_ABORT_EL1: u32 = 0x25 << 26; // Data abort from EL1
pub const EC_DATA_ABORT_EL0: u32 = 0x24 << 26; // Data abort from EL0
pub const EC_INSN_ABORT_EL1: u32 = 0x21 << 26; // Instruction abort from EL1
pub const EC_INSN_ABORT_EL0: u32 = 0x20 << 26; // Instruction abort from EL0
pub const EC_UNKNOWN: u32 = 0x00 << 26;
pub const EC_BRK_A64: u32 = 0x3C << 26; // BRK instruction from AArch64

const HCR_TSC: u64 = 1 << 19;
const HCR_TVM: u64 = 1 << 26;
const HCR_TGE: u64 = 1 << 27;
const HCR_IMO: u64 = 1 << 4;
const SCR_IRQ: u64 = 1 << 1;

fn source_mode(a: &Aarch64ArchState) -> u32 {
    match (a.current_el, a.spsel) {
        (0, _) => 0b0000,     // EL0t
        (1, false) => 0b0100, // EL1t
        (1, true) => 0b0101,  // EL1h
        (2, false) => 0b1000, // EL2t
        (2, true) => 0b1001,  // EL2h
        (3, false) => 0b1100, // EL3t
        (3, true) => 0b1101,  // EL3h
        _ => 0b0000,
    }
}

fn save_pstate(a: &Aarch64ArchState) -> u32 {
    a.nzcv | ((a.daif & 0xF) << 6) | source_mode(a)
}

fn restore_pstate(a: &mut Aarch64ArchState, spsr: u32) {
    a.nzcv = spsr & 0xF000_0000;
    a.daif = (spsr >> 6) & 0xF;
    a.current_el = ((spsr >> 2) & 0x3) as u8;
    a.spsel = (spsr & 1) != 0;
}

fn vector_offset(a: &Aarch64ArchState, target_el: u8) -> u64 {
    if a.current_el == target_el {
        if a.spsel {
            SYNC_EL1_SP1
        } else {
            SYNC_EL1_SP0
        }
    } else {
        SYNC_EL0_64
    }
}

pub fn irq_vector_offset(a: &Aarch64ArchState, target_el: u8) -> u64 {
    if a.current_el == target_el {
        if a.spsel {
            IRQ_EL1_SP1
        } else {
            IRQ_EL1_SP0
        }
    } else {
        IRQ_EL0_64
    }
}

fn return_address(a: &Aarch64ArchState, syndrome: u32) -> u64 {
    // ARM ARM D1.10.1: ELR = PC of the instruction that caused the exception.
    // For SVC (EC=0x15) / HVC (0x16) / SMC (0x17), the ELR points to the
    // instruction *after* the trap (caller wants to resume past the call).
    // For BRK (EC=0x3C) and all other synchronous exceptions, ELR = faulting PC.
    // The exception handler (e.g. do_debug_exception for WARN_ON BRK) advances
    // ELR itself when it wants to skip the BRK.
    match syndrome >> 26 {
        0x15 | 0x16 | 0x17 => a.pc.wrapping_add(4),
        _ => a.pc,
    }
}

pub fn route_sync_exception(a: &Aarch64ArchState, syndrome: u32) -> u8 {
    match a.current_el {
        0 => {
            if a.hcr_el2 & HCR_TGE != 0 {
                2
            } else {
                1
            }
        }
        1 => match syndrome {
            EC_HVC_A64 => 2,
            EC_SMC_A64 => {
                if a.hcr_el2 & HCR_TSC != 0 {
                    2
                } else {
                    3
                }
            }
            EC_SYSREG_TRAP => {
                if a.hcr_el2 & HCR_TVM != 0 {
                    2
                } else {
                    1
                }
            }
            _ => 1,
        },
        2 => match syndrome {
            EC_SMC_A64 => 3,
            _ => 2,
        },
        3 => 3,
        _ => 1,
    }
}

pub fn route_physical_irq(a: &Aarch64ArchState) -> u8 {
    if a.current_el < 3 && (a.scr_el3 & SCR_IRQ) != 0 {
        return 3;
    }

    match a.current_el {
        0 => {
            if a.hcr_el2 & HCR_TGE != 0 {
                2
            } else {
                1
            }
        }
        1 => {
            if a.hcr_el2 & HCR_IMO != 0 {
                2
            } else {
                1
            }
        }
        2 => 2,
        _ => 3,
    }
}

/// Enter a synchronous exception at the chosen target EL.
pub fn exception_entry(a: &mut Aarch64ArchState, target_el: u8, syndrome: u32, far: u64) {
    exception_entry_with_offset(a, target_el, vector_offset(a, target_el), syndrome, far);
}

pub fn exception_entry_with_offset(
    a: &mut Aarch64ArchState,
    target_el: u8,
    offset: u64,
    syndrome: u32,
    far: u64,
) {
    let saved_pstate = save_pstate(a);
    let elr = return_address(a, syndrome);

    match target_el {
        2 => {
            a.spsr_el2 = saved_pstate;
            a.elr_el2 = elr;
            a.esr_el2 = syndrome;
            a.far_el2 = far;
            a.pc = a.vbar_el2.wrapping_add(offset);
        }
        3 => {
            a.spsr_el3 = saved_pstate;
            a.elr_el3 = elr;
            a.esr_el3 = syndrome;
            a.far_el3 = far;
            a.pc = a.vbar_el3.wrapping_add(offset);
        }
        _ => {
            a.spsr_el1 = saved_pstate;
            a.elr_el1 = elr;
            a.esr_el1 = syndrome;
            a.far_el1 = far;
            a.pc = a.vbar_el1.wrapping_add(offset);
        }
    }

    a.daif = 0xF;
    a.current_el = if matches!(target_el, 2 | 3) {
        target_el
    } else {
        1
    };
    a.spsel = true;
}

/// Enter an EL1 exception using an explicit vector offset.
///
/// FS mode still computes IRQ vs sync vector offsets externally; keep that
/// surface available while the generalized EL2/EL3 routing uses
/// [`exception_entry`].
pub fn exception_entry_el1(a: &mut Aarch64ArchState, vector_offset: u64, syndrome: u32, far: u64) {
    let saved_pstate = save_pstate(a);
    let elr = return_address(a, syndrome);
    a.spsr_el1 = saved_pstate;
    a.elr_el1 = elr;
    a.esr_el1 = syndrome;
    a.far_el1 = far;
    a.daif = 0xF;
    a.current_el = 1;
    a.spsel = true;
    a.pc = a.vbar_el1.wrapping_add(vector_offset);
}

/// Return from exception (ERET instruction).
///
/// Restores PSTATE and PC from the current EL's ELR/SPSR.
pub fn exception_return(a: &mut Aarch64ArchState) {
    let (pc, spsr) = match a.current_el {
        2 => (a.elr_el2, a.spsr_el2),
        3 => (a.elr_el3, a.spsr_el3),
        _ => (a.elr_el1, a.spsr_el1),
    };
    restore_pstate(a, spsr);
    a.pc = pc;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_physical_irq_from_el1_to_el2_with_imo() {
        let mut a = Aarch64ArchState::new();
        a.current_el = 1;
        a.hcr_el2 = HCR_IMO;
        assert_eq!(route_physical_irq(&a), 2);
    }

    #[test]
    fn route_physical_irq_to_el3_with_scr_irq() {
        let mut a = Aarch64ArchState::new();
        a.current_el = 1;
        a.scr_el3 = SCR_IRQ;
        assert_eq!(route_physical_irq(&a), 3);
    }

    #[test]
    fn irq_vector_offset_uses_current_el_slot() {
        let mut a = Aarch64ArchState::new();
        a.current_el = 2;
        a.spsel = true;
        assert_eq!(irq_vector_offset(&a, 2), IRQ_EL1_SP1);
    }

    #[test]
    fn irq_vector_offset_uses_lower_el_slot() {
        let mut a = Aarch64ArchState::new();
        a.current_el = 0;
        assert_eq!(irq_vector_offset(&a, 2), IRQ_EL0_64);
    }

    #[test]
    fn exception_entry_with_offset_targets_el2() {
        let mut a = Aarch64ArchState::new();
        a.current_el = 1;
        a.spsel = true;
        a.nzcv = 0x6000_0000;
        a.daif = 0x2;
        a.pc = 0x4000;
        a.vbar_el2 = 0x80_000;

        exception_entry_with_offset(&mut a, 2, IRQ_EL0_64, 0, 0);

        assert_eq!(a.current_el, 2);
        assert_eq!(a.pc, 0x80_000 + IRQ_EL0_64);
        assert_eq!(a.elr_el2, 0x4000);
        assert_eq!((a.spsr_el2 >> 2) & 0x3, 1);
    }
}
