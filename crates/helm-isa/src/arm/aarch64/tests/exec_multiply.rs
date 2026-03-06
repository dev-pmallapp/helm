//! AArch64 Multiply (3-source) instruction tests.
//!
//! MADD, MSUB, SMADDL, UMADDL, SMULH, UMULH — all with boundary values.

use crate::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;

fn cpu_exec(insns: &[u32]) -> (Aarch64Cpu, AddressSpace) {
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();
    let base = 0x40_0000u64;
    let size = (insns.len() * 4 + 0x1000) as u64;
    mem.map(base, size, (true, true, true));
    for (i, insn) in insns.iter().enumerate() {
        mem.write(base + (i as u64 * 4), &insn.to_le_bytes()).unwrap();
    }
    cpu.regs.pc = base;
    mem.map(0x7FFF_0000, 0x10000, (true, true, false));
    cpu.regs.sp = 0x7FFF_8000;
    (cpu, mem)
}

// sf 00 11011 op31[2:0] rm o0 ra rn rd
fn madd(sf: u32, rm: u32, ra: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b0011011 << 24) | (0b000 << 21) | (rm << 16) | (0 << 15) | (ra << 10) | (rn << 5) | rd
}
fn msub(sf: u32, rm: u32, ra: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b0011011 << 24) | (0b000 << 21) | (rm << 16) | (1 << 15) | (ra << 10) | (rn << 5) | rd
}
fn smaddl(rm: u32, ra: u32, rn: u32, rd: u32) -> u32 {
    (1 << 31) | (0b0011011 << 24) | (0b001 << 21) | (rm << 16) | (0 << 15) | (ra << 10) | (rn << 5) | rd
}
fn umaddl(rm: u32, ra: u32, rn: u32, rd: u32) -> u32 {
    (1 << 31) | (0b0011011 << 24) | (0b101 << 21) | (rm << 16) | (0 << 15) | (ra << 10) | (rn << 5) | rd
}
fn smulh(rm: u32, rn: u32, rd: u32) -> u32 {
    (1 << 31) | (0b0011011 << 24) | (0b010 << 21) | (rm << 16) | (0 << 15) | (31 << 10) | (rn << 5) | rd
}
fn umulh(rm: u32, rn: u32, rd: u32) -> u32 {
    (1 << 31) | (0b0011011 << 24) | (0b110 << 21) | (rm << 16) | (0 << 15) | (31 << 10) | (rn << 5) | rd
}

macro_rules! test_mul3 {
    ($name:ident, $insn:expr, $rn:expr, $rm:expr, $ra:expr, $expected:expr) => {
        #[test] fn $name() {
            let (mut c, mut m) = cpu_exec(&[$insn]);
            c.set_xn(1, $rn); c.set_xn(2, $rm); c.set_xn(3, $ra);
            c.step(&mut m).unwrap();
            assert_eq!(c.xn(0), $expected);
        }
    };
}

macro_rules! test_mul2 {
    ($name:ident, $insn:expr, $rn:expr, $rm:expr, $expected:expr) => {
        #[test] fn $name() {
            let (mut c, mut m) = cpu_exec(&[$insn]);
            c.set_xn(1, $rn); c.set_xn(2, $rm);
            c.step(&mut m).unwrap();
            assert_eq!(c.xn(0), $expected);
        }
    };
}

// ===================================================================
//  MADD — Rd = Ra + Rn * Rm
// ===================================================================

// MUL alias: MADD with Ra=XZR
test_mul3!(mul_64_basic,      madd(1, 2, 31, 1, 0), 7u64, 6u64, 0u64, 42u64);
test_mul3!(mul_64_by_zero,    madd(1, 2, 31, 1, 0), 0u64, 12345u64, 0u64, 0u64);
test_mul3!(mul_64_by_one,     madd(1, 2, 31, 1, 0), 42u64, 1u64, 0u64, 42u64);
test_mul3!(mul_64_large,      madd(1, 2, 31, 1, 0), 0x1_0000_0000u64, 0x1_0000_0000u64, 0u64, 0u64);
test_mul3!(mul_64_overflow,   madd(1, 2, 31, 1, 0), u64::MAX, 2u64, 0u64, u64::MAX.wrapping_mul(2));
test_mul3!(mul_32_basic,      madd(0, 2, 31, 1, 0), 7u64, 6u64, 0u64, 42u64);
test_mul3!(mul_32_overflow,   madd(0, 2, 31, 1, 0), 0xFFFF_FFFFu64, 2u64, 0u64, 0xFFFF_FFFEu64);
test_mul3!(mul_32_truncate,   madd(0, 2, 31, 1, 0), 0x1_0000_0001u64, 3u64, 0u64, 3u64);

