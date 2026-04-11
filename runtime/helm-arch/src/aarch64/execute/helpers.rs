//! AArch64 execute — shared helper functions.
#![allow(dead_code, unused_imports)]
use crate::aarch64::arch_state::Aarch64ArchState;
use crate::aarch64::insn::{Instruction, Opcode};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
#[allow(unused_imports)]
use helm_diag::{sim_stub, sim_warn};

// ── Helpers: arithmetic ───────────────────────────────────────────────────────

#[inline]
#[allow(dead_code)]

/// Sign-extend an n-bit value to i64 for signed comparison.
pub(super) fn sext_mask(val: u64, bits: usize) -> i64 {
    let shift = 64 - bits;
    ((val as i64) << shift) >> shift
}
#[allow(dead_code)]
pub(super) fn add_overflow64(a: u64, b: u64, res: u64) -> bool {
    ((!(a ^ b)) & (a ^ res)) >> 63 != 0
}
#[inline]
#[allow(dead_code)]
#[allow(dead_code)]
pub(super) fn sub_overflow64(a: u64, b: u64, res: u64) -> bool {
    ((a ^ b) & (a ^ res)) >> 63 != 0
}
#[inline]
#[allow(dead_code)]
#[allow(dead_code)]
pub(super) fn add_overflow32(a: u32, b: u32, res: u32) -> bool {
    ((!(a ^ b)) & (a ^ res)) >> 31 != 0
}
#[inline]
#[allow(dead_code)]
#[allow(dead_code)]
pub(super) fn sub_overflow32(a: u32, b: u32, res: u32) -> bool {
    ((a ^ b) & (a ^ res)) >> 31 != 0
}

/// Add-with-carry: (a + b + cin) returning (result, carry, overflow).
/// Correctly handles 32-bit vs 64-bit arithmetic.
#[inline]
pub(super) fn awc(a: u64, b: u64, cin: bool, is64: bool) -> (u64, bool, bool) {
    if is64 {
        let (s1, c1) = a.overflowing_add(b);
        let (s2, c2) = s1.overflowing_add(cin as u64);
        let carry = c1 || c2;
        let ov = {
            let sa = (a >> 63) & 1;
            let sb = (b >> 63) & 1;
            let sr = (s2 >> 63) & 1;
            sa == sb && sa != sr
        };
        (s2, carry, ov)
    } else {
        let a = a as u32;
        let b = b as u32;
        let (s1, c1) = a.overflowing_add(b);
        let (s2, c2) = s1.overflowing_add(cin as u32);
        let carry = c1 || c2;
        let ov = {
            let sa = (a >> 31) & 1;
            let sb = (b >> 31) & 1;
            let sr = (s2 >> 31) & 1;
            sa == sb && sa != sr
        };
        (s2 as u64, carry, ov)
    }
}

/// Set NZCV flags from result, carry, overflow and width.
/// For 32-bit operations, checks bit 31 for N flag (not bit 63).
#[inline]
pub(super) fn set_flags(a: &mut Aarch64ArchState, r: u64, c: bool, v: bool, is64: bool) {
    let n = if is64 {
        r >> 63 != 0
    } else {
        (r >> 31) & 1 != 0
    };
    let z = if is64 { r == 0 } else { r & 0xFFFF_FFFF == 0 };
    a.set_nzcv(n, z, c, v);
}

#[inline]
pub(super) fn sign_extend(v: u64, size: usize) -> u64 {
    let shift = 64 - size * 8;
    ((v as i64) << shift >> shift) as u64
}

/// Sign-extend a value given width in *bits*.
#[inline]
pub(super) fn sign_extend_bits(v: u64, width: usize) -> u64 {
    if width == 0 || width >= 64 {
        return v;
    }
    let shift = 64 - width;
    ((v as i64) << shift >> shift) as u64
}

pub(super) fn apply_shift(val: u64, stype: u32, amt: u32, sf: bool) -> u64 {
    let amt = amt & if sf { 63 } else { 31 };
    if sf {
        match stype {
            0 => val << amt,
            1 => val >> amt,
            2 => ((val as i64) >> amt) as u64,
            3 => val.rotate_right(amt),
            _ => val,
        }
    } else {
        // 32-bit: operate on the lower 32 bits only
        let v = val as u32;
        match stype {
            0 => (v << amt) as u64,
            1 => (v >> amt) as u64,
            2 => ((v as i32) >> amt) as u32 as u64,
            3 => v.rotate_right(amt) as u64,
            _ => val,
        }
    }
}

// ── Helpers: binary ops ───────────────────────────────────────────────────────

/// Logical immediate: AND/ORR/EOR. Rn=31 means XZR (not SP). Rd=31 also XZR.
pub(super) fn binop_imm(a: &mut Aarch64ArchState, i: &Instruction, f: impl Fn(u64, u64) -> u64) {
    let src = a.read_x(i.rn);
    let res = f(src, i.imm as u64);
    if i.sf {
        a.write_x(i.rd, res);
    } else {
        a.write_x(i.rd, (res as u32) as u64);
    }
}

/// Logical immediate with flag-setting (ANDS). Rn=31 means XZR. Rd=31 also XZR.
pub(super) fn binop_imm_ret(
    a: &mut Aarch64ArchState,
    i: &Instruction,
    f: impl Fn(u64, u64) -> u64,
) -> u64 {
    let src = a.read_x(i.rn);
    let res = f(src, i.imm as u64);
    if i.sf {
        a.write_x(i.rd, res);
    } else {
        a.write_x(i.rd, (res as u32) as u64);
    }
    res
}

pub(super) fn log_reg(
    a: &mut Aarch64ArchState,
    i: &Instruction,
    f: impl Fn(u64, u64) -> u64,
    setf: bool,
) -> u64 {
    let rn = a.read_x(i.rn);
    let rm = apply_shift(a.read_x(i.rm), i.shift_type, i.shift_amt, i.sf);
    let res = f(rn, rm);
    let res = if i.sf { res } else { (res as u32) as u64 };
    if setf {
        set_flags(a, res, false, false, i.sf);
    }
    res
}

/// Execute ADD/SUB (shifted register). Rd=31 and Rn=31 mean XZR, not SP.
pub(super) fn exec_addsub_reg(
    a: &mut Aarch64ArchState,
    i: &Instruction,
    src: u64,
    rm: u64,
) -> Result<(), HartException> {
    let is_sub = matches!(i.opcode, Opcode::SubReg | Opcode::SubsReg);
    let setf = matches!(i.opcode, Opcode::AddsReg | Opcode::SubsReg);
    let (res, c, v) = if is_sub {
        awc(src, !rm, true, i.sf) // a + NOT(b) + 1 = a - b
    } else {
        awc(src, rm, false, i.sf)
    };
    let res = if i.sf { res } else { (res as u32) as u64 };
    if setf {
        set_flags(a, res, c, v, i.sf);
    }
    // Shifted register variant: Rd=31 is XZR (write discarded), never SP.
    a.write_x(i.rd, res);
    Ok(())
}

