//! AArch64 exception entry and return.

use super::arch_state::Aarch64ArchState;

/// Cause that triggered an exception entry.
///
/// Distinguishes synchronous exceptions (HVC/SVC/SMC/aborts/sysreg traps)
/// from physical IRQ delivery so plugins can subscribe to either flavour
/// without re-decoding the syndrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionCause {
    /// Synchronous exception with the given ESR_ELx syndrome.
    Sync,
    /// Physical IRQ delivered via the IRQ vector slot.
    Irq,
}

/// Captured by [`exception_entry_with_offset`] / [`exception_entry_el1`] each
/// time an EL transition happens, so the engine can broadcast a single
/// `on_exception` plugin/probe event without instrumenting every call site.
///
/// The fields snapshot the entry state *after* PSTATE has been saved and
/// the new PC has been computed: `from_el` is the originating EL,
/// `target_el` is the entered EL, `vector_pc` is the PC of the vector
/// dispatched to (i.e. `VBAR_ELx + offset`), `elr` is the saved return
/// address, `spsr` is the saved PSTATE encoding, `esr` is the syndrome
/// (`0` for IRQ entries that don't update ESR), and `far` is the saved
/// fault address (`0` if not applicable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExceptionEvent {
    pub cause: ExceptionCause,
    pub from_el: u8,
    pub target_el: u8,
    pub vector_pc: u64,
    pub elr: u64,
    pub spsr: u32,
    pub esr: u32,
    pub far: u64,
}

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
pub const EC_FP_SIMD_TRAP: u32 = 0x07 << 26; // trapped FP/SIMD (CPTR_EL2.TFP)
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
    exception_entry_with_offset_kind(a, target_el, offset, syndrome, far, ExceptionCause::Sync);
}

/// Enter a physical IRQ exception at the chosen target EL using the given
/// IRQ vector offset.
///
/// Functionally identical to [`exception_entry_with_offset`] but tags the
/// recorded [`ExceptionEvent`] as [`ExceptionCause::Irq`] so plugins can
/// distinguish IRQ delivery from synchronous traps.
pub fn irq_entry_with_offset(a: &mut Aarch64ArchState, target_el: u8, offset: u64) {
    exception_entry_with_offset_kind(a, target_el, offset, 0, 0, ExceptionCause::Irq);
}

fn exception_entry_with_offset_kind(
    a: &mut Aarch64ArchState,
    target_el: u8,
    offset: u64,
    syndrome: u32,
    far: u64,
    cause: ExceptionCause,
) {
    let from_el = a.current_el;
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

    a.pending_exception_event = Some(ExceptionEvent {
        cause,
        from_el,
        target_el: a.current_el,
        vector_pc: a.pc,
        elr,
        spsr: saved_pstate,
        esr: syndrome,
        far,
    });
}

/// Enter an EL1 exception using an explicit vector offset.
///
/// FS mode still computes IRQ vs sync vector offsets externally; keep that
/// surface available while the generalized EL2/EL3 routing uses
/// [`exception_entry`].
pub fn exception_entry_el1(a: &mut Aarch64ArchState, vector_offset: u64, syndrome: u32, far: u64) {
    let from_el = a.current_el;
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

    a.pending_exception_event = Some(ExceptionEvent {
        cause: ExceptionCause::Sync,
        from_el,
        target_el: 1,
        vector_pc: a.pc,
        elr,
        spsr: saved_pstate,
        esr: syndrome,
        far,
    });
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

    #[test]
    fn exception_entry_records_sync_event_with_imm16() {
        let mut a = Aarch64ArchState::new();
        a.current_el = 1;
        a.spsel = true;
        a.pc = 0xfff0_0010;
        a.vbar_el2 = 0x4000_0000;

        // EC=0x16 (HVC) syndrome with IL=1 and imm16=0x42.
        let syndrome = (0x16u32 << 26) | (1 << 25) | 0x42;
        exception_entry(&mut a, 2, syndrome, 0);

        let event = a.pending_exception_event.expect("event recorded");
        assert_eq!(event.cause, ExceptionCause::Sync);
        assert_eq!(event.from_el, 1);
        assert_eq!(event.target_el, 2);
        // current_el (1) != target_el (2), so the vector is the lower-EL sync slot.
        assert_eq!(event.vector_pc, 0x4000_0000 + SYNC_EL0_64);
        // HVC is treated as a "skip-past-trap" syndrome so ELR == PC + 4.
        assert_eq!(event.elr, 0xfff0_0014);
        assert_eq!(event.esr, syndrome);
        // The recorded SPSR encodes the originating EL (EL1h = 0b0101).
        assert_eq!(event.spsr & 0xF, 0b0101);
    }

    #[test]
    fn irq_entry_records_irq_event_distinct_from_sync() {
        let mut a = Aarch64ArchState::new();
        a.current_el = 0;
        a.spsel = false;
        a.pc = 0x100;
        a.vbar_el2 = 0x4000_0000;

        irq_entry_with_offset(&mut a, 2, IRQ_EL0_64);

        let event = a.pending_exception_event.expect("event recorded");
        assert_eq!(event.cause, ExceptionCause::Irq);
        assert_eq!(event.from_el, 0);
        assert_eq!(event.target_el, 2);
        assert_eq!(event.vector_pc, 0x4000_0000 + IRQ_EL0_64);
        // Faulting PC is preserved on IRQ entry (no +4 skip).
        assert_eq!(event.elr, 0x100);
        assert_eq!(event.esr, 0);
    }

    #[test]
    fn exception_event_is_one_shot() {
        let mut a = Aarch64ArchState::new();
        a.current_el = 1;
        a.spsel = true;
        exception_entry(&mut a, 2, EC_HVC_A64, 0);
        assert!(a.pending_exception_event.is_some());
        // Drainer takes the event; subsequent reads must see None.
        let _ = a.pending_exception_event.take();
        assert!(a.pending_exception_event.is_none());
    }
}
