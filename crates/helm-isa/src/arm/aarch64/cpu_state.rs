//! CpuState trait implementation for AArch64.

use crate::arm::regs::Aarch64Regs;
use helm_core::cpu::CpuState;
use helm_core::types::{Addr, RegId};

/// AArch64 CPU state implementing the `CpuState` trait.
///
/// Wraps `Aarch64Regs` and provides the trait-based interface.
pub struct Aarch64CpuState {
    pub regs: Aarch64Regs,
    pub halted: bool,
    pub exit_code: u64,
    pub insn_count: u64,
}

impl Aarch64CpuState {
    pub fn new() -> Self {
        Self {
            regs: Aarch64Regs::default(),
            halted: false,
            exit_code: 0,
            insn_count: 0,
        }
    }
}

impl Default for Aarch64CpuState {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuState for Aarch64CpuState {
    fn pc(&self) -> Addr {
        self.regs.pc
    }

    fn set_pc(&mut self, pc: Addr) {
        self.regs.pc = pc;
    }

    /// GPR mapping: 0-30 = X0-X30, 31 = SP.
    fn gpr(&self, id: RegId) -> u64 {
        match id {
            0..=30 => self.regs.x[id as usize],
            31 => self.regs.sp,
            _ => 0,
        }
    }

    fn set_gpr(&mut self, id: RegId, val: u64) {
        match id {
            0..=30 => self.regs.x[id as usize] = val,
            31 => self.regs.sp = val,
            _ => {}
        }
    }

    /// System register access via 16-bit encoding.
    /// Uses direct field mapping for known registers.
    fn sysreg(&self, enc: u32) -> u64 {
        use crate::arm::aarch64::sysreg::*;
        match enc {
            SCTLR_EL1 => self.regs.sctlr_el1,
            TCR_EL1 => self.regs.tcr_el1,
            TTBR0_EL1 => self.regs.ttbr0_el1,
            TTBR1_EL1 => self.regs.ttbr1_el1,
            MAIR_EL1 => self.regs.mair_el1,
            VBAR_EL1 => self.regs.vbar_el1,
            TPIDR_EL0 => self.regs.tpidr_el0,
            TPIDR_EL1 => self.regs.tpidr_el1,
            ESR_EL1 => self.regs.esr_el1 as u64,
            FAR_EL1 => self.regs.far_el1,
            ELR_EL1 => self.regs.elr_el1,
            SPSR_EL1 => self.regs.spsr_el1 as u64,
            SP_EL1 => self.regs.sp_el1,
            CPACR_EL1 => self.regs.cpacr_el1,
            HCR_EL2 => self.regs.hcr_el2,
            SCR_EL3 => self.regs.scr_el3,
            CNTVCT_EL0 => self.regs.cntvct_el0,
            CNTFRQ_EL0 => self.regs.cntfrq_el0,
            MIDR_EL1 => self.regs.midr_el1,
            MPIDR_EL1 => self.regs.mpidr_el1,
            ID_AA64PFR0_EL1 => self.regs.id_aa64pfr0_el1,
            ID_AA64MMFR0_EL1 => self.regs.id_aa64mmfr0_el1,
            ID_AA64ISAR0_EL1 => self.regs.id_aa64isar0_el1,
            CTR_EL0 => self.regs.ctr_el0,
            DCZID_EL0 => self.regs.dczid_el0,
            _ => 0,
        }
    }

    fn set_sysreg(&mut self, enc: u32, val: u64) {
        use crate::arm::aarch64::sysreg::*;
        match enc {
            SCTLR_EL1 => self.regs.sctlr_el1 = val,
            TCR_EL1 => self.regs.tcr_el1 = val,
            TTBR0_EL1 => self.regs.ttbr0_el1 = val,
            TTBR1_EL1 => self.regs.ttbr1_el1 = val,
            MAIR_EL1 => self.regs.mair_el1 = val,
            VBAR_EL1 => self.regs.vbar_el1 = val,
            TPIDR_EL0 => self.regs.tpidr_el0 = val,
            TPIDR_EL1 => self.regs.tpidr_el1 = val,
            ESR_EL1 => self.regs.esr_el1 = val as u32,
            FAR_EL1 => self.regs.far_el1 = val,
            ELR_EL1 => self.regs.elr_el1 = val,
            SPSR_EL1 => self.regs.spsr_el1 = val as u32,
            SP_EL1 => self.regs.sp_el1 = val,
            CPACR_EL1 => self.regs.cpacr_el1 = val,
            HCR_EL2 => self.regs.hcr_el2 = val,
            SCR_EL3 => self.regs.scr_el3 = val,
            CNTVCT_EL0 => self.regs.cntvct_el0 = val,
            CNTFRQ_EL0 => self.regs.cntfrq_el0 = val,
            _ => {} // unknown sysreg — silently ignore
        }
    }

    /// Flags = NZCV (bits 31:28) | DAIF (bits 9:6) | CurrentEL (bits 3:2) | SPSel (bit 0).
    fn flags(&self) -> u64 {
        let nzcv = (self.regs.nzcv as u64) & 0xF000_0000;
        let daif = ((self.regs.daif as u64) & 0xF) << 6;
        let el = ((self.regs.current_el as u64) & 3) << 2;
        let sp = (self.regs.sp_sel as u64) & 1;
        nzcv | daif | el | sp
    }

    fn set_flags(&mut self, flags: u64) {
        self.regs.nzcv = ((flags >> 28) as u32) << 28;
        self.regs.daif = ((flags >> 6) & 0xF) as u32;
        self.regs.current_el = ((flags >> 2) & 3) as u8;
        self.regs.sp_sel = (flags & 1) as u8;
    }

    fn privilege_level(&self) -> u8 {
        self.regs.current_el
    }

    /// Wide regs: ids 32-63 map to V0-V31 (128-bit SIMD).
    fn gpr_wide(&self, id: RegId, dst: &mut [u8]) -> usize {
        let vreg_idx = id.wrapping_sub(32) as usize;
        if vreg_idx < 32 {
            let val = self.regs.v[vreg_idx];
            let bytes = val.to_le_bytes();
            let n = dst.len().min(16);
            dst[..n].copy_from_slice(&bytes[..n]);
            n
        } else {
            0
        }
    }

    fn set_gpr_wide(&mut self, id: RegId, src: &[u8]) {
        let vreg_idx = id.wrapping_sub(32) as usize;
        if vreg_idx < 32 {
            let mut bytes = [0u8; 16];
            let n = src.len().min(16);
            bytes[..n].copy_from_slice(&src[..n]);
            self.regs.v[vreg_idx] = u128::from_le_bytes(bytes);
        }
    }
}