// ── Helpers: bitfield ────────────────────────────────────────────────────────

pub(super) fn exec_sbfm(a: &mut Aarch64ArchState, i: &Instruction) {
    let immr = i.imm as u32;
    let imms = i.imm2 as u32;
    let esize = if i.sf { 64u32 } else { 32u32 };
    let src = if i.sf {
        a.read_x(i.rn)
    } else {
        a.read_x(i.rn) & 0xFFFF_FFFF
    };
    let val = if imms >= immr {
        // SBFX / ASR: extract bits [imms:immr] and sign-extend
        let width = imms - immr + 1;
        let extracted = (src >> immr) & ((1u64 << width) - 1);
        sign_extend_bits(extracted, width as usize)
    } else {
        // SXTB/SXTH/SXTW / shift-insert (imms < immr = left-shift case)
        // extract low (imms+1) bits, shift left by (esize - immr), then
        // sign-extend from the actual top bit of the inserted field.
        let width = imms + 1;
        let shift = esize - immr;
        let bits = src & ((1u64 << width) - 1);
        let shifted = bits << shift;
        sign_extend_bits(shifted, (width + shift) as usize)
    };
    let val = if i.sf { val } else { val & 0xFFFF_FFFF };
    a.write_x(i.rd, val);
}

pub(super) fn exec_ubfm(a: &mut Aarch64ArchState, i: &Instruction) {
    let immr = i.imm as u32;
    let imms = i.imm2 as u32;
    let esize = if i.sf { 64u32 } else { 32u32 };
    let src = if i.sf {
        a.read_x(i.rn)
    } else {
        a.read_x(i.rn) & 0xFFFF_FFFF
    };
    let val = if imms >= immr {
        // UBFX / LSR: extract bitfield [imms:immr]
        let width = imms - immr + 1;
        (src >> immr) & ((1u64 << width) - 1)
    } else {
        // LSL / zero-insert (imms < immr = left-shift case)
        // extract low (imms+1) bits, shift left by (esize - immr)
        let width = imms + 1;
        let bits = src & ((1u64 << width) - 1);
        bits << (esize - immr)
    };
    let val = if i.sf { val } else { val & 0xFFFF_FFFF };
    a.write_x(i.rd, val);
}

pub(super) fn exec_bfm(a: &mut Aarch64ArchState, i: &Instruction) {
    let immr = i.imm as u32;
    let imms = i.imm2 as u32;
    let regsize = if i.sf { 64u32 } else { 32 };
    let src = if i.sf {
        a.read_x(i.rn)
    } else {
        a.read_x(i.rn) & 0xFFFF_FFFF
    };
    let dst = if i.sf {
        a.read_x(i.rd)
    } else {
        a.read_x(i.rd) & 0xFFFF_FFFF
    };
    let width = if imms >= immr {
        imms - immr + 1
    } else {
        imms + 1
    };
    let mask = (1u64 << width) - 1;
    let extracted = if imms >= immr {
        (src >> immr) & mask
    } else {
        src & mask
    };
    let shift = if imms >= immr {
        0
    } else {
        (regsize - immr) & (regsize - 1)
    };
    let val = (dst & !(mask << shift)) | ((extracted & mask) << shift);
    let val = if i.sf { val } else { val & 0xFFFF_FFFF };
    a.write_x(i.rd, val);
}

// ── Helpers: load/store address ───────────────────────────────────────────────

pub(super) fn compute_ea(a: &Aarch64ArchState, base: u64, i: &Instruction) -> u64 {
    if i.extend_type != 0 || (i.rm != 0 && !i.post_index) {
        // Register offset
        let rm = a.read_x(i.rm);
        let ext = apply_extend(rm, i.extend_type, i.extend_amt);
        base.wrapping_add(ext)
    } else if i.post_index {
        base // effective address is base; writeback applies offset after
    } else {
        base.wrapping_add(i.imm as u64)
    }
}

pub(super) fn apply_extend(val: u64, etype: u32, amt: u32) -> u64 {
    let extended = match etype {
        0 => val & 0xFF,          // UXTB
        1 => val & 0xFFFF,        // UXTH
        2 => val & 0xFFFF_FFFF,   // UXTW / LSL
        3 => val,                 // UXTX / LSL64
        4 => (val as i8) as u64,  // SXTB
        5 => (val as i16) as u64, // SXTH
        6 => (val as i32) as u64, // SXTW
        7 => val,                 // SXTX
        _ => val,
    };
    extended << amt
}

pub(super) fn writeback_pre(a: &mut Aarch64ArchState, i: &Instruction, _base: u64, ea: u64) {
    if i.pre_index {
        a.write_xsp(i.rn, ea);
    }
}

pub(super) fn writeback_post(a: &mut Aarch64ArchState, i: &Instruction, ea: u64) {
    if i.post_index {
        let new_base = ea.wrapping_add(i.imm as u64);
        a.write_xsp(i.rn, new_base);
    }
}

pub(super) fn ldst_size(op: Opcode) -> (usize, bool) {
    match op {
        Opcode::Ldrb | Opcode::Strb | Opcode::Ldurb | Opcode::Sturb => (1, false),
        Opcode::Ldrsb | Opcode::Ldursb => (1, true),
        Opcode::Ldrh | Opcode::Strh | Opcode::Ldurh | Opcode::Sturh => (2, false),
        Opcode::Ldrsh | Opcode::Ldursh => (2, true),
        Opcode::Ldrsw | Opcode::Ldursw => (4, true),
        _ => (8, false),
    }
}

// ── Helpers: system registers ─────────────────────────────────────────────────

/// Decode a packed sysreg encoding into its op0:op1:CRn:CRm:op2 components.
pub(super) fn sysreg_name(encoded: u32) -> String {
    let op0 = (encoded >> 14) & 0x3;
    let op1 = (encoded >> 11) & 0x7;
    let crn = (encoded >> 7) & 0xF;
    let crm = (encoded >> 3) & 0xF;
    let op2 = encoded & 0x7;
    format!("s{op0}_{op1}_c{crn}_c{crm}_{op2}")
}

const HCR_E2H: u64 = 1 << 34;
const HCR_TVM: u64 = 1 << 26;

