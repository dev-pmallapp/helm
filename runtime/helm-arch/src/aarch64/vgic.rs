//! Live derivation of read-only EL2 virtual-interrupt status registers.
//!
//! GICv3 expects `ICH_MISR_EL2`, `ICH_EISR_EL2`, and `ICH_ELRSR_EL2` to be
//! read-as-derived from the writable list-register / control register state
//! (`ICH_LR<n>_EL2`, `ICH_HCR_EL2`, `ICH_VMCR_EL2`). An EL2 hypervisor
//! polls these every world-switch to decide whether to deliver a
//! maintenance interrupt to itself, so reporting them as zero — as the
//! prior stub did — masks every real maintenance event.
//!
//! This module computes those derivations from
//! [`crate::aarch64::Aarch64ArchState`]. The intent is to keep the sysreg
//! dispatcher free of the bit-field shuffling and to keep the field layout
//! co-located with focused unit tests.
//!
//! # `ICH_LR<n>_EL2` field layout
//! - bits 63:62 = `State` (00 Invalid, 01 Pending, 10 Active, 11 P+A)
//! - bit 61    = `HW`     (1 = hardware physical IRQ tracked, 0 = SW only)
//! - bit 60    = `Group`  (0 = Group 0, 1 = Group 1)
//! - bits 55:48 = `Priority`
//! - bit 41    = `EOI`    (only meaningful when `HW=0`)
//! - bits 31:0 = `vINTID`
//!
//! # `ICH_HCR_EL2` field layout (subset used here)
//! - bit 0 `En`        — virtual CPU interface enable
//! - bit 1 `UIE`       — underflow maintenance interrupt enable
//! - bit 2 `LRENPIE`   — list-register-entry-not-present maintenance enable
//! - bit 3 `NPIE`      — no-pending maintenance enable
//! - bit 4 `VGrp0EIE`  — vgroup0-enabled maintenance enable
//! - bit 5 `VGrp0DIE`  — vgroup0-disabled maintenance enable
//! - bit 6 `VGrp1EIE`  — vgroup1-enabled maintenance enable
//! - bit 7 `VGrp1DIE`  — vgroup1-disabled maintenance enable
//! - bits 31:27 `EOIcount` (5-bit saturating counter of EOI maintenance)
//!
//! # `ICH_VMCR_EL2` field layout (subset used here)
//! - bit 0 `VENG0` — virtual Group-0 enable
//! - bit 1 `VENG1` — virtual Group-1 enable

use crate::aarch64::arch_state::Aarch64ArchState;

/// Total number of list registers Helm models. Must match the array length
/// in `Aarch64ArchState::ich_lr_el2` and the `ICH_VTR_EL2.ListRegs` value
/// reported by the sysreg dispatcher.
pub const ICH_LR_COUNT: usize = 16;

// ICH_LR.State encoding.
const LR_STATE_INVALID: u64 = 0b00;
const LR_STATE_PENDING: u64 = 0b01;
#[cfg_attr(not(test), allow(dead_code))]
const LR_STATE_ACTIVE: u64 = 0b10;
const LR_STATE_PEND_ACTIVE: u64 = 0b11;

// ICH_HCR_EL2 maintenance enable bits.
const ICH_HCR_UIE: u64 = 1 << 1;
const ICH_HCR_LRENPIE: u64 = 1 << 2;
const ICH_HCR_NPIE: u64 = 1 << 3;
const ICH_HCR_VGRP0EIE: u64 = 1 << 4;
const ICH_HCR_VGRP0DIE: u64 = 1 << 5;
const ICH_HCR_VGRP1EIE: u64 = 1 << 6;
const ICH_HCR_VGRP1DIE: u64 = 1 << 7;
/// Mask covering ICH_HCR_EL2.EOIcount (`bits[31:27]`).
const ICH_HCR_EOICOUNT_MASK: u64 = 0xF800_0000;
const ICH_HCR_EOICOUNT_SHIFT: u32 = 27;