// MADD with accumulate
test_mul3!(madd_64_accum,     madd(1, 2, 3, 1, 0), 7u64, 6u64, 100u64, 142u64);
test_mul3!(madd_64_accum0,    madd(1, 2, 3, 1, 0), 10u64, 10u64, 0u64, 100u64);
test_mul3!(madd_32_accum,     madd(0, 2, 3, 1, 0), 5u64, 5u64, 10u64, 35u64);

// ===================================================================
//  MSUB — Rd = Ra - Rn * Rm
// ===================================================================

test_mul3!(mneg_64_basic,     msub(1, 2, 31, 1, 0), 7u64, 6u64, 0u64, (-(42i64)) as u64);
test_mul3!(msub_64_accum,     msub(1, 2, 3, 1, 0), 7u64, 6u64, 100u64, 58u64);
test_mul3!(msub_64_underflow, msub(1, 2, 3, 1, 0), 10u64, 10u64, 50u64, (50u64).wrapping_sub(100));
test_mul3!(msub_32_basic,     msub(0, 2, 3, 1, 0), 5u64, 5u64, 100u64, 75u64);

// ===================================================================
//  SMADDL — Xd = Xa + sext(Wn) * sext(Wm) (signed widening)
// ===================================================================

test_mul3!(smull_pos,       smaddl(2, 31, 1, 0), 100u64, 200u64, 0u64, 20000u64);
test_mul3!(smull_neg,       smaddl(2, 31, 1, 0), (-10i32) as u32 as u64, 20u64, 0u64, (-200i64) as u64);
test_mul3!(smull_neg_neg,   smaddl(2, 31, 1, 0), (-10i32) as u32 as u64, (-20i32) as u32 as u64, 0u64, 200u64);
test_mul3!(smaddl_accum,    smaddl(2, 3, 1, 0), 10u64, 20u64, 1000u64, 1200u64);
test_mul3!(smull_max,       smaddl(2, 31, 1, 0), 0x7FFF_FFFFu64, 0x7FFF_FFFFu64, 0u64, 0x3FFF_FFFF_0000_0001u64);

// ===================================================================
//  UMADDL — Xd = Xa + Wn * Wm (unsigned widening)
// ===================================================================

test_mul3!(umull_basic,     umaddl(2, 31, 1, 0), 100u64, 200u64, 0u64, 20000u64);
test_mul3!(umull_max,       umaddl(2, 31, 1, 0), 0xFFFF_FFFFu64, 0xFFFF_FFFFu64, 0u64, 0xFFFF_FFFE_0000_0001u64);
test_mul3!(umaddl_accum,    umaddl(2, 3, 1, 0), 0x1000u64, 0x1000u64, 42u64, 0x100_0000u64 + 42);

// ===================================================================
//  SMULH — Xd = (sext(Xn) * sext(Xm)) >> 64
// ===================================================================

test_mul2!(smulh_small,     smulh(2, 1, 0), 7u64, 6u64, 0u64);
test_mul2!(smulh_large,     smulh(2, 1, 0), 0x1_0000_0000u64, 0x1_0000_0000u64, 1u64);
test_mul2!(smulh_neg,       smulh(2, 1, 0), (-1i64) as u64, 2u64, u64::MAX);
test_mul2!(smulh_max_max,   smulh(2, 1, 0), i64::MAX as u64, 2u64, 0u64);

// ===================================================================
//  UMULH — Xd = (Xn * Xm) >> 64
// ===================================================================

test_mul2!(umulh_small,     umulh(2, 1, 0), 7u64, 6u64, 0u64);
test_mul2!(umulh_large,     umulh(2, 1, 0), 0x1_0000_0000u64, 0x1_0000_0000u64, 1u64);
test_mul2!(umulh_max,       umulh(2, 1, 0), u64::MAX, u64::MAX, u64::MAX - 1);
test_mul2!(umulh_max_2,     umulh(2, 1, 0), u64::MAX, 2u64, 1u64);
test_mul2!(umulh_half,      umulh(2, 1, 0), 1u64 << 63, 2u64, 1u64);