pub(super) fn redirect_sysreg(a: &Aarch64ArchState, encoded: u32) -> u32 {
    if a.current_el != 2 || (a.hcr_el2 & HCR_E2H) == 0 {
        return encoded;
    }
    match encoded {
        0b11_000_0001_0000_000 => 0b11_100_0001_0000_000, // SCTLR_EL1 -> SCTLR_EL2
        0b11_000_0010_0000_010 => 0b11_100_0010_0000_010, // TCR_EL1 -> TCR_EL2
        0b11_000_0010_0000_000 => 0b11_100_0010_0000_000, // TTBR0_EL1 -> TTBR0_EL2
        0b11_000_0010_0000_001 => 0b11_100_0010_0000_001, // TTBR1_EL1 -> TTBR1_EL2
        0b11_000_1010_0010_000 => 0b11_100_1010_0010_000, // MAIR_EL1 -> MAIR_EL2
        0b11_000_1100_0000_000 => 0b11_100_1100_0000_000, // VBAR_EL1 -> VBAR_EL2
        0b11_000_1110_0001_000 => 0b11_100_1110_0001_000, // CNTKCTL_EL1 -> CNTHCTL_EL2
        0b11_000_0100_0000_001 => 0b11_100_0100_0000_001, // ELR_EL1 -> ELR_EL2
        0b11_000_0100_0000_000 => 0b11_100_0100_0000_000, // SPSR_EL1 -> SPSR_EL2
        0b11_000_0110_0000_000 => 0b11_100_0110_0000_000, // FAR_EL1 -> FAR_EL2
        _ => encoded,
    }
}

pub(super) fn should_tvm_trap(a: &Aarch64ArchState, encoded: u32) -> bool {
    a.current_el == 1
        && (a.hcr_el2 & HCR_TVM) != 0
        && matches!(
            encoded,
            0b11_000_0001_0000_000 // SCTLR_EL1
                | 0b11_000_0010_0000_000 // TTBR0_EL1
                | 0b11_000_0010_0000_001 // TTBR1_EL1
                | 0b11_000_0010_0000_010 // TCR_EL1
                | 0b11_000_0101_0010_000 // ESR_EL1
                | 0b11_000_0110_0000_000 // FAR_EL1
                | 0b11_000_1010_0010_000 // MAIR_EL1
                | 0b11_000_1010_0011_000 // AMAIR_EL1
                | 0b11_000_1101_0000_001 // CONTEXTIDR_EL1
        )
}