// ICH_MISR_EL2 status bits.
const ICH_MISR_EOI: u64 = 1 << 0;
const ICH_MISR_U: u64 = 1 << 1;
const ICH_MISR_LRENP: u64 = 1 << 2;
const ICH_MISR_NP: u64 = 1 << 3;
const ICH_MISR_VGRP0E: u64 = 1 << 4;
const ICH_MISR_VGRP0D: u64 = 1 << 5;
const ICH_MISR_VGRP1E: u64 = 1 << 6;
const ICH_MISR_VGRP1D: u64 = 1 << 7;

// ICH_VMCR_EL2 group enables.
const ICH_VMCR_VENG0: u64 = 1 << 0;
const ICH_VMCR_VENG1: u64 = 1 << 1;

/// Extract ICH_LR.State for a single list register value.
#[inline]
pub fn lr_state(lr: u64) -> u64 {
    (lr >> 62) & 0b11
}

/// Extract ICH_LR.HW (1 = hardware-tracked physical IRQ, 0 = software).
#[inline]
pub fn lr_hw(lr: u64) -> bool {
    (lr >> 61) & 1 != 0
}

/// Extract ICH_LR.EOI (only meaningful when `HW=0`).
#[inline]
pub fn lr_eoi_request(lr: u64) -> bool {
    (lr >> 41) & 1 != 0
}

/// True if list register `i` should request an EOI maintenance interrupt:
/// the entry has retired (`State==Invalid`), but the hypervisor asked to be
/// notified by setting `EOI=1` on a software-tracked entry.
#[inline]
fn lr_requests_eoi_maintenance(lr: u64) -> bool {
    lr_state(lr) == LR_STATE_INVALID && !lr_hw(lr) && lr_eoi_request(lr)
}

/// True if list register `i` is currently pending (either pure pending or
/// pending+active). Used by both the underflow and no-pending derivations.
#[inline]
fn lr_is_pending(lr: u64) -> bool {
    matches!(lr_state(lr), LR_STATE_PENDING | LR_STATE_PEND_ACTIVE)
}

/// True if list register `i` is occupied (any state other than Invalid).
#[inline]
pub fn lr_is_occupied(lr: u64) -> bool {
    !matches!(lr_state(lr), LR_STATE_INVALID)
}

/// Derive `ICH_ELRSR_EL2` (Empty List Register Status). Bit `i` set if
/// LR_i is empty (`State==Invalid`).
///
/// Co-located here so the sysreg dispatcher can route every read-as-derived
/// ICH register through one module; the inline body is identical to the
/// previous open-coded loop in `helpers.rs`.
#[inline]
pub fn derive_ich_elrsr_el2(a: &Aarch64ArchState) -> u64 {
    let mut mask = 0u64;
    for (i, lr) in a.ich_lr_el2.iter().enumerate() {
        if lr_state(*lr) == LR_STATE_INVALID {
            mask |= 1u64 << i;
        }
    }
    mask
}

/// Derive `ICH_EISR_EL2` (EOIed list register status).
///
/// Bit `i` is set when LR_i has retired into the Invalid state but is
/// still asking the hypervisor for an EOI maintenance interrupt
/// (`HW=0 && EOI=1`).
#[inline]
pub fn derive_ich_eisr_el2(a: &Aarch64ArchState) -> u64 {
    let mut mask = 0u64;
    for (i, lr) in a.ich_lr_el2.iter().enumerate() {
        if lr_requests_eoi_maintenance(*lr) {
            mask |= 1u64 << i;
        }
    }
    mask
}

