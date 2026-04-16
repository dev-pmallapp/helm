//! SIMD/FP instruction tests -- ported from helm.git exec_simd.rs.
//!
//! Covers: CCMP immediate, SIMD Q/D register loads/stores, FMOV GPR<->FP,
//! vector EOR/CMEQ/INS/UMOV/MOVI/CMLT/ADD/DUP, string-processing patterns.
#![allow(dead_code)]
use super::harness::{cpu_with_code, step, CODE_BASE};

const BASE: u64 = CODE_BASE;

// -- CCMP immediate variant --------------------------------------------------

#[test]
fn ccmp_imm_eq_taken() {
    let (mut a, mut m) = cpu_with_code(&[
        0xF100_003F, // CMP X1, #0
        0x7A42_1800, // CCMP W0, #2, #0, NE
    ]);
    a.x[0] = 2;
    a.x[1] = 5;
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert!(a.flag_z(), "CCMP W0=#2 vs W0=2 should set Z");
    assert!(a.flag_c(), "CCMP W0=#2 vs W0=2 should set C");
}

#[test]
fn ccmp_imm_gt_taken() {
    let (mut a, mut m) = cpu_with_code(&[
        0xF100_003F, // CMP X1, #0
        0x7A42_1800, // CCMP W0, #2, #0, NE
    ]);
    a.x[0] = 5;
    a.x[1] = 3;
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert!(!a.flag_z(), "5 != 2, Z should be clear");
    assert!(a.flag_c(), "5 > 2 unsigned, C should be set");
}

#[test]
fn ccmp_imm_cond_false_uses_nzcv() {
    let (mut a, mut m) = cpu_with_code(&[
        0xF100_003F, // CMP X1, #0 -> sets Z if X1==0
        0x7A42_180A, // CCMP W0, #2, #0xA, NE -> NE=false when Z=1
    ]);
    a.x[0] = 5;
    a.x[1] = 0;
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert_eq!(
        a.nzcv >> 28,
        0xA,
        "When cond=NE is false, NZCV should be set to imm=0xA"
    );
}

#[test]
fn ccmp_reg_variant() {
    let (mut a, mut m) = cpu_with_code(&[
        0xF100_003F, // CMP X1, #0
        0x7A42_1000, // CCMP W0, W2, #0, NE (register variant)
    ]);
    a.x[0] = 10;
    a.x[1] = 1;
    a.x[2] = 10;
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert!(a.flag_z(), "CCMP W0=10, W2=10 should set Z");
}

// -- SIMD Q-register load/store ----------------------------------------------

#[test]
fn stur_q_stores_correctly() {
    let (mut a, mut m) = cpu_with_code(&[
        0x3C80_03E0, // STUR Q0, [SP]
    ]);
    a.v[0] = 0xDEAD_BEEF_CAFE_BABE_1234_5678_9ABC_DEF0;
    step(&mut a, &mut m).unwrap();
    let lo = m.read_u64(a.sp);
    let hi = m.read_u64(a.sp + 8);
    let stored = (lo as u128) | ((hi as u128) << 64);
    assert_eq!(stored, 0xDEAD_BEEF_CAFE_BABE_1234_5678_9ABC_DEF0);
}

#[test]
fn ldur_q_loads_correctly() {
    let (mut a, mut m) = cpu_with_code(&[
        0x3CC0_03E0, // LDUR Q0, [SP]
    ]);
    let val: u128 = 0x1111_2222_3333_4444_5555_6666_7777_8888;
    m.load(a.sp, &val.to_le_bytes());
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0], val);
}

#[test]
fn ned_style_q_copy_fragment_preserves_all_72_bytes() {
    let src = 0x10_0000u64;
    let dst = 0x10_1000u64;
    let (mut a, mut m) = cpu_with_code(&[
        0x3DC0_02BF, // LDR  Q31, [X21]
        0xF940_0AA0, // LDR  X0, [X21, #16]
        0xF900_0A80, // STR  X0, [X20, #16]
        0x3D80_029F, // STR  Q31, [X20]
        0x3CC1_82BF, // LDUR Q31, [X21, #24]
        0x3C81_829F, // STUR Q31, [X20, #24]
        0x3CC2_82BF, // LDUR Q31, [X21, #40]
        0x3C82_829F, // STUR Q31, [X20, #40]
        0x3CC3_82BF, // LDUR Q31, [X21, #56]
        0x3C83_829F, // STUR Q31, [X20, #56]
    ]);

    a.x[20] = dst;
    a.x[21] = src;

    let mut bytes = [0u8; 72];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(3).wrapping_add(1);
    }
    m.load(src, &bytes);
    m.load(dst, &[0u8; 72]);

    for _ in 0..10 {
        step(&mut a, &mut m).unwrap();
    }

    for i in 0..72u64 {
        assert_eq!(
            m.read_u8(dst + i),
            bytes[i as usize],
            "byte offset {i} must match after Ned-style Q copy"
        );
    }
}