pub(super) fn read_sysreg(a: &Aarch64ArchState, encoded: u32) -> u64 {
    // Decode: [15:14]=op0, [13:11]=op1, [10:7]=CRn, [6:3]=CRm, [2:0]=op2
    // Common system registers in SE mode:
    match encoded {
        // TPIDR_EL0
        0b11_011_1101_0000_010 => a.tpidr_el0,
        // TPIDRRO_EL0
        0b11_011_1101_0000_011 => a.tpidrro_el0,
        // NZCV
        0b11_011_0100_0010_000 => a.nzcv as u64,
        // FPCR
        0b11_011_0100_0100_000 => a.fpcr as u64,
        // FPSR
        0b11_011_0100_0100_001 => a.fpsr as u64,
        // CTR_EL0 (cache type register)
        0b11_011_0000_0000_001 => 0x8444_C004,
        // DCZID_EL0
        // Match ../helm.git: 64-byte DC ZVA block, not prohibited.
        0b11_011_0000_0000_111 => 0x0000_0004,
        // CNTVCT_EL0
       0b11_011_1110_0000_010 => a.cntvct_el0,
       // CNTFRQ_EL0
       0b11_011_1110_0000_000 => a.cntfrq_el0,
        // CNTPCT_EL0 (3, 3, 14, 0, 1) — physical counter: same tick source as CNTVCT.
        // Without this, Linux programs CNTP_CVAL = 0 + period on every tick,
        // causing an immediate re-fire storm once fs.tick > period.
        0b11_011_1110_0000_001 => a.cntvct_el0,
       // MIDR_EL1
        0b11_000_0000_0000_000 => a.midr_el1,
        // MPIDR_EL1
        0b11_000_0000_0000_101 => a.mpidr_el1,
        // ID_AA64PFR0_EL1
        0b11_000_0000_0100_000 => a.id_aa64pfr0_el1,
        // ID_AA64ISAR0_EL1
        0b11_000_0000_0110_000 => a.id_aa64isar0_el1,
        // ID_AA64MMFR0_EL1
        0b11_000_0000_0111_000 => a.id_aa64mmfr0_el1,
        // SCTLR_EL1
        0b11_000_0001_0000_000 => a.sctlr_el1,
        // VBAR_EL1 (3, 0, 12, 0, 0)
        0b11_000_1100_0000_000 => a.vbar_el1,
        // ELR_EL1 (3, 0, 4, 0, 1)
        0b11_000_0100_0000_001 => a.elr_el1,
        // SPSR_EL1 (3, 0, 4, 0, 0)
        0b11_000_0100_0000_000 => a.spsr_el1 as u64,
        // ESR_EL1 (3, 0, 5, 2, 0)
        0b11_000_0101_0010_000 => a.esr_el1 as u64,
        // FAR_EL1 (3, 0, 6, 0, 0)
        0b11_000_0110_0000_000 => a.far_el1,
        // SP_EL0 (3, 0, 4, 1, 0) -- reads the EL0 stack pointer from EL1
        0b11_000_0100_0001_000 => a.sp,
        // CPACR_EL1 (3, 0, 1, 0, 2)
        0b11_000_0001_0000_010 => a.cpacr_el1,
        // TPIDR_EL1 (3, 0, 13, 0, 4)
        0b11_000_1101_0000_100 => a.tpidr_el1,
        // CONTEXTIDR_EL1 (3, 0, 13, 0, 1)
        0b11_000_1101_0000_001 => a.contextidr_el1,
        // TCR_EL1 (3, 0, 2, 0, 2)
        0b11_000_0010_0000_010 => a.tcr_el1,
        // TTBR0_EL1 (3, 0, 2, 0, 0)
        0b11_000_0010_0000_000 => a.ttbr0_el1,
        // TTBR1_EL1 (3, 0, 2, 0, 1)
        0b11_000_0010_0000_001 => a.ttbr1_el1,
        // MAIR_EL1 (3, 0, 10, 2, 0)
        0b11_000_1010_0010_000 => a.mair_el1,
        // HCR_EL2
        0b11_100_0001_0001_000 => a.hcr_el2,
        // MDCR_EL2
        0b11_100_0001_0001_001 => a.mdcr_el2,
        // CPTR_EL2
        0b11_100_0001_0001_010 => a.cptr_el2,
        // HSTR_EL2
        0b11_100_0001_0001_011 => a.hstr_el2,
        // SCTLR_EL2
        0b11_100_0001_0000_000 => a.sctlr_el2,
        // TCR_EL2
        0b11_100_0010_0000_010 => a.tcr_el2,
        // TTBR0_EL2
        0b11_100_0010_0000_000 => a.ttbr0_el2,
        // TTBR1_EL2
        0b11_100_0010_0000_001 => a.ttbr1_el2,
        // VTTBR_EL2
        0b11_100_0010_0001_000 => a.vttbr_el2,
        // VTCR_EL2
        0b11_100_0010_0001_010 => a.vtcr_el2,
        // MAIR_EL2
        0b11_100_1010_0010_000 => a.mair_el2,
        // VBAR_EL2
        0b11_100_1100_0000_000 => a.vbar_el2,
        // ELR_EL2
        0b11_100_0100_0000_001 => a.elr_el2,
        // SPSR_EL2
        0b11_100_0100_0000_000 => a.spsr_el2 as u64,
        // ESR_EL2
        0b11_100_0101_0010_000 => a.esr_el2 as u64,
        // FAR_EL2
        0b11_100_0110_0000_000 => a.far_el2,
        // HPFAR_EL2
        0b11_100_0110_0000_100 => a.hpfar_el2,
        // SP_EL2
        0b11_100_0100_0001_000 => a.sp_el2,
        // SCR_EL3
        0b11_110_0001_0001_000 => a.scr_el3,
        // SCTLR_EL3
        0b11_110_0001_0000_000 => a.sctlr_el3,
        // TCR_EL3
        0b11_110_0010_0000_010 => a.tcr_el3,
        // TTBR0_EL3
        0b11_110_0010_0000_000 => a.ttbr0_el3,
        // MAIR_EL3
        0b11_110_1010_0010_000 => a.mair_el3,
        // VBAR_EL3
        0b11_110_1100_0000_000 => a.vbar_el3,
        // ELR_EL3
        0b11_110_0100_0000_001 => a.elr_el3,
        // SPSR_EL3
        0b11_110_0100_0000_000 => a.spsr_el3 as u64,
        // ESR_EL3
        0b11_110_0101_0010_000 => a.esr_el3 as u64,
        // FAR_EL3
        0b11_110_0110_0000_000 => a.far_el3,
        // SP_EL3
        0b11_110_0100_0001_000 => a.sp_el3,
        // DAIF (3, 3, 4, 2, 1)
        0b11_011_0100_0010_001 => (a.daif as u64) << 6,
        // CurrentEL (3, 0, 4, 2, 2)
        0b11_000_0100_0010_010 => (a.current_el as u64) << 2,
        // PAR_EL1 (3, 0, 7, 4, 0)
        0b11_000_0111_0100_000 => a.par_el1,
        // AMAIR_EL1 (3, 0, 10, 3, 0)
        0b11_000_1010_0011_000 => a.amair_el1,
        // MDSCR_EL1 (2, 0, 0, 2, 2) - note op0=2
        0b10_000_0000_0010_010 => a.mdscr_el1 as u64,
        // CNTKCTL_EL1 (3, 0, 14, 1, 0)
        0b11_000_1110_0001_000 => a.cntkctl_el1 as u64,
        // CNTHCTL_EL2 (3, 4, 14, 1, 0)
        0b11_100_1110_0001_000 => a.cnthctl_el2,
        // CNTHP_CTL_EL2 (3, 4, 14, 2, 1)
        0b11_100_1110_0010_001 => a.cnthp_ctl_el2 as u64,
        // CNTHP_CVAL_EL2 (3, 4, 14, 2, 2)
        0b11_100_1110_0010_010 => a.cnthp_cval_el2,
        // CNTHP_TVAL_EL2 (3, 4, 14, 2, 0)
        0b11_100_1110_0010_000 => {
            (a.cnthp_cval_el2.wrapping_sub(a.cntvct_el0) as i32) as i64 as u64
        }
        // CNTVOFF_EL2 (3, 4, 14, 0, 3)
        0b11_100_1110_0000_011 => a.cntvoff_el2,
        // CNTP_CTL_EL0 (3, 3, 14, 2, 1)
       0b11_011_1110_0010_001 => a.cntp_ctl_el0 as u64,
       // CNTP_CVAL_EL0 (3, 3, 14, 2, 2)
       0b11_011_1110_0010_010 => a.cntp_cval_el0,
       // CNTV_CTL_EL0 (3, 3, 14, 3, 1)
       0b11_011_1110_0011_001 => a.cntv_ctl_el0 as u64,
       // CNTV_CVAL_EL0 (3, 3, 14, 3, 2)
       0b11_011_1110_0011_010 => a.cntv_cval_el0,
        // CNTP_TVAL_EL0 (3, 3, 14, 2, 0): read = (i32)(CVAL - CNTPCT) sign-extended
        0b11_011_1110_0010_000 => (a.cntp_cval_el0.wrapping_sub(a.cntvct_el0) as i32) as i64 as u64,
        // CNTV_TVAL_EL0 (3, 3, 14, 3, 0): read = (i32)(CVAL - CNTVCT) sign-extended
        0b11_011_1110_0011_000 => (a.cntv_cval_el0.wrapping_sub(a.cntvct_el0) as i32) as i64 as u64,
        // ID_AA64MMFR1_EL1 (3, 0, 0, 7, 1)
        0b11_000_0000_0111_001 => a.id_aa64mmfr1_el1,
        // ID_AA64ISAR1_EL1 (3, 0, 0, 6, 1)
        0b11_000_0000_0110_001 => a.id_aa64isar1_el1,
        // ID_AA64PFR1_EL1 (3, 0, 0, 4, 1)
        0b11_000_0000_0100_001 => a.id_aa64pfr1_el1,
        // PMCR_EL0 (3, 3, 9, 12, 0) -- performance monitors control (stub)
        0b11_011_1001_1100_000 => 0,
        // PMCCNTR_EL0 (3, 3, 9, 13, 0) -- cycle counter (stub)
        0b11_011_1001_1101_000 => 0,
        // PMCNTENSET_EL0 (3, 3, 9, 12, 1) -- counter enable set (stub)
        0b11_011_1001_1100_001 => 0,
        // PMUSERENR_EL0 (3, 3, 9, 14, 0) -- PMU user enable (stub)
        0b11_011_1001_1110_000 => 0,
        // OSDLR_EL1 (2, 0, 1, 3, 4)
        0b10_000_0001_0011_100 => 0,
        // OSLAR_EL1 (2, 0, 1, 0, 4)
        0b10_000_0001_0000_100 => 0,
        // OSLSR_EL1 (2, 0, 1, 1, 4)
        0b10_000_0001_0001_100 => 0,
        // ID_AA64DFR0_EL1 (3, 0, 0, 5, 0)
        0b11_000_0000_0101_000 => 0,
        // REVIDR_EL1 (3, 0, 0, 0, 6)
        0b11_000_0000_0000_110 => 0,
        // ID_AA64AFR0_EL1 (3, 0, 0, 5, 4)
        0b11_000_0000_0101_100 => 0,
        // ID_AA64MMFR2_EL1 (3, 0, 0, 7, 2)
        0b11_000_0000_0111_010 => 0,
        // ID_AA64MMFR3_EL1 (3, 0, 0, 7, 3)
        0b11_000_0000_0111_011 => 0,
        // ID_AA64MMFR4_EL1 (3, 0, 0, 7, 4)
        0b11_000_0000_0111_100 => 0,
        // ID_AA64ISAR2_EL1 (3, 0, 0, 6, 2)
        0b11_000_0000_0110_010 => 0,
        // ── Pointer Authentication keys (ARMv8.3-PAuth) ─────────────────
        // APIAKey (3,0,2,1,0/1)
        0b11_000_0010_0001_000 => a.apia_key[0],
        0b11_000_0010_0001_001 => a.apia_key[1],
        // APIBKey (3,0,2,1,2/3)
        0b11_000_0010_0001_010 => a.apib_key[0],
        0b11_000_0010_0001_011 => a.apib_key[1],
        // APDAKey (3,0,2,2,0/1)
        0b11_000_0010_0010_000 => a.apda_key[0],
        0b11_000_0010_0010_001 => a.apda_key[1],
        // APDBKey (3,0,2,2,2/3)
        0b11_000_0010_0010_010 => a.apdb_key[0],
        0b11_000_0010_0010_011 => a.apdb_key[1],
        // APGAKey (3,0,2,3,0/1)
        0b11_000_0010_0011_000 => a.apga_key[0],
        0b11_000_0010_0011_001 => a.apga_key[1],
        // AIDR_EL1 (3,1,0,0,7) -- implementation defined, return 0
        0b11_001_0000_0000_111 => 0,
        // ── GICv3 Hypervisor / Virtual interface (ICH_*) ────────────────
        // ICH_AP0R0_EL2 .. ICH_AP0R3_EL2 (3,4,12,8,0..3)
        0b11_100_1100_1000_000 => 0,
        0b11_100_1100_1000_001 => 0,
        0b11_100_1100_1000_010 => 0,
        0b11_100_1100_1000_011 => 0,
        // ICH_AP1R0_EL2 .. ICH_AP1R3_EL2 (3,4,12,9,0..3) -- already handled
        // ICH_HCR_EL2 (3,4,12,11,0)
        0b11_100_1100_1011_000 => 0,
        // ICH_VTR_EL2 (3,4,12,11,1)
        0b11_100_1100_1011_001 => 0,
        // ICH_MISR_EL2 (3,4,12,11,2)
        0b11_100_1100_1011_010 => 0,
        // ICH_VMCR_EL2 (3,4,12,11,7)
        0b11_100_1100_1011_111 => 0,
        // ICV_* mapped through EL1 when HCR_EL2.IMO/FMO:
        // (3,0,12,8,4..7) -- the kernel writes these on secondary CPU init
        0b11_000_1100_1000_100 => 0,
        0b11_000_1100_1000_101 => 0,
        0b11_000_1100_1000_110 => 0,
        0b11_000_1100_1000_111 => 0,
        // Legacy AArch32 ID registers -- read as zero on AArch64-only CPUs
        0b11_000_0000_0001_000  // ID_PFR0_EL1
        | 0b11_000_0000_0001_001 // ID_PFR1_EL1
        | 0b11_000_0000_0001_010 // ID_PFR2_EL1
        | 0b11_000_0000_0001_011 // ID_DFR0_EL1
        | 0b11_000_0000_0001_111 // ID_AFR0_EL1
        | 0b11_000_0000_0001_100 // ID_MMFR0_EL1
        | 0b11_000_0000_0001_101 // ID_MMFR1_EL1
        | 0b11_000_0000_0001_110 // ID_MMFR2_EL1
        | 0b11_000_0000_0010_110 // ID_MMFR4_EL1
        | 0b11_000_0000_0010_000 // ID_ISAR0_EL1
        | 0b11_000_0000_0010_001 // ID_ISAR1_EL1
        | 0b11_000_0000_0010_010 // ID_ISAR2_EL1
        | 0b11_000_0000_0010_011 // ID_ISAR3_EL1
        | 0b11_000_0000_0010_100 // ID_ISAR4_EL1
        | 0b11_000_0000_0010_101 // ID_ISAR5_EL1
        | 0b11_000_0000_0010_111 // ID_ISAR6_EL1
            => 0,
        // ── ARMv8.x newer ID registers (all 0 = feature not present) ──────
        // ID_AA64PFR2_EL1  (3, 0, 0, 4, 2) — ARMv8.9 processor features
        0b11_000_0000_0100_010 => 0,
        // ID_AA64ZFR0_EL1  (3, 0, 0, 4, 4) — SVE feature register (no SVE)
        0b11_000_0000_0100_100 => 0,
        // ID_AA64SMFR0_EL1 (3, 0, 0, 4, 5) — SME feature register (no SME)
        0b11_000_0000_0100_101 => 0,
        // ID_AA64FPFR0_EL1 (3, 0, 0, 4, 7) — FP feature register (ARMv8.9+)
        0b11_000_0000_0100_111 => 0,
        // ID_AA64DFR1_EL1  (3, 0, 0, 5, 1) — debug features register 1
        0b11_000_0000_0101_001 => 0,
        // ID_AA64ISAR3_EL1 (3, 0, 0, 6, 3) — ISA features register 3 (ARMv8.9+)
        0b11_000_0000_0110_011 => 0,
        // ── GICv3 ICC_* system registers ───────────────────────────────────
        // When ID_AA64PFR0_EL1.GIC=0 (no GICv3 sysreg interface), the kernel
        // probes ICC_SRE_EL1 early: if SRE reads back 1 despite PFR0.GIC=0,
        // the kernel warns "GICv3 system registers enabled, broken firmware!".
        // Return 0 for ICC_SRE so the kernel falls back to MMIO GICv2.
        // ICC_SRE_EL1 (3, 0, 12, 12, 5): SRE=0 — sysreg interface disabled
        0b11_000_1100_1100_101 => 0,
        // ICC_SRE_EL2 (3, 4, 12,  9, 5)
        0b11_100_1100_1001_101 => 0,
        // ICC_SRE_EL3 (3, 6, 12, 12, 5)
        0b11_110_1100_1100_101 => 0,
        // ICC_CTLR_EL1 (3, 0, 12, 12, 4)
        0b11_000_1100_1100_100 => 0,
        // ICC_PMR_EL1  (3, 0,  4,  6, 0)
        0b11_000_0100_0110_000 => 0xFF,
        // ICC_IAR1_EL1 (3, 0, 12, 12, 0): spurious (no GICv3 wired)
        0b11_000_1100_1100_000 => 1023,
        // ICC_HPPIR1_EL1 (3, 0, 12, 12, 2): spurious
        0b11_000_1100_1100_010 => 1023,
        // ICC_BPR1_EL1   (3, 0, 12, 12, 3)
        0b11_000_1100_1100_011 => 0,
        // ICC_RPR_EL1    (3, 0, 12, 11, 3): idle priority
        0b11_000_1100_1011_011 => 0xFF,
        // ICC_IGRPEN0_EL1 (3, 0, 12, 12, 6)
        0b11_000_1100_1100_110 => 0,
        // ICC_IGRPEN1_EL1 (3, 0, 12, 12, 7)
        0b11_000_1100_1100_111 => 0,
        // ICC_IGRPEN1_EL3 (3, 6, 12, 12, 7)
        0b11_110_1100_1100_111 => 0,
        // ICC_AP1R0..3_EL1 (3, 0, 12, 9, 0..3)
        0b11_000_1100_1001_000 => 0,
        0b11_000_1100_1001_001 => 0,
        0b11_000_1100_1001_010 => 0,
        0b11_000_1100_1001_011 => 0,
        // CLIDR_EL1 (3, 1, 0, 0, 1) -- cache level ID
        0b11_001_0000_0000_001 => 0x0000_0000_0A00_0023, // L1 I+D, L2 unified
        // CCSIDR_EL1 (3, 1, 0, 0, 0) -- cache size ID
        0b11_001_0000_0000_000 => 0x7000_01FE, // 32KB, 64B line
        // CSSELR_EL1 (3, 2, 0, 0, 0) -- cache size selection
        0b11_010_0000_0000_000 => 0,
        // Unknown — always visible; unimplemented sysreg stubs logged as STUB level.
        _ => {
            sim_stub!(component="aarch64-sysreg", pc=a.pc,
                "MRS from unimplemented sysreg {} (enc={encoded:#06x}) -> 0",
                sysreg_name(encoded));
            0
        }
    }
}