/// Derive `ICH_MISR_EL2` (Maintenance Interrupt Status Register).
///
/// Each bit reflects whether the corresponding maintenance condition is
/// both *enabled* in `ICH_HCR_EL2` and *currently true* given the live
/// LR / VMCR state. The hypervisor reads MISR from its IRQ handler to
/// dispatch on the cause; reporting zero before this slice meant every
/// real maintenance interrupt looked spurious.
#[inline]
pub fn derive_ich_misr_el2(a: &Aarch64ArchState) -> u64 {
    let hcr = a.ich_hcr_el2;
    let vmcr = a.ich_vmcr_el2;
    let mut misr = 0u64;

    // EOI: any LR is asking for an EOI maintenance — gated by HCR.En via
    // the rule that maintenance interrupts only fire while the virtual
    // CPU interface is enabled. This bit is *not* gated by a dedicated
    // HCR enable; the EOI request itself comes from the LR.
    let any_eoi_pending = a.ich_lr_el2.iter().any(|lr| lr_requests_eoi_maintenance(*lr));
    if any_eoi_pending {
        misr |= ICH_MISR_EOI;
    }

    // U (Underflow): UIE set, and the number of LRs currently in Pending
    // state (alone or as P+A) is less than 2 — the architectural threshold
    // that triggers underflow.
    if hcr & ICH_HCR_UIE != 0 {
        let pending_count = a.ich_lr_el2.iter().filter(|lr| lr_is_pending(**lr)).count();
        if pending_count < 2 {
            misr |= ICH_MISR_U;
        }
    }

    // LRENP (List Register Entry Not Present): LRENPIE set, and HCR.EOIcount
    // is non-zero. EOIcount tracks how many EOIs have been received with
    // no matching LR, so a non-zero value means the guest asked the hyper
    // for help refilling list registers.
    if hcr & ICH_HCR_LRENPIE != 0 && (hcr & ICH_HCR_EOICOUNT_MASK) != 0 {
        misr |= ICH_MISR_LRENP;
    }

    // NP (No Pending): NPIE set, and zero LRs are in pending state.
    if hcr & ICH_HCR_NPIE != 0
        && !a.ich_lr_el2.iter().any(|lr| lr_is_pending(*lr))
    {
        misr |= ICH_MISR_NP;
    }

    // VGrp0E / VGrp0D: VENG0 transitions vs. the corresponding xIE bits.
    let veng0 = vmcr & ICH_VMCR_VENG0 != 0;
    if hcr & ICH_HCR_VGRP0EIE != 0 && veng0 {
        misr |= ICH_MISR_VGRP0E;
    }
    if hcr & ICH_HCR_VGRP0DIE != 0 && !veng0 {
        misr |= ICH_MISR_VGRP0D;
    }

    // VGrp1E / VGrp1D: same as above for Group 1.
    let veng1 = vmcr & ICH_VMCR_VENG1 != 0;
    if hcr & ICH_HCR_VGRP1EIE != 0 && veng1 {
        misr |= ICH_MISR_VGRP1E;
    }
    if hcr & ICH_HCR_VGRP1DIE != 0 && !veng1 {
        misr |= ICH_MISR_VGRP1D;
    }

    misr
}

