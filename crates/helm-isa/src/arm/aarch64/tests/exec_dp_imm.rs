//! AArch64 Data Processing — Immediate instruction tests.
//!
//! Covers: ADR, ADRP, ADD/SUB imm (±flags), logical imm, MOV wide,
//! SBFM/BFM/UBFM (and all aliases), EXTR.
//! Each instruction tested in 32-bit and 64-bit variants with flag checks.

use crate::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;

fn cpu_with_code(insns: &[u32]) -> (Aarch64Cpu, AddressSpace) {
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();
    let base = 0x40_0000u64;
    let size = (insns.len() * 4 + 0x1000) as u64;
    mem.map(base, size, (true, true, true));
    for (i, insn) in insns.iter().enumerate() {
        let addr = base + (i as u64 * 4);
        mem.write(addr, &insn.to_le_bytes()).unwrap();
    }
    cpu.regs.pc = base;
    mem.map(0x7FFF_0000, 0x10000, (true, true, false));
    cpu.regs.sp = 0x7FFF_8000;
    (cpu, mem)
}

fn set_nzcv(cpu: &mut Aarch64Cpu, n: bool, z: bool, c: bool, v: bool) {
    cpu.regs.nzcv =
        ((n as u32) << 31) | ((z as u32) << 30) | ((c as u32) << 29) | ((v as u32) << 28);
}

// Encoding helpers (from aarch64-dp-imm.decode)

fn encode_add_imm(sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b10001 << 24) | (sh << 22) | (imm12 << 10) | (rn << 5) | rd
}
fn encode_adds_imm(sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (1 << 29) | (0b10001 << 24) | (sh << 22) | (imm12 << 10) | (rn << 5) | rd
}
fn encode_sub_imm(sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (1 << 30) | (0b10001 << 24) | (sh << 22) | (imm12 << 10) | (rn << 5) | rd
}
fn encode_subs_imm(sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (1 << 30) | (1 << 29) | (0b10001 << 24) | (sh << 22) | (imm12 << 10) | (rn << 5) | rd
}
fn encode_movz(sf: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (0b10 << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}
fn encode_movn(sf: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (0b00 << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}
fn encode_movk(sf: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (0b11 << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}
fn encode_and_imm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b00 << 29) | (0b100100 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn encode_orr_imm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b01 << 29) | (0b100100 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn encode_eor_imm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b10 << 29) | (0b100100 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn encode_ands_imm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b11 << 29) | (0b100100 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn encode_sbfm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b00 << 29) | (0b100110 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn encode_bfm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b01 << 29) | (0b100110 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn encode_ubfm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b10 << 29) | (0b100110 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn encode_extr(sf: u32, n: u32, rm: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b00 << 29) | (0b100111 << 23) | (n << 22) | (rm << 16) | (imms << 10) | (rn << 5) | rd
}
fn encode_adr(immhi: u32, immlo: u32, rd: u32) -> u32 {
    (immlo << 29) | (0b10000 << 24) | (immhi << 5) | rd
}
fn encode_adrp(immhi: u32, immlo: u32, rd: u32) -> u32 {
    (1 << 31) | (immlo << 29) | (0b10000 << 24) | (immhi << 5) | rd
}

// ===================================================================
//  ADD / SUB (immediate) — 32 and 64-bit, with and without flags
// ===================================================================

#[test] fn add_imm_64_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_add_imm(1, 0, 42, 1, 0)]);
    c.set_xn(1, 100); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 142);
}
#[test] fn add_imm_32_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_add_imm(0, 0, 42, 1, 0)]);
    c.set_xn(1, 100); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 142);
}
#[test] fn add_imm_64_shifted() {
    let (mut c, mut m) = cpu_with_code(&[encode_add_imm(1, 1, 1, 1, 0)]);
    c.set_xn(1, 0); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x1000, "ADD X0, X1, #1, LSL #12");
}
#[test] fn add_imm_32_wraps() {
    let (mut c, mut m) = cpu_with_code(&[encode_add_imm(0, 0, 1, 1, 0)]);
    c.set_xn(1, 0xFFFF_FFFF); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0, "32-bit ADD wraps");
}
#[test] fn add_imm_64_max() {
    let (mut c, mut m) = cpu_with_code(&[encode_add_imm(1, 0, 0xFFF, 1, 0)]);
    c.set_xn(1, 0); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFF, "max imm12");
}
#[test] fn sub_imm_64_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_sub_imm(1, 0, 10, 1, 0)]);
    c.set_xn(1, 50); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 40);
}
#[test] fn sub_imm_32_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_sub_imm(0, 0, 10, 1, 0)]);
    c.set_xn(1, 50); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 40);
}
#[test] fn sub_imm_32_wraps() {
    let (mut c, mut m) = cpu_with_code(&[encode_sub_imm(0, 0, 1, 1, 0)]);
    c.set_xn(1, 0); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF, "32-bit SUB underflow wraps");
}
#[test] fn sub_imm_64_shifted() {
    let (mut c, mut m) = cpu_with_code(&[encode_sub_imm(1, 1, 1, 1, 0)]);
    c.set_xn(1, 0x2000); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x1000, "SUB X0, X1, #1, LSL #12");
}

