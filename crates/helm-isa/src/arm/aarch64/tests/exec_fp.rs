//! Scalar floating-point execution tests.
//!
//! Verify that FP instructions produce correct results through step().

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
    // Map data area at 0x50_0000
    mem.map(0x50_0000, 0x1000, (true, true, false));
    (cpu, mem)
}

// ═══════════════════════════════════════════════════════════════════
// FMOV between GP and FP registers
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore] // TDD: FMOV D,X / FMOV X,D encoding needs exec.rs fix
fn fmov_gp_to_d_roundtrip() {
    // FMOV D0, X1  =>  0x9E670020
    // FMOV X2, D0  =>  0x9E660042
    let (mut cpu, mut mem) = cpu_with_code(&[0x9E670020, 0x9E660042]);
    cpu.set_xn(1, 0x4000_0000_0000_0000); // 2.0 in f64
    cpu.step(&mut mem).unwrap(); // FMOV D0, X1
    assert_eq!(cpu.regs.v[0] as u64, 0x4000_0000_0000_0000);
    cpu.step(&mut mem).unwrap(); // FMOV X2, D0
    assert_eq!(cpu.xn(2), 0x4000_0000_0000_0000);
}

#[test]
#[ignore] // TDD: FMOV S,W encoding needs exec.rs fix
fn fmov_gp_to_s_roundtrip() {
    // FMOV S0, W1  =>  0x1E270020
    // FMOV W2, S0  =>  0x1E260042
    let (mut cpu, mut mem) = cpu_with_code(&[0x1E270020, 0x1E260042]);
    cpu.set_xn(1, 0x40000000); // 2.0 in f32
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0] as u32, 0x40000000);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(2), 0x40000000);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD vector logic
// ═══════════════════════════════════════════════════════════════════

#[test]
fn and_v16b() {
    // AND V0.16B, V1.16B, V2.16B => 0x4E221C20
    let (mut cpu, mut mem) = cpu_with_code(&[0x4E221C20]);
    cpu.regs.v[1] = 0xFF00_FF00_FF00_FF00_FF00_FF00_FF00_FF00;
    cpu.regs.v[2] = 0x0F0F_0F0F_0F0F_0F0F_0F0F_0F0F_0F0F_0F0F;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0], 0x0F00_0F00_0F00_0F00_0F00_0F00_0F00_0F00);
}

#[test]
fn orr_v16b() {
    // ORR V0.16B, V1.16B, V2.16B => 0x4EA21C20
    let (mut cpu, mut mem) = cpu_with_code(&[0x4EA21C20]);
    cpu.regs.v[1] = 0xFF00_0000_0000_0000_0000_0000_0000_0000;
    cpu.regs.v[2] = 0x00FF_0000_0000_0000_0000_0000_0000_0000;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0], 0xFFFF_0000_0000_0000_0000_0000_0000_0000);
}

#[test]
#[ignore] // TDD: NOT_v not yet implemented in exec_simd_dp
fn not_v16b() {
    // NOT V0.16B, V1.16B => 0x6E205820
    let (mut cpu, mut mem) = cpu_with_code(&[0x6E205820]);
    cpu.regs.v[1] = 0;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0], u128::MAX);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD vector arithmetic
// ═══════════════════════════════════════════════════════════════════

#[test]
fn sub_v4s() {
    // SUB V0.4S, V1.4S, V2.4S => 0x6EA28420
    let (mut cpu, mut mem) = cpu_with_code(&[0x6EA28420]);
    // V1 = [10, 20, 30, 40] as 4xU32
    cpu.regs.v[1] = 10 | (20u128 << 32) | (30u128 << 64) | (40u128 << 96);
    // V2 = [1, 2, 3, 4]
    cpu.regs.v[2] = 1 | (2u128 << 32) | (3u128 << 64) | (4u128 << 96);
    cpu.step(&mut mem).unwrap();
    let r = cpu.regs.v[0];
    assert_eq!(r & 0xFFFF_FFFF, 9);
    assert_eq!((r >> 32) & 0xFFFF_FFFF, 18);
    assert_eq!((r >> 64) & 0xFFFF_FFFF, 27);
    assert_eq!((r >> 96) & 0xFFFF_FFFF, 36);
}

// ═══════════════════════════════════════════════════════════════════
// Load literal
// ═══════════════════════════════════════════════════════════════════