#[test]
fn stur_d_stores_correctly() {
    let (mut a, mut m) = cpu_with_code(&[
        0xFC00_03E0, // STUR D0, [SP]
    ]);
    a.v[0] = 0xCAFE_BABE_DEAD_BEEF;
    step(&mut a, &mut m).unwrap();
    let stored = m.read_u64(a.sp);
    assert_eq!(stored, 0xCAFE_BABE_DEAD_BEEF);
}

// -- SIMD pair D-register ----------------------------------------------------

#[test]
fn stp_d_pair() {
    let (mut a, mut m) = cpu_with_code(&[
        0x6D00_03E0, // STP D0, D0, [SP]
    ]);
    a.v[0] = 0xAAAA_BBBB_CCCC_DDDDu128;
    step(&mut a, &mut m).unwrap();
    let lo = m.read_u64(a.sp);
    let hi = m.read_u64(a.sp + 8);
    assert_eq!(lo, 0xAAAA_BBBB_CCCC_DDDD);
    assert_eq!(hi, 0xAAAA_BBBB_CCCC_DDDD);
}

#[test]
fn ldp_d_pair() {
    let (mut a, mut m) = cpu_with_code(&[
        0x6D40_07E0, // LDP D0, D1, [SP]
    ]);
    m.load_u64(a.sp, 42);
    m.load_u64(a.sp + 8, 99);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0] as u64, 42);
    assert_eq!(a.v[1] as u64, 99);
}

// -- FMOV between FP and GP -------------------------------------------------

#[test]
fn fmov_x_to_d() {
    let (mut a, mut m) = cpu_with_code(&[0x9E67_0020]); // FMOV D0, X1
    a.x[1] = 0xDEAD_BEEF_CAFE_BABE;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0] as u64, 0xDEAD_BEEF_CAFE_BABE);
}

#[test]
fn fmov_d_to_x() {
    let (mut a, mut m) = cpu_with_code(&[0x9E66_0020]); // FMOV X0, D1
    a.v[1] = 0x1234_5678_9ABC_DEF0u128;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x1234_5678_9ABC_DEF0);
}

#[test]
fn cmge_d_zero_followed_by_fmov_x_preserves_all_one_bits() {
    let (mut a, mut m) = cpu_with_code(&[
        0xFD41_A29F, // LDR D31, [X20, #832]
        0x7EE0_8BFF, // CMGE D31, D31, #0
        0x9E66_03E2, // FMOV X2, D31
    ]);
    let src = 0x4000_0000_0000_0000u64;
    a.x[20] = 0x10_0000;
    m.load_u64(a.x[20] + 832, src);

    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[31] as u64, src, "LDR D31 should preserve raw bits");

    step(&mut a, &mut m).unwrap();
    assert_eq!(
        a.v[31] as u64,
        u64::MAX,
        "CMGE D31, D31, #0 should set all bits for a non-negative D lane"
    );

    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[2], u64::MAX, "FMOV X2, D31 should move raw bits");
}

#[test]
fn cmge_d_register_form_compares_single_scalar_lane() {
    let (mut a, mut m) = cpu_with_code(&[0x5EE2_3C20]); // CMGE D0, D1, D2
    a.v[1] = 7u128;
    a.v[2] = 3u128;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0] as u64, u64::MAX);

    let (mut a, mut m) = cpu_with_code(&[0x5EE2_3C20]); // CMGE D0, D1, D2
    a.v[1] = (-5i64 as u64) as u128;
    a.v[2] = 1u128;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0] as u64, 0);
}