pub(super) fn write_sysreg(a: &mut Aarch64ArchState, encoded: u32, val: u64) {
    match encoded {
        // TPIDR_EL0
        0b11_011_1101_0000_010 => a.tpidr_el0 = val,
        // NZCV
        0b11_011_0100_0010_000 => a.nzcv = val as u32,
        // FPCR
        0b11_011_0100_0100_000 => a.fpcr = val as u32,
        // FPSR
        0b11_011_0100_0100_001 => a.fpsr = val as u32,
        // SCTLR_EL1 — MMU enable/disable; flush TLB to avoid stale entries.
        0b11_000_0001_0000_000 => { a.sctlr_el1 = val; a.tlb_flush_pending = true; }
        // TCR_EL1 — address-space sizes / granule change; invalidates all entries.
        0b11_000_0010_0000_010 => { a.tcr_el1 = val; a.tlb_flush_pending = true; }
        // TTBR0_EL1 — new user page tables (context switch or early boot setup).
        0b11_000_0010_0000_000 => { a.ttbr0_el1 = val; a.tlb_flush_pending = true; }
        // TTBR1_EL1 — new kernel page tables; must flush to avoid stale mappings.
        0b11_000_0010_0000_001 => { a.ttbr1_el1 = val; a.tlb_flush_pending = true; }
        // VBAR_EL1
        0b11_000_1100_0000_000 => a.vbar_el1 = val,
        // MAIR_EL1
        0b11_000_1010_0010_000 => a.mair_el1 = val,
        // HCR_EL2
        0b11_100_0001_0001_000 => a.hcr_el2 = val,
        // MDCR_EL2
        0b11_100_0001_0001_001 => a.mdcr_el2 = val,
        // CPTR_EL2
        0b11_100_0001_0001_010 => a.cptr_el2 = val,
        // HSTR_EL2
        0b11_100_0001_0001_011 => a.hstr_el2 = val,
        // SCTLR_EL2
        0b11_100_0001_0000_000 => { a.sctlr_el2 = val; a.tlb_flush_pending = true; }
        // TCR_EL2
        0b11_100_0010_0000_010 => { a.tcr_el2 = val; a.tlb_flush_pending = true; }
        // TTBR0_EL2
        0b11_100_0010_0000_000 => { a.ttbr0_el2 = val; a.tlb_flush_pending = true; }
        // TTBR1_EL2
        0b11_100_0010_0000_001 => { a.ttbr1_el2 = val; a.tlb_flush_pending = true; }
        // VTTBR_EL2
        0b11_100_0010_0001_000 => { a.vttbr_el2 = val; a.tlb_flush_pending = true; }
        // VTCR_EL2
        0b11_100_0010_0001_010 => { a.vtcr_el2 = val; a.tlb_flush_pending = true; }
        // MAIR_EL2
        0b11_100_1010_0010_000 => a.mair_el2 = val,
        // VBAR_EL2
        0b11_100_1100_0000_000 => a.vbar_el2 = val,
        // ELR_EL2
        0b11_100_0100_0000_001 => a.elr_el2 = val,
        // SPSR_EL2
        0b11_100_0100_0000_000 => a.spsr_el2 = val as u32,
        // ESR_EL2
        0b11_100_0101_0010_000 => a.esr_el2 = val as u32,
        // FAR_EL2
        0b11_100_0110_0000_000 => a.far_el2 = val,
        // HPFAR_EL2
        0b11_100_0110_0000_100 => a.hpfar_el2 = val,
        // SP_EL2
        0b11_100_0100_0001_000 => a.sp_el2 = val,
        // SCR_EL3
        0b11_110_0001_0001_000 => a.scr_el3 = val,
        // SCTLR_EL3
        0b11_110_0001_0000_000 => { a.sctlr_el3 = val; a.tlb_flush_pending = true; }
        // TCR_EL3
        0b11_110_0010_0000_010 => { a.tcr_el3 = val; a.tlb_flush_pending = true; }
        // TTBR0_EL3
        0b11_110_0010_0000_000 => { a.ttbr0_el3 = val; a.tlb_flush_pending = true; }
        // MAIR_EL3
        0b11_110_1010_0010_000 => a.mair_el3 = val,
        // VBAR_EL3
        0b11_110_1100_0000_000 => a.vbar_el3 = val,
        // ELR_EL3
        0b11_110_0100_0000_001 => a.elr_el3 = val,
        // SPSR_EL3
        0b11_110_0100_0000_000 => a.spsr_el3 = val as u32,
        // ESR_EL3
        0b11_110_0101_0010_000 => a.esr_el3 = val as u32,
        // FAR_EL3
        0b11_110_0110_0000_000 => a.far_el3 = val,
        // SP_EL3
        0b11_110_0100_0001_000 => a.sp_el3 = val,
        // ELR_EL1
        0b11_000_0100_0000_001 => a.elr_el1 = val,
        // SPSR_EL1
        0b11_000_0100_0000_000 => a.spsr_el1 = val as u32,
        // ESR_EL1
        0b11_000_0101_0010_000 => a.esr_el1 = val as u32,
        // FAR_EL1
        0b11_000_0110_0000_000 => a.far_el1 = val,
        // SP_EL0
        0b11_000_0100_0001_000 => a.sp = val,
        // CPACR_EL1
        0b11_000_0001_0000_010 => a.cpacr_el1 = val,
        // TPIDR_EL1
        0b11_000_1101_0000_100 => a.tpidr_el1 = val,
        // CONTEXTIDR_EL1
        0b11_000_1101_0000_001 => a.contextidr_el1 = val,
        // DAIF
        0b11_011_0100_0010_001 => a.daif = ((val >> 6) & 0xF) as u32,
        // PAR_EL1
        0b11_000_0111_0100_000 => a.par_el1 = val,
        // AMAIR_EL1
        0b11_000_1010_0011_000 => a.amair_el1 = val,
        // MDSCR_EL1
        0b10_000_0000_0010_010 => a.mdscr_el1 = val as u32,
        // CNTKCTL_EL1
        0b11_000_1110_0001_000 => a.cntkctl_el1 = val as u32,
        // CNTHCTL_EL2
        0b11_100_1110_0001_000 => a.cnthctl_el2 = val,
        // CNTHP_CTL_EL2
        0b11_100_1110_0010_001 => a.cnthp_ctl_el2 = val as u32,
        // CNTHP_CVAL_EL2
        0b11_100_1110_0010_010 => a.cnthp_cval_el2 = val,
        // CNTHP_TVAL_EL2
        0b11_100_1110_0010_000 => {
            a.cnthp_cval_el2 = a.cntvct_el0.wrapping_add((val as i32) as i64 as u64);
        }
        // CNTVOFF_EL2
        0b11_100_1110_0000_011 => a.cntvoff_el2 = val,
        // CNTP_CTL_EL0
        0b11_011_1110_0010_001 => a.cntp_ctl_el0 = val as u32,
        // CNTP_CVAL_EL0
       0b11_011_1110_0010_010 => a.cntp_cval_el0 = val,
       // CNTV_CTL_EL0
       0b11_011_1110_0011_001 => a.cntv_ctl_el0 = val as u32,
       // CNTV_CVAL_EL0
       0b11_011_1110_0011_010 => a.cntv_cval_el0 = val,
        // CNTP_TVAL_EL0 (3, 3, 14, 2, 0): write sets CVAL = CNTPCT + (i32)tval
        0b11_011_1110_0010_000 => {
            a.cntp_cval_el0 = a.cntvct_el0.wrapping_add((val as i32) as i64 as u64);
        }
        // CNTV_TVAL_EL0 (3, 3, 14, 3, 0): write sets CVAL = CNTVCT + (i32)tval
        0b11_011_1110_0011_000 => {
            a.cntv_cval_el0 = a.cntvct_el0.wrapping_add((val as i32) as i64 as u64);
        }
        // CSSELR_EL1
        0b11_010_0000_0000_000 => { /* ignore cache size selection writes */ }
        // PMCR_EL0
        0b11_011_1001_1100_000 => { /* ignore perf monitor control writes */ }
        // PMCNTENSET_EL0
        0b11_011_1001_1100_001 => { /* ignore */ }
        // PMCNTENCLR_EL0 (3, 3, 9, 12, 2)
        0b11_011_1001_1100_010 => { /* ignore */ }
        // PMUSERENR_EL0
        0b11_011_1001_1110_000 => { /* ignore */ }
        // PMINTENSET_EL1 (3, 0, 9, 14, 1)
        0b11_000_1001_1110_001 => { /* ignore */ }
        // PMINTENCLR_EL1 (3, 0, 9, 14, 2)
        0b11_000_1001_1110_010 => { /* ignore */ }
        // OSDLR_EL1
        0b10_000_0001_0011_100 => { /* ignore */ }
        // OSLAR_EL1
        0b10_000_0001_0000_100 => { /* ignore */ }
        // TPIDRRO_EL0
        0b11_011_1101_0000_011 => a.tpidrro_el0 = val,
        // ── Pointer Authentication keys (ARMv8.3-PAuth) ─────────────────
        0b11_000_0010_0001_000 => a.apia_key[0] = val,
        0b11_000_0010_0001_001 => a.apia_key[1] = val,
        0b11_000_0010_0001_010 => a.apib_key[0] = val,
        0b11_000_0010_0001_011 => a.apib_key[1] = val,
        0b11_000_0010_0010_000 => a.apda_key[0] = val,
        0b11_000_0010_0010_001 => a.apda_key[1] = val,
        0b11_000_0010_0010_010 => a.apdb_key[0] = val,
        0b11_000_0010_0010_011 => a.apdb_key[1] = val,
        0b11_000_0010_0011_000 => a.apga_key[0] = val,
        0b11_000_0010_0011_001 => a.apga_key[1] = val,
        // ── GICv3 Hypervisor / Virtual interface writes ─────────────────
        0b11_100_1100_1000_000 // ICH_AP0R0_EL2
        | 0b11_100_1100_1000_001
        | 0b11_100_1100_1000_010
        | 0b11_100_1100_1000_011
        | 0b11_100_1100_1011_000 // ICH_HCR_EL2
        | 0b11_100_1100_1011_111 // ICH_VMCR_EL2
        | 0b11_000_1100_1000_100 // ICV_AP1R0..3_EL1 / ICH redirect
        | 0b11_000_1100_1000_101
        | 0b11_000_1100_1000_110
        | 0b11_000_1100_1000_111
            => { /* GICv3 ICH/ICV — silently ignored */ }
        // ── GICv3 system register interface writes — silently ignored ─────
        // Writes to ICC_SRE_EL1 try to enable GICv3 SRE. We always return 0
        // on read (SRE bit clear), so the kernel sees the write as denied by
        // EL2 and falls back to MMIO GICv2. No stub message needed.
        0b11_000_1100_1100_101  // ICC_SRE_EL1
        | 0b11_000_1100_1100_100 // ICC_CTLR_EL1
        | 0b11_000_0100_0110_000 // ICC_PMR_EL1
        | 0b11_000_1100_1100_010 // ICC_EOIR1_EL1
        | 0b11_000_1100_1100_011 // ICC_BPR1_EL1
        | 0b11_000_1100_1100_110 // ICC_IGRPEN0_EL1
        | 0b11_000_1100_1100_111 // ICC_IGRPEN1_EL1
        | 0b11_000_1100_0001_001 // additional GIC sysreg probe seen during boot
        // Architectural capability/debug probes observed during Linux boot.
        | 0b10_000_0000_0000_100
        | 0b10_000_0000_0000_101
        | 0b10_000_0000_0000_110
        | 0b10_000_0000_0000_111
            => { /* GICv3 SRE disabled — ignore */ }
        // Unknown — always visible
        _ => {
            sim_stub!(component="aarch64-sysreg", pc=a.pc,
                "MSR to unimplemented sysreg {} (enc={encoded:#06x}) val={val:#x} (ignored)",
                sysreg_name(encoded));
        }
    }
}

