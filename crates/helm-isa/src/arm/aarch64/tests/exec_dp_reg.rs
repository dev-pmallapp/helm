//! Comprehensive AArch64 data-processing (register) instruction tests.
//!
//! Each instruction family is tested in both 32-bit (sf=0) and 64-bit (sf=1)
//! variants.  Conditional-flag side-effects are verified where applicable.
//!
//! Encoding reference: crates/helm-isa/src/arm/decode_files/aarch64-dp-reg.decode
//! and QEMU qemu/a64.decode.

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

// ---------------------------------------------------------------------------
// Encoding helpers — derived from the QEMU a64.decode bit patterns
// ---------------------------------------------------------------------------

/// CSEL / CSINC / CSINV / CSNEG
/// sf else_inv 0 11010100 rm cond 0 else_inc rn rd
fn encode_csel_family(sf: u32, else_inv: u32, else_inc: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (else_inv << 30) | (0b011010100 << 21) | (rm << 16) | (cond << 12) | (else_inc << 10) | (rn << 5) | rd
}
fn encode_csel(sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 { encode_csel_family(sf, 0, 0, rm, cond, rn, rd) }
fn encode_csinc(sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 { encode_csel_family(sf, 0, 1, rm, cond, rn, rd) }
fn encode_csinv(sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 { encode_csel_family(sf, 1, 0, rm, cond, rn, rd) }
fn encode_csneg(sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 { encode_csel_family(sf, 1, 1, rm, cond, rn, rd) }

/// Data processing (2-source): sf 0 0 11010110 rm opcode rn rd
fn encode_dp2(sf: u32, opcode: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b0011010110 << 21) | (rm << 16) | (opcode << 10) | (rn << 5) | rd
}
fn encode_udiv(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_dp2(sf, 0b000010, rm, rn, rd) }
fn encode_sdiv(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_dp2(sf, 0b000011, rm, rn, rd) }
fn encode_lslv(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_dp2(sf, 0b001000, rm, rn, rd) }
fn encode_lsrv(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_dp2(sf, 0b001001, rm, rn, rd) }
fn encode_asrv(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_dp2(sf, 0b001010, rm, rn, rd) }
fn encode_rorv(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_dp2(sf, 0b001011, rm, rn, rd) }

/// Data processing (1-source): sf 1 0 11010110 00000 opcode rn rd
fn encode_dp1(sf: u32, opcode: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b1011010110 << 21) | (opcode << 10) | (rn << 5) | rd
}
fn encode_rbit(sf: u32, rn: u32, rd: u32) -> u32 { encode_dp1(sf, 0b000000, rn, rd) }
fn encode_rev16(sf: u32, rn: u32, rd: u32) -> u32 { encode_dp1(sf, 0b000001, rn, rd) }
fn encode_rev32(sf: u32, rn: u32, rd: u32) -> u32 { encode_dp1(sf, 0b000010, rn, rd) }
fn encode_rev(sf: u32, rn: u32, rd: u32) -> u32 { encode_dp1(sf, 0b000011, rn, rd) }
fn encode_clz(sf: u32, rn: u32, rd: u32) -> u32 { encode_dp1(sf, 0b000100, rn, rd) }
fn encode_cls(sf: u32, rn: u32, rd: u32) -> u32 { encode_dp1(sf, 0b000101, rn, rd) }

/// ADC / ADCS / SBC / SBCS: sf op S 11010000 rm 000000 rn rd
fn encode_adc_family(sf: u32, op: u32, s_flag: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s_flag << 29) | (0b11010000 << 21) | (rm << 16) | (rn << 5) | rd
}
fn encode_adc(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_adc_family(sf, 0, 0, rm, rn, rd) }
fn encode_adcs(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_adc_family(sf, 0, 1, rm, rn, rd) }
fn encode_sbc(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_adc_family(sf, 1, 0, rm, rn, rd) }
fn encode_sbcs(sf: u32, rm: u32, rn: u32, rd: u32) -> u32 { encode_adc_family(sf, 1, 1, rm, rn, rd) }

/// CCMP / CCMN: sf op 1 11010010 Rm cond 0 0 Rn 0 nzcv
fn encode_ccmp(sf: u32, rm: u32, cond: u32, rn: u32, nzcv: u32) -> u32 {
    (sf << 31) | (1 << 30) | (1 << 29) | (0b11010010 << 21) | (rm << 16)
        | (cond << 12) | (rn << 5) | nzcv
}
fn encode_ccmn(sf: u32, rm: u32, cond: u32, rn: u32, nzcv: u32) -> u32 {
    (sf << 31) | (0 << 30) | (1 << 29) | (0b11010010 << 21) | (rm << 16)
        | (cond << 12) | (rn << 5) | nzcv
}

/// Logical immediate: sf opc 100100 N immr imms rn rd
fn encode_and_imm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b00 << 29) | (0b100100 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn encode_orr_imm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b01 << 29) | (0b100100 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}

