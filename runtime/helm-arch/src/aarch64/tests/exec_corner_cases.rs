//! Corner-case tests for specific code paths. Ported from exec_corner_cases.rs.
#![allow(dead_code)]
use super::harness::*;
use crate::Aarch64ArchState;
use helm_core::{AccessType, HartException, MemFault, MemInterface};

const D: u64 = DATA_BASE;

struct FaultingMem {
    inner: TestMem,
    fault_addr: u64,
    fault_on_write: bool,
}

impl MemInterface for FaultingMem {
    fn read(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault> {
        if ty != AccessType::Fetch && !self.fault_on_write && addr == self.fault_addr {
            return Err(MemFault::AccessFault { addr });
        }
        self.inner.read(addr, size, ty)
    }

    fn write(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault> {
        if self.fault_on_write && addr == self.fault_addr {
            return Err(MemFault::AccessFault { addr });
        }
        self.inner.write(addr, size, val, ty)
    }
}

fn step_any(a: &mut Aarch64ArchState, mem: &mut impl MemInterface) -> Result<(), HartException> {
    let raw = mem
        .read(a.pc, 4, AccessType::Fetch)
        .map_err(|_| HartException::InstructionAccessFault { addr: a.pc })? as u32;
    let insn =
        crate::aarch64::decode(raw, a.pc).map_err(|_| HartException::IllegalInstruction {
            pc: a.pc,
            raw,
        })?;
    let pc_written = crate::aarch64::execute(&insn, a, mem)?;
    if !pc_written {
        a.pc = a.pc.wrapping_add(4);
    }
    Ok(())
}

fn add_sub_imm(sf: u32, op: u32, s: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (op << 30)
        | (s << 29)
        | (0b10001 << 24)
        | (sh << 22)
        | (imm12 << 10)
        | (rn << 5)
        | rd
}
fn mov_wide(sf: u32, opc: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}
fn add_sub_ext(sf: u32, op: u32, s: u32, rm: u32, option: u32, imm3: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (op << 30)
        | (s << 29)
        | (0b01011 << 24)
        | (1 << 21)
        | (rm << 16)
        | (option << 13)
        | (imm3 << 10)
        | (rn << 5)
        | rd
}
fn log_reg(sf: u32, opc: u32, n: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (opc << 29)
        | (0b01010 << 24)
        | (shift << 22)
        | (n << 21)
        | (rm << 16)
        | (imm6 << 10)
        | (rn << 5)
        | rd
}
fn add_sub_reg(sf: u32, op: u32, s: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (op << 30)
        | (s << 29)
        | (0b01011 << 24)
        | (shift << 22)
        | (rm << 16)
        | (imm6 << 10)
        | (rn << 5)
        | rd
}
fn stur_x(imm9: i32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30)
        | (0b111000 << 24)
        | (0b00 << 22)
        | (((imm9 as u32) & 0x1FF) << 12)
        | (0b00 << 10)
        | (rn << 5)
        | rt
}
fn ldur_x(imm9: i32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30)
        | (0b111000 << 24)
        | (0b01 << 22)
        | (((imm9 as u32) & 0x1FF) << 12)
        | (0b00 << 10)
        | (rn << 5)
        | rt
}
fn str_x_pre(imm9: i32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30)
        | (0b111000 << 24)
        | (0b00 << 22)
        | (((imm9 as u32) & 0x1FF) << 12)
        | (0b11 << 10)
        | (rn << 5)
        | rt
}
fn ldr_x_pre(imm9: i32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30)
        | (0b111000 << 24)
        | (0b01 << 22)
        | (((imm9 as u32) & 0x1FF) << 12)
        | (0b11 << 10)
        | (rn << 5)
        | rt
}
fn str_x_post(imm9: i32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30)
        | (0b111000 << 24)
        | (0b00 << 22)
        | (((imm9 as u32) & 0x1FF) << 12)
        | (0b01 << 10)
        | (rn << 5)
        | rt
}
fn ldr_x_post(imm9: i32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30)
        | (0b111000 << 24)
        | (0b01 << 22)
        | (((imm9 as u32) & 0x1FF) << 12)
        | (0b01 << 10)
        | (rn << 5)
        | rt
}
fn ldr_x_reg(rm: u32, option: u32, s_flag: u32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30)
        | (0b111000 << 24)
        | (0b01 << 22)
        | (1 << 21)
        | (rm << 16)
        | (option << 13)
        | (s_flag << 12)
        | (0b10 << 10)
        | (rn << 5)
        | rt
}

