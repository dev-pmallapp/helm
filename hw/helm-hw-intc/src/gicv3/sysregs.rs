//! GICv3 ICC_* system register handlers.
//!
//! Called from `helm-arch` `read_sysreg` / `write_sysreg` when the encoded
//! register value matches an ICC_* register.
//!
//! Encoding: bits[15:14]=op0, [13:11]=op1, [10:7]=CRn, [6:3]=CRm, [2:0]=op2

use super::{GicV3SharedState, SPURIOUS_IRQ};

// ── Encoding helpers ──────────────────────────────────────────────────────────

/// Pack (op0, op1, crn, crm, op2) into the sysreg encoded format used by
/// helm-arch helpers.rs: bits[15:14]=op0, [13:11]=op1, [10:7]=crn, [6:3]=crm, [2:0]=op2
#[inline]
const fn enc(op0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    (op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2
}

// ICC_* register encodings (op0=3 for all EL1/EL2/EL3 system registers):
const ICC_PMR_EL1: u32 = enc(3, 0, 4, 6, 0);
const ICC_IAR1_EL1: u32 = enc(3, 0, 12, 12, 0);
const ICC_EOIR1_EL1: u32 = enc(3, 0, 12, 12, 1);
const ICC_HPPIR1_EL1: u32 = enc(3, 0, 12, 12, 2);
const ICC_BPR1_EL1: u32 = enc(3, 0, 12, 12, 3);
const ICC_CTLR_EL1: u32 = enc(3, 0, 12, 12, 4);
const ICC_SRE_EL1: u32 = enc(3, 0, 12, 12, 5);
const ICC_IGRPEN0_EL1: u32 = enc(3, 0, 12, 12, 6);
const ICC_IGRPEN1_EL1: u32 = enc(3, 0, 12, 12, 7);
const ICC_RPR_EL1: u32 = enc(3, 0, 12, 11, 3);
const ICC_DIR_EL1: u32 = enc(3, 0, 12, 11, 1);
const ICC_SGI1R_EL1: u32 = enc(3, 0, 12, 11, 5);
const ICC_ASGI1R_EL1: u32 = enc(3, 0, 12, 11, 6);
const ICC_SGI0R_EL1: u32 = enc(3, 0, 12, 11, 7);
const ICC_AP1R0_EL1: u32 = enc(3, 0, 12, 9, 0);
const ICC_AP1R1_EL1: u32 = enc(3, 0, 12, 9, 1);
const ICC_AP1R2_EL1: u32 = enc(3, 0, 12, 9, 2);
const ICC_AP1R3_EL1: u32 = enc(3, 0, 12, 9, 3);
const ICC_SRE_EL2: u32 = enc(3, 4, 12, 9, 5);
const ICC_SRE_EL3: u32 = enc(3, 6, 12, 12, 5);
const ICC_IGRPEN1_EL3: u32 = enc(3, 6, 12, 12, 7);
const ICC_CTLR_EL3: u32 = enc(3, 6, 12, 12, 4);

#[inline]
pub fn is_icc_reg(encoded: u32) -> bool {
    matches!(
        encoded,
        ICC_PMR_EL1
            | ICC_IAR1_EL1
            | ICC_EOIR1_EL1
            | ICC_HPPIR1_EL1
            | ICC_BPR1_EL1
            | ICC_CTLR_EL1
            | ICC_SRE_EL1
            | ICC_IGRPEN0_EL1
            | ICC_IGRPEN1_EL1
            | ICC_RPR_EL1
            | ICC_DIR_EL1
            | ICC_SGI1R_EL1
            | ICC_ASGI1R_EL1
            | ICC_SGI0R_EL1
            | ICC_AP1R0_EL1
            | ICC_AP1R1_EL1
            | ICC_AP1R2_EL1
            | ICC_AP1R3_EL1
            | ICC_SRE_EL2
            | ICC_SRE_EL3
            | ICC_IGRPEN1_EL3
            | ICC_CTLR_EL3
    )
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Handle an MRS to an ICC_* register.
/// Returns Some(value) if handled, None if not an ICC register (caller falls through).
pub fn icc_read(shared: &mut GicV3SharedState, cpu_idx: usize, encoded: u32) -> Option<u64> {
    Some(match encoded {
        ICC_PMR_EL1 => u64::from(shared.redists.get(cpu_idx)?.cpu_if.icc_pmr),
        ICC_IAR1_EL1 => u64::from(shared.cpu_acknowledge(cpu_idx)),
        ICC_HPPIR1_EL1 => u64::from(
            shared
                .highest_pending_for_cpu(cpu_idx)
                .map(|(id, _)| id)
                .unwrap_or(SPURIOUS_IRQ),
        ),
        ICC_BPR1_EL1 => u64::from(shared.redists.get(cpu_idx)?.cpu_if.icc_bpr1),
        ICC_RPR_EL1 => u64::from(shared.redists.get(cpu_idx)?.cpu_if.running_pri),
        ICC_CTLR_EL1 => {
            // IDbits[10:8]=7 (1020 INTIDs), A3V[6]=1, EOImode[1], CBPR[0]
            let ro_bits: u32 = (0b111 << 8) | (1 << 6);
            u64::from(ro_bits | (shared.redists.get(cpu_idx)?.cpu_if.icc_ctlr & 0x3))
        }
        ICC_SRE_EL1 | ICC_SRE_EL2 | ICC_SRE_EL3 => 0x7, // SRE=DFB=DIB=1, hardwired
        ICC_IGRPEN0_EL1 => u64::from(shared.redists.get(cpu_idx)?.cpu_if.icc_igrpen0),
        ICC_IGRPEN1_EL1 | ICC_IGRPEN1_EL3 => {
            u64::from(shared.redists.get(cpu_idx)?.cpu_if.icc_igrpen1)
        }
        ICC_AP1R0_EL1 => u64::from(shared.redists.get(cpu_idx)?.cpu_if.active_priorities[0]),
        ICC_AP1R1_EL1 => u64::from(shared.redists.get(cpu_idx)?.cpu_if.active_priorities[1]),
        ICC_AP1R2_EL1 => u64::from(shared.redists.get(cpu_idx)?.cpu_if.active_priorities[2]),
        ICC_AP1R3_EL1 => u64::from(shared.redists.get(cpu_idx)?.cpu_if.active_priorities[3]),
        ICC_CTLR_EL3 => 0,
        _ => return None,
    })
}

/// Handle an MSR to an ICC_* register.
/// Returns true if handled, false if not an ICC register.
pub fn icc_write(shared: &mut GicV3SharedState, cpu_idx: usize, encoded: u32, val: u64) -> bool {
    match encoded {
        ICC_PMR_EL1 => {
            let Some(cpu_if) = shared.redists.get_mut(cpu_idx).map(|r| &mut r.cpu_if) else {
                return false;
            };
            cpu_if.icc_pmr = val as u8;
            shared.update_irq_line(cpu_idx);
        }
        ICC_EOIR1_EL1 => {
            shared.cpu_eoi(cpu_idx, val as u32);
        }
        ICC_DIR_EL1 => {
            let Some(cpu_if) = shared.redists.get(cpu_idx).map(|r| &r.cpu_if) else {
                return false;
            };
            // Deactivate only (EOImode=1 path)
            if cpu_if.icc_ctlr & 0x2 != 0 {
                shared.cpu_deactivate(cpu_idx, val as u32);
            }
            // In EOImode=0, DIR is RAZ/WI
        }
        ICC_BPR1_EL1 => {
            let Some(cpu_if) = shared.redists.get_mut(cpu_idx).map(|r| &mut r.cpu_if) else {
                return false;
            };
            cpu_if.icc_bpr1 = (val as u8) & 0x7;
        }
        ICC_CTLR_EL1 | ICC_CTLR_EL3 => {
            let Some(cpu_if) = shared.redists.get_mut(cpu_idx).map(|r| &mut r.cpu_if) else {
                return false;
            };
            // Only EOImode[1] and CBPR[0] are RW
            cpu_if.icc_ctlr = (cpu_if.icc_ctlr & !0x3) | (val as u32 & 0x3);
        }
        ICC_SRE_EL1 | ICC_SRE_EL2 | ICC_SRE_EL3 => {
            // SRE is hardwired 1 — writes ignored
        }
        ICC_IGRPEN0_EL1 => {
            let Some(cpu_if) = shared.redists.get_mut(cpu_idx).map(|r| &mut r.cpu_if) else {
                return false;
            };
            cpu_if.icc_igrpen0 = val as u32 & 1;
            shared.update_irq_line(cpu_idx);
        }
        ICC_IGRPEN1_EL1 | ICC_IGRPEN1_EL3 => {
            let Some(cpu_if) = shared.redists.get_mut(cpu_idx).map(|r| &mut r.cpu_if) else {
                return false;
            };
            cpu_if.icc_igrpen1 = val as u32 & 1;
            shared.update_irq_line(cpu_idx);
        }
        ICC_SGI1R_EL1 | ICC_ASGI1R_EL1 => {
            // Parse ICC_SGI1R_EL1 fields
            let aff3 = ((val >> 48) & 0xFF) as u8;
            let aff2 = ((val >> 32) & 0xFF) as u8;
            let aff1 = ((val >> 16) & 0xFF) as u8;
            let rs = ((val >> 44) & 0xF) as u8;
            let irm = (val >> 40) & 1 != 0;
            let intid = ((val >> 24) & 0xF) as u32;
            let tlist = (val & 0xFFFF) as u16;
            shared.generate_sgi(cpu_idx, intid, aff3, aff2, aff1, rs, tlist, irm);
        }
        ICC_SGI0R_EL1 => {
            // Group 0 SGI — same format, treat as Group 1 in sim (no EL3/secure model)
            let aff3 = ((val >> 48) & 0xFF) as u8;
            let aff2 = ((val >> 32) & 0xFF) as u8;
            let aff1 = ((val >> 16) & 0xFF) as u8;
            let rs = ((val >> 44) & 0xF) as u8;
            let irm = (val >> 40) & 1 != 0;
            let intid = ((val >> 24) & 0xF) as u32;
            let tlist = (val & 0xFFFF) as u16;
            shared.generate_sgi(cpu_idx, intid, aff3, aff2, aff1, rs, tlist, irm);
        }
        ICC_AP1R0_EL1 => {
            let Some(cpu_if) = shared.redists.get_mut(cpu_idx).map(|r| &mut r.cpu_if) else {
                return false;
            };
            cpu_if.active_priorities[0] = val as u32;
        }
        ICC_AP1R1_EL1 => {
            let Some(cpu_if) = shared.redists.get_mut(cpu_idx).map(|r| &mut r.cpu_if) else {
                return false;
            };
            cpu_if.active_priorities[1] = val as u32;
        }
        ICC_AP1R2_EL1 => {
            let Some(cpu_if) = shared.redists.get_mut(cpu_idx).map(|r| &mut r.cpu_if) else {
                return false;
            };
            cpu_if.active_priorities[2] = val as u32;
        }
        ICC_AP1R3_EL1 => {
            let Some(cpu_if) = shared.redists.get_mut(cpu_idx).map(|r| &mut r.cpu_if) else {
                return false;
            };
            cpu_if.active_priorities[3] = val as u32;
        }
        _ => return false,
    }
    true
}
