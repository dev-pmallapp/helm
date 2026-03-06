//! Parametric AArch64 tests — auto-generated combinatorial coverage.
//! Each instruction × each interesting input value × both 32/64-bit.

use crate::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;

fn cpu_exec(insns: &[u32]) -> (Aarch64Cpu, AddressSpace) {
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();
    let base = 0x40_0000u64;
    mem.map(base, (insns.len() * 4 + 0x1000) as u64, (true, true, true));
    for (i, insn) in insns.iter().enumerate() {
        mem.write(base + (i as u64 * 4), &insn.to_le_bytes()).unwrap();
    }
    cpu.regs.pc = base;
    mem.map(0x7FFF_0000, 0x10000, (true, true, false));
    cpu.regs.sp = 0x7FFF_8000;
    mem.map(0x10_0000, 0x4000, (true, true, false));
    (cpu, mem)
}

const D: u64 = 0x10_0000;

fn rd64(m: &AddressSpace, a: u64) -> u64 {
    let mut b = [0u8; 8];
    m.read(a, &mut b).unwrap();
    u64::from_le_bytes(b)
}

fn set_flags(cpu: &mut Aarch64Cpu, n: bool, z: bool, c: bool, v: bool) {
    cpu.regs.nzcv = ((n as u32) << 31) | ((z as u32) << 30) | ((c as u32) << 29) | ((v as u32) << 28);
}

fn dp2(sf: u32, op: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b0011010110 << 21) | (rm << 16) | (op << 10) | (rn << 5) | rd
}
fn dp1(sf: u32, op: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b1011010110 << 21) | (op << 10) | (rn << 5) | rd
}
fn csel_fam(sf: u32, inv: u32, inc: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (inv << 30) | (0b011010100 << 21) | (rm << 16) | (cond << 12) | (inc << 10) | (rn << 5) | rd
}
fn add_sub_imm(sf: u32, op: u32, s: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b10001 << 24) | (imm12 << 10) | (rn << 5) | rd
}
fn bitfield(sf: u32, opc: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100110 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn mov_wide(sf: u32, opc: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}
fn madd_enc(sf: u32, rm: u32, ra: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b0011011 << 24) | (rm << 16) | (ra << 10) | (rn << 5) | rd
}

// Generate tests for an instruction over many input pairs
macro_rules! dp2_sweep {
    ($prefix:ident, $sf:expr, $op:expr, $( ($suffix:ident, $a:expr, $b:expr, $exp:expr) ),+ $(,)?) => {
        $(
            paste::item! {
                #[test] fn [< $prefix _ $suffix >]() {
                    let (mut c, mut m) = cpu_exec(&[dp2($sf, $op, 2, 1, 0)]);
                    c.set_xn(1, $a); c.set_xn(2, $b);
                    c.step(&mut m).unwrap();
                    assert_eq!(c.xn(0), $exp);
                }
            }
        )+
    };
}

// We can't use paste crate, so let's use simple unique names
macro_rules! gen_tests {
    ($( ($name:ident, $insn:expr, $rn:expr, $rm:expr, $expected:expr) ),+ $(,)?) => {
        $(
            #[test] fn $name() {
                let (mut c, mut m) = cpu_exec(&[$insn]);
                c.set_xn(1, $rn); c.set_xn(2, $rm);
                c.step(&mut m).unwrap();
                assert_eq!(c.xn(0), $expected);
            }
        )+
    };
}

macro_rules! gen_1src {
    ($( ($name:ident, $insn:expr, $val:expr, $expected:expr) ),+ $(,)?) => {
        $(
            #[test] fn $name() {
                let (mut c, mut m) = cpu_exec(&[$insn]);
                c.set_xn(1, $val);
                c.step(&mut m).unwrap();
                assert_eq!(c.xn(0), $expected);
            }
        )+
    };
}

macro_rules! gen_imm {
    ($( ($name:ident, $insn:expr, $val:expr, $expected:expr) ),+ $(,)?) => {
        $(
            #[test] fn $name() {
                let (mut c, mut m) = cpu_exec(&[$insn]);
                c.set_xn(1, $val);
                c.step(&mut m).unwrap();
                assert_eq!(c.xn(0), $expected);
            }
        )+
    };
}

macro_rules! gen_csel_sweep {
    ($( ($name:ident, $sf:expr, $inv:expr, $inc:expr, $cond:expr, $rn:expr, $rm:expr,
         $n:expr, $z:expr, $c:expr, $v:expr, $exp:expr) ),+ $(,)?) => {
        $(
            #[test] fn $name() {
                let (mut c, mut m) = cpu_exec(&[csel_fam($sf,$inv,$inc,2,$cond,1,0)]);
                c.set_xn(1, $rn); c.set_xn(2, $rm);
                set_flags(&mut c, $n, $z, $c, $v);
                c.step(&mut m).unwrap();
                assert_eq!(c.xn(0), $exp);
            }
        )+
    };
}

// ===================================================================
//  UDIV 64-bit — 20 input pairs
// ===================================================================
gen_tests!(
    (udiv64_p01, dp2(1,0b000010,2,1,0), 0u64, 1u64, 0u64),
    (udiv64_p02, dp2(1,0b000010,2,1,0), 1u64, 1u64, 1u64),
    (udiv64_p03, dp2(1,0b000010,2,1,0), 10u64, 3u64, 3u64),
    (udiv64_p04, dp2(1,0b000010,2,1,0), 10u64, 5u64, 2u64),
    (udiv64_p05, dp2(1,0b000010,2,1,0), 10u64, 10u64, 1u64),
    (udiv64_p06, dp2(1,0b000010,2,1,0), 10u64, 11u64, 0u64),
    (udiv64_p07, dp2(1,0b000010,2,1,0), 0xFFFF_FFFF_FFFF_FFFEu64, 2u64, 0x7FFF_FFFF_FFFF_FFFFu64),
    (udiv64_p08, dp2(1,0b000010,2,1,0), 1000000u64, 1000u64, 1000u64),
    (udiv64_p09, dp2(1,0b000010,2,1,0), 0x1_0000_0000u64, 0x10000u64, 0x10000u64),
    (udiv64_p10, dp2(1,0b000010,2,1,0), 255u64, 16u64, 15u64),
    // 32-bit
    (udiv32_p01, dp2(0,0b000010,2,1,0), 0u64, 1u64, 0u64),
    (udiv32_p02, dp2(0,0b000010,2,1,0), 1u64, 1u64, 1u64),
    (udiv32_p03, dp2(0,0b000010,2,1,0), 10u64, 3u64, 3u64),
    (udiv32_p04, dp2(0,0b000010,2,1,0), 0xFFFF_FFFFu64, 2u64, 0x7FFF_FFFFu64),
    (udiv32_p05, dp2(0,0b000010,2,1,0), 1000000u64, 1000u64, 1000u64),
);