// --- SP disambiguation: rn=31 in ADD-imm is SP (not XZR per ARM spec) ---
#[test]
#[ignore = "rn=31 in ADD-imm is SP, not XZR; sp_el1=0x7FFF_8000 in test harness, result not 100"]
fn add_to_xzr_writes_rd_not_sp() {
    // ARM spec: rn=31 in ADD/SUB-imm is SP alias, not XZR.
    // This test assumes sp=0 but our harness sets sp_el1=STACK_BASE.
    let (mut c, mut m) = cpu_with_code(&[add_sub_imm(1, 0, 0, 0, 100, 31, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 100, "ADD X0, SP, #100 with SP=0");
}
#[test]
fn add_to_sp_uses_sp_el1() {
    let (mut c, mut m) = cpu_with_code(&[add_sub_imm(1, 0, 0, 0, 0, 31, 0)]);
    c.sp_el1 = 0x1000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x1000, "ADD X0, SP, #0 reads SP_EL1");
}

// --- 32-bit zero-extension ---
#[test]
fn add_32_zero_extends_result() {
    // ADD W0, W1, #0 where W1 = 0xFFFF_FFFF
    let (mut c, mut m) = cpu_with_code(&[add_sub_imm(0, 0, 0, 0, 0, 1, 0)]);
    c.x[1] = 0x1_FFFF_FFFF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xFFFF_FFFF, "32-bit zero-extends to 64");
}

// --- Flag preservation by non-flag-setting instructions ---
#[test]
fn add_imm_no_flag_update() {
    // ADD (no S) does not change flags
    let (mut c, mut m) = cpu_with_code(&[add_sub_imm(1, 0, 0, 0, 1, 1, 0)]);
    set_nzcv(&mut c, true, true, true, true);
    c.x[1] = 5;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 6);
    // Flags should be unchanged
    assert!(flag_n(&c));
    assert!(flag_z(&c));
    assert!(flag_c(&c));
    assert!(flag_v(&c));
}

// --- Logical register shifts ---
#[test]
fn and_reg_lsl() {
    // AND X0, X1, X2, LSL #8
    let (mut c, mut m) = cpu_with_code(&[log_reg(1, 0b00, 0, 0b00, 2, 8, 1, 0)]);
    c.x[1] = 0xFFFF;
    c.x[2] = 0x01;
    step(&mut c, &mut m).unwrap();
    assert_eq!(
        c.x[0], 0x100,
        "AND with LSL#8: X2<<8 = 0x100 & 0xFFFF = 0x100"
    );
}
#[test]
fn orr_reg_lsr() {
    // ORR X0, X1, X2, LSR #4
    let (mut c, mut m) = cpu_with_code(&[log_reg(1, 0b01, 0, 0b01, 2, 4, 1, 0)]);
    c.x[1] = 0;
    c.x[2] = 0xF0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xF, "ORR with LSR#4: 0xF0>>4 = 0xF");
}

// --- STUR/LDUR negative offsets ---
#[test]
fn stur_x_negative_offset() {
    let (mut c, mut m) = cpu_with_code(&[stur_x(-8, 2, 0), ldur_x(-8, 2, 1)]);
    c.x[0] = 0xDEAD_CAFE;
    c.x[2] = D + 16;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 0xDEAD_CAFE);
}

// --- Pre/post indexed single loads ---
#[test]
fn str_x_pre_index() {
    let (mut c, mut m) = cpu_with_code(&[str_x_pre(8, 2, 0), ldr_x_post(8, 2, 1)]);
    c.x[0] = 0xABCD;
    c.x[2] = D;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], D + 8, "pre-index updates base");
    assert_eq!(m.read_u64(D + 8), 0xABCD);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 0xABCD);
    assert_eq!(c.x[2], D + 16, "post-index updates base after");
}

