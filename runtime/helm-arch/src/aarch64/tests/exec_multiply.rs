//! AArch64 Multiply (3-source) instruction tests.
//! Ported from exec_multiply.rs.
use super::harness::*;

fn madd(sf: u32, rm: u32, ra: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (0b0011011 << 24)
        | (0b000 << 21)
        | (rm << 16)
        | (0 << 15)
        | (ra << 10)
        | (rn << 5)
        | rd
}
fn msub(sf: u32, rm: u32, ra: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (0b0011011 << 24)
        | (0b000 << 21)
        | (rm << 16)
        | (1 << 15)
        | (ra << 10)
        | (rn << 5)
        | rd
}
fn smaddl(rm: u32, ra: u32, rn: u32, rd: u32) -> u32 {
    (1 << 31)
        | (0b0011011 << 24)
        | (0b001 << 21)
        | (rm << 16)
        | (0 << 15)
        | (ra << 10)
        | (rn << 5)
        | rd
}
fn umaddl(rm: u32, ra: u32, rn: u32, rd: u32) -> u32 {
    (1 << 31)
        | (0b0011011 << 24)
        | (0b101 << 21)
        | (rm << 16)
        | (0 << 15)
        | (ra << 10)
        | (rn << 5)
        | rd
}
fn smulh(rm: u32, rn: u32, rd: u32) -> u32 {
    (1 << 31)
        | (0b0011011 << 24)
        | (0b010 << 21)
        | (rm << 16)
        | (0 << 15)
        | (31 << 10)
        | (rn << 5)
        | rd
}
fn umulh(rm: u32, rn: u32, rd: u32) -> u32 {
    (1 << 31)
        | (0b0011011 << 24)
        | (0b110 << 21)
        | (rm << 16)
        | (0 << 15)
        | (31 << 10)
        | (rn << 5)
        | rd
}

macro_rules! test_mul3 {
    ($name:ident, $insn:expr, $rn:expr, $rm:expr, $ra:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_with_code(&[$insn]);
            c.x[1] = $rn;
            c.x[2] = $rm;
            c.x[3] = $ra;
            step(&mut c, &mut m).unwrap();
            assert_eq!(c.x[0], $expected);
        }
    };
}
macro_rules! test_mul2 {
    ($name:ident, $insn:expr, $rn:expr, $rm:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_with_code(&[$insn]);
            c.x[1] = $rn;
            c.x[2] = $rm;
            step(&mut c, &mut m).unwrap();
            assert_eq!(c.x[0], $expected);
        }
    };
}