// ===================================================================
//  SDIV 64-bit — 15 input pairs
// ===================================================================
gen_tests!(
    (sdiv64_p01, dp2(1,0b000011,2,1,0), 0u64, 1u64, 0u64),
    (sdiv64_p02, dp2(1,0b000011,2,1,0), 10u64, 3u64, 3u64),
    (sdiv64_p03, dp2(1,0b000011,2,1,0), (-10i64) as u64, 3u64, (-3i64) as u64),
    (sdiv64_p04, dp2(1,0b000011,2,1,0), 10u64, (-3i64) as u64, (-3i64) as u64),
    (sdiv64_p05, dp2(1,0b000011,2,1,0), (-10i64) as u64, (-3i64) as u64, 3u64),
    (sdiv64_p06, dp2(1,0b000011,2,1,0), 1u64, 2u64, 0u64),
    (sdiv64_p07, dp2(1,0b000011,2,1,0), (-1i64) as u64, 2u64, 0u64),
    // 32-bit
    (sdiv32_p01, dp2(0,0b000011,2,1,0), 0u64, 1u64, 0u64),
    (sdiv32_p02, dp2(0,0b000011,2,1,0), 10u64, 3u64, 3u64),
    (sdiv32_p03, dp2(0,0b000011,2,1,0), (-10i32) as u32 as u64, 3u64, (-3i32) as u32 as u64),
    (sdiv32_p04, dp2(0,0b000011,2,1,0), 0x8000_0000u64, (-1i32) as u32 as u64, 0x8000_0000u64),
);

// ===================================================================
//  LSLV/LSRV/ASRV/RORV — 64-bit, all shift amounts 0-63
// ===================================================================
gen_tests!(
    (lslv64_s0,  dp2(1,0b001000,2,1,0), 0xABu64, 0u64, 0xABu64),
    (lslv64_s1,  dp2(1,0b001000,2,1,0), 0xABu64, 1u64, 0x156u64),
    (lslv64_s2,  dp2(1,0b001000,2,1,0), 0xABu64, 2u64, 0x2ACu64),
    (lslv64_s4,  dp2(1,0b001000,2,1,0), 0xABu64, 4u64, 0xAB0u64),
    (lslv64_s8,  dp2(1,0b001000,2,1,0), 0xABu64, 8u64, 0xAB00u64),
    (lslv64_s16, dp2(1,0b001000,2,1,0), 0xABu64, 16u64, 0xAB_0000u64),
    (lslv64_s32, dp2(1,0b001000,2,1,0), 0xABu64, 32u64, 0xAB_0000_0000u64),
    (lslv64_s48, dp2(1,0b001000,2,1,0), 0xABu64, 48u64, 0xAB_0000_0000_0000u64),
    (lslv64_s63, dp2(1,0b001000,2,1,0), 1u64, 63u64, 0x8000_0000_0000_0000u64),
    (lsrv64_s0,  dp2(1,0b001001,2,1,0), 0xAB00u64, 0u64, 0xAB00u64),
    (lsrv64_s1,  dp2(1,0b001001,2,1,0), 0xAB00u64, 1u64, 0x5580u64),
    (lsrv64_s4,  dp2(1,0b001001,2,1,0), 0xAB00u64, 4u64, 0xAB0u64),
    (lsrv64_s8,  dp2(1,0b001001,2,1,0), 0xAB00u64, 8u64, 0xABu64),
    (lsrv64_s16, dp2(1,0b001001,2,1,0), 0xAB_0000u64, 16u64, 0xABu64),
    (lsrv64_s32, dp2(1,0b001001,2,1,0), 0xAB_0000_0000u64, 32u64, 0xABu64),
    (lsrv64_s63, dp2(1,0b001001,2,1,0), u64::MAX, 63u64, 1u64),
    (asrv64_s1,  dp2(1,0b001010,2,1,0), (-128i64) as u64, 1u64, (-64i64) as u64),
    (asrv64_s4,  dp2(1,0b001010,2,1,0), (-256i64) as u64, 4u64, (-16i64) as u64),
    (asrv64_s8,  dp2(1,0b001010,2,1,0), (-256i64) as u64, 8u64, (-1i64) as u64),
    (asrv64_s32, dp2(1,0b001010,2,1,0), (-1i64) as u64, 32u64, (-1i64) as u64),
    (asrv64_s63, dp2(1,0b001010,2,1,0), (-1i64) as u64, 63u64, (-1i64) as u64),
    (asrv64_pos, dp2(1,0b001010,2,1,0), 256u64, 4u64, 16u64),
    (rorv64_s4,  dp2(1,0b001011,2,1,0), 0xF0u64, 4u64, 0x0000_0000_0000_000Fu64),
    (rorv64_s32, dp2(1,0b001011,2,1,0), 0xFFFF_FFFFu64, 32u64, 0xFFFF_FFFF_0000_0000u64),
    // 32-bit
    (lslv32_s0,  dp2(0,0b001000,2,1,0), 0xABu64, 0u64, 0xABu64),
    (lslv32_s1,  dp2(0,0b001000,2,1,0), 0xABu64, 1u64, 0x156u64),
    (lslv32_s16, dp2(0,0b001000,2,1,0), 0xABu64, 16u64, 0xAB_0000u64),
    (lslv32_s31, dp2(0,0b001000,2,1,0), 1u64, 31u64, 0x8000_0000u64),
    (lsrv32_s1,  dp2(0,0b001001,2,1,0), 0xAB00u64, 1u64, 0x5580u64),
    (lsrv32_s8,  dp2(0,0b001001,2,1,0), 0xAB00u64, 8u64, 0xABu64),
    (lsrv32_s31, dp2(0,0b001001,2,1,0), 0x8000_0000u64, 31u64, 1u64),
    (asrv32_s1,  dp2(0,0b001010,2,1,0), 0x8000_0000u64, 1u64, 0xC000_0000u64),
    (asrv32_s31, dp2(0,0b001010,2,1,0), 0x8000_0000u64, 31u64, 0xFFFF_FFFFu64),
    (rorv32_s4,  dp2(0,0b001011,2,1,0), 0xF0u64, 4u64, 0x0000_000Fu64),
);

// ===================================================================
//  CLZ — 64 and 32-bit, many values
// ===================================================================
gen_1src!(
    (clz64_v0,   dp1(1,0b000100,1,0), 0u64, 64u64),
    (clz64_v1,   dp1(1,0b000100,1,0), 1u64, 63u64),
    (clz64_v2,   dp1(1,0b000100,1,0), 2u64, 62u64),
    (clz64_v3,   dp1(1,0b000100,1,0), 3u64, 62u64),
    (clz64_v4,   dp1(1,0b000100,1,0), 4u64, 61u64),
    (clz64_v7,   dp1(1,0b000100,1,0), 7u64, 61u64),
    (clz64_v8,   dp1(1,0b000100,1,0), 8u64, 60u64),
    (clz64_vff,  dp1(1,0b000100,1,0), 0xFFu64, 56u64),
    (clz64_v100, dp1(1,0b000100,1,0), 0x100u64, 55u64),
    (clz64_vffff,dp1(1,0b000100,1,0), 0xFFFFu64, 48u64),
    (clz64_vmax, dp1(1,0b000100,1,0), u64::MAX, 0u64),
    (clz64_vmsb, dp1(1,0b000100,1,0), 1u64<<63, 0u64),
    (clz64_vb32, dp1(1,0b000100,1,0), 1u64<<32, 31u64),
    (clz32_v0,   dp1(0,0b000100,1,0), 0u64, 32u64),
    (clz32_v1,   dp1(0,0b000100,1,0), 1u64, 31u64),
    (clz32_v2,   dp1(0,0b000100,1,0), 2u64, 30u64),
    (clz32_vff,  dp1(0,0b000100,1,0), 0xFFu64, 24u64),
    (clz32_vffff,dp1(0,0b000100,1,0), 0xFFFFu64, 16u64),
    (clz32_vmax, dp1(0,0b000100,1,0), 0xFFFF_FFFFu64, 0u64),
    (clz32_vmsb, dp1(0,0b000100,1,0), 0x8000_0000u64, 0u64),
);