/// Bitfield: sf opc 100110 N immr imms rn rd
fn encode_sbfm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b00 << 29) | (0b100110 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn encode_bfm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b01 << 29) | (0b100110 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn encode_ubfm(sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b10 << 29) | (0b100110 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}

// Condition codes
const EQ: u32 = 0;
const NE: u32 = 1;
const CS: u32 = 2;
const CC: u32 = 3;
const MI: u32 = 4;
const PL: u32 = 5;
const HI: u32 = 8;
const LS: u32 = 9;
const GE: u32 = 10;
const LT: u32 = 11;
const GT: u32 = 12;
const LE: u32 = 13;
const AL: u32 = 14;

fn set_nzcv(cpu: &mut Aarch64Cpu, n: bool, z: bool, c: bool, v: bool) {
    cpu.regs.nzcv =
        ((n as u32) << 31) | ((z as u32) << 30) | ((c as u32) << 29) | ((v as u32) << 28);
}

// ===================================================================
//  Conditional Select — CSEL / CSINC / CSINV / CSNEG
//  Encoding: sf else_inv 0 11010100 rm cond 0 else_inc rn rd
// ===================================================================

#[test]
fn csel_64_eq_taken() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_csel(1, 2, EQ, 1, 0)]);
    cpu.set_xn(1, 0xAAAA);
    cpu.set_xn(2, 0xBBBB);
    set_nzcv(&mut cpu, false, true, false, false); // Z=1 → EQ true
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xAAAA, "CSEL should pick Rn when cond true");
}

#[test]
fn csel_64_eq_not_taken() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_csel(1, 2, EQ, 1, 0)]);
    cpu.set_xn(1, 0xAAAA);
    cpu.set_xn(2, 0xBBBB);
    set_nzcv(&mut cpu, false, false, false, false); // Z=0 → EQ false
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xBBBB, "CSEL should pick Rm when cond false");
}

#[test]
fn csel_32_truncates() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_csel(0, 2, NE, 1, 0)]);
    cpu.set_xn(1, 0x1_FFFF_FFFF);
    set_nzcv(&mut cpu, false, false, false, false); // Z=0 → NE true
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xFFFF_FFFF, "32-bit CSEL must zero-extend to 64");
}

#[test]
fn csinc_64_false_increments() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_csinc(1, 2, EQ, 1, 0)]);
    cpu.set_xn(2, 10);
    set_nzcv(&mut cpu, false, false, false, false); // EQ false
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 11, "CSINC: Rm+1 when cond false");
}

#[test]
fn csinc_32_false_increments() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_csinc(0, 2, EQ, 1, 0)]);
    cpu.set_xn(2, 0xFFFF_FFFF);
    set_nzcv(&mut cpu, false, false, false, false); // EQ false
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0, "32-bit CSINC wraps at 32 bits");
}

#[test]
fn csinv_64_false_inverts() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_csinv(1, 2, EQ, 1, 0)]);
    cpu.set_xn(2, 0);
    set_nzcv(&mut cpu, false, false, false, false); // EQ false
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), u64::MAX, "CSINV: ~Rm when cond false");
}

#[test]
fn csneg_64_false_negates() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_csneg(1, 2, EQ, 1, 0)]);
    cpu.set_xn(2, 5);
    set_nzcv(&mut cpu, false, false, false, false); // EQ false
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), (-5i64) as u64, "CSNEG: -Rm when cond false");
}

#[test]
fn csel_cc_condition() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_csel(1, 2, CC, 1, 0)]);
    cpu.set_xn(1, 0xAAAA);
    cpu.set_xn(2, 0xBBBB);
    set_nzcv(&mut cpu, false, false, true, false); // C=1 → CC is false
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xBBBB, "CC false when C=1");
}

#[test]
fn csel_lt_condition() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_csel(1, 2, LT, 1, 0)]);
    cpu.set_xn(1, 0xAAAA);
    cpu.set_xn(2, 0xBBBB);
    set_nzcv(&mut cpu, true, false, false, false); // N=1,V=0 → N≠V → LT true
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xAAAA, "LT true when N≠V");
}

// ===================================================================
//  Data Processing (2-source) — UDIV / SDIV / LSLV / LSRV / ASRV
// ===================================================================