// --- ADDS / SUBS flags ------------------------------------------------

#[test] fn adds_imm_64_zero_flag() {
    let (mut c, mut m) = cpu_with_code(&[encode_adds_imm(1, 0, 0, 1, 31)]);
    c.set_xn(1, 0); c.step(&mut m).unwrap();
    assert!(c.regs.z(), "ADDS 0+0 sets Z");
    assert!(!c.regs.n());
}
#[test] fn adds_imm_64_negative_flag() {
    let (mut c, mut m) = cpu_with_code(&[encode_adds_imm(1, 0, 1, 1, 31)]);
    c.set_xn(1, u64::MAX); c.step(&mut m).unwrap();
    assert!(c.regs.c(), "ADDS overflow sets C");
    assert!(c.regs.z(), "0xFFFFFFFF_FFFFFFFF + 1 = 0 sets Z");
}
#[test] fn adds_imm_32_carry() {
    let (mut c, mut m) = cpu_with_code(&[encode_adds_imm(0, 0, 1, 1, 31)]);
    c.set_xn(1, 0xFFFF_FFFF); c.step(&mut m).unwrap();
    assert!(c.regs.c(), "32-bit ADDS carry");
    assert!(c.regs.z(), "32-bit wraps to 0");
}
#[test] fn subs_imm_64_equal() {
    let (mut c, mut m) = cpu_with_code(&[encode_subs_imm(1, 0, 42, 1, 31)]);
    c.set_xn(1, 42); c.step(&mut m).unwrap();
    assert!(c.regs.z(), "CMP 42,42 → Z");
    assert!(c.regs.c(), "CMP 42,42 → C (no borrow)");
}
#[test] fn subs_imm_64_less() {
    let (mut c, mut m) = cpu_with_code(&[encode_subs_imm(1, 0, 100, 1, 31)]);
    c.set_xn(1, 50); c.step(&mut m).unwrap();
    assert!(c.regs.n(), "CMP 50,100 → N");
    assert!(!c.regs.c(), "CMP 50,100 → no C (borrow)");
}
#[test] fn subs_imm_32_flags() {
    let (mut c, mut m) = cpu_with_code(&[encode_subs_imm(0, 0, 1, 1, 31)]);
    c.set_xn(1, 1); c.step(&mut m).unwrap();
    assert!(c.regs.z()); assert!(c.regs.c());
}
#[test] fn subs_imm_64_overflow() {
    let (mut c, mut m) = cpu_with_code(&[encode_subs_imm(1, 0, 1, 1, 0)]);
    c.set_xn(1, 0x8000_0000_0000_0000); c.step(&mut m).unwrap();
    assert!(c.regs.v(), "signed overflow: MIN_i64 - 1");
}
#[test] fn adds_imm_64_signed_overflow() {
    let (mut c, mut m) = cpu_with_code(&[encode_adds_imm(1, 0, 1, 1, 0)]);
    c.set_xn(1, 0x7FFF_FFFF_FFFF_FFFF); c.step(&mut m).unwrap();
    assert!(c.regs.v(), "signed overflow: MAX_i64 + 1");
    assert!(c.regs.n(), "result is negative");
}