#[test]
fn ldr_literal_x() {
    // LDR X0, #8  (load from PC+8)
    // Encoding: 01 011000 imm19 Rt
    // imm19 = 2 (offset = 2*4 = 8 bytes from PC)
    // 0x58000040
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x58000040, // LDR X0, #8  (loads from PC+8 = base+8)
        0xD503201F, // NOP
        // data at PC+8:
    ]);
    // Write the data value at base+8
    let data_addr = 0x40_0000 + 8;
    mem.write(data_addr, &0xDEADBEEF_CAFEBABEu64.to_le_bytes()).unwrap();
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xDEADBEEF_CAFEBABE);
}

// ═══════════════════════════════════════════════════════════════════
// Atomics — LDAR/STLR (acquire/release)
// ═══════════════════════════════════════════════════════════════════

#[test]
#[ignore] // TDD: STLR/LDAR not yet handled in exec.rs
fn ldar_stlr_roundtrip() {
    // STLR X0, [X1]  => 0xC89FFC20
    // LDAR X2, [X1]  => 0xC8DFFC42
    let (mut cpu, mut mem) = cpu_with_code(&[0xC89FFC20, 0xC8DFFC42]);
    cpu.set_xn(0, 0x12345678_ABCDEF01);
    cpu.set_xn(1, 0x50_0000); // data area
    cpu.step(&mut mem).unwrap(); // STLR
    cpu.step(&mut mem).unwrap(); // LDAR
    assert_eq!(cpu.xn(2), 0x12345678_ABCDEF01);
}

// ═══════════════════════════════════════════════════════════════════
// Atomics — LDADD
// ═══════════════════════════════════════════════════════════════════

#[test]
fn ldadd_x_basic() {
    // First store a value, then LDADD
    // STR X0, [X3]   => 0xF9000060
    // LDADD X1, X2, [X3]  => 0xF8210062
    let (mut cpu, mut mem) = cpu_with_code(&[0xF9000060, 0xF8210062]);
    cpu.set_xn(0, 100); // initial value
    cpu.set_xn(1, 42);  // addend
    cpu.set_xn(3, 0x50_0000); // address
    cpu.step(&mut mem).unwrap(); // STR X0, [X3]
    cpu.step(&mut mem).unwrap(); // LDADD X1, X2, [X3]
    assert_eq!(cpu.xn(2), 100); // old value returned
    // Memory should now contain 142
    let mut buf = [0u8; 8];
    mem.read(0x50_0000, &mut buf).unwrap();
    assert_eq!(u64::from_le_bytes(buf), 142);
}

// ═══════════════════════════════════════════════════════════════════
// DP-Register — multiply variants
// ═══════════════════════════════════════════════════════════════════

#[test]
fn smulh_basic() {
    // SMULH X0, X1, X2 => 0x9B427C20
    let (mut cpu, mut mem) = cpu_with_code(&[0x9B427C20]);
    cpu.set_xn(1, 0x1_0000_0000); // 2^32
    cpu.set_xn(2, 0x1_0000_0000); // 2^32
    cpu.step(&mut mem).unwrap();
    // 2^32 * 2^32 = 2^64, high 64 bits = 1
    assert_eq!(cpu.xn(0), 1);
}

#[test]
fn umulh_basic() {
    // UMULH X0, X1, X2 => 0x9BC27C20
    let (mut cpu, mut mem) = cpu_with_code(&[0x9BC27C20]);
    cpu.set_xn(1, u64::MAX);
    cpu.set_xn(2, 2);
    cpu.step(&mut mem).unwrap();
    // MAX * 2 = 0x1_FFFF_FFFF_FFFF_FFFE, high = 1
    assert_eq!(cpu.xn(0), 1);
}