// --- MOVN 32-bit truncation ---
#[test]
fn movn_w_zero_must_be_32bit() {
    let (mut c, mut m) = cpu_with_code(&[mov_wide(0, 0b00, 0, 0, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(
        c.x[0], 0xFFFF_FFFF,
        "MOVN W0, #0 must produce 0xFFFFFFFF not u64::MAX"
    );
}

// --- Extended register addressing ---
#[test]
fn add_ext_uxtw() {
    // ADD X0, X1, W2, UXTW: zero-extends W2 to 64 bits
    let (mut c, mut m) = cpu_with_code(&[add_sub_ext(1, 0, 0, 2, 0b010, 0, 1, 0)]);
    c.x[1] = 100;
    c.x[2] = 0x1_0000_0032; // W2 = 50 (0x32)
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 150, "ADD X0, X1, W2 UXTW: 100 + UXTW(50) = 150");
}

// --- LDR register offset ---
#[test]
fn ldr_x_reg_lsl() {
    // LDR X0, [X1, X2, LSL #3]
    let (mut c, mut m) = cpu_with_code(&[ldr_x_reg(2, 0b111, 1, 1, 0)]);
    m.load_u64(D + 8 * 2, 0xBEEF);
    c.x[1] = D;
    c.x[2] = 2;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xBEEF);
}

// --- Add/sub shifted register ---
#[test]
fn add_reg_asr_shift() {
    // ADD X0, X1, X2, ASR #4 (shift=10=0b10)
    let (mut c, mut m) = cpu_with_code(&[add_sub_reg(1, 0, 0, 0b10, 2, 4, 1, 0)]);
    c.x[1] = 100;
    c.x[2] = 0x8000_0000_0000_0000; // negative >> 4
    step(&mut c, &mut m).unwrap();
    let shifted = ((0x8000_0000_0000_0000u64 as i64) >> 4) as u64;
    assert_eq!(c.x[0], 100u64.wrapping_add(shifted));
}

// ── Additional encoding helpers ──────────────────────────────────────────
fn ldrb_reg(rm: u32, option: u32, s_flag: u32, rn: u32, rt: u32) -> u32 {
    (0b00_111_000_01_1 << 21)
        | (rm << 16)
        | (option << 13)
        | (s_flag << 12)
        | (0b10 << 10)
        | (rn << 5)
        | rt
}
fn ldr_w_reg(rm: u32, option: u32, s_flag: u32, rn: u32, rt: u32) -> u32 {
    (0b10_111_000_01_1 << 21)
        | (rm << 16)
        | (option << 13)
        | (s_flag << 12)
        | (0b10 << 10)
        | (rn << 5)
        | rt
}
fn encode_dp2(sf: u32, op: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b0011010110 << 21) | (rm << 16) | (op << 10) | (rn << 5) | rd
}
fn sturb(imm9: i32, rn: u32, rt: u32) -> u32 {
    let imm = (imm9 as u32) & 0x1FF;
    (0b00_111_000_00_0 << 21) | (imm << 12) | (rn << 5) | rt
}
fn ldurb(imm9: i32, rn: u32, rt: u32) -> u32 {
    let imm = (imm9 as u32) & 0x1FF;
    (0b00_111_000_01_0 << 21) | (imm << 12) | (rn << 5) | rt
}
fn stur_w(imm9: i32, rn: u32, rt: u32) -> u32 {
    let imm = (imm9 as u32) & 0x1FF;
    (0b10_111_000_00_0 << 21) | (imm << 12) | (rn << 5) | rt
}
fn ldur_w(imm9: i32, rn: u32, rt: u32) -> u32 {
    let imm = (imm9 as u32) & 0x1FF;
    (0b10_111_000_01_0 << 21) | (imm << 12) | (rn << 5) | rt
}
fn ldursb_x(imm9: i32, rn: u32, rt: u32) -> u32 {
    let imm = (imm9 as u32) & 0x1FF;
    (0b00_111_000_10_0 << 21) | (imm << 12) | (rn << 5) | rt
}
fn ldursw(imm9: i32, rn: u32, rt: u32) -> u32 {
    let imm = (imm9 as u32) & 0x1FF;
    (0b10_111_000_10_0 << 21) | (imm << 12) | (rn << 5) | rt
}
fn encode_b_cond(imm19: i32, cond: u32) -> u32 {
    let imm = (imm19 as u32) & 0x7FFFF;
    (0b01010100 << 24) | (imm << 5) | cond
}
fn encode_tbz(b5: u32, b40: u32, imm14: i32, rt: u32) -> u32 {
    let imm = (imm14 as u32) & 0x3FFF;
    (b5 << 31) | (0b011011 << 25) | (0 << 24) | (b40 << 19) | (imm << 5) | rt
}
fn encode_tbnz(b5: u32, b40: u32, imm14: i32, rt: u32) -> u32 {
    let imm = (imm14 as u32) & 0x3FFF;
    (b5 << 31) | (0b011011 << 25) | (1 << 24) | (b40 << 19) | (imm << 5) | rt
}
const NOP: u32 = 0xD503_201F;

// ── SP vs XZR disambiguation ─────────────────────────────────────────────
#[test]
fn add_imm_to_sp() {
    // ADD SP, SP, #16
    let insn = add_sub_imm(1, 0, 0, 0, 16, 31, 31);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    let orig_sp = a.sp_el1; // harness uses EL1 + SPSel=1
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.sp_el1, orig_sp + 16);
}

#[test]
fn sub_imm_from_sp() {
    let insn = add_sub_imm(1, 1, 0, 0, 16, 31, 31);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    let orig_sp = a.sp_el1; // harness uses EL1 + SPSel=1
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.sp_el1, orig_sp - 16);
}