// ===================================================================
//  MOVZ / MOVN / MOVK
// ===================================================================

#[test] fn movz_64_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_movz(1, 0, 0x1234, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x1234);
}
#[test] fn movz_64_hw1() {
    let (mut c, mut m) = cpu_with_code(&[encode_movz(1, 1, 0xABCD, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xABCD_0000);
}
#[test] fn movz_64_hw2() {
    let (mut c, mut m) = cpu_with_code(&[encode_movz(1, 2, 0xFFFF, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_0000_0000);
}
#[test] fn movz_64_hw3() {
    let (mut c, mut m) = cpu_with_code(&[encode_movz(1, 3, 1, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 1u64 << 48);
}
#[test] fn movz_32_clears_upper() {
    let (mut c, mut m) = cpu_with_code(&[encode_movz(0, 0, 0xFFFF, 0)]);
    c.set_xn(0, u64::MAX); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF);
}
#[test] fn movn_64_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_movn(1, 0, 0, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), u64::MAX, "MOVN #0 = ~0 = all ones");
}
#[test] fn movn_32_basic() {
    let (mut c, mut m) = cpu_with_code(&[encode_movn(0, 0, 0, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0) & 0xFFFF_FFFF, 0xFFFF_FFFF, "32-bit MOVN low bits");
}
#[test] fn movk_preserves_other_hw() {
    let (mut c, mut m) = cpu_with_code(&[encode_movk(1, 0, 0x5678, 0)]);
    c.set_xn(0, 0xAAAA_0000_0000_0000); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xAAAA_0000_0000_5678, "MOVK only replaces hw0");
}
#[test] fn movk_hw2() {
    let (mut c, mut m) = cpu_with_code(&[encode_movk(1, 2, 0x1111, 0)]);
    c.set_xn(0, 0xFFFF_FFFF_FFFF_FFFF); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_1111_FFFF_FFFF);
}
#[test] fn movz_movk_chain() {
    let (mut c, mut m) = cpu_with_code(&[
        encode_movz(1, 3, 0x0001, 0),
        encode_movk(1, 2, 0x0002, 0),
        encode_movk(1, 1, 0x0003, 0),
        encode_movk(1, 0, 0x0004, 0),
    ]);
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x0001_0002_0003_0004);
}

// ===================================================================
//  Logical immediate — AND / ORR / EOR / ANDS
// ===================================================================

#[test] fn and_imm_64_all_ones() {
    // AND X0, X1, #0xFFFFFFFFFFFFFFFF (N=1, immr=0, imms=63)
    let (mut c, mut m) = cpu_with_code(&[encode_and_imm(1, 1, 0, 63, 1, 0)]);
    c.set_xn(1, 0xDEAD_BEEF); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xDEAD_BEEF);
}
#[test] fn and_imm_32_low_byte() {
    // AND W0, W1, #0xFF (N=0, immr=0, imms=7)
    let (mut c, mut m) = cpu_with_code(&[encode_and_imm(0, 0, 0, 7, 1, 0)]);
    c.set_xn(1, 0x1234_5678); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x78);
}
#[test] fn orr_imm_64_set_bits() {
    // ORR X0, XZR, #0xFF (N=1, immr=0, imms=7) — MOV X0, #0xFF
    let (mut c, mut m) = cpu_with_code(&[encode_orr_imm(1, 1, 0, 7, 31, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFF);
}
#[test] fn eor_imm_64() {
    // EOR X0, X1, #0xFF
    let (mut c, mut m) = cpu_with_code(&[encode_eor_imm(1, 1, 0, 7, 1, 0)]);
    c.set_xn(1, 0xAA); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x55);
}
#[test] fn ands_imm_64_sets_zero() {
    // TST X1, #0xFF (ANDS XZR, X1, #0xFF)
    let (mut c, mut m) = cpu_with_code(&[encode_ands_imm(1, 1, 0, 7, 1, 31)]);
    c.set_xn(1, 0x100); c.step(&mut m).unwrap();
    assert!(c.regs.z(), "TST: no bits in common → Z");
}
#[test] fn ands_imm_64_sets_negative() {
    let (mut c, mut m) = cpu_with_code(&[encode_ands_imm(1, 1, 0, 63, 1, 0)]);
    c.set_xn(1, 0x8000_0000_0000_0000); c.step(&mut m).unwrap();
    assert!(c.regs.n(), "ANDS sets N when result MSB set");
}
#[test] fn and_imm_32_rotation() {
    // AND W0, W0, #0xFFFFF000 (N=0, immr=20, imms=19)
    let (mut c, mut m) = cpu_with_code(&[encode_and_imm(0, 0, 20, 19, 0, 0)]);
    c.set_xn(0, 0x12345678); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x12345000);
}
#[test] fn orr_imm_32_set_bit5() {
    // ORR W0, W0, #0x20 (N=0, immr=27, imms=0)
    let (mut c, mut m) = cpu_with_code(&[encode_orr_imm(0, 0, 27, 0, 0, 0)]);
    c.set_xn(0, 0x41); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x61, "ORR sets bit 5: 'A'→'a'");
}
#[test] fn and_imm_32_repeated_pattern() {
    // AND W0, W1, #0x55555555 (alternating bits, esize=2)
    // N=0, immr=0, imms=0 → esize=2, welem=0b01, replicated
    let (mut c, mut m) = cpu_with_code(&[encode_and_imm(0, 0, 0, 60, 1, 0)]);
    c.set_xn(1, 0xFFFF_FFFF); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x5555_5555, "repeated 2-bit pattern");
}