#[test]
fn udiv_64() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_udiv(1, 2, 1, 0)]);
    cpu.set_xn(1, 100);
    cpu.set_xn(2, 7);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 14, "UDIV 64-bit: 100/7 = 14");
}

#[test]
fn udiv_32() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_udiv(0, 2, 1, 0)]);
    cpu.set_xn(1, 0x1_0000_0064); // upper 32 bits should be ignored
    cpu.set_xn(2, 10);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 10, "UDIV 32-bit: 0x64/10 = 10");
}

#[test]
fn udiv_by_zero() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_udiv(1, 2, 1, 0)]);
    cpu.set_xn(1, 42);
    cpu.set_xn(2, 0);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0, "UDIV by zero returns 0");
}

#[test]
fn sdiv_64_negative() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_sdiv(1, 2, 1, 0)]);
    cpu.set_xn(1, (-100i64) as u64);
    cpu.set_xn(2, 7);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), (-14i64) as u64, "SDIV 64-bit: -100/7 = -14");
}

#[test]
fn sdiv_32() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_sdiv(0, 2, 1, 0)]);
    cpu.set_xn(1, (-100i32) as u32 as u64);
    cpu.set_xn(2, 7);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), ((-14i32) as u32) as u64, "SDIV 32-bit: -100/7");
}

#[test]
fn lslv_64() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_lslv(1, 2, 1, 0)]);
    cpu.set_xn(1, 1);
    cpu.set_xn(2, 40);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 1u64 << 40, "LSLV 64-bit: 1 << 40");
}

#[test]
fn lslv_32() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_lslv(0, 2, 1, 0)]);
    cpu.set_xn(1, 1);
    cpu.set_xn(2, 16);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x10000, "LSLV 32-bit: 1 << 16");
}

#[test]
fn lsrv_64() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_lsrv(1, 2, 1, 0)]);
    cpu.set_xn(1, 0x8000_0000_0000_0000);
    cpu.set_xn(2, 63);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 1, "LSRV 64-bit: logical shift right");
}

#[test]
fn lsrv_32() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_lsrv(0, 2, 1, 0)]);
    cpu.set_xn(1, 0x8000_0000);
    cpu.set_xn(2, 31);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 1, "LSRV 32-bit: logical shift right");
}

#[test]
fn asrv_64_sign_extends() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_asrv(1, 2, 1, 0)]);
    cpu.set_xn(1, 0x8000_0000_0000_0000); // negative
    cpu.set_xn(2, 4);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xF800_0000_0000_0000, "ASRV 64-bit preserves sign");
}

#[test]
fn asrv_32_sign_extends() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_asrv(0, 2, 1, 0)]);
    cpu.set_xn(1, 0x8000_0000); // negative 32-bit
    cpu.set_xn(2, 4);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xF800_0000, "ASRV 32-bit: arithmetic shift preserves sign");
}

#[test]
fn rorv_64() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_rorv(1, 2, 1, 0)]);
    cpu.set_xn(1, 1);
    cpu.set_xn(2, 1);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x8000_0000_0000_0000, "RORV 64-bit: rotate 1 right by 1");
}

// ===================================================================
//  Data Processing (1-source) — RBIT / REV / CLZ / CLS
// ===================================================================

#[test]
fn rbit_64() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_rbit(1, 1, 0)]);
    cpu.set_xn(1, 0x8000_0000_0000_0001);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x8000_0000_0000_0001, "RBIT 64-bit palindrome");
}

#[test]
fn rbit_32() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_rbit(0, 1, 0)]);
    cpu.set_xn(1, 0x80000001);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x80000001, "RBIT 32-bit palindrome");
}

#[test]
fn rev_64() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_rev(1, 1, 0)]);
    cpu.set_xn(1, 0x01_02_03_04_05_06_07_08);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x08_07_06_05_04_03_02_01, "REV 64-bit byte swap");
}

#[test]
fn rev_32() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_rev(0, 1, 0)]);
    cpu.set_xn(1, 0x01_02_03_04);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x04_03_02_01, "REV 32-bit byte swap");
}

#[test]
fn rev16_64() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_rev16(1, 1, 0)]);
    cpu.set_xn(1, 0x0102_0304_0506_0708);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x0201_0403_0605_0807, "REV16 64-bit: swap bytes within halfwords");
}

#[test]
fn clz_64() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_clz(1, 1, 0)]);
    cpu.set_xn(1, 0x0000_0000_0000_0100);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 55, "CLZ 64-bit: 55 leading zeros");
}