#[test]
fn cmgt_d_zero_compares_single_scalar_lane() {
    let (mut a, mut m) = cpu_with_code(&[0x5EE0_8820]); // CMGT D0, D1, #0
    a.v[1] = 1u128;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0] as u64, u64::MAX);

    let (mut a, mut m) = cpu_with_code(&[0x5EE0_8820]); // CMGT D0, D1, #0
    a.v[1] = 0u128;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0] as u64, 0);
}

#[test]
fn abs_d_scalar_uses_single_signed_lane() {
    let (mut a, mut m) = cpu_with_code(&[0x5EE0_B820]); // ABS D0, D1
    a.v[1] = (-9i64 as u64) as u128;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0] as u64, 9);
}

#[test]
fn neg_d_scalar_uses_single_signed_lane() {
    let (mut a, mut m) = cpu_with_code(&[0x7EE0_B820]); // NEG D0, D1
    a.v[1] = 9u128;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0] as u64, (-9i64 as u64));
}

#[test]
fn fmov_w_to_s() {
    let (mut a, mut m) = cpu_with_code(&[0x1E27_0020]); // FMOV S0, W1
    a.x[1] = 0xDEAD_BEEF;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0] as u32, 0xDEAD_BEEF);
}

#[test]
fn fmov_s_to_w() {
    let (mut a, mut m) = cpu_with_code(&[0x1E26_0020]); // FMOV W0, S1
    a.v[1] = 0xCAFE_BABEu128;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xCAFE_BABE);
}

// -- SIMD vector operations --------------------------------------------------

#[test]
fn eor_v16b() {
    let (mut a, mut m) = cpu_with_code(&[0x6E21_1C00]); // EOR V0.16B, V0.16B, V1.16B
    a.v[0] = 0xFF00_FF00_FF00_FF00_FF00_FF00_FF00_FF00;
    a.v[1] = 0x0F0F_0F0F_0F0F_0F0F_0F0F_0F0F_0F0F_0F0F;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0], 0xF00F_F00F_F00F_F00F_F00F_F00F_F00F_F00F);
}

#[test]
fn cmeq_v8b_eq() {
    let (mut a, mut m) = cpu_with_code(&[0x2E21_8C20]); // CMEQ V0.8B, V1.8B, V1.8B
    a.v[1] = 0x0102030405060708;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0] as u64, 0xFFFF_FFFF_FFFF_FFFF);
}

#[test]
fn cmeq_v8b_neq() {
    let (mut a, mut m) = cpu_with_code(&[0x2E22_8C20]); // CMEQ V0.8B, V1.8B, V2.8B
    a.v[1] = 0x0102030405060708;
    a.v[2] = 0x0102030400060708;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0] as u64, 0xFFFFFFFF00FFFFFF);
}

#[test]
fn ins_d0_from_x() {
    let (mut a, mut m) = cpu_with_code(&[0x4E08_1C00]); // INS V0.D[0], X0
    a.v[0] = 0xAAAA_BBBB_CCCC_DDDD_0000_0000_0000_0000;
    a.x[0] = 0x1234_5678_9ABC_DEF0;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0], 0xAAAA_BBBB_CCCC_DDDD_1234_5678_9ABC_DEF0);
}

#[test]
fn umov_x_from_d() {
    let (mut a, mut m) = cpu_with_code(&[0x4E08_3C20]); // UMOV X0, V1.D[0]
    a.v[1] = 0xAAAA_BBBB_1234_5678u128;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xAAAA_BBBB_1234_5678);
}

#[test]
fn movi_v0_2d_zero() {
    let (mut a, mut m) = cpu_with_code(&[0x6F00_E400]); // MOVI V0.2D, #0
    a.v[0] = 0xDEAD_BEEF_CAFE_BABEu128;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0], 0);
}

#[test]
fn movi_v0_2d_allones() {
    let (mut a, mut m) = cpu_with_code(&[0x6F07_E7E0]); // MOVI V0.2D, #0xFF...FF
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0], u128::MAX);
}

#[test]
fn mvni_v31_2s_zero_imm_sets_all_ones_in_low_64() {
    let (mut a, mut m) = cpu_with_code(&[0x2F00_041F]); // MVNI V31.2S, #0
    a.v[31] = 0;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[31], 0xFFFF_FFFF_FFFF_FFFFu128);
}