// ===================================================================
//  SBFM / UBFM / BFM — aliases and edge cases
// ===================================================================

#[test] fn sbfm_sxtb() {
    // SXTB X0, W1 = SBFM X0, X1, #0, #7
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 0, 7, 1, 0)]);
    c.set_xn(1, 0x80); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF_FFFF_FF80, "SXTB sign-extends 0x80");
}
#[test] fn sbfm_sxtb_positive() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 0, 7, 1, 0)]);
    c.set_xn(1, 0x7F); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x7F, "SXTB of 0x7F stays positive");
}
#[test] fn sbfm_sxth() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 0, 15, 1, 0)]);
    c.set_xn(1, 0xFFFF); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF_FFFF_FFFF, "SXTH: -1");
}
#[test] fn sbfm_sxtw() {
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 0, 31, 1, 0)]);
    c.set_xn(1, 0x8000_0000); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF_8000_0000, "SXTW: MIN_i32");
}
#[test] fn sbfm_asr_64() {
    // ASR X0, X1, #4 = SBFM X0, X1, #4, #63
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 4, 63, 1, 0)]);
    c.set_xn(1, 0x8000_0000_0000_0000); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xF800_0000_0000_0000, "ASR preserves sign");
}
#[test] fn sbfm_asr_32() {
    // ASR W0, W1, #4 = SBFM W0, W1, #4, #31
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(0, 0, 4, 31, 1, 0)]);
    c.set_xn(1, 0x8000_0000); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xF800_0000, "32-bit ASR preserves sign");
}
#[test] fn sbfm_sbfx() {
    // SBFX X0, X1, #8, #8 = SBFM X0, X1, #8, #15
    let (mut c, mut m) = cpu_with_code(&[encode_sbfm(1, 1, 8, 15, 1, 0)]);
    c.set_xn(1, 0xFF00); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF_FFFF_FFFF, "SBFX: extract byte 0xFF, sign-extend to -1");
}
#[test] fn ubfm_lsl_64() {
    // LSL X0, X1, #4 = UBFM X0, X1, #60, #59
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(1, 1, 60, 59, 1, 0)]);
    c.set_xn(1, 0xF); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xF0);
}
#[test] fn ubfm_lsr_64() {
    // LSR X0, X1, #4 = UBFM X0, X1, #4, #63
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(1, 1, 4, 63, 1, 0)]);
    c.set_xn(1, 0xF0); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xF);
}
#[test] fn ubfm_lsl_32() {
    // LSL W0, W1, #4 = UBFM W0, W1, #28, #27
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(0, 0, 28, 27, 1, 0)]);
    c.set_xn(1, 0xF); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xF0);
}
#[test] fn ubfm_lsr_32() {
    // LSR W0, W1, #4 = UBFM W0, W1, #4, #31
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(0, 0, 4, 31, 1, 0)]);
    c.set_xn(1, 0xF0); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xF);
}
#[test] fn ubfm_uxtb() {
    // UXTB W0, W1 = UBFM W0, W1, #0, #7
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(0, 0, 0, 7, 1, 0)]);
    c.set_xn(1, 0x1234_5680); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x80);
}
#[test] fn ubfm_uxth() {
    // UXTH W0, W1 = UBFM W0, W1, #0, #15
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(0, 0, 0, 15, 1, 0)]);
    c.set_xn(1, 0xFFFF_ABCD); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xABCD);
}
#[test] fn ubfm_ubfx() {
    // UBFX W0, W1, #4, #8 = UBFM W0, W1, #4, #11
    let (mut c, mut m) = cpu_with_code(&[encode_ubfm(0, 0, 4, 11, 1, 0)]);
    c.set_xn(1, 0xABCD); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xBC);
}
#[test] fn bfm_bfi_64() {
    // BFI X0, X1, #12, #52 = BFM X0, X1, #52, #51
    let (mut c, mut m) = cpu_with_code(&[encode_bfm(1, 1, 52, 51, 1, 0)]);
    c.set_xn(0, 0); c.set_xn(1, 2); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x2000);
}
#[test] fn bfm_bfi_32() {
    // BFI W0, W1, #8, #8 = BFM W0, W1, #24, #7
    let (mut c, mut m) = cpu_with_code(&[encode_bfm(0, 0, 24, 7, 1, 0)]);
    c.set_xn(0, 0xFF); c.set_xn(1, 0xAB); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xABFF);
}
#[test] fn bfm_bfi_32_low() {
    // BFI W0, W1, #0, #8 = BFM W0, W1, #0, #7
    let (mut c, mut m) = cpu_with_code(&[encode_bfm(0, 0, 0, 7, 1, 0)]);
    c.set_xn(0, 0xFF00); c.set_xn(1, 0xAB);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFAB, "BFI low 8 bits");
}