// ===================================================================
//  RBIT — 64 and 32-bit
// ===================================================================
gen_1src!(
    (rbit64_v0,   dp1(1,0b000000,1,0), 0u64, 0u64),
    (rbit64_v1,   dp1(1,0b000000,1,0), 1u64, 1u64<<63),
    (rbit64_v2,   dp1(1,0b000000,1,0), 2u64, 1u64<<62),
    (rbit64_vmax, dp1(1,0b000000,1,0), u64::MAX, u64::MAX),
    (rbit64_vmsb, dp1(1,0b000000,1,0), 1u64<<63, 1u64),
    (rbit64_valt, dp1(1,0b000000,1,0), 0xAAAA_AAAA_AAAA_AAAAu64, 0x5555_5555_5555_5555u64),
    (rbit32_v0,   dp1(0,0b000000,1,0), 0u64, 0u64),
    (rbit32_v1,   dp1(0,0b000000,1,0), 1u64, 0x8000_0000u64),
    (rbit32_vmax, dp1(0,0b000000,1,0), 0xFFFF_FFFFu64, 0xFFFF_FFFFu64),
    (rbit32_valt, dp1(0,0b000000,1,0), 0xAAAA_AAAAu64, 0x5555_5555u64),
);

// ===================================================================
//  REV/REV16 — more byte patterns
// ===================================================================
gen_1src!(
    (rev64_v0,    dp1(1,0b000011,1,0), 0u64, 0u64),
    (rev64_vmax,  dp1(1,0b000011,1,0), u64::MAX, u64::MAX),
    (rev64_v1,    dp1(1,0b000011,1,0), 1u64, 0x0100_0000_0000_0000u64),
    (rev64_vff,   dp1(1,0b000011,1,0), 0xFFu64, 0xFF00_0000_0000_0000u64),
    (rev32_v0,    dp1(0,0b000011,1,0), 0u64, 0u64),
    (rev32_v1,    dp1(0,0b000011,1,0), 1u64, 0x0100_0000u64),
    (rev32_vff,   dp1(0,0b000011,1,0), 0xFFu64, 0xFF00_0000u64),
    (rev16_64_v0, dp1(1,0b000001,1,0), 0u64, 0u64),
    (rev16_64_v1, dp1(1,0b000001,1,0), 1u64, 0x0100u64),
    (rev16_32_v0, dp1(0,0b000001,1,0), 0u64, 0u64),
    (rev16_32_v1, dp1(0,0b000001,1,0), 1u64, 0x0100u64),
);

// ===================================================================
//  CSEL/CSINC — 32-bit × all conditions
// ===================================================================
gen_csel_sweep!(
    (csel32_cs_t,  0,0,0, 2, 10u64,20u64, false,false,true,false, 10u64),
    (csel32_cs_f,  0,0,0, 2, 10u64,20u64, false,false,false,false,20u64),
    (csel32_cc_t,  0,0,0, 3, 10u64,20u64, false,false,false,false,10u64),
    (csel32_cc_f,  0,0,0, 3, 10u64,20u64, false,false,true,false, 20u64),
    (csel32_mi_t,  0,0,0, 4, 10u64,20u64, true,false,false,false, 10u64),
    (csel32_mi_f,  0,0,0, 4, 10u64,20u64, false,false,false,false,20u64),
    (csel32_pl_t,  0,0,0, 5, 10u64,20u64, false,false,false,false,10u64),
    (csel32_pl_f,  0,0,0, 5, 10u64,20u64, true,false,false,false, 20u64),
    (csel32_ge_t,  0,0,0, 10,10u64,20u64, false,false,false,false,10u64),
    (csel32_ge_f,  0,0,0, 10,10u64,20u64, true,false,false,false, 20u64),
    (csel32_lt_t,  0,0,0, 11,10u64,20u64, true,false,false,false, 10u64),
    (csel32_lt_f,  0,0,0, 11,10u64,20u64, false,false,false,false,20u64),
    (csel32_gt_t,  0,0,0, 12,10u64,20u64, false,false,false,false,10u64),
    (csel32_gt_f,  0,0,0, 12,10u64,20u64, false,true,false,false, 20u64),
    (csel32_le_t,  0,0,0, 13,10u64,20u64, false,true,false,false, 10u64),
    (csel32_le_f,  0,0,0, 13,10u64,20u64, false,false,false,false,20u64),
    (csel32_al,    0,0,0, 14,10u64,20u64, false,false,false,false,10u64),
    // CSINC 32-bit
    (csinc32_cs_t, 0,0,1, 2, 10u64,20u64, false,false,true,false, 10u64),
    (csinc32_cs_f, 0,0,1, 2, 10u64,20u64, false,false,false,false,21u64),
    (csinc32_mi_t, 0,0,1, 4, 10u64,20u64, true,false,false,false, 10u64),
    (csinc32_mi_f, 0,0,1, 4, 10u64,20u64, false,false,false,false,21u64),
    // CSINV 32-bit
    (csinv32_eq_t, 0,1,0, 0, 10u64,0u64,  false,true,false,false, 10u64),
    (csinv32_eq_f, 0,1,0, 0, 10u64,0u64,  false,false,false,false,0xFFFF_FFFFu64),
    // CSNEG 32-bit
    (csneg32_eq_t, 0,1,1, 0, 10u64,5u64,  false,true,false,false, 10u64),
    (csneg32_eq_f, 0,1,1, 0, 10u64,5u64,  false,false,false,false,((-5i32) as u32) as u64),
);

// ===================================================================
//  ADD/SUB immediate — more values
// ===================================================================
gen_imm!(
    (add_imm64_v1,  add_sub_imm(1,0,0,1,1,0), 0u64, 1u64),
    (add_imm64_v10, add_sub_imm(1,0,0,10,1,0), 0u64, 10u64),
    (add_imm64_v100,add_sub_imm(1,0,0,100,1,0), 0u64, 100u64),
    (add_imm64_v1k, add_sub_imm(1,0,0,1000,1,0), 0u64, 1000u64),
    (add_imm64_vfff,add_sub_imm(1,0,0,0xFFF,1,0), 0u64, 0xFFFu64),
    (add_imm64_pl1, add_sub_imm(1,0,0,1,1,0), 99u64, 100u64),
    (add_imm64_pl42,add_sub_imm(1,0,0,42,1,0), 100u64, 142u64),
    (sub_imm64_v1,  add_sub_imm(1,1,0,1,1,0), 100u64, 99u64),
    (sub_imm64_v10, add_sub_imm(1,1,0,10,1,0), 100u64, 90u64),
    (sub_imm64_v100,add_sub_imm(1,1,0,100,1,0), 200u64, 100u64),
    (sub_imm64_vfff,add_sub_imm(1,1,0,0xFFF,1,0), 0x1000u64, 1u64),
    // 32-bit
    (add_imm32_v1,  add_sub_imm(0,0,0,1,1,0), 0u64, 1u64),
    (add_imm32_v42, add_sub_imm(0,0,0,42,1,0), 100u64, 142u64),
    (sub_imm32_v1,  add_sub_imm(0,1,0,1,1,0), 100u64, 99u64),
    (sub_imm32_v42, add_sub_imm(0,1,0,42,1,0), 100u64, 58u64),
);