test_mul3!(mul_64_basic, madd(1, 2, 31, 1, 0), 7u64, 6u64, 0u64, 42u64);
test_mul3!(
    mul_64_by_zero,
    madd(1, 2, 31, 1, 0),
    0u64,
    12345u64,
    0u64,
    0u64
);
test_mul3!(
    mul_64_by_one,
    madd(1, 2, 31, 1, 0),
    42u64,
    1u64,
    0u64,
    42u64
);
test_mul3!(
    mul_64_overflow,
    madd(1, 2, 31, 1, 0),
    u64::MAX,
    2u64,
    0u64,
    u64::MAX.wrapping_mul(2)
);
test_mul3!(mul_32_basic, madd(0, 2, 31, 1, 0), 7u64, 6u64, 0u64, 42u64);
test_mul3!(
    mul_32_overflow,
    madd(0, 2, 31, 1, 0),
    0xFFFF_FFFFu64,
    2u64,
    0u64,
    0xFFFF_FFFEu64
);
test_mul3!(
    mul_32_truncate,
    madd(0, 2, 31, 1, 0),
    0x1_0000_0001u64,
    3u64,
    0u64,
    3u64
);
test_mul3!(
    madd_64_accum,
    madd(1, 2, 3, 1, 0),
    7u64,
    6u64,
    100u64,
    142u64
);
test_mul3!(madd_32_accum, madd(0, 2, 3, 1, 0), 5u64, 5u64, 10u64, 35u64);
test_mul3!(
    mneg_64_basic,
    msub(1, 2, 31, 1, 0),
    7u64,
    6u64,
    0u64,
    (-(42i64)) as u64
);
test_mul3!(
    msub_64_accum,
    msub(1, 2, 3, 1, 0),
    7u64,
    6u64,
    100u64,
    58u64
);
test_mul3!(
    msub_32_basic,
    msub(0, 2, 3, 1, 0),
    5u64,
    5u64,
    100u64,
    75u64
);
test_mul3!(
    smull_pos,
    smaddl(2, 31, 1, 0),
    100u64,
    200u64,
    0u64,
    20000u64
);
test_mul3!(
    smull_neg,
    smaddl(2, 31, 1, 0),
    (-10i32) as u32 as u64,
    20u64,
    0u64,
    (-200i64) as u64
);
test_mul3!(
    smull_neg_neg,
    smaddl(2, 31, 1, 0),
    (-10i32) as u32 as u64,
    (-20i32) as u32 as u64,
    0u64,
    200u64
);
test_mul3!(
    smaddl_accum,
    smaddl(2, 3, 1, 0),
    10u64,
    20u64,
    1000u64,
    1200u64
);
test_mul3!(
    smull_max,
    smaddl(2, 31, 1, 0),
    0x7FFF_FFFFu64,
    0x7FFF_FFFFu64,
    0u64,
    0x3FFF_FFFF_0000_0001u64
);
test_mul3!(
    umull_basic,
    umaddl(2, 31, 1, 0),
    100u64,
    200u64,
    0u64,
    20000u64
);
test_mul3!(
    umull_max,
    umaddl(2, 31, 1, 0),
    0xFFFF_FFFFu64,
    0xFFFF_FFFFu64,
    0u64,
    0xFFFF_FFFE_0000_0001u64
);
test_mul3!(
    umaddl_accum,
    umaddl(2, 3, 1, 0),
    0x1000u64,
    0x1000u64,
    42u64,
    0x100_0000u64 + 42
);
test_mul2!(smulh_small, smulh(2, 1, 0), 7u64, 6u64, 0u64);
test_mul2!(
    smulh_large,
    smulh(2, 1, 0),
    0x1_0000_0000u64,
    0x1_0000_0000u64,
    1u64
);
test_mul2!(smulh_neg, smulh(2, 1, 0), (-1i64) as u64, 2u64, u64::MAX);
test_mul2!(smulh_max_max, smulh(2, 1, 0), i64::MAX as u64, 2u64, 0u64);
test_mul2!(umulh_small, umulh(2, 1, 0), 7u64, 6u64, 0u64);
test_mul2!(
    umulh_large,
    umulh(2, 1, 0),
    0x1_0000_0000u64,
    0x1_0000_0000u64,
    1u64
);
test_mul2!(umulh_max, umulh(2, 1, 0), u64::MAX, u64::MAX, u64::MAX - 1);
test_mul2!(umulh_max_2, umulh(2, 1, 0), u64::MAX, 2u64, 1u64);
test_mul2!(umulh_half, umulh(2, 1, 0), 1u64 << 63, 2u64, 1u64);
test_mul3!(
    mul_64_neg1_pos,
    madd(1, 2, 31, 1, 0),
    u64::MAX,
    42u64,
    0u64,
    (-42i64) as u64
);
test_mul3!(
    mul_64_neg1_neg1,
    madd(1, 2, 31, 1, 0),
    u64::MAX,
    u64::MAX,
    0u64,
    1u64
);
test_mul3!(
    mul_64_1_max,
    madd(1, 2, 31, 1, 0),
    1u64,
    u64::MAX,
    0u64,
    u64::MAX
);
test_mul3!(
    mul_32_neg1_neg1,
    madd(0, 2, 31, 1, 0),
    0xFFFF_FFFFu64,
    0xFFFF_FFFFu64,
    0u64,
    1u64
);
test_mul3!(smull_0_0, smaddl(2, 31, 1, 0), 0u64, 0u64, 0u64, 0u64);
test_mul3!(smull_1_1, smaddl(2, 31, 1, 0), 1u64, 1u64, 0u64, 1u64);
test_mul3!(
    smull_neg1_1,
    smaddl(2, 31, 1, 0),
    0xFFFF_FFFFu64,
    1u64,
    0u64,
    (-1i64) as u64
);
test_mul3!(
    smull_min_min,
    smaddl(2, 31, 1, 0),
    0x8000_0000u64,
    0x8000_0000u64,
    0u64,
    0x4000_0000_0000_0000u64
);
test_mul3!(umull_0_0, umaddl(2, 31, 1, 0), 0u64, 0u64, 0u64, 0u64);
test_mul3!(
    umull_ff_ff,
    umaddl(2, 31, 1, 0),
    0xFFFF_FFFFu64,
    0xFFFF_FFFFu64,
    0u64,
    0xFFFF_FFFE_0000_0001u64
);
test_mul2!(smulh_0_0, smulh(2, 1, 0), 0u64, 0u64, 0u64);
test_mul2!(smulh_1_1, smulh(2, 1, 0), 1u64, 1u64, 0u64);
test_mul2!(smulh_neg1_neg1, smulh(2, 1, 0), u64::MAX, u64::MAX, 0u64);
test_mul2!(smulh_neg1_1, smulh(2, 1, 0), u64::MAX, 1u64, u64::MAX);
test_mul2!(umulh_0_0, umulh(2, 1, 0), 0u64, 0u64, 0u64);
test_mul2!(umulh_1_1, umulh(2, 1, 0), 1u64, 1u64, 0u64);
test_mul2!(umulh_max_1, umulh(2, 1, 0), u64::MAX, 1u64, 0u64);
test_mul2!(
    umulh_max_max2,
    umulh(2, 1, 0),
    u64::MAX,
    u64::MAX,
    u64::MAX - 1
);