// ===================================================================
//  EXTR (extract / ROR alias)
// ===================================================================

#[test] fn extr_64_ror() {
    // ROR X0, X1, #4 = EXTR X0, X1, X1, #4
    let (mut c, mut m) = cpu_with_code(&[encode_extr(1, 1, 1, 4, 1, 0)]);
    c.set_xn(1, 0xF); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xF000_0000_0000_0000);
}
#[test] fn extr_32_ror() {
    // ROR W0, W1, #4 = EXTR W0, W1, W1, #4
    let (mut c, mut m) = cpu_with_code(&[encode_extr(0, 0, 1, 4, 1, 0)]);
    c.set_xn(1, 0xF); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xF000_0000);
}
#[test] fn extr_64_concat() {
    // EXTR X0, X1, X2, #8: take high 56 bits of X1 and low 8 of X2 shifted
    let (mut c, mut m) = cpu_with_code(&[encode_extr(1, 1, 2, 8, 1, 0)]);
    c.set_xn(1, 0xFF);
    c.set_xn(2, 0xAB00_0000_0000_0000);
    c.step(&mut m).unwrap();
    // result = (X1:X2) >> 8 = (0xFF : 0xAB00..00) >> 8
    // = high 56 of concat then low 8 from X2 top
    assert_eq!(c.xn(0), 0xFFAB_0000_0000_0000);
}

// ===================================================================
//  ADR / ADRP
// ===================================================================

#[test] fn adr_forward() {
    let (mut c, mut m) = cpu_with_code(&[encode_adr(0x20, 0, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x40_0000 + 0x80);
}
#[test] fn adr_backward() {
    // ADR X0, .-1 (immhi=0x7FFFF, immlo=3 → signed imm = -1)
    let (mut c, mut m) = cpu_with_code(&[encode_adr(0x7FFFF, 3, 0)]);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x40_0000u64.wrapping_sub(1));
}
#[test] fn adrp_page_aligned() {
    let (mut c, mut m) = cpu_with_code(&[encode_adrp(1, 0, 0)]);
    c.step(&mut m).unwrap();
    let expected = (0x40_0000u64 & !0xFFF) + 0x4000;
    assert_eq!(c.xn(0), expected);
}