#[test]
fn smaddl_basic() {
    // SMADDL X0, W1, W2, X3 => 0x9B220C60  (X0 = W1*W2 + X3)
    // Actually: SMADDL X0, W1, W2, X3 = sf=1 00 11011 001 Rm 0 Ra Rn Rd
    // = 1_00_11011_001_00010_0_00011_00001_00000
    let (mut cpu, mut mem) = cpu_with_code(&[0x9B220C20]);
    cpu.set_xn(1, (-3i64) as u64); // W1 = -3 (sign-extended)
    cpu.set_xn(2, 10);             // W2 = 10
    cpu.set_xn(3, 100);            // X3 = 100
    cpu.step(&mut mem).unwrap();
    // Result = (-3) * 10 + 100 = 70 (but SMADDL uses W regs, so W1=-3 as i32)
    assert_eq!(cpu.xn(0) as i64, 70);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD — SHL vector
// ═══════════════════════════════════════════════════════════════════

#[test]
fn shl_v4s_by_8() {
    // SHL V0.4S, V1.4S, #8
    // Encoding: 0 q=1 0 0 11110 immh=0100 immb=000 01010 1 Rn Rd
    // immh=0100 → 32-bit, shift = (immh:immb) - 32 = 0b0100_000 - 32 = 0
    // Actually for SHL 4S #8: immh:immb = 32+8 = 40 = 0b0101000
    // immh=0101, immb=000 → 0x4F285420
    let (mut cpu, mut mem) = cpu_with_code(&[0x4F285420]);
    cpu.regs.v[1] = 1 | (2u128 << 32) | (3u128 << 64) | (4u128 << 96);
    cpu.step(&mut mem).unwrap();
    let r = cpu.regs.v[0];
    assert_eq!(r & 0xFFFF_FFFF, 1 << 8);
    assert_eq!((r >> 32) & 0xFFFF_FFFF, 2 << 8);
}

// ═══════════════════════════════════════════════════════════════════
// SIMD — permute
// ═══════════════════════════════════════════════════════════════════

#[test]
fn zip1_v4s() {
    // ZIP1 V0.4S, V1.4S, V2.4S => 0x4E823820
    let (mut cpu, mut mem) = cpu_with_code(&[0x4E823820]);
    // V1 = [A, B, C, D], V2 = [E, F, G, H]
    cpu.regs.v[1] = 0xA | (0xBu128 << 32) | (0xCu128 << 64) | (0xDu128 << 96);
    cpu.regs.v[2] = 0xE | (0xFu128 << 32) | (0x10u128 << 64) | (0x11u128 << 96);
    cpu.step(&mut mem).unwrap();
    // ZIP1 interleaves lower halves: [A, E, B, F]
    let r = cpu.regs.v[0];
    assert_eq!(r & 0xFFFF_FFFF, 0xA);
    assert_eq!((r >> 32) & 0xFFFF_FFFF, 0xE);
    assert_eq!((r >> 64) & 0xFFFF_FFFF, 0xB);
    assert_eq!((r >> 96) & 0xFFFF_FFFF, 0xF);
}

// ═══════════════════════════════════════════════════════════════════
// System — barriers
// ═══════════════════════════════════════════════════════════════════

#[test]
fn dsb_is_nop_in_se() {
    // DSB SY => 0xD5033F9F
    let (mut cpu, mut mem) = cpu_with_code(&[0xD5033F9F]);
    let pc_before = cpu.regs.pc;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.pc, pc_before + 4);
}

#[test]
fn dmb_is_nop_in_se() {
    // DMB SY => 0xD5033FBF
    let (mut cpu, mut mem) = cpu_with_code(&[0xD5033FBF]);
    let pc_before = cpu.regs.pc;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.pc, pc_before + 4);
}

#[test]
fn isb_is_nop_in_se() {
    // ISB => 0xD5033FDF
    let (mut cpu, mut mem) = cpu_with_code(&[0xD5033FDF]);
    let pc_before = cpu.regs.pc;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.pc, pc_before + 4);
}

// ═══════════════════════════════════════════════════════════════════
// MRS TPIDR_EL0
// ═══════════════════════════════════════════════════════════════════

#[test]
fn mrs_tpidr_el0() {
    // MRS X0, TPIDR_EL0 => 0xD53BD040
    let (mut cpu, mut mem) = cpu_with_code(&[0xD53BD040]);
    cpu.regs.tpidr_el0 = 0xCAFE_0000;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xCAFE_0000);
}

#[test]
fn msr_tpidr_el0() {
    // MSR TPIDR_EL0, X0 => 0xD51BD040
    let (mut cpu, mut mem) = cpu_with_code(&[0xD51BD040]);
    cpu.set_xn(0, 0xBEEF_0000);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.tpidr_el0, 0xBEEF_0000);
}