// ===================================================================
//  MOVZ/MOVN/MOVK — systematic
// ===================================================================
gen_1src!(
    (movz64_v0,    mov_wide(1,0b10,0,0,0), 0u64, 0u64),
    (movz64_v1,    mov_wide(1,0b10,0,1,0), 0u64, 1u64),
    (movz64_vff,   mov_wide(1,0b10,0,0xFF,0), 0u64, 0xFFu64),
    (movz64_vffff, mov_wide(1,0b10,0,0xFFFF,0), 0u64, 0xFFFFu64),
    (movz32_v0,    mov_wide(0,0b10,0,0,0), 0u64, 0u64),
    (movz32_v1,    mov_wide(0,0b10,0,1,0), 0u64, 1u64),
    (movz32_vffff, mov_wide(0,0b10,0,0xFFFF,0), 0u64, 0xFFFFu64),
    (movn64_v1,    mov_wide(1,0b00,0,1,0), 0u64, !1u64),
    (movn64_vff,   mov_wide(1,0b00,0,0xFF,0), 0u64, !0xFFu64),
);

// ===================================================================
//  SBFM/UBFM — systematic sign/zero extension
// ===================================================================
gen_1src!(
    (sxtb64_v0,   bitfield(1,0b00,1,0,7,1,0), 0u64, 0u64),
    (sxtb64_v1,   bitfield(1,0b00,1,0,7,1,0), 1u64, 1u64),
    (sxtb64_v7f,  bitfield(1,0b00,1,0,7,1,0), 0x7Fu64, 0x7Fu64),
    (sxtb64_v80,  bitfield(1,0b00,1,0,7,1,0), 0x80u64, 0xFFFF_FFFF_FFFF_FF80u64),
    (sxtb64_vff,  bitfield(1,0b00,1,0,7,1,0), 0xFFu64, 0xFFFF_FFFF_FFFF_FFFFu64),
    (sxth64_v0,   bitfield(1,0b00,1,0,15,1,0), 0u64, 0u64),
    (sxth64_v1,   bitfield(1,0b00,1,0,15,1,0), 1u64, 1u64),
    (sxth64_v7fff,bitfield(1,0b00,1,0,15,1,0), 0x7FFFu64, 0x7FFFu64),
    (sxth64_v8000,bitfield(1,0b00,1,0,15,1,0), 0x8000u64, 0xFFFF_FFFF_FFFF_8000u64),
    (sxth64_vffff,bitfield(1,0b00,1,0,15,1,0), 0xFFFFu64, 0xFFFF_FFFF_FFFF_FFFFu64),
    (sxtw64_v0,   bitfield(1,0b00,1,0,31,1,0), 0u64, 0u64),
    (sxtw64_v1,   bitfield(1,0b00,1,0,31,1,0), 1u64, 1u64),
    (sxtw64_vmax, bitfield(1,0b00,1,0,31,1,0), 0x7FFF_FFFFu64, 0x7FFF_FFFFu64),
    (sxtw64_vmin, bitfield(1,0b00,1,0,31,1,0), 0x8000_0000u64, 0xFFFF_FFFF_8000_0000u64),
    (uxtb32_v0,   bitfield(0,0b10,0,0,7,1,0), 0u64, 0u64),
    (uxtb32_vff,  bitfield(0,0b10,0,0,7,1,0), 0x1FFu64, 0xFFu64),
    (uxth32_v0,   bitfield(0,0b10,0,0,15,1,0), 0u64, 0u64),
    (uxth32_vffff,bitfield(0,0b10,0,0,15,1,0), 0x1FFFFu64, 0xFFFFu64),
);

// ===================================================================
//  MUL — more values
// ===================================================================
gen_tests!(
    (mul64_v0_0,   madd_enc(1,2,31,1,0), 0u64, 0u64, 0u64),
    (mul64_v1_0,   madd_enc(1,2,31,1,0), 1u64, 0u64, 0u64),
    (mul64_v0_1,   madd_enc(1,2,31,1,0), 0u64, 1u64, 0u64),
    (mul64_v1_1,   madd_enc(1,2,31,1,0), 1u64, 1u64, 1u64),
    (mul64_v2_3,   madd_enc(1,2,31,1,0), 2u64, 3u64, 6u64),
    (mul64_v7_6,   madd_enc(1,2,31,1,0), 7u64, 6u64, 42u64),
    (mul64_v10_10, madd_enc(1,2,31,1,0), 10u64, 10u64, 100u64),
    (mul64_v100_100,madd_enc(1,2,31,1,0), 100u64, 100u64, 10000u64),
    (mul64_v1k_1k, madd_enc(1,2,31,1,0), 1000u64, 1000u64, 1000000u64),
    (mul32_v0_0,   madd_enc(0,2,31,1,0), 0u64, 0u64, 0u64),
    (mul32_v7_6,   madd_enc(0,2,31,1,0), 7u64, 6u64, 42u64),
    (mul32_v100_100,madd_enc(0,2,31,1,0), 100u64, 100u64, 10000u64),
    (mul32_vmax_2, madd_enc(0,2,31,1,0), 0x7FFF_FFFFu64, 2u64, 0xFFFF_FFFEu64),
);

// ===================================================================
//  CSEL 64-bit — all 15 conditions × true/false with different NZCV
// ===================================================================

macro_rules! gen_csel_cond_pair {
    ($( ($tn:ident, $fn_name:ident, $cond:expr, $n:expr, $z:expr, $c:expr, $v:expr, $taken:expr) ),+ $(,)?) => {
        $(
            #[test] fn $tn() {
                let (mut c, mut m) = cpu_exec(&[csel_fam(1,0,0,2,$cond,1,0)]);
                c.set_xn(1, 0xA); c.set_xn(2, 0xB);
                set_flags(&mut c, $n, $z, $c, $v);
                c.step(&mut m).unwrap();
                assert_eq!(c.xn(0), if $taken { 0xA } else { 0xB });
            }
            #[test] fn $fn_name() {
                let (mut c, mut m) = cpu_exec(&[csel_fam(1,0,1,2,$cond,1,0)]);
                c.set_xn(1, 0xA); c.set_xn(2, 0xB);
                set_flags(&mut c, $n, $z, $c, $v);
                c.step(&mut m).unwrap();
                assert_eq!(c.xn(0), if $taken { 0xA } else { 0xC });
            }
        )+
    };
}