#[test]
fn cmlt_v8b_zero() {
    let (mut a, mut m) = cpu_with_code(&[0x0E20_A820]); // CMLT V0.8B, V1.8B, #0
    a.v[1] = 0x00FF_0180_7F01_FE00;
    step(&mut a, &mut m).unwrap();
    let r = a.v[0] as u64;
    assert_eq!(r & 0xFF, 0x00, "byte 0 (0x00) is not < 0");
    assert_eq!((r >> 8) & 0xFF, 0xFF, "byte 1 (0xFE) is < 0 (signed)");
    assert_eq!((r >> 48) & 0xFF, 0xFF, "byte 6 (0xFF) is < 0 (signed)");
}

#[test]
fn add_v2d() {
    let (mut a, mut m) = cpu_with_code(&[0x4EE1_8400]); // ADD V0.2D, V0.2D, V1.2D
    a.v[0] = ((100u128) << 64) | 200u128;
    a.v[1] = ((300u128) << 64) | 400u128;
    step(&mut a, &mut m).unwrap();
    let lo = a.v[0] as u64;
    let hi = (a.v[0] >> 64) as u64;
    assert_eq!(lo, 600);
    assert_eq!(hi, 400);
}

#[test]
fn scalar_addp_d() {
    let (mut a, mut m) = cpu_with_code(&[0x5EF1_B800]); // ADDP D0, V0.2D
    a.v[0] = ((100u128) << 64) | 200u128;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0] as u64, 300);
}

#[test]
#[ignore = "SimdUshr is not yet implemented (silently skipped in Phase 0)"]
fn ushr_v2d_63() {
    let (mut a, mut m) = cpu_with_code(&[0x6F41_0420]); // USHR V0.2D, V1.2D, #63
    a.v[1] = ((0x8000_0000_0000_0000u128) << 64) | 0x4000_0000_0000_0000u128;
    step(&mut a, &mut m).unwrap();
    let lo = a.v[0] as u64;
    let hi = (a.v[0] >> 64) as u64;
    assert_eq!(lo, 0, "0x4000...>>63 = 0");
    assert_eq!(hi, 1, "0x8000...>>63 = 1");
}

// -- DUP element size handling -----------------------------------------------

#[test]
fn dup_v16b_byte() {
    let (mut a, mut m) = cpu_with_code(&[0x4E01_0C00]); // DUP V0.16B, W0
    a.x[0] = 0xDEAD_BEEF_CAFE_BA42;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0], 0x42424242424242424242424242424242);
}

#[test]
fn dup_v4s_word() {
    let (mut a, mut m) = cpu_with_code(&[0x4E04_0C00]); // DUP V0.4S, W0
    a.x[0] = 0xDEAD_BEEF;
    step(&mut a, &mut m).unwrap();
    let lo = a.v[0] as u64;
    let hi = (a.v[0] >> 64) as u64;
    assert_eq!(lo, 0xDEAD_BEEF_DEAD_BEEF);
    assert_eq!(hi, 0xDEAD_BEEF_DEAD_BEEF);
}

#[test]
fn dup_v2d_doubleword() {
    let (mut a, mut m) = cpu_with_code(&[0x4E08_0C00]); // DUP V0.2D, X0
    a.x[0] = 0xCAFE_BABE_DEAD_BEEF;
    step(&mut a, &mut m).unwrap();
    let lo = a.v[0] as u64;
    let hi = (a.v[0] >> 64) as u64;
    assert_eq!(lo, 0xCAFE_BABE_DEAD_BEEF);
    assert_eq!(hi, 0xCAFE_BABE_DEAD_BEEF);
}

#[test]
fn dup_v2d_not_truncated_to_byte() {
    let (mut a, mut m) = cpu_with_code(&[0x4E08_0C00]); // DUP V0.2D, X0
    a.x[0] = 0x1234_5678_9ABC_DEF0;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.v[0] as u64, 0x1234_5678_9ABC_DEF0);
}

// -- SIMD string-processing pattern ------------------------------------------

#[test]
fn cmeq_zero_finds_nul_byte() {
    let (mut a, mut m) = cpu_with_code(&[0x4E20_9800]); // CMEQ V0.16B, V0.16B, #0
    let data: [u8; 16] = [
        b'h', b'e', b'l', b'l', b'o', 0, b'X', b'X', b'X', b'X', b'X', b'X', b'X', b'X', b'X', b'X',
    ];
    a.v[0] = u128::from_le_bytes(data);
    step(&mut a, &mut m).unwrap();
    let result = a.v[0].to_le_bytes();
    assert_eq!(result[0], 0x00, "h != 0");
    assert_eq!(result[5], 0xFF, "NUL byte should match");
    assert_eq!(result[6], 0x00, "X != 0");
}