#[test]
fn mov_to_sp_via_add() {
    // ADD SP, X1, #0 = move X1 to SP
    let insn = add_sub_imm(1, 0, 0, 0, 0, 1, 31);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 0x7FFF_8000;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.sp_el1, 0x7FFF_8000); // harness uses EL1 + SPSel=1
}

#[test]
fn add_to_xzr_discards() {
    // ADD X0, XZR, X1 — XZR as source reads 0
    let insn = add_sub_reg(1, 0, 0, 0, 1, 0, 31, 0);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 42;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 42);
}

#[test]
fn orr_to_xzr_discards() {
    // ORR XZR, X0, X1 — write to XZR is discarded
    let insn = log_reg(1, 0b01, 0, 0, 1, 0, 0, 31);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[0] = 100;
    a.x[1] = 200;
    step(&mut a, &mut m).unwrap();
    // No side effect observable — just check it doesn't crash
    assert_eq!(a.pc, super::harness::CODE_BASE + 4);
}

#[test]
fn adds_imm_rd31_is_xzr_not_sp() {
    // ADDS XZR, X1, #42 = CMN X1, #42
    let insn = add_sub_imm(1, 0, 1, 0, 42, 1, 31);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 0;
    step(&mut a, &mut m).unwrap();
    // Z flag set (0 + 42 != 0 so Z is clear actually, just check SP not written)
    assert_eq!(a.sp, super::harness::STACK_BASE, "SP should be unchanged");
}

// ── W-register truncation/zero-extension ────────────────────────────────
#[test]
fn add_w_clears_upper32() {
    let insn = add_sub_imm(0, 0, 0, 0, 1, 0, 0); // ADD W0, W0, #1 (sf=0)
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[0] = 0xFFFF_FFFF_FFFF_FFFF;
    step(&mut a, &mut m).unwrap();
    assert_eq!(
        a.x[0], 0x0000_0000_0000_0000,
        "32-bit ADD wraps and zero-extends"
    );
}

#[test]
fn orr_w_clears_upper32() {
    let insn = log_reg(0, 0b01, 0, 0, 1, 0, 0, 0); // ORR W0, W0, W1 (sf=0)
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[0] = 0xFFFF_FFFF_0000_0000;
    a.x[1] = 0x0000_0000_FFFF_FFFF;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] >> 32, 0, "ORR W should zero-extend result");
}

#[test]
fn sub_w_clears_upper32() {
    let insn = add_sub_imm(0, 1, 0, 0, 0, 0, 0); // SUB W0, W0, #0 (sf=0)
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[0] = 0xFFFF_FFFF_DEAD_BEEF;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] >> 32, 0, "32-bit result must zero-extend");
    assert_eq!(a.x[0], 0xDEAD_BEEF);
}

#[test]
fn ldr_w_clears_upper32() {
    let (mut a, mut m) = cpu_with_code(&[0xB940_0060]); // LDR W0, [X3]
    a.x[3] = super::harness::DATA_BASE;
    m.load_u32(super::harness::DATA_BASE, 0xDEAD_BEEF);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xDEAD_BEEF, "zero-extended from 32-bit load");
}

// ── Extended register encode tests ──────────────────────────────────────
#[test]
fn add_ext_uxtb() {
    // ADD X0, X1, W2, UXTB — option=000
    let insn = add_sub_ext(1, 0, 0, 2, 0b000, 0, 1, 0);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 100;
    a.x[2] = 0x1FF; // UXTB: only low 8 bits = 0xFF
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 100 + 0xFF);
}

#[test]
fn add_ext_uxtb_lsl1() {
    let insn = add_sub_ext(1, 0, 0, 2, 0b000, 1, 1, 0);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 0;
    a.x[2] = 0xFF;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xFF << 1);
}

#[test]
fn add_ext_uxtb_lsl2() {
    let insn = add_sub_ext(1, 0, 0, 2, 0b000, 2, 1, 0);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 0;
    a.x[2] = 4;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 4 << 2);
}

#[test]
fn add_ext_uxth() {
    let insn = add_sub_ext(1, 0, 0, 2, 0b001, 0, 1, 0);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 100;
    a.x[2] = 0x1FFFF; // UXTH: low 16 bits = 0xFFFF
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 100 + 0xFFFF);
}

#[test]
fn add_ext_sxtb() {
    let insn = add_sub_ext(1, 0, 0, 2, 0b100, 0, 1, 0);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 0;
    a.x[2] = 0x80; // SXTB: 0x80 → -128
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] as i64, -128);
}

#[test]
fn add_ext_sxth() {
    let insn = add_sub_ext(1, 0, 0, 2, 0b101, 0, 1, 0);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 0;
    a.x[2] = 0x8000; // SXTH: 0x8000 → -32768
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] as i64, -32768);
}