/// Read the live `ICH_HCR_EL2.EOIcount` field. Useful for tests that want
/// to check the cached count without re-deriving the whole MISR.
#[inline]
pub fn ich_hcr_eoicount(a: &Aarch64ArchState) -> u8 {
    ((a.ich_hcr_el2 & ICH_HCR_EOICOUNT_MASK) >> ICH_HCR_EOICOUNT_SHIFT) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lr(state: u64, hw: bool, eoi: bool) -> u64 {
        let mut v = (state & 0b11) << 62;
        if hw {
            v |= 1 << 61;
        }
        if eoi {
            v |= 1 << 41;
        }
        v
    }

    #[test]
    fn elrsr_marks_only_invalid_lrs() {
        let mut a = Aarch64ArchState::new();
        a.ich_lr_el2[0] = lr(LR_STATE_INVALID, false, false);
        a.ich_lr_el2[1] = lr(LR_STATE_PENDING, false, false);
        a.ich_lr_el2[2] = lr(LR_STATE_ACTIVE, false, false);
        a.ich_lr_el2[3] = lr(LR_STATE_PEND_ACTIVE, false, false);

        // bits 0, 4..15 should be set (Invalid); 1, 2, 3 cleared.
        let mask = derive_ich_elrsr_el2(&a);
        assert_eq!(mask & 0xF, 0b0001);
        assert_eq!(mask >> 4, (1u64 << 12) - 1);
    }

    #[test]
    fn eisr_only_for_invalid_sw_lrs_with_eoi_set() {
        let mut a = Aarch64ArchState::new();
        a.ich_lr_el2[0] = lr(LR_STATE_INVALID, false, true); // qualifies
        a.ich_lr_el2[1] = lr(LR_STATE_INVALID, true, true); // HW=1 disqualifies
        a.ich_lr_el2[2] = lr(LR_STATE_PENDING, false, true); // not Invalid
        a.ich_lr_el2[3] = lr(LR_STATE_INVALID, false, false); // EOI=0

        assert_eq!(derive_ich_eisr_el2(&a), 0b0001);
    }

    #[test]
    fn misr_eoi_bit_set_when_any_lr_requests_maintenance() {
        let mut a = Aarch64ArchState::new();
        // No LRs requesting => EOI bit clear.
        assert_eq!(derive_ich_misr_el2(&a) & ICH_MISR_EOI, 0);
        a.ich_lr_el2[5] = lr(LR_STATE_INVALID, false, true);
        assert_eq!(derive_ich_misr_el2(&a) & ICH_MISR_EOI, ICH_MISR_EOI);
    }

    #[test]
    fn misr_underflow_requires_uie_and_few_pending() {
        let mut a = Aarch64ArchState::new();
        // UIE clear: no underflow even with no pending LRs.
        a.ich_hcr_el2 = 0;
        assert_eq!(derive_ich_misr_el2(&a) & ICH_MISR_U, 0);

        // UIE set, zero pending LRs => underflow.
        a.ich_hcr_el2 = ICH_HCR_UIE;
        assert_eq!(derive_ich_misr_el2(&a) & ICH_MISR_U, ICH_MISR_U);

        // One pending LR is still below the threshold of 2 => underflow.
        a.ich_lr_el2[0] = lr(LR_STATE_PENDING, false, false);
        assert_eq!(derive_ich_misr_el2(&a) & ICH_MISR_U, ICH_MISR_U);

        // Two pending LRs clears the underflow bit.
        a.ich_lr_el2[1] = lr(LR_STATE_PENDING, false, false);
        assert_eq!(derive_ich_misr_el2(&a) & ICH_MISR_U, 0);
    }

    #[test]
    fn misr_lrenp_requires_lrenpie_and_nonzero_eoicount() {
        let mut a = Aarch64ArchState::new();
        // EOIcount non-zero but LRENPIE clear: no bit.
        a.ich_hcr_el2 = 1u64 << ICH_HCR_EOICOUNT_SHIFT;
        assert_eq!(derive_ich_misr_el2(&a) & ICH_MISR_LRENP, 0);

        // LRENPIE set + EOIcount non-zero => bit set.
        a.ich_hcr_el2 = ICH_HCR_LRENPIE | (3u64 << ICH_HCR_EOICOUNT_SHIFT);
        assert_eq!(derive_ich_misr_el2(&a) & ICH_MISR_LRENP, ICH_MISR_LRENP);
        assert_eq!(ich_hcr_eoicount(&a), 3);

        // LRENPIE set but EOIcount zero => bit clear.
        a.ich_hcr_el2 = ICH_HCR_LRENPIE;
        assert_eq!(derive_ich_misr_el2(&a) & ICH_MISR_LRENP, 0);
    }

    #[test]
    fn misr_np_requires_npie_and_no_pending_lrs() {
        let mut a = Aarch64ArchState::new();
        a.ich_hcr_el2 = ICH_HCR_NPIE;
        // No pending LRs => bit set.
        assert_eq!(derive_ich_misr_el2(&a) & ICH_MISR_NP, ICH_MISR_NP);
        // Pending LR clears the bit.
        a.ich_lr_el2[0] = lr(LR_STATE_PEND_ACTIVE, false, false);
        assert_eq!(derive_ich_misr_el2(&a) & ICH_MISR_NP, 0);
    }

    #[test]
    fn misr_vgroup_bits_track_vmcr_enable() {
        let mut a = Aarch64ArchState::new();
        // VGrp0E gated by EIE && VENG0=1.
        a.ich_hcr_el2 = ICH_HCR_VGRP0EIE | ICH_HCR_VGRP1DIE;
        a.ich_vmcr_el2 = ICH_VMCR_VENG0; // VENG0=1, VENG1=0

        let misr = derive_ich_misr_el2(&a);
        assert_eq!(misr & ICH_MISR_VGRP0E, ICH_MISR_VGRP0E);
        assert_eq!(misr & ICH_MISR_VGRP1D, ICH_MISR_VGRP1D);
        assert_eq!(misr & (ICH_MISR_VGRP0D | ICH_MISR_VGRP1E), 0);

        // Flip VMCR enables: VGrp0E should drop, VGrp1D should drop too
        // because now VENG1=1.
        a.ich_vmcr_el2 = ICH_VMCR_VENG1;
        let misr = derive_ich_misr_el2(&a);
        assert_eq!(misr & ICH_MISR_VGRP0E, 0);
        assert_eq!(misr & ICH_MISR_VGRP1D, 0);
    }

    #[test]
    fn lr_helpers_decode_state_hw_eoi() {
        let v = lr(LR_STATE_PEND_ACTIVE, true, true);
        assert_eq!(lr_state(v), LR_STATE_PEND_ACTIVE);
        assert!(lr_hw(v));
        assert!(lr_eoi_request(v));
        assert!(lr_is_pending(v));
        assert!(lr_is_occupied(v));
    }

    fn lr_full(state: u64, group: u8, prio: u8, vintid: u32) -> u64 {
        let mut v = (state & 0b11) << 62;
        v |= ((group as u64) & 0x1) << 60;
        v |= ((prio as u64) & 0xFF) << 48;
        v |= vintid as u64;
        v
    }

    #[test]
    fn maintenance_pending_requires_hcr_enabled() {
        let mut a = Aarch64ArchState::new();
        // Set up an EOI-pending LR but leave HCR.En clear.
        a.ich_lr_el2[0] = lr(LR_STATE_INVALID, false, true);
        assert!(!maintenance_pending(&a));
        // En=1 surfaces the maintenance condition.
        a.ich_hcr_el2 = 1;
        assert!(maintenance_pending(&a));
        // En=1 but no qualifying condition => quiescent.
        a.ich_lr_el2[0] = 0;
        assert!(!maintenance_pending(&a));
    }

    #[test]
    fn next_pending_virtual_irq_picks_lowest_priority_first() {
        let mut a = Aarch64ArchState::new();
        a.ich_hcr_el2 = 1; // En=1
        // VENG0=1, VPMR=0xF0 (only priorities < 0xF0 fire).
        a.ich_vmcr_el2 = ICH_VMCR_VENG0 | (0xF0 << 24);

        a.ich_lr_el2[3] = lr_full(LR_STATE_PENDING, 0, 0x80, 33);
        a.ich_lr_el2[5] = lr_full(LR_STATE_PENDING, 0, 0x40, 42);
        a.ich_lr_el2[7] = lr_full(LR_STATE_PENDING, 0, 0xF8, 99); // masked

        let pick = next_pending_virtual_irq(&a).unwrap();
        assert_eq!(pick, (5, 42));
    }

    #[test]
    fn next_pending_virtual_irq_respects_group_disable() {
        let mut a = Aarch64ArchState::new();
        a.ich_hcr_el2 = 1;
        // Only Group 1 enabled.
        a.ich_vmcr_el2 = ICH_VMCR_VENG1 | (0xFF << 24);
        a.ich_lr_el2[0] = lr_full(LR_STATE_PENDING, 0, 0x10, 33); // Group 0
        a.ich_lr_el2[1] = lr_full(LR_STATE_PENDING, 1, 0x80, 44); // Group 1

        let pick = next_pending_virtual_irq(&a).unwrap();
        assert_eq!(pick.1, 44);
    }

    #[test]
    fn next_pending_virtual_irq_skips_non_pending_states() {
        let mut a = Aarch64ArchState::new();
        a.ich_hcr_el2 = 1;
        a.ich_vmcr_el2 = ICH_VMCR_VENG0 | ICH_VMCR_VENG1 | (0xFF << 24);
        a.ich_lr_el2[0] = lr_full(LR_STATE_INVALID, 0, 0x10, 33);
        a.ich_lr_el2[1] = lr_full(LR_STATE_ACTIVE, 0, 0x10, 44);
        a.ich_lr_el2[2] = lr_full(LR_STATE_PEND_ACTIVE, 0, 0x10, 55);
        // P+A is "active+pending" but the spec says delivery happens via the
        // pure Pending path; a P+A entry is already running on the guest.
        assert!(next_pending_virtual_irq(&a).is_none());
    }

    #[test]
    fn next_pending_virtual_irq_none_when_hcr_disabled() {
        let mut a = Aarch64ArchState::new();
        a.ich_hcr_el2 = 0;
        a.ich_vmcr_el2 = ICH_VMCR_VENG0 | (0xFF << 24);
        a.ich_lr_el2[0] = lr_full(LR_STATE_PENDING, 0, 0x10, 33);
        assert!(next_pending_virtual_irq(&a).is_none());
    }

    #[test]
    fn maintenance_irq_ppi_is_arm_virt_default() {
        // The arm-virt board fixes the EL2 maintenance interrupt at PPI25.
        // Pin the constant so future board-side wiring stays in sync.
        assert_eq!(MAINTENANCE_IRQ_PPI, 25);
    }
}
/// Architectural ID of the GICv3 EL2 maintenance interrupt PPI on
/// `arm-virt`. See "Arm GIC Architecture Specification" rev G v1
/// (PE physical EL2 maintenance interrupt is a PPI; the platform
/// fixes it at this ID).
pub const MAINTENANCE_IRQ_PPI: u32 = 25;