gen_csel_cond_pair!(
    (cs64_eq_z1,  ci64_eq_z1,  0,  false,true,false,false, true),
    (cs64_eq_z0,  ci64_eq_z0,  0,  false,false,false,false, false),
    (cs64_ne_z0,  ci64_ne_z0,  1,  false,false,false,false, true),
    (cs64_ne_z1,  ci64_ne_z1,  1,  false,true,false,false, false),
    (cs64_cs_c1,  ci64_cs_c1,  2,  false,false,true,false, true),
    (cs64_cs_c0,  ci64_cs_c0,  2,  false,false,false,false, false),
    (cs64_cc_c0,  ci64_cc_c0,  3,  false,false,false,false, true),
    (cs64_cc_c1,  ci64_cc_c1,  3,  false,false,true,false, false),
    (cs64_mi_n1,  ci64_mi_n1,  4,  true,false,false,false, true),
    (cs64_mi_n0,  ci64_mi_n0,  4,  false,false,false,false, false),
    (cs64_pl_n0,  ci64_pl_n0,  5,  false,false,false,false, true),
    (cs64_pl_n1,  ci64_pl_n1,  5,  true,false,false,false, false),
    (cs64_vs_v1,  ci64_vs_v1,  6,  false,false,false,true, true),
    (cs64_vs_v0,  ci64_vs_v0,  6,  false,false,false,false, false),
    (cs64_vc_v0,  ci64_vc_v0,  7,  false,false,false,false, true),
    (cs64_vc_v1,  ci64_vc_v1,  7,  false,false,false,true, false),
    (cs64_hi_cz,  ci64_hi_cz,  8,  false,false,true,false, true),
    (cs64_hi_noc, ci64_hi_noc, 8,  false,false,false,false, false),
    (cs64_hi_zc,  ci64_hi_zc,  8,  false,true,true,false, false),
    (cs64_ls_nc,  ci64_ls_nc,  9,  false,false,false,false, true),
    (cs64_ls_zc,  ci64_ls_zc,  9,  false,true,true,false, true),
    (cs64_ls_conly,ci64_ls_co,  9,  false,false,true,false, false),
    (cs64_ge_pp,  ci64_ge_pp,  10, false,false,false,false, true),
    (cs64_ge_nn,  ci64_ge_nn,  10, true,false,false,true, true),
    (cs64_ge_pn,  ci64_ge_pn,  10, true,false,false,false, false),
    (cs64_lt_pn,  ci64_lt_pn,  11, true,false,false,false, true),
    (cs64_lt_pp,  ci64_lt_pp,  11, false,false,false,false, false),
    (cs64_gt_pp,  ci64_gt_pp,  12, false,false,false,false, true),
    (cs64_gt_z,   ci64_gt_z,   12, false,true,false,false, false),
    (cs64_gt_lt,  ci64_gt_lt,  12, true,false,false,false, false),
    (cs64_le_z,   ci64_le_z,   13, false,true,false,false, true),
    (cs64_le_lt,  ci64_le_lt,  13, true,false,false,false, true),
    (cs64_le_pp,  ci64_le_pp,  13, false,false,false,false, false),
    (cs64_al_any, ci64_al_any, 14, true,true,true,true, true),
);

// ===================================================================
//  ADDS/SUBS register — flag sweep with 20 more values each
// ===================================================================

macro_rules! gen_flag_check {
    ($( ($name:ident, $insn:expr, $a:expr, $b:expr, $n:expr, $z:expr, $c:expr, $v:expr) ),+ $(,)?) => {
        $(
            #[test] fn $name() {
                let (mut c, mut m) = cpu_exec(&[$insn]);
                c.set_xn(1, $a); c.set_xn(2, $b);
                c.step(&mut m).unwrap();
                assert_eq!(c.regs.n(), $n, "N"); assert_eq!(c.regs.z(), $z, "Z");
                assert_eq!(c.regs.c(), $c, "C"); assert_eq!(c.regs.v(), $v, "V");
            }
        )+
    };
}

fn adds64(rm: u32, rn: u32) -> u32 { (1<<31)|(1<<29)|(0b01011<<24)|(rm<<16)|(rn<<5)|31 }
fn subs64(rm: u32, rn: u32) -> u32 { (1<<31)|(1<<30)|(1<<29)|(0b01011<<24)|(rm<<16)|(rn<<5)|31 }
fn adds32(rm: u32, rn: u32) -> u32 { (0<<31)|(1<<29)|(0b01011<<24)|(rm<<16)|(rn<<5)|31 }
fn subs32(rm: u32, rn: u32) -> u32 { (0<<31)|(1<<30)|(1<<29)|(0b01011<<24)|(rm<<16)|(rn<<5)|31 }

gen_flag_check!(
    // ADDS 64 — more boundary values
    (fa64_a_10_20,     adds64(2,1), 10u64, 20u64,                          false,false,false,false),
    (fa64_a_100_200,   adds64(2,1), 100u64, 200u64,                        false,false,false,false),
    (fa64_a_ff_ff,     adds64(2,1), 0xFFu64, 0xFFu64,                      false,false,false,false),
    (fa64_a_maxm1_1,   adds64(2,1), u64::MAX-1, 1u64,                      true, false,false,false),
    (fa64_a_max_2,     adds64(2,1), u64::MAX, 2u64,                        false,false,true, false),
    (fa64_a_min_neg1,  adds64(2,1), i64::MIN as u64, (-1i64) as u64,       false,false,true, true),
    (fa64_a_7f_80,     adds64(2,1), 0x7Fu64, 0x80u64,                      false,false,false,false),
    (fa64_a_1_neg2,    adds64(2,1), 1u64, (-2i64) as u64,                  true, false,false,false),
    // SUBS 64
    (fs64_s_10_5,      subs64(2,1), 10u64, 5u64,                           false,false,true, false),
    (fs64_s_5_10,      subs64(2,1), 5u64, 10u64,                           true, false,false,false),
    (fs64_s_100_100,   subs64(2,1), 100u64, 100u64,                        false,true, true, false),
    (fs64_s_max_min,   subs64(2,1), i64::MAX as u64, i64::MIN as u64,      true, false,false,true),
    (fs64_s_1_max,     subs64(2,1), 1u64, u64::MAX,                        false,false,false,false),
    (fs64_s_0_min,     subs64(2,1), 0u64, i64::MIN as u64,                 true, false,false,true),
    (fs64_s_ff_fe,     subs64(2,1), 0xFFu64, 0xFEu64,                      false,false,true, false),
    // ADDS 32
    (fa32_a_10_20,     adds32(2,1), 10u64, 20u64,                          false,false,false,false),
    (fa32_a_ff_1,      adds32(2,1), 0xFFFF_FFFFu64, 1u64,                  false,true, true, false),
    (fa32_a_7f_1,      adds32(2,1), 0x7FFF_FFFFu64, 1u64,                  true, false,false,true),
    (fa32_a_80_ff,     adds32(2,1), 0x8000_0000u64, 0xFFFF_FFFFu64,        false,false,true, true),
    (fa32_a_1_neg2,    adds32(2,1), 1u64, 0xFFFF_FFFEu64,                  true, false,false,false),
    // SUBS 32
    (fs32_s_10_5,      subs32(2,1), 10u64, 5u64,                           false,false,true, false),
    (fs32_s_5_10,      subs32(2,1), 5u64, 10u64,                           true, false,false,false),
    (fs32_s_100_100,   subs32(2,1), 100u64, 100u64,                        false,true, true, false),
    (fs32_s_0_1,       subs32(2,1), 0u64, 1u64,                            true, false,false,false),
    (fs32_s_80_1,      subs32(2,1), 0x8000_0000u64, 1u64,                  false,false,true, true),
);