#[test]
fn clz_32() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_clz(0, 1, 0)]);
    cpu.set_xn(1, 0x0000_0100);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 23, "CLZ 32-bit: 23 leading zeros");
}

#[test]
fn clz_64_zero_input() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_clz(1, 1, 0)]);
    cpu.set_xn(1, 0);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 64, "CLZ 64-bit of 0 = 64");
}

#[test]
fn clz_32_zero_input() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_clz(0, 1, 0)]);
    cpu.set_xn(1, 0);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 32, "CLZ 32-bit of 0 = 32");
}

#[test]
fn cls_64_positive() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_cls(1, 1, 0)]);
    cpu.set_xn(1, 0x0FFF_FFFF_FFFF_FFFF);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 3, "CLS 64-bit positive: 3 leading sign bits (after MSB)");
}

#[test]
fn cls_64_negative() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_cls(1, 1, 0)]);
    cpu.set_xn(1, 0xF000_0000_0000_0000);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 3, "CLS 64-bit negative: 3 leading 1s after MSB");
}

// ===================================================================
//  ADC / ADCS / SBC / SBCS
//  Encoding: sf op S 11010000 rm 000000 rn rd
// ===================================================================

#[test]
fn adc_64_with_carry() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_adc(1, 2, 1, 0)]);
    cpu.set_xn(1, 100);
    cpu.set_xn(2, 50);
    set_nzcv(&mut cpu, false, false, true, false); // C=1
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 151, "ADC: 100 + 50 + 1 = 151");
}

#[test]
fn adc_64_without_carry() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_adc(1, 2, 1, 0)]);
    cpu.set_xn(1, 100);
    cpu.set_xn(2, 50);
    set_nzcv(&mut cpu, false, false, false, false); // C=0
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 150, "ADC: 100 + 50 + 0 = 150");
}

#[test]
fn adc_32() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_adc(0, 2, 1, 0)]);
    cpu.set_xn(1, 0xFFFF_FFFF);
    cpu.set_xn(2, 1);
    set_nzcv(&mut cpu, false, false, true, false); // C=1
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 1, "ADC 32-bit: 0xFFFFFFFF + 1 + 1 wraps to 1");
}

#[test]
fn adcs_64_sets_zero_flag() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_adcs(1, 2, 1, 0)]);
    cpu.set_xn(1, 0);
    cpu.set_xn(2, 0);
    set_nzcv(&mut cpu, false, false, false, false); // C=0
    cpu.step(&mut mem).unwrap();
    assert!(cpu.regs.z(), "ADCS: 0+0+0 sets Z");
    assert!(!cpu.regs.n(), "ADCS: result not negative");
}

#[test]
fn sbc_64() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_sbc(1, 2, 1, 0)]);
    cpu.set_xn(1, 100);
    cpu.set_xn(2, 30);
    set_nzcv(&mut cpu, false, false, true, false); // C=1 (no borrow)
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 70, "SBC: 100 - 30 - 0 = 70");
}

#[test]
fn sbc_64_with_borrow() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_sbc(1, 2, 1, 0)]);
    cpu.set_xn(1, 100);
    cpu.set_xn(2, 30);
    set_nzcv(&mut cpu, false, false, false, false); // C=0 (borrow)
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 69, "SBC: 100 - 30 - 1 = 69");
}

#[test]
fn sbcs_64_sets_negative() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_sbcs(1, 2, 1, 0)]);
    cpu.set_xn(1, 5);
    cpu.set_xn(2, 10);
    set_nzcv(&mut cpu, false, false, true, false); // C=1
    cpu.step(&mut mem).unwrap();
    assert!(cpu.regs.n(), "SBCS: 5-10 is negative");
    assert!(!cpu.regs.z(), "SBCS: result not zero");
}

// ===================================================================
//  CCMP / CCMN
//  sf op 1 11010010 Rm cond 0 0 Rn 0 nzcv
// ===================================================================

#[test]
fn ccmp_64_cond_true_performs_compare() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_ccmp(1, 2, EQ, 1, 0b0000)]);
    cpu.set_xn(1, 10);
    cpu.set_xn(2, 10);
    set_nzcv(&mut cpu, false, true, false, false); // Z=1 → EQ true
    cpu.step(&mut mem).unwrap();
    assert!(cpu.regs.z(), "CCMP: 10-10 → Z");
    assert!(cpu.regs.c(), "CCMP: 10-10 → C (no borrow)");
}