#[test]
fn add_ext_sxtw() {
    let insn = add_sub_ext(1, 0, 0, 2, 0b110, 0, 1, 0);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 0;
    a.x[2] = 0x8000_0000; // SXTW: MIN_i32
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] as i64, i32::MIN as i64);
}

#[test]
fn add_ext_sxtw_lsl3() {
    let insn = add_sub_ext(1, 0, 0, 2, 0b110, 3, 1, 0);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 0;
    a.x[2] = 1; // SXTW: 1 << 3 = 8
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 8);
}

// ── Flag preservation ────────────────────────────────────────────────────
#[test]
fn add_reg_preserves_flags() {
    let insn = add_sub_reg(1, 0, 0, 0, 1, 0, 0, 0); // ADD X0, X0, X1 (no S)
    let (mut a, mut m) = cpu_with_code(&[insn]);
    set_nzcv(&mut a, true, true, true, true);
    a.x[0] = 1;
    a.x[1] = 1;
    step(&mut a, &mut m).unwrap();
    assert!(flag_n(&a), "N should be preserved");
    assert!(flag_z(&a), "Z should be preserved");
    assert!(flag_c(&a), "C should be preserved");
    assert!(flag_v(&a), "V should be preserved");
}

#[test]
fn sub_reg_preserves_flags() {
    let insn = add_sub_reg(1, 1, 0, 0, 1, 0, 0, 0);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    set_nzcv(&mut a, true, false, true, false);
    a.x[0] = 5;
    a.x[1] = 3;
    step(&mut a, &mut m).unwrap();
    assert!(
        flag_n(&a) && flag_c(&a),
        "flags should be preserved by non-S SUB"
    );
}

#[test]
fn ldr_preserves_flags() {
    let (mut a, mut m) = cpu_with_code(&[0xF940_0060]); // LDR X0, [X3]
    a.x[3] = super::harness::DATA_BASE;
    m.load_u64(super::harness::DATA_BASE, 0xDEAD);
    set_nzcv(&mut a, true, true, false, true);
    step(&mut a, &mut m).unwrap();
    assert!(
        flag_n(&a) && flag_z(&a) && flag_v(&a),
        "LDR must not touch flags"
    );
}

// ── Load/store corner cases ──────────────────────────────────────────────
#[test]
fn ldrsb_x_pre_index_neg() {
    // LDRSB X0, [X3, #-1]!  = 0x389FFC60 (pre-index: bits[11:10]=11)
    let (mut a, mut m) = cpu_with_code(&[0x389F_FC60]);
    a.x[3] = super::harness::DATA_BASE + 1;
    m.load_u8(super::harness::DATA_BASE, 0x80u8); // -128 signed
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] as i64, -128, "LDRSB should sign-extend 0x80 to -128");
    assert_eq!(a.x[3], super::harness::DATA_BASE, "pre-index writeback");
}

#[test]
fn ldrsw_post_index() {
    // LDRSW X0, [X3], #4 (post-index: bits[11:10]=01)
    let (mut a, mut m) = cpu_with_code(&[0xB880_4460]); // LDRSW X0, [X3], #4
    a.x[3] = super::harness::DATA_BASE;
    m.load_u32(super::harness::DATA_BASE, 0x8000_0000);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] as i64, i32::MIN as i64, "LDRSW sign-extends");
    assert_eq!(
        a.x[3],
        super::harness::DATA_BASE + 4,
        "post-index writeback"
    );
}

#[test]
fn stur_ldur_x_negative() {
    let da = super::harness::DATA_BASE + 8;
    let (mut a, mut m) = cpu_with_code(&[stur_x(-8, 3, 0), ldur_x(-8, 3, 1)]);
    a.x[3] = da;
    a.x[0] = 0xCAFE_BABE_DEAD_BEEF;
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[1], 0xCAFE_BABE_DEAD_BEEF);
}

#[test]
fn stur_ldur_x_positive() {
    let (mut a, mut m) = cpu_with_code(&[stur_x(8, 3, 0), ldur_x(8, 3, 1)]);
    a.x[3] = super::harness::DATA_BASE;
    a.x[0] = 0x1234_5678;
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[1], 0x1234_5678);
}

#[test]
fn stur_ldur_x_zero() {
    let (mut a, mut m) = cpu_with_code(&[stur_x(0, 3, 0), ldur_x(0, 3, 1)]);
    a.x[3] = super::harness::DATA_BASE;
    a.x[0] = 0xABCD_EF01;
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[1], 0xABCD_EF01);
}