#[test]
fn umaxv_detects_nul_after_cmeq() {
    let (mut a, mut m) = cpu_with_code(&[
        0x4E20_9800, // CMEQ V0.16B, V0.16B, #0
        0x6E30_A800, // UMAXV B0, V0.16B
        0x1E26_0000, // FMOV W0, S0
    ]);
    let data: [u8; 16] = [
        b'h', b'e', b'l', b'l', b'o', 0, b'X', b'X', b'X', b'X', b'X', b'X', b'X', b'X', b'X', b'X',
    ];
    a.v[0] = u128::from_le_bytes(data);
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0xFF, "UMAXV should find the 0xFF NUL-match byte");
}

#[test]
fn umaxv_no_nul_in_string() {
    let (mut a, mut m) = cpu_with_code(&[
        0x4E20_9800, // CMEQ V0.16B, V0.16B, #0
        0x6E30_A800, // UMAXV B0, V0.16B
        0x1E26_0000, // FMOV W0, S0
    ]);
    let data: [u8; 16] = *b"abcdefghijklmnop";
    a.v[0] = u128::from_le_bytes(data);
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0, "No NUL bytes means UMAXV should be 0");
}

#[test]
fn umaxp_4s_pairwise_max() {
    // UMAXP V0.4S, V1.4S, V2.4S => 0x6EA2A420
    let (mut a, mut m) = cpu_with_code(&[0x6EA2_A420]);
    // V1 = [10, 30, 20, 40] (4 x 32-bit), V2 = [5, 15, 25, 35]
    a.v[1] = 10u128 | (30u128 << 32) | (20u128 << 64) | (40u128 << 96);
    a.v[2] = 5u128 | (15u128 << 32) | (25u128 << 64) | (35u128 << 96);
    step(&mut a, &mut m).unwrap();
    // Result lower half: max(10,30)=30, max(20,40)=40
    // Result upper half: max(5,15)=15, max(25,35)=35
    let r0 = (a.v[0] >> 0) as u32;
    let r1 = (a.v[0] >> 32) as u32;
    let r2 = (a.v[0] >> 64) as u32;
    let r3 = (a.v[0] >> 96) as u32;
    assert_eq!(r0, 30, "max(10,30)");
    assert_eq!(r1, 40, "max(20,40)");
    assert_eq!(r2, 15, "max(5,15)");
    assert_eq!(r3, 35, "max(25,35)");
}

#[test]
fn ld1_single_d_element_index1() {
    // LD1 {V31.D}[1], [X19]  →  0x4D40867F
    let (mut a, mut m) = cpu_with_code(&[0x4D40_867F]);
    a.x[19] = 0x1000;
    a.v[31] = 0xDEAD_BEEF_CAFE_BABEu128; // lower 64 bits pre-set
    // Write 8 bytes at address 0x1000
    m.load_u64(0x1000, 0x1234_5678_9ABC_DEF0);
    step(&mut a, &mut m).unwrap();
    // Upper 64 bits should be loaded, lower 64 bits preserved
    assert_eq!(a.v[31] & 0xFFFF_FFFF_FFFF_FFFF, 0xDEAD_BEEF_CAFE_BABE);
    assert_eq!(a.v[31] >> 64, 0x1234_5678_9ABC_DEF0);
}

#[test]
fn st1_single_s_element_index0() {
    // ST1 {V0.S}[0], [X1]  →  0x0D008020
    // Q=0, L=0, opcode=100, S=0, size=00 → S element, index = 0
    let (mut a, mut m) = cpu_with_code(&[0x0D00_8020]);
    a.x[1] = 0x2000;
    a.v[0] = 0xAAAA_BBBB_CCCC_DDDDu128 | (0x1111_2222_3333_4444u128 << 64);
    step(&mut a, &mut m).unwrap();
    let stored = m.read_u32(0x2000) as u64;
    assert_eq!(stored, 0xCCCC_DDDD); // S[0] = lowest 32 bits
}