/// True if the EL2 virtual CPU interface is enabled and any maintenance
/// condition is currently asserted. The companion driver (engine /
/// arm-virt board) is expected to assert the maintenance PPI line when
/// this transitions false → true.
///
/// `ICH_HCR_EL2.En` (bit 0) gates every maintenance source: a guest
/// running with the virtual interface disabled does not get spurious
/// maintenance interrupts even when the LR / VMCR state would otherwise
/// satisfy the conditions.
#[inline]
pub fn maintenance_pending(a: &Aarch64ArchState) -> bool {
    const ICH_HCR_EN: u64 = 1 << 0;
    if a.ich_hcr_el2 & ICH_HCR_EN == 0 {
        return false;
    }
    derive_ich_misr_el2(a) != 0
}

/// Read priority field (bits 55:48) from an LR.
#[inline]
pub fn lr_priority(lr: u64) -> u8 {
    ((lr >> 48) & 0xFF) as u8
}

/// Read group field (bit 60) from an LR.
#[inline]
pub fn lr_group(lr: u64) -> u8 {
    ((lr >> 60) & 0x1) as u8
}

/// Read vINTID (bits 31:0) from an LR.
#[inline]
pub fn lr_vintid(lr: u64) -> u32 {
    (lr & 0xFFFF_FFFF) as u32
}