// ── Helpers: FP ──────────────────────────────────────────────────────────────

pub(super) fn fp_imm8_to_f32(imm8: u32) -> f32 {
    // ARM VFP 8-bit FP immediate: sign(1) exp(4) mantissa(3)
    let sign = (imm8 >> 7) & 1;
    let exp4 = (imm8 >> 4) & 0xF;
    let mant3 = imm8 & 0x7;
    let exp = if exp4 & 0x8 != 0 {
        (exp4 | 0xFFFF_FFF8) as i32
    } else {
        exp4 as i32
    };
    let exp_biased = (exp + 127) as u32;
    let bits = (sign << 31) | ((exp_biased & 0xFF) << 23) | (mant3 << 20);
    f32::from_bits(bits)
}

pub(super) fn exec_fp_binary(a: &mut Aarch64ArchState, i: &Instruction) {
    if i.ftype == 1 {
        // Double precision
        let rn = f64::from_bits(a.v[i.rn as usize] as u64);
        let rm = f64::from_bits(a.v[i.rm as usize] as u64);
        let res: f64 = match i.opcode {
            Opcode::Fadd => rn + rm,
            Opcode::Fsub => rn - rm,
            Opcode::Fmul => rn * rm,
            Opcode::Fdiv => rn / rm,
            Opcode::Fmax => {
                if rn >= rm {
                    rn
                } else {
                    rm
                }
            }
            Opcode::Fmin => {
                if rn <= rm {
                    rn
                } else {
                    rm
                }
            }
            Opcode::Fmaxnm => rn.max(rm),
            Opcode::Fminnm => rn.min(rm),
            _ => 0.0,
        };
        a.v[i.rd as usize] = res.to_bits() as u128;
    } else {
        // Single precision
        let rn = f32::from_bits(a.v[i.rn as usize] as u32);
        let rm = f32::from_bits(a.v[i.rm as usize] as u32);
        let res: f32 = match i.opcode {
            Opcode::Fadd => rn + rm,
            Opcode::Fsub => rn - rm,
            Opcode::Fmul => rn * rm,
            Opcode::Fdiv => rn / rm,
            Opcode::Fmax => {
                if rn >= rm {
                    rn
                } else {
                    rm
                }
            }
            Opcode::Fmin => {
                if rn <= rm {
                    rn
                } else {
                    rm
                }
            }
            Opcode::Fmaxnm => rn.max(rm),
            Opcode::Fminnm => rn.min(rm),
            _ => 0.0,
        };
        a.v[i.rd as usize] = res.to_bits() as u128;
    }
}