// ===================================================================
//  More MADD/MSUB boundary values
// ===================================================================
test_mul3!(mul_64_neg1_pos,   madd(1,2,31,1,0), u64::MAX, 42u64, 0u64, (-42i64) as u64);
test_mul3!(mul_64_neg1_neg1,  madd(1,2,31,1,0), u64::MAX, u64::MAX, 0u64, 1u64);
test_mul3!(mul_64_1_max,      madd(1,2,31,1,0), 1u64, u64::MAX, 0u64, u64::MAX);
test_mul3!(mul_64_2_half,     madd(1,2,31,1,0), 2u64, 1u64<<63, 0u64, 0u64);
test_mul3!(mul_32_neg1_1,     madd(0,2,31,1,0), 0xFFFF_FFFFu64, 1u64, 0u64, 0xFFFF_FFFFu64);
test_mul3!(mul_32_neg1_neg1,  madd(0,2,31,1,0), 0xFFFF_FFFFu64, 0xFFFF_FFFFu64, 0u64, 1u64);
test_mul3!(mul_32_256_256,    madd(0,2,31,1,0), 256u64, 256u64, 0u64, 65536u64);
test_mul3!(madd_64_max_acc,   madd(1,2,3,1,0), 1u64, 1u64, u64::MAX, 0u64);
test_mul3!(madd_32_max_acc,   madd(0,2,3,1,0), 1u64, 1u64, 0xFFFF_FFFFu64, 0u64);
test_mul3!(msub_64_max_acc,   msub(1,2,3,1,0), 1u64, 1u64, 0u64, u64::MAX);
test_mul3!(msub_32_max_acc,   msub(0,2,3,1,0), 1u64, 1u64, 0u64, 0xFFFF_FFFFu64);
test_mul3!(msub_64_eq,        msub(1,2,3,1,0), 5u64, 5u64, 25u64, 0u64);
test_mul3!(madd_64_0_acc,     madd(1,2,3,1,0), 0u64, 999u64, 42u64, 42u64);

// ===================================================================
//  More SMADDL boundary values
// ===================================================================
test_mul3!(smull_0_0,         smaddl(2,31,1,0), 0u64, 0u64, 0u64, 0u64);
test_mul3!(smull_1_1,         smaddl(2,31,1,0), 1u64, 1u64, 0u64, 1u64);
test_mul3!(smull_neg1_1,      smaddl(2,31,1,0), 0xFFFF_FFFFu64, 1u64, 0u64, (-1i64) as u64);
test_mul3!(smull_min_min,     smaddl(2,31,1,0), 0x8000_0000u64, 0x8000_0000u64, 0u64, 0x4000_0000_0000_0000u64);
test_mul3!(smull_min_1,       smaddl(2,31,1,0), 0x8000_0000u64, 1u64, 0u64, (-0x8000_0000i64) as u64);
test_mul3!(smaddl_neg_accum,  smaddl(2,3,1,0), 0xFFFF_FFFFu64, 10u64, 100u64, 90u64);

// ===================================================================
//  More UMADDL boundary values
// ===================================================================
test_mul3!(umull_0_0,         umaddl(2,31,1,0), 0u64, 0u64, 0u64, 0u64);
test_mul3!(umull_1_1,         umaddl(2,31,1,0), 1u64, 1u64, 0u64, 1u64);
test_mul3!(umull_ff_ff,       umaddl(2,31,1,0), 0xFFFF_FFFFu64, 0xFFFF_FFFFu64, 0u64, 0xFFFF_FFFE_0000_0001u64);
test_mul3!(umull_1000_1000,   umaddl(2,31,1,0), 0x1000u64, 0x1000u64, 0u64, 0x100_0000u64);
test_mul3!(umaddl_with_acc,   umaddl(2,3,1,0), 100u64, 100u64, 1000u64, 11000u64);

// ===================================================================
//  More SMULH / UMULH boundary values
// ===================================================================
test_mul2!(smulh_0_0,         smulh(2,1,0), 0u64, 0u64, 0u64);
test_mul2!(smulh_1_1,         smulh(2,1,0), 1u64, 1u64, 0u64);
test_mul2!(smulh_neg1_neg1,   smulh(2,1,0), u64::MAX, u64::MAX, 0u64);
test_mul2!(smulh_neg1_1,      smulh(2,1,0), u64::MAX, 1u64, u64::MAX);
test_mul2!(smulh_min_2,       smulh(2,1,0), i64::MIN as u64, 2u64, u64::MAX);
test_mul2!(smulh_min_min,     smulh(2,1,0), i64::MIN as u64, i64::MIN as u64, 1u64<<62);
test_mul2!(umulh_0_0,         umulh(2,1,0), 0u64, 0u64, 0u64);
test_mul2!(umulh_1_1,         umulh(2,1,0), 1u64, 1u64, 0u64);
test_mul2!(umulh_max_1,       umulh(2,1,0), u64::MAX, 1u64, 0u64);
test_mul2!(umulh_max_max2,    umulh(2,1,0), u64::MAX, u64::MAX, u64::MAX-1);
test_mul2!(umulh_half_4,      umulh(2,1,0), 1u64<<62, 4u64, 1u64);
test_mul2!(umulh_half_2b,     umulh(2,1,0), 1u64<<63, 4u64, 2u64);