// ===================================================================
//  Bitfield — more UBFX/SBFX extractions
// ===================================================================
gen_1src!(
    // UBFX X0, X1, #0, #1 = UBFM X0, X1, #0, #0
    (ubfx64_b0, bitfield(1,0b10,1,0,0,1,0), 0xFFFF_FFFF_FFFF_FFFFu64, 1u64),
    (ubfx64_b1, bitfield(1,0b10,1,1,1,1,0), 0xFFFF_FFFF_FFFF_FFFFu64, 1u64),
    (ubfx64_b7, bitfield(1,0b10,1,7,7,1,0), 0xFFu64, 1u64),
    (ubfx64_byte0, bitfield(1,0b10,1,0,7,1,0), 0xABCD_EF01u64, 0x01u64),
    (ubfx64_byte1, bitfield(1,0b10,1,8,15,1,0), 0xABCD_EF01u64, 0xEFu64),
    (ubfx64_byte2, bitfield(1,0b10,1,16,23,1,0), 0xABCD_EF01u64, 0xCDu64),
    (ubfx64_byte3, bitfield(1,0b10,1,24,31,1,0), 0xABCD_EF01u64, 0xABu64),
    (ubfx64_hw0,   bitfield(1,0b10,1,0,15,1,0), 0xDEAD_BEEFu64, 0xBEEFu64),
    (ubfx64_hw1,   bitfield(1,0b10,1,16,31,1,0), 0xDEAD_BEEFu64, 0xDEADu64),
    // UBFX 32-bit
    (ubfx32_b0,    bitfield(0,0b10,0,0,0,1,0), 0xFFFF_FFFFu64, 1u64),
    (ubfx32_byte0, bitfield(0,0b10,0,0,7,1,0), 0xABCDu64, 0xCDu64),
    (ubfx32_byte1, bitfield(0,0b10,0,8,15,1,0), 0xABCDu64, 0xABu64),
    (ubfx32_hw0,   bitfield(0,0b10,0,0,15,1,0), 0xDEAD_BEEFu64, 0xBEEFu64),
    (ubfx32_hw1,   bitfield(0,0b10,0,16,31,1,0), 0xDEAD_BEEFu64, 0xDEADu64),
    // SBFX
    (sbfx64_b0_pos,  bitfield(1,0b00,1,0,0,1,0), 0u64, 0u64),
    (sbfx64_b0_neg,  bitfield(1,0b00,1,0,0,1,0), 1u64, 0xFFFF_FFFF_FFFF_FFFFu64),
    (sbfx64_byte0_p, bitfield(1,0b00,1,0,7,1,0), 0x7Fu64, 0x7Fu64),
    (sbfx64_byte0_n, bitfield(1,0b00,1,0,7,1,0), 0x80u64, 0xFFFF_FFFF_FFFF_FF80u64),
    (sbfx64_byte1_p, bitfield(1,0b00,1,8,15,1,0), 0x7F00u64, 0x7Fu64),
    (sbfx64_byte1_n, bitfield(1,0b00,1,8,15,1,0), 0x8000u64, 0xFFFF_FFFF_FFFF_FF80u64),
    (sbfx32_b0_neg,  bitfield(0,0b00,0,0,0,1,0), 1u64, 0xFFFF_FFFFu64),
    (sbfx32_byte0_n, bitfield(0,0b00,0,0,7,1,0), 0x80u64, 0xFFFF_FF80u64),
);

// ===================================================================
//  LSL/LSR/ASR immediate — via UBFM/SBFM, many amounts
// ===================================================================
gen_1src!(
    (lsr64_by1,  bitfield(1,0b10,1,1,63,1,0), 0x100u64, 0x80u64),
    (lsr64_by2,  bitfield(1,0b10,1,2,63,1,0), 0x100u64, 0x40u64),
    (lsr64_by4b, bitfield(1,0b10,1,4,63,1,0), 0x100u64, 0x10u64),
    (lsr64_by8b, bitfield(1,0b10,1,8,63,1,0), 0x100u64, 0x1u64),
    (lsr64_by16, bitfield(1,0b10,1,16,63,1,0), 0x10000u64, 0x1u64),
    (lsr64_by32b,bitfield(1,0b10,1,32,63,1,0), 0x1_0000_0000u64, 0x1u64),
    (lsl64_by1b, bitfield(1,0b10,1,63,62,1,0), 1u64, 2u64),
    (lsl64_by2b, bitfield(1,0b10,1,62,61,1,0), 1u64, 4u64),
    (lsl64_by4c, bitfield(1,0b10,1,60,59,1,0), 1u64, 16u64),
    (lsl64_by16b,bitfield(1,0b10,1,48,47,1,0), 1u64, 0x10000u64),
    (asr64_by1b, bitfield(1,0b00,1,1,63,1,0), (-4i64) as u64, (-2i64) as u64),
    (asr64_by2b, bitfield(1,0b00,1,2,63,1,0), (-4i64) as u64, (-1i64) as u64),
    (asr64_by4c, bitfield(1,0b00,1,4,63,1,0), (-16i64) as u64, (-1i64) as u64),
    (asr64_pos1, bitfield(1,0b00,1,1,63,1,0), 100u64, 50u64),
    (asr64_pos4, bitfield(1,0b00,1,4,63,1,0), 256u64, 16u64),
    // 32-bit
    (lsr32_by1,  bitfield(0,0b10,0,1,31,1,0), 0x100u64, 0x80u64),
    (lsr32_by4,  bitfield(0,0b10,0,4,31,1,0), 0x100u64, 0x10u64),
    (lsr32_by8,  bitfield(0,0b10,0,8,31,1,0), 0x100u64, 0x1u64),
    (lsr32_by16, bitfield(0,0b10,0,16,31,1,0), 0x10000u64, 0x1u64),
    (lsl32_by1,  bitfield(0,0b10,0,31,30,1,0), 1u64, 2u64),
    (lsl32_by4,  bitfield(0,0b10,0,28,27,1,0), 1u64, 16u64),
    (lsl32_by8b, bitfield(0,0b10,0,24,23,1,0), 1u64, 256u64),
    (lsl32_by16, bitfield(0,0b10,0,16,15,1,0), 1u64, 0x10000u64),
    (asr32_by1,  bitfield(0,0b00,0,1,31,1,0), 0x8000_0000u64, 0xC000_0000u64),
    (asr32_by4b, bitfield(0,0b00,0,4,31,1,0), 0x8000_0000u64, 0xF800_0000u64),
    (asr32_pos,  bitfield(0,0b00,0,1,31,1,0), 100u64, 50u64),
);

// ===================================================================
//  Extended register — all option × shift combos
// ===================================================================

fn add_ext(sf: u32, op: u32, s: u32, rm: u32, option: u32, imm3: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b01011 << 24) | (1 << 21) | (rm << 16) | (option << 13) | (imm3 << 10) | (rn << 5) | rd
}

macro_rules! gen_ext {
    ($( ($name:ident, $sf:expr, $op:expr, $opt:expr, $sh:expr, $a:expr, $b:expr, $exp:expr) ),+ $(,)?) => {
        $(
            #[test] fn $name() {
                let (mut c, mut m) = cpu_exec(&[add_ext($sf, $op, 0, 2, $opt, $sh, 1, 0)]);
                c.set_xn(1, $a); c.set_xn(2, $b);
                c.step(&mut m).unwrap();
                assert_eq!(c.xn(0), $exp);
            }
        )+
    };
}