pub(super) fn exec_fp_unary(a: &mut Aarch64ArchState, i: &Instruction) {
    if i.ftype == 1 {
        let rn = f64::from_bits(a.v[i.rn as usize] as u64);
        let res: f64 = match i.opcode {
            Opcode::Fsqrt => rn.sqrt(),
            Opcode::Fabs => rn.abs(),
            Opcode::Fneg => -rn,
            _ => rn,
        };
        a.v[i.rd as usize] = res.to_bits() as u128;
    } else {
        let rn = f32::from_bits(a.v[i.rn as usize] as u32);
        let res: f32 = match i.opcode {
            Opcode::Fsqrt => rn.sqrt(),
            Opcode::Fabs => rn.abs(),
            Opcode::Fneg => -rn,
            _ => rn,
        };
        a.v[i.rd as usize] = res.to_bits() as u128;
    }
}

pub(super) fn exec_fcmp(a: &mut Aarch64ArchState, i: &Instruction) {
    let (_rn_is_zero, z, n, c, v) = if i.ftype == 1 {
        let rn = f64::from_bits(a.v[i.rn as usize] as u64);
        let rm = f64::from_bits(a.v[i.rm as usize] as u64);
        let unordered = rn.is_nan() || rm.is_nan();
        (false, rn == rm, rn < rm, !(rn < rm) || unordered, unordered)
    } else {
        let rn = f32::from_bits(a.v[i.rn as usize] as u32);
        let rm = f32::from_bits(a.v[i.rm as usize] as u32);
        let unordered = rn.is_nan() || rm.is_nan();
        (false, rn == rm, rn < rm, !(rn < rm) || unordered, unordered)
    };
    a.set_nzcv(n, z, c, v);
}