#[test]
fn ccmp_64_cond_false_uses_nzcv_imm() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_ccmp(1, 2, EQ, 1, 0b1010)]);
    cpu.set_xn(1, 10);
    cpu.set_xn(2, 5);
    set_nzcv(&mut cpu, false, false, false, false); // Z=0 → EQ false
    cpu.step(&mut mem).unwrap();
    assert!(cpu.regs.n(), "CCMP cond-false: nzcv imm bit 3 (N)");
    assert!(!cpu.regs.z(), "CCMP cond-false: nzcv imm bit 2 (Z=0)");
    assert!(cpu.regs.c(), "CCMP cond-false: nzcv imm bit 1 (C)");
    assert!(!cpu.regs.v(), "CCMP cond-false: nzcv imm bit 0 (V=0)");
}

// ===================================================================
//  Logical Immediate — decode_bitmask sub-element rotation
// ===================================================================

#[test]
fn and_imm_32_sub_element_rotation() {
    // AND W0, W0, #mask where mask = 0xFFFFF000
    // N=0, immr=20, imms=19 → esize=32, rotate 20 within 32 bits
    let (mut cpu, mut mem) = cpu_with_code(&[encode_and_imm(0, 0, 20, 19, 0, 0)]);
    cpu.set_xn(0, 0x2000);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x2000, "AND W0, #0xFFFFF000 preserves 0x2000");
}

#[test]
fn and_imm_32_masks_low_bits() {
    let (mut cpu, mut mem) = cpu_with_code(&[encode_and_imm(0, 0, 20, 19, 0, 0)]);
    cpu.set_xn(0, 0x123);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0, "AND W0, #0xFFFFF000 clears low 12 bits");
}

#[test]
fn orr_imm_32_set_bit5() {
    // ORR W0, W0, #0x20: N=0, immr=27, imms=0
    let (mut cpu, mut mem) = cpu_with_code(&[encode_orr_imm(0, 0, 27, 0, 0, 0)]);
    cpu.set_xn(0, 0x06);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x26, "ORR W0, #0x20: sets bit 5");
}

// ===================================================================
//  Bitfield — BFM / SBFM / UBFM edge cases
// ===================================================================

#[test]
fn bfi_64_insert_at_bit12() {
    // BFI X0, X1, #12, #52 → BFM X0, X1, #52, #51
    let (mut cpu, mut mem) = cpu_with_code(&[encode_bfm(1, 1, 52, 51, 1, 0)]);
    cpu.set_xn(0, 0);
    cpu.set_xn(1, 2);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x2000, "BFI X0, X1, #12, #52: insert 2 at bit 12");
}

#[test]
fn ubfm_lsl_alias() {
    // LSL X0, X1, #4 = UBFM X0, X1, #60, #59
    let (mut cpu, mut mem) = cpu_with_code(&[encode_ubfm(1, 1, 60, 59, 1, 0)]);
    cpu.set_xn(1, 0xF);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xF0, "UBFM LSL alias: 0xF << 4 = 0xF0");
}

#[test]
fn ubfm_lsr_alias() {
    // LSR X0, X1, #4 = UBFM X0, X1, #4, #63
    let (mut cpu, mut mem) = cpu_with_code(&[encode_ubfm(1, 1, 4, 63, 1, 0)]);
    cpu.set_xn(1, 0xF0);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xF, "UBFM LSR alias: 0xF0 >> 4 = 0xF");
}

#[test]
fn sbfm_sxtw() {
    // SXTW X0, W1 = SBFM X0, X1, #0, #31
    let (mut cpu, mut mem) = cpu_with_code(&[encode_sbfm(1, 1, 0, 31, 1, 0)]);
    cpu.set_xn(1, 0xFFFF_FFFF); // -1 as i32
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xFFFF_FFFF_FFFF_FFFF, "SXTW sign-extends -1");
}

#[test]
fn sbfm_sxth() {
    // SXTH X0, W1 = SBFM X0, X1, #0, #15
    let (mut cpu, mut mem) = cpu_with_code(&[encode_sbfm(1, 1, 0, 15, 1, 0)]);
    cpu.set_xn(1, 0x8000); // -32768 as i16
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xFFFF_FFFF_FFFF_8000, "SXTH sign-extends -32768");
}

#[test]
fn ubfx_extracts_bits() {
    // UBFX W0, W1, #4, #8 = UBFM W0, W1, #4, #11
    let (mut cpu, mut mem) = cpu_with_code(&[encode_ubfm(0, 0, 4, 11, 1, 0)]);
    cpu.set_xn(1, 0xABCD);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xBC, "UBFX extracts bits [11:4] = 0xBC");
}