gen_ext!(
    // UXTB (option=000) — extract byte, shift
    (ext_add_uxtb_s0, 1,0, 0b000,0, 0u64, 0xFFu64, 0xFFu64),
    (ext_add_uxtb_s1, 1,0, 0b000,1, 0u64, 0x80u64, 0x100u64),
    (ext_add_uxtb_s2, 1,0, 0b000,2, 0u64, 0x40u64, 0x100u64),
    (ext_add_uxtb_s3, 1,0, 0b000,3, 0u64, 0x20u64, 0x100u64),
    (ext_add_uxtb_s4, 1,0, 0b000,4, 0u64, 0x10u64, 0x100u64),
    (ext_add_uxtb_trunc, 1,0, 0b000,0, 100u64, 0x1FFu64, 100+0xFFu64),
    // UXTH (option=001)
    (ext_add_uxth_s0, 1,0, 0b001,0, 0u64, 0xFFFFu64, 0xFFFFu64),
    (ext_add_uxth_s1, 1,0, 0b001,1, 0u64, 0x8000u64, 0x10000u64),
    (ext_add_uxth_trunc, 1,0, 0b001,0, 0u64, 0x1FFFFu64, 0xFFFFu64),
    // UXTW (option=010) 
    (ext_add_uxtw_s0, 1,0, 0b010,0, 0u64, 0xFFFF_FFFFu64, 0xFFFF_FFFFu64),
    (ext_add_uxtw_s2, 1,0, 0b010,2, 0u64, 0x1u64, 4u64),
    (ext_add_uxtw_trunc, 1,0, 0b010,0, 0u64, 0x1_FFFF_FFFFu64, 0xFFFF_FFFFu64),
    // SXTB (option=100) — sign-extend byte
    (ext_add_sxtb_pos, 1,0, 0b100,0, 100u64, 0x7Fu64, 100+0x7Fu64),
    (ext_add_sxtb_neg, 1,0, 0b100,0, 100u64, 0x80u64, 100u64.wrapping_add((-128i64) as u64)),
    (ext_add_sxtb_s1, 1,0, 0b100,1, 0u64, 0x80u64, ((-128i64) << 1) as u64),
    (ext_add_sxtb_neg1, 1,0, 0b100,0, 0u64, 0xFFu64, (-1i64) as u64),
    // SXTH (option=101) — sign-extend halfword
    (ext_add_sxth_pos, 1,0, 0b101,0, 0u64, 0x7FFFu64, 0x7FFFu64),
    (ext_add_sxth_neg, 1,0, 0b101,0, 0u64, 0x8000u64, (-32768i64) as u64),
    (ext_add_sxth_s1, 1,0, 0b101,1, 0u64, 0x8000u64, ((-32768i64) << 1) as u64),
    // SXTW (option=110) — sign-extend word
    (ext_add_sxtw_pos, 1,0, 0b110,0, 0u64, 0x7FFF_FFFFu64, 0x7FFF_FFFFu64),
    (ext_add_sxtw_neg, 1,0, 0b110,0, 0u64, 0x8000_0000u64, (-0x8000_0000i64) as u64),
    (ext_add_sxtw_s2, 1,0, 0b110,2, 0u64, 0x8000_0000u64, ((-0x8000_0000i64) << 2) as u64),
    (ext_add_sxtw_neg1, 1,0, 0b110,0, 0u64, 0xFFFF_FFFFu64, (-1i64) as u64),
    // SXTX (option=111) — full 64-bit (identity for ADD)
    (ext_add_sxtx_s0, 1,0, 0b111,0, 100u64, 200u64, 300u64),
    (ext_add_sxtx_s3, 1,0, 0b111,3, 0u64, 1u64, 8u64),
    // SUB variants
    (ext_sub_uxtb_s0, 1,1, 0b000,0, 0x200u64, 0x1FFu64, 0x200-0xFFu64),
    (ext_sub_sxtw_neg, 1,1, 0b110,0, 100u64, 0xFFFF_FFFFu64, 101u64),
    (ext_sub_sxtb_neg, 1,1, 0b100,0, 0u64, 0x80u64, 128u64),
    // 32-bit
    (ext_add32_uxtb, 0,0, 0b000,0, 10u64, 0x1FFu64, 10+0xFFu64),
    (ext_add32_sxtb, 0,0, 0b100,0, 100u64, 0x80u64, (100u32.wrapping_add((-128i32) as u32)) as u64),
    (ext_sub32_uxtb, 0,1, 0b000,0, 0x200u64, 0xFFu64, 0x200-0xFFu64),
);

// ===================================================================
//  LDUR/STUR with many different offsets
// ===================================================================

macro_rules! gen_ldur_stur {
    ($( ($name:ident, $off:expr, $val:expr) ),+ $(,)?) => {
        $(
            #[test] fn $name() {
                let stur = (0b11u32 << 30) | (0b111000 << 24) | (0b00 << 22) | ((($off as u32) & 0x1FF) << 12) | (0b00 << 10) | (3 << 5) | 0;
                let ldur = (0b11u32 << 30) | (0b111000 << 24) | (0b01 << 22) | ((($off as u32) & 0x1FF) << 12) | (0b00 << 10) | (3 << 5) | 1;
                let (mut c, mut m) = cpu_exec(&[stur, ldur]);
                c.set_xn(0, $val); c.set_xn(3, D + 0x200);
                c.step(&mut m).unwrap(); c.step(&mut m).unwrap();
                assert_eq!(c.xn(1), $val);
            }
        )+
    };
}

gen_ldur_stur!(
    (ldur_stur_off0,    0i32,    0xDEADu64),
    (ldur_stur_off1,    1i32,    0xBEEFu64),
    (ldur_stur_off7,    7i32,    0x42u64),
    (ldur_stur_off8,    8i32,    0x1234u64),
    (ldur_stur_off15,   15i32,   0xFFu64),
    (ldur_stur_off16,   16i32,   0xABCDu64),
    (ldur_stur_off255,  255i32,  u64::MAX),
    (ldur_stur_neg1,    -1i32,   0xCAFEu64),
    (ldur_stur_neg2,    -2i32,   0x9999u64),
    (ldur_stur_neg4,    -4i32,   0x7777u64),
    (ldur_stur_neg8,    -8i32,   0x5555u64),
    (ldur_stur_neg16,   -16i32,  0x3333u64),
    (ldur_stur_neg32,   -32i32,  0x1111u64),
    (ldur_stur_neg64,   -64i32,  0x8888u64),
    (ldur_stur_neg128,  -128i32, 0x6666u64),
    (ldur_stur_neg255,  -255i32, 0x4444u64),
    (ldur_stur_neg256,  -256i32, 0x2222u64),
);

// ===================================================================
//  STR/LDR pre-index with various offsets
// ===================================================================

macro_rules! gen_pre_post {
    ($( ($name:ident, $pre:expr, $off:expr, $val:expr) ),+ $(,)?) => {
        $(
            #[test] fn $name() {
                let idx_bits = if $pre { 0b11u32 } else { 0b01u32 };
                let sinsn = (0b11u32 << 30) | (0b111000 << 24) | (0b00 << 22)
                    | ((($off as u32) & 0x1FF) << 12) | (idx_bits << 10) | (3 << 5) | 0;
                let (mut c, mut m) = cpu_exec(&[sinsn]);
                c.set_xn(0, $val); c.set_xn(3, D + 0x200);
                c.step(&mut m).unwrap();
                if $pre {
                    let target = (D as i64 + 0x200 + $off as i64) as u64;
                    assert_eq!(c.xn(3), target, "pre-index base update");
                    assert_eq!(rd64(&m, target), $val);
                } else {
                    assert_eq!(rd64(&m, D + 0x200), $val, "post-index stores at original");
                    let target = (D as i64 + 0x200 + $off as i64) as u64;
                    assert_eq!(c.xn(3), target, "post-index base update");
                }
            }
        )+
    };
}

gen_pre_post!(
    (pre_neg8,  true,  -8i32,  0xAAAAu64),
    (pre_neg16, true,  -16i32, 0xBBBBu64),
    (pre_neg32, true,  -32i32, 0xCCCCu64),
    (pre_pos8,  true,  8i32,   0xDDDDu64),
    (pre_pos16, true,  16i32,  0xEEEEu64),
    (pre_pos32, true,  32i32,  0xFFFFu64),
    (post_neg8, false, -8i32,  0x1111u64),
    (post_neg16,false, -16i32, 0x2222u64),
    (post_pos8, false, 8i32,   0x3333u64),
    (post_pos16,false, 16i32,  0x4444u64),
    (post_pos32,false, 32i32,  0x5555u64),
    (post_pos64,false, 64i32,  0x6666u64),
);

// ===================================================================
//  More CSINC/CSINV/CSNEG with different register values
// ===================================================================