pub(super) fn exec_fcvt(a: &mut Aarch64ArchState, i: &Instruction) {
    // FCVT between FP sizes — simplified
    if i.ftype == 0 && (i.raw >> 15) & 3 == 1 {
        // SP → DP
        let rn = f32::from_bits(a.v[i.rn as usize] as u32);
        a.v[i.rd as usize] = f64::from(rn).to_bits() as u128;
    } else if i.ftype == 1 && (i.raw >> 15) & 3 == 0 {
        // DP → SP
        let rn = f64::from_bits(a.v[i.rn as usize] as u64);
        a.v[i.rd as usize] = (rn as f32).to_bits() as u128;
    }
}

pub(super) fn exec_fp_gpr_convert(a: &mut Aarch64ArchState, i: &Instruction) {
    match i.opcode {
        Opcode::FcvtzsGpr => {
            let rn = f64::from_bits(a.v[i.rn as usize] as u64);
            a.write_x(i.rd, rn as i64 as u64);
        }
        Opcode::FcvtzuGpr => {
            let rn = f64::from_bits(a.v[i.rn as usize] as u64);
            a.write_x(i.rd, rn as u64);
        }
        Opcode::ScvtfGpr => {
            let rn = a.read_x(i.rn) as i64 as f64;
            a.v[i.rd as usize] = rn.to_bits() as u128;
        }
        Opcode::UcvtfGpr => {
            let rn = a.read_x(i.rn) as f64;
            a.v[i.rd as usize] = rn.to_bits() as u128;
        }
        _ => {}
    }
}

pub(super) fn exec_fp_fused(a: &mut Aarch64ArchState, i: &Instruction) {
    if i.ftype == 1 {
        let rn = f64::from_bits(a.v[i.rn as usize] as u64);
        let rm = f64::from_bits(a.v[i.rm as usize] as u64);
        let ra = f64::from_bits(a.v[i.ra as usize] as u64);
        let res = match i.opcode {
            Opcode::Fmadd => rn * rm + ra,
            Opcode::Fmsub => -rn * rm + ra,
            Opcode::Fnmadd => -rn * rm - ra,
            Opcode::Fnmsub => rn * rm - ra,
            _ => 0.0,
        };
        a.v[i.rd as usize] = res.to_bits() as u128;
    }
}

// ── Memory fault conversion ───────────────────────────────────────────────────

pub(super) fn mem_fault_load(e: MemFault, addr: u64) -> HartException {
    match e {
        // Use the ISS from the MMU fault (correct DFSC level and fault class).
        MemFault::PageFault {
            iss,
            target_el,
            ipa,
            ..
        } => HartException::DataAbort {
            addr,
            iss,
            target_el,
            ipa,
        },
        _ => HartException::LoadAccessFault { addr },
    }
}
pub(super) fn mem_fault_store(e: MemFault, addr: u64) -> HartException {
    match e {
        // ISS already has correct DFSC; OR in WnR (bit 6) to indicate store.
        MemFault::PageFault {
            iss,
            target_el,
            ipa,
            ..
        } => HartException::DataAbort {
            addr,
            iss: iss | (1 << 6),
            target_el,
            ipa,
        },
        _ => HartException::StoreAccessFault { addr },
    }
}

pub(super) fn illegal_instruction(insn: &Instruction) -> HartException {
    HartException::IllegalInstruction {
        pc: insn.pc,
        raw: insn.raw,
    }
}
