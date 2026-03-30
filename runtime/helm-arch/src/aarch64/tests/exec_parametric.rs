//! Parametric AArch64 tests. Ported from exec_parametric.rs.
//! (paste::item! macro removed; explicit test names used instead.)
use super::harness::*;

fn dp2(sf: u32, op: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b0011010110 << 21) | (rm << 16) | (op << 10) | (rn << 5) | rd
}
fn dp1(sf: u32, op: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b1011010110 << 21) | (op << 10) | (rn << 5) | rd
}
fn madd_enc(sf: u32, rm: u32, ra: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b0011011 << 24) | (rm << 16) | (ra << 10) | (rn << 5) | rd
}
fn add_sub_imm(sf: u32, op: u32, s: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b10001 << 24) | (imm12 << 10) | (rn << 5) | rd
}
fn bitfield(sf: u32, opc: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (opc << 29)
        | (0b100110 << 23)
        | (n << 22)
        | (immr << 16)
        | (imms << 10)
        | (rn << 5)
        | rd
}
fn mov_wide(sf: u32, opc: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}

macro_rules! gen_tests {
    ($( ($name:ident, $insn:expr, $rn:expr, $rm:expr, $expected:expr) ),+ $(,)?) => {
        $(
            #[test] fn $name() {
                let (mut c, mut m) = cpu_with_code(&[$insn]);
                c.x[1] = $rn; c.x[2] = $rm;
                step(&mut c, &mut m).unwrap();
                assert_eq!(c.x[0], $expected);
            }
        )+
    };
}
macro_rules! gen_1src {
    ($( ($name:ident, $insn:expr, $val:expr, $expected:expr) ),+ $(,)?) => {
        $(
            #[test] fn $name() {
                let (mut c, mut m) = cpu_with_code(&[$insn]);
                c.x[1] = $val;
                step(&mut c, &mut m).unwrap();
                assert_eq!(c.x[0], $expected);
            }
        )+
    };
}
macro_rules! gen_imm {
    ($( ($name:ident, $insn:expr, $val:expr, $expected:expr) ),+ $(,)?) => {
        $(
            #[test] fn $name() {
                let (mut c, mut m) = cpu_with_code(&[$insn]);
                c.x[1] = $val;
                step(&mut c, &mut m).unwrap();
                assert_eq!(c.x[0], $expected);
            }
        )+
    };
}

// UDIV sweep
gen_tests!(
    (
        param_udiv64_0_1,
        dp2(1, 0b000010, 2, 1, 0),
        0u64,
        1u64,
        0u64
    ),
    (
        param_udiv64_7_3,
        dp2(1, 0b000010, 2, 1, 0),
        7u64,
        3u64,
        2u64
    ),
    (
        param_udiv64_100_10,
        dp2(1, 0b000010, 2, 1, 0),
        100u64,
        10u64,
        10u64
    ),
    (
        param_udiv32_0_1,
        dp2(0, 0b000010, 2, 1, 0),
        0u64,
        1u64,
        0u64
    ),
    (
        param_udiv32_9_2,
        dp2(0, 0b000010, 2, 1, 0),
        9u64,
        2u64,
        4u64
    ),
);

// SDIV sweep
gen_tests!(
    (
        param_sdiv64_neg10_2,
        dp2(1, 0b000011, 2, 1, 0),
        (-10i64) as u64,
        2u64,
        (-5i64) as u64
    ),
    (
        param_sdiv64_10_neg2,
        dp2(1, 0b000011, 2, 1, 0),
        10u64,
        (-2i64) as u64,
        (-5i64) as u64
    ),
    (
        param_sdiv32_neg10_2,
        dp2(0, 0b000011, 2, 1, 0),
        (-10i32) as u32 as u64,
        2u64,
        (-5i32) as u32 as u64
    ),
);