#[test]
fn sturb_ldurb() {
    let (mut a, mut m) = cpu_with_code(&[sturb(0, 3, 0), ldurb(0, 3, 1)]);
    a.x[3] = super::harness::DATA_BASE;
    a.x[0] = 0xDE_AD_BE_EF;
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[1], 0xEF, "STURB stores lowest byte, LDURB zero-extends");
}

#[test]
fn stur_ldur_w() {
    let (mut a, mut m) = cpu_with_code(&[stur_w(4, 3, 0), ldur_w(4, 3, 1)]);
    a.x[3] = super::harness::DATA_BASE;
    a.x[0] = 0xDEAD_BEEF;
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[1], 0xDEAD_BEEF);
}

#[test]
fn ldursb_x_negative_offset() {
    let (mut a, mut m) = cpu_with_code(&[ldursb_x(-4, 3, 0)]);
    a.x[3] = super::harness::DATA_BASE + 4;
    m.load_u8(super::harness::DATA_BASE, 0xFF);
    step(&mut a, &mut m).unwrap();
    assert_eq!(
        a.x[0] as i64, -1,
        "LDURSB with negative offset sign-extends"
    );
}

#[test]
fn ldursw_negative_offset() {
    let (mut a, mut m) = cpu_with_code(&[ldursw(-4, 3, 0)]);
    a.x[3] = super::harness::DATA_BASE + 4;
    m.load_u32(super::harness::DATA_BASE, 0xFFFF_FFFF);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] as i64, -1);
}

// ── Register-offset load/store ───────────────────────────────────────────
#[test]
fn ldr_x_reg_lsl3() {
    let (mut a, mut m) = cpu_with_code(&[ldr_x_reg(2, 0b011, 1, 3, 0)]);
    a.x[3] = super::harness::DATA_BASE;
    a.x[2] = 1; // LSL #3 → offset 8
    m.load_u64(super::harness::DATA_BASE + 8, 0x1234_5678);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x1234_5678);
}

#[test]
fn ldr_x_reg_no_shift() {
    let (mut a, mut m) = cpu_with_code(&[ldr_x_reg(2, 0b011, 0, 3, 0)]);
    a.x[3] = super::harness::DATA_BASE;
    a.x[2] = 8;
    m.load_u64(super::harness::DATA_BASE + 8, 0xABCD);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xABCD);
}

#[test]
fn ldr_x_reg_sxtw() {
    // LDR X0, [X3, W2, SXTW #3]
    let insn = ldr_x_reg(2, 0b110, 1, 3, 0);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[3] = super::harness::DATA_BASE + 16;
    a.x[2] = (-1i32) as u32 as u64; // W2 = -1, SXTW = -1, LSL#3 = -8
    m.load_u64(super::harness::DATA_BASE + 8, 0xFEED);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xFEED);
}

#[test]
fn ldr_x_pre_index() {
    let (mut a, mut m) = cpu_with_code(&[ldr_x_pre(8, 3, 0)]);
    a.x[3] = super::harness::DATA_BASE;
    m.load_u64(super::harness::DATA_BASE + 8, 0x5A5A_5A5A);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x5A5A_5A5A);
    assert_eq!(a.x[3], super::harness::DATA_BASE + 8);
}

#[test]
fn ldr_x_pre_index_fault_does_not_update_base() {
    let (mut a, mem) = cpu_with_code(&[ldr_x_pre(8, 3, 0)]);
    a.x[3] = super::harness::DATA_BASE;
    let mut m = FaultingMem {
        inner: mem,
        fault_addr: super::harness::DATA_BASE + 8,
        fault_on_write: false,
    };

    let err = step_any(&mut a, &mut m).unwrap_err();
    assert!(matches!(err, HartException::LoadAccessFault { addr } if addr == super::harness::DATA_BASE + 8));
    assert_eq!(
        a.x[3],
        super::harness::DATA_BASE,
        "faulting pre-index load must not commit base writeback"
    );
}

#[test]
fn ldr_x_post_index() {
    let (mut a, mut m) = cpu_with_code(&[ldr_x_post(8, 3, 0)]);
    a.x[3] = super::harness::DATA_BASE;
    m.load_u64(super::harness::DATA_BASE, 0xBEEF_CAFE);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xBEEF_CAFE);
    assert_eq!(a.x[3], super::harness::DATA_BASE + 8);
}

#[test]
fn str_x_post_index() {
    let (mut a, mut m) = cpu_with_code(&[str_x_post(8, 3, 0)]);
    a.x[3] = super::harness::DATA_BASE;
    a.x[0] = 0x1234;
    step(&mut a, &mut m).unwrap();
    assert_eq!(m.read_u64(super::harness::DATA_BASE), 0x1234);
    assert_eq!(a.x[3], super::harness::DATA_BASE + 8);
}