/// Highest-priority list register that is currently *eligible* to be
/// delivered as a virtual IRQ to the guest, plus its index. Returns
/// `None` if no entry qualifies.
///
/// Eligibility rules (subset that the EL1 guest IRQ delivery path needs):
/// * `ICH_HCR_EL2.En` is set,
/// * the LR is in `Pending` state (Pend+Active counts as already-active),
/// * the LR's group is enabled in `ICH_VMCR_EL2` (`VENG0`/`VENG1`),
/// * the LR's priority is *numerically less* than the unmasked threshold
///   reported by `ICH_VMCR_EL2.VPMR`.
///
/// Tie-break is by lower priority value, then by lower LR index — which
/// matches the deterministic selection rule the hypervisor already
/// programs in.
pub fn next_pending_virtual_irq(a: &Aarch64ArchState) -> Option<(usize, u32)> {
    const ICH_HCR_EN: u64 = 1 << 0;
    if a.ich_hcr_el2 & ICH_HCR_EN == 0 {
        return None;
    }
    let vmcr = a.ich_vmcr_el2;
    let veng0 = vmcr & ICH_VMCR_VENG0 != 0;
    let veng1 = vmcr & ICH_VMCR_VENG1 != 0;
    let vpmr = ((vmcr >> 24) & 0xFF) as u8;

    let mut best: Option<(usize, u8, u32)> = None;
    for (i, lr) in a.ich_lr_el2.iter().enumerate() {
        if lr_state(*lr) != LR_STATE_PENDING {
            continue;
        }
        let group = lr_group(*lr);
        let group_enabled = if group == 0 { veng0 } else { veng1 };
        if !group_enabled {
            continue;
        }
        let prio = lr_priority(*lr);
        if prio >= vpmr {
            continue;
        }
        let vintid = lr_vintid(*lr);
        match best {
            None => best = Some((i, prio, vintid)),
            Some((_, best_prio, _)) if prio < best_prio => {
                best = Some((i, prio, vintid));
            }
            _ => {}
        }
    }
    best.map(|(idx, _, vintid)| (idx, vintid))
}