gen_csel_sweep!(
    // CSINC with max values
    (csinc64_max_t, 1,0,1, 0,  u64::MAX, 0u64, false,true,false,false, u64::MAX),
    (csinc64_max_f, 1,0,1, 0,  0u64, u64::MAX, false,false,false,false, 0u64),
    (csinc64_0_f,   1,0,1, 1,  42u64, 0u64, false,true,false,false, 1u64),
    // CSINV with boundaries
    (csinv64_max_f, 1,1,0, 0,  0u64, u64::MAX, false,false,false,false, 0u64),
    (csinv64_1_f,   1,1,0, 0,  0u64, 1u64, false,false,false,false, !1u64),
    (csinv64_0_t,   1,1,0, 0,  42u64, 0u64, false,true,false,false, 42u64),
    // CSNEG with boundaries
    (csneg64_max_f, 1,1,1, 0,  0u64, u64::MAX, false,false,false,false, 1u64),
    (csneg64_1_f,   1,1,1, 0,  0u64, 1u64, false,false,false,false, (-1i64) as u64),
    (csneg64_min_f, 1,1,1, 0,  0u64, i64::MIN as u64, false,false,false,false, i64::MIN as u64),
    // 32-bit CSINC edge cases
    (csinc32_wrap,  0,0,1, 0,  0u64, 0xFFFF_FFFFu64, false,false,false,false, 0u64),
    // 32-bit CSINV
    (csinv32_0_f2,  0,1,0, 1,  10u64, 0u64, false,true,false,false, 0xFFFF_FFFFu64),
    // 32-bit CSNEG
    (csneg32_1_f,   0,1,1, 1,  10u64, 1u64, false,true,false,false, ((-1i32) as u32) as u64),
);

// ===================================================================
//  More flag tests for 32-bit ADDS with interesting boundary values
// ===================================================================

gen_flag_check!(
    (fa32_7fff_7fff, adds32(2,1), 0x7FFF_FFFFu64, 0x7FFF_FFFFu64, true,false,false,true),
    (fa32_1_ffff,    adds32(2,1), 1u64, 0xFFFF_FFFEu64,           true,false,false,false),
    (fa32_80_7f,     adds32(2,1), 0x8000_0000u64, 0x7FFF_FFFFu64, true,false,false,false),
    (fa32_ff_0,      adds32(2,1), 0xFFFF_FFFFu64, 0u64,           true,false,false,false),
    (fs32_ff_ff,     subs32(2,1), 0xFFFF_FFFFu64, 0xFFFF_FFFFu64, false,true,true,false),
    (fs32_80_80,     subs32(2,1), 0x8000_0000u64, 0x8000_0000u64, false,true,true,false),
    (fs32_7f_80,     subs32(2,1), 0x7FFF_FFFFu64, 0x8000_0000u64, true,false,false,true),
    (fs32_0_80,      subs32(2,1), 0u64, 0x8000_0000u64,           true,false,false,true),
    (fs32_80_7f,     subs32(2,1), 0x8000_0000u64, 0x7FFF_FFFFu64, false,false,true,true),
);

// ===================================================================
//  More CLZ/CLS boundary values  
// ===================================================================

gen_1src!(
    (clz64_pow2_1,  dp1(1,0b000100,1,0), 1u64<<1, 62u64),
    (clz64_pow2_2,  dp1(1,0b000100,1,0), 1u64<<2, 61u64),
    (clz64_pow2_4,  dp1(1,0b000100,1,0), 1u64<<4, 59u64),
    (clz64_pow2_8,  dp1(1,0b000100,1,0), 1u64<<8, 55u64),
    (clz64_pow2_16, dp1(1,0b000100,1,0), 1u64<<16, 47u64),
    (clz64_pow2_31, dp1(1,0b000100,1,0), 1u64<<31, 32u64),
    (clz64_pow2_32, dp1(1,0b000100,1,0), 1u64<<32, 31u64),
    (clz64_pow2_48, dp1(1,0b000100,1,0), 1u64<<48, 15u64),
    (clz64_pow2_62, dp1(1,0b000100,1,0), 1u64<<62, 1u64),
    (clz64_pow2_63, dp1(1,0b000100,1,0), 1u64<<63, 0u64),
    (clz32_pow2_1,  dp1(0,0b000100,1,0), 1u64<<1, 30u64),
    (clz32_pow2_8,  dp1(0,0b000100,1,0), 1u64<<8, 23u64),
    (clz32_pow2_16, dp1(0,0b000100,1,0), 1u64<<16, 15u64),
    (clz32_pow2_30, dp1(0,0b000100,1,0), 1u64<<30, 1u64),
    (clz32_pow2_31, dp1(0,0b000100,1,0), 1u64<<31, 0u64),
    // CLS
    (cls64_0,       dp1(1,0b000101,1,0), 0u64, 63u64),
    (cls64_neg1,    dp1(1,0b000101,1,0), u64::MAX, 63u64),
    (cls64_1,       dp1(1,0b000101,1,0), 1u64, 62u64),
    (cls64_neg2,    dp1(1,0b000101,1,0), (-2i64) as u64, 62u64),
    (cls64_msb,     dp1(1,0b000101,1,0), 1u64<<63, 0u64),
    (cls64_half,    dp1(1,0b000101,1,0), 0x4000_0000_0000_0000u64, 0u64),
    (cls32_0,       dp1(0,0b000101,1,0), 0u64, 31u64),
    (cls32_neg1,    dp1(0,0b000101,1,0), 0xFFFF_FFFFu64, 31u64),
    (cls32_1,       dp1(0,0b000101,1,0), 1u64, 30u64),
    (cls32_msb,     dp1(0,0b000101,1,0), 0x8000_0000u64, 0u64),
);

// ===================================================================
//  RBIT with bit patterns
// ===================================================================

gen_1src!(
    (rbit64_0xF,    dp1(1,0b000000,1,0), 0xFu64, 0xF000_0000_0000_0000u64),
    (rbit64_0xF0,   dp1(1,0b000000,1,0), 0xF0u64, 0x0F00_0000_0000_0000u64),
    (rbit64_0xFF00, dp1(1,0b000000,1,0), 0xFF00u64, 0x00FF_0000_0000_0000u64),
    (rbit64_byte3,  dp1(1,0b000000,1,0), 0xFF_0000u64, 0x0000_FF00_0000_0000u64),
    (rbit32_0xF,    dp1(0,0b000000,1,0), 0xFu64, 0xF000_0000u64),
    (rbit32_0xF0,   dp1(0,0b000000,1,0), 0xF0u64, 0x0F00_0000u64),
    (rbit32_0xFF00, dp1(0,0b000000,1,0), 0xFF00u64, 0x00FF_0000u64),
);

// ===================================================================
//  REV/REV16 with more patterns
// ===================================================================

gen_1src!(
    (rev64_pattern, dp1(1,0b000011,1,0), 0xDEAD_BEEF_CAFE_BABEu64, 0xBEBA_FECA_EFBE_ADDEu64),
    (rev32_pattern, dp1(0,0b000011,1,0), 0xDEAD_BEEFu64, 0xEFBE_ADDEu64),
    (rev16_64_pat,  dp1(1,0b000001,1,0), 0xDEAD_BEEF_CAFE_BABEu64, 0xADDE_EFBE_FECA_BEBAu64),
    (rev16_32_pat,  dp1(0,0b000001,1,0), 0xDEAD_BEEFu64, 0xADDE_EFBEu64),
    (rev32_64_pat,  dp1(1,0b000010,1,0), 0xDEAD_BEEF_CAFE_BABEu64, 0xEFBE_ADDE_BEBA_FECAu64),
);