#[test]
fn str_x_pre_index_fault_does_not_update_base() {
    let (mut a, mem) = cpu_with_code(&[str_x_pre(8, 3, 0)]);
    a.x[3] = super::harness::DATA_BASE;
    a.x[0] = 0x1234_5678_9abc_def0;
    let mut m = FaultingMem {
        inner: mem,
        fault_addr: super::harness::DATA_BASE + 8,
        fault_on_write: true,
    };

    let err = step_any(&mut a, &mut m).unwrap_err();
    assert!(matches!(err, HartException::StoreAccessFault { addr } if addr == super::harness::DATA_BASE + 8));
    assert_eq!(
        a.x[3],
        super::harness::DATA_BASE,
        "faulting pre-index store must not commit base writeback"
    );
}

#[test]
fn ldr_x_reg_offset_exact_kernel_opcode() {
    // From pcpu_chunk_relocate: ldr x1, [x3, x4]
    let (mut a, mut m) = cpu_with_code(&[0xF864_6861]);
    a.x[3] = super::harness::DATA_BASE;
    a.x[4] = 16;
    m.load_u64(super::harness::DATA_BASE + 16, 0x1122_3344_5566_7788);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[1], 0x1122_3344_5566_7788);
}

#[test]
fn str_x_reg_offset_exact_kernel_opcode() {
    // From pcpu_chunk_relocate: str x0, [x3, x4]
    let (mut a, mut m) = cpu_with_code(&[0xF824_6860]);
    a.x[0] = 0x8877_6655_4433_2211;
    a.x[3] = super::harness::DATA_BASE;
    a.x[4] = 16;
    step(&mut a, &mut m).unwrap();
    assert_eq!(
        m.read_u64(super::harness::DATA_BASE + 16),
        0x8877_6655_4433_2211
    );
}

// ── Shift variable tests ─────────────────────────────────────────────────
#[test]
fn lslv_32_by33_is_mod32() {
    let insn = encode_dp2(0, 0b001000, 2, 1, 0); // LSLV W0, W1, W2
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 1;
    a.x[2] = 33; // 33 mod 32 = 1
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 2, "32-bit shift mod 32");
}

#[test]
fn lslv_64_by65_is_mod64() {
    let insn = encode_dp2(1, 0b001000, 2, 1, 0); // LSLV X0, X1, X2
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 1;
    a.x[2] = 65; // 65 mod 64 = 1
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 2, "64-bit shift mod 64");
}

#[test]
fn lsrv_64_by128_is_mod64() {
    let insn = encode_dp2(1, 0b001001, 2, 1, 0); // LSRV X0, X1, X2
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 0x8000_0000_0000_0000;
    a.x[2] = 128; // 128 mod 64 = 0 → no shift
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x8000_0000_0000_0000);
}

// ── MOVN 32-bit truncation (additional) ──────────────────────────────────
#[test]
fn movn_w_1_must_be_32bit() {
    let insn = mov_wide(0, 0b00, 0, 1, 0); // MOVN W0, #1
    let (mut a, mut m) = cpu_with_code(&[insn]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xFFFF_FFFE);
}

#[test]
fn movn_w_ffff_must_be_32bit() {
    let insn = mov_wide(0, 0b00, 0, 0xFFFF, 0); // MOVN W0, #0xFFFF
    let (mut a, mut m) = cpu_with_code(&[insn]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xFFFF_0000);
}

#[test]
fn movn_w_hw1_must_be_32bit() {
    let insn = mov_wide(0, 0b00, 1, 1, 0); // MOVN W0, #1, LSL #16
    let (mut a, mut m) = cpu_with_code(&[insn]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xFFFE_FFFF); // ~(1 << 16) & 0xFFFFFFFF
}

// ── MOVK 32-bit ──────────────────────────────────────────────────────────
#[test]
fn movk_w_hw0() {
    let movz = mov_wide(0, 0b10, 0, 0xFFFF, 0); // MOVZ W0, #0xFFFF
    let movk = mov_wide(0, 0b11, 1, 0x1234, 0); // MOVK W0, #0x1234, LSL #16
    let (mut a, mut m) = cpu_with_code(&[movz, movk]);
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    // MOVK W: result must be 32-bit zero-extended
    assert_eq!(a.x[0] >> 32, 0, "MOVK W must zero-extend to 64 bits");
    assert_eq!(a.x[0], 0x1234_FFFF);
}

#[test]
fn movk_w_hw1() {
    let movz = mov_wide(0, 0b10, 0, 0x1234, 0); // MOVZ W0, #0x1234
    let movk = mov_wide(0, 0b11, 1, 0x5678, 0); // MOVK W0, #0x5678, LSL #16
    let (mut a, mut m) = cpu_with_code(&[movz, movk]);
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] >> 32, 0);
    assert_eq!(a.x[0], 0x5678_1234);
}