// CLZ sweep
gen_1src!(
    (param_clz64_0, dp1(1, 0b000100, 1, 0), 0u64, 64u64),
    (param_clz64_1, dp1(1, 0b000100, 1, 0), 1u64, 63u64),
    (
        param_clz64_msb,
        dp1(1, 0b000100, 1, 0),
        0x8000_0000_0000_0000u64,
        0u64
    ),
    (param_clz32_0, dp1(0, 0b000100, 1, 0), 0u64, 32u64),
    (param_clz32_1, dp1(0, 0b000100, 1, 0), 1u64, 31u64),
    (
        param_clz32_msb,
        dp1(0, 0b000100, 1, 0),
        0x8000_0000u64,
        0u64
    ),
);

// RBIT sweep
gen_1src!(
    (
        param_rbit64_1,
        dp1(1, 0b000000, 1, 0),
        1u64,
        0x8000_0000_0000_0000u64
    ),
    (
        param_rbit64_msb,
        dp1(1, 0b000000, 1, 0),
        0x8000_0000_0000_0000u64,
        1u64
    ),
    (param_rbit64_max, dp1(1, 0b000000, 1, 0), u64::MAX, u64::MAX),
    (param_rbit32_1, dp1(0, 0b000000, 1, 0), 1u64, 0x8000_0000u64),
);

// ADD immediate sweep — add_sub_imm(sf, op, s, imm12, rn, rd)
gen_imm!(
    (param_add64_0_0, add_sub_imm(1, 0, 0, 1, 1, 0), 0u64, 1u64),
    (
        param_add64_max_0,
        add_sub_imm(1, 0, 0, 1, 1, 0),
        u64::MAX,
        0u64
    ),
    (param_add32_0_0, add_sub_imm(0, 0, 0, 1, 1, 0), 0u64, 1u64),
    (
        param_add32_max_0,
        add_sub_imm(0, 0, 0, 0xFFF, 1, 0),
        0xFFFF_FFFFu64,
        0xFFEu64
    ),
);

// MOVZ sweep
gen_1src!(
    (param_movz64_0, mov_wide(1, 0b10, 0, 0, 0), 0u64, 0u64),
    (
        param_movz64_ffff,
        mov_wide(1, 0b10, 0, 0xFFFF, 0),
        0u64,
        0xFFFFu64
    ),
    (param_movz32_0, mov_wide(0, 0b10, 0, 0, 0), 0u64, 0u64),
    (
        param_movz32_ffff,
        mov_wide(0, 0b10, 0, 0xFFFF, 0),
        0u64,
        0xFFFFu64
    ),
);

// MADD sweep
gen_tests!(
    (
        param_madd64_7_6,
        madd_enc(1, 2, 31, 1, 0),
        7u64,
        6u64,
        42u64
    ),
    (
        param_madd64_0_99,
        madd_enc(1, 2, 31, 1, 0),
        0u64,
        99u64,
        0u64
    ),
    (
        param_madd32_5_5,
        madd_enc(0, 2, 31, 1, 0),
        5u64,
        5u64,
        25u64
    ),
    (
        param_madd32_max_2,
        madd_enc(0, 2, 31, 1, 0),
        0xFFFF_FFFFu64,
        2u64,
        0xFFFF_FFFEu64
    ),
);

// UBFM / SBFM sweep
gen_1src!(
    (
        param_ubfm_lsl4,
        bitfield(1, 0b10, 1, 60, 59, 1, 0),
        0xFu64,
        0xF0u64
    ),
    (
        param_ubfm_lsr4,
        bitfield(1, 0b10, 1, 4, 63, 1, 0),
        0xF0u64,
        0xFu64
    ),
    (
        param_sbfm_sxtb_neg,
        bitfield(1, 0b00, 1, 0, 7, 1, 0),
        0x80u64,
        0xFFFF_FFFF_FFFF_FF80u64
    ),
    (
        param_sbfm_sxtb_pos,
        bitfield(1, 0b00, 1, 0, 7, 1, 0),
        0x7Fu64,
        0x7Fu64
    ),
    (
        param_sbfm_sxth,
        bitfield(1, 0b00, 1, 0, 15, 1, 0),
        0xFFFFu64,
        0xFFFF_FFFF_FFFF_FFFFu64
    ),
    (
        param_sbfm_sxtw,
        bitfield(1, 0b00, 1, 0, 31, 1, 0),
        0x8000_0000u64,
        0xFFFF_FFFF_8000_0000u64
    ),
);