#[test]
fn movk_w_must_truncate() {
    // Precondition: X0 has upper bits set, MOVK W should clear them
    let movk = mov_wide(0, 0b11, 0, 0xABCD, 0); // MOVK W0, #0xABCD
    let (mut a, mut m) = cpu_with_code(&[movk]);
    a.x[0] = 0xFFFF_FFFF_FFFF_0000; // upper bits poisoned
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0] >> 32, 0, "MOVK W must zero-extend");
}

// ── CMP extended register ────────────────────────────────────────────────
#[test]
fn cmp_ext_sxtw_equal() {
    let insn = add_sub_ext(1, 1, 1, 2, 0b110, 0, 1, 31); // SUBS XZR, X1, W2, SXTW
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 0xFFFF_FFFF_FFFF_FFFF; // -1 as i64
    a.x[2] = 0xFFFF_FFFF; // SXTW(-1) = -1
    step(&mut a, &mut m).unwrap();
    assert!(flag_z(&a), "CMP -1 == -1 should set Z");
}

#[test]
fn cmp_ext_sxtw_less() {
    let insn = add_sub_ext(1, 1, 1, 2, 0b110, 0, 1, 31);
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 0;
    a.x[2] = 0xFFFF_FFFF; // SXTW: -1
    step(&mut a, &mut m).unwrap();
    assert!(!flag_z(&a));
    assert!(!flag_n(&a), "0 - (-1) = 1, N should be clear");
}

#[test]
fn cmp_ext_uxtb() {
    let insn = add_sub_ext(1, 1, 1, 2, 0b000, 0, 1, 31); // CMP X1, W2, UXTB
    let (mut a, mut m) = cpu_with_code(&[insn]);
    a.x[1] = 255;
    a.x[2] = 0x1FF; // UXTB: 0xFF = 255
    step(&mut a, &mut m).unwrap();
    assert!(flag_z(&a));
}

// ── TBZ/TBNZ high bits ──────────────────────────────────────────────────
#[test]
fn tbz_bit32_taken() {
    let (mut a, mut m) = cpu_with_code(&[encode_tbz(1, 0, 2, 0), NOP, NOP]);
    a.x[0] = 0xFFFF_FFFE_FFFF_FFFF; // bit 32 = 0
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.pc, super::harness::CODE_BASE + 8);
}

#[test]
fn tbnz_bit48() {
    let (mut a, mut m) = cpu_with_code(&[encode_tbnz(1, 16, 2, 0), NOP, NOP]);
    a.x[0] = 1u64 << 48;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.pc, super::harness::CODE_BASE + 8);
}

// ── EXTR tests ──────────────────────────────────────────────────────────
#[test]
fn extr_32_ror_4() {
    let (mut a, mut m) = cpu_with_code(&[0x1382_0C20]); // EXTR W0, W1, W2, #3
    a.x[1] = 0xF0;
    a.x[2] = 0x0F;
    step(&mut a, &mut m).unwrap();
    // EXTR W0, W1, W2, #3: result = (W1 << (32-3)) | (W2 >> 3)
    // = (0xF0 << 29) | (0x0F >> 3) — 0xF0 << 29 overflows u32 to 0
    let expected = ((0xF0u32).wrapping_shl(29) | (0x0Fu32 >> 3)) as u64;
    assert_eq!(a.x[0], expected);
}

#[test]
fn b_cond_backward() {
    // B.NE -4 but with Z set — NE not taken
    let (mut a, mut m) = cpu_with_code(&[encode_b_cond(-1, 1), NOP]);
    set_nzcv(&mut a, false, true, false, false); // Z=1, so NE is not taken
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.pc, super::harness::CODE_BASE + 4, "NE not taken when Z=1");
}

#[test]
fn str_xzr_stores_zero() {
    let (mut a, mut m) = cpu_with_code(&[0xF900_0060]); // STR XZR, [X3]
    a.x[3] = super::harness::DATA_BASE;
    m.load_u64(super::harness::DATA_BASE, 0xDEAD_BEEF);
    step(&mut a, &mut m).unwrap();
    assert_eq!(m.read_u64(super::harness::DATA_BASE), 0, "STR XZR stores 0");
}

#[test]
fn clrex_is_nop() {
    // CLREX = 0xD503305F
    let (mut a, mut m) = cpu_with_code(&[0xD503_305F]);
    step(&mut a, &mut m).unwrap(); // should not crash
    assert_eq!(a.pc, super::harness::CODE_BASE + 4);
}
