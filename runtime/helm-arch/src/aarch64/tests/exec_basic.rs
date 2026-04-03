//! Basic AArch64 execution tests — ported from old exec.rs.
use super::harness::*;

const BASE: u64 = CODE_BASE;
const NOP: u32 = 0xD503_201F;

#[test]
fn exec_add_imm() {
    let (mut a, mut m) = cpu_with_code(&[0x91_00A8_20]);
    a.x[1] = 100;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 142);
}
#[test]
fn exec_sub_imm() {
    let (mut a, mut m) = cpu_with_code(&[0xD1_0028_20]);
    a.x[1] = 50;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 40);
}
#[test]
fn exec_cmp_sets_zero_flag() {
    let (mut a, mut m) = cpu_with_code(&[0xF100_003F]);
    a.x[1] = 0;
    step(&mut a, &mut m).unwrap();
    assert!(flag_z(&a));
}
#[test]
fn exec_movz() {
    let (mut a, mut m) = cpu_with_code(&[0xD282_4680]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x1234);
}
#[test]
fn exec_movz_movk_chain() {
    let (mut a, mut m) = cpu_with_code(&[0xD2AA_CF00, 0xF282_4680]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x5678_0000);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x5678_1234);
}
#[test]
fn exec_adrp() {
    let (mut a, mut m) = cpu_with_code(&[0x9000_0020]);
    step(&mut a, &mut m).unwrap();
    let expected = (BASE & !0xFFF) + 0x4000;
    assert_eq!(a.x[0], expected);
}
#[test]
fn exec_b_forward() {
    let (mut a, mut m) = cpu_with_code(&[0x1400_0002, NOP]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.pc, BASE + 8);
}
#[test]
fn exec_bl_saves_lr() {
    let (mut a, mut m) = cpu_with_code(&[0x9400_0002, NOP]);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[30], BASE + 4);
    assert_eq!(a.pc, BASE + 8);
}
#[test]
fn exec_ret() {
    let (mut a, mut m) = cpu_with_code(&[0xD65F_03C0]);
    a.x[30] = 0x50_0000;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.pc, 0x50_0000);
}
#[test]
fn exec_cbz_taken() {
    let (mut a, mut m) = cpu_with_code(&[0xB400_0040, NOP]);
    a.x[0] = 0;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.pc, BASE + 8);
}
#[test]
fn exec_cbz_not_taken() {
    let (mut a, mut m) = cpu_with_code(&[0xB400_0040, NOP]);
    a.x[0] = 1;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.pc, BASE + 4);
}
#[test]
fn exec_str_ldr_roundtrip() {
    let (mut a, mut m) = cpu_with_code(&[0xF900_03E0, 0xF940_03E1]);
    a.x[0] = 0xDEAD_BEEF_CAFE;
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[1], 0xDEAD_BEEF_CAFE);
}
#[test]
fn exec_stp_ldp_pair() {
    let (mut a, mut m) = cpu_with_code(&[0xA9BF_07E0, 0xA8C1_0FE2]);
    a.x[0] = 0xAAAA;
    a.x[1] = 0xBBBB;
    let orig_sp = a.sp_el1;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.sp_el1, orig_sp - 16);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[2], 0xAAAA);
    assert_eq!(a.x[3], 0xBBBB);
    assert_eq!(a.sp_el1, orig_sp);
}

#[test]
fn stp_preindex_sp_writes_expected_stack_slots() {
    // stp x29, x30, [sp, #-32]!
    let (mut a, mut m) = cpu_with_code(&[0xA9BE_7BFD]);
    let orig_sp = a.sp_el1;
    a.x[29] = 0x1111_2222_3333_4444;
    a.x[30] = 0x5555_6666_7777_8888;

    step(&mut a, &mut m).unwrap();

    assert_eq!(a.sp_el1, orig_sp - 32);
    assert_eq!(m.read_u64(orig_sp - 32), 0x1111_2222_3333_4444);
    assert_eq!(m.read_u64(orig_sp - 24), 0x5555_6666_7777_8888);
}

#[test]
fn ldp_postindex_sp_reads_expected_stack_slots() {
    // ldp x29, x30, [sp], #32
    let (mut a, mut m) = cpu_with_code(&[0xA8C2_7BFD]);
    let orig_sp = a.sp_el1;
    m.load_u64(orig_sp, 0x1111_2222_3333_4444);
    m.load_u64(orig_sp + 8, 0x5555_6666_7777_8888);

    step(&mut a, &mut m).unwrap();

    assert_eq!(a.x[29], 0x1111_2222_3333_4444);
    assert_eq!(a.x[30], 0x5555_6666_7777_8888);
    assert_eq!(a.sp_el1, orig_sp + 32);
}

#[test]
fn stp_preindex_sp_with_48byte_frame_writes_expected_stack_slots() {
    // stp x29, x30, [sp, #-48]!
    let (mut a, mut m) = cpu_with_code(&[0xA9BD_7BFD]);
    let orig_sp = a.sp_el1;
    a.x[29] = 0x1111_2222_3333_4444;
    a.x[30] = 0x5555_6666_7777_8888;

    step(&mut a, &mut m).unwrap();

    assert_eq!(a.sp_el1, orig_sp - 48);
    assert_eq!(m.read_u64(orig_sp - 48), 0x1111_2222_3333_4444);
    assert_eq!(m.read_u64(orig_sp - 40), 0x5555_6666_7777_8888);
}

#[test]
fn ldp_postindex_sp_with_48byte_frame_reads_expected_stack_slots() {
    // ldp x29, x30, [sp], #48
    let (mut a, mut m) = cpu_with_code(&[0xA8C3_7BFD]);
    let orig_sp = a.sp_el1;
    m.load_u64(orig_sp, 0x1111_2222_3333_4444);
    m.load_u64(orig_sp + 8, 0x5555_6666_7777_8888);

    step(&mut a, &mut m).unwrap();

    assert_eq!(a.x[29], 0x1111_2222_3333_4444);
    assert_eq!(a.x[30], 0x5555_6666_7777_8888);
    assert_eq!(a.sp_el1, orig_sp + 48);
}

#[test]
fn stp_offset_sp_writes_expected_stack_slots() {
    // stp x19, x20, [sp, #16]
    let (mut a, mut m) = cpu_with_code(&[0xA901_53F3]);
    let orig_sp = a.sp_el1;
    a.x[19] = 0x0123_4567_89AB_CDEF;
    a.x[20] = 0x0FED_CBA9_8765_4321;

    step(&mut a, &mut m).unwrap();

    assert_eq!(a.sp_el1, orig_sp);
    assert_eq!(m.read_u64(orig_sp + 16), 0x0123_4567_89AB_CDEF);
    assert_eq!(m.read_u64(orig_sp + 24), 0x0FED_CBA9_8765_4321);
}

#[test]
fn ldp_offset_sp_reads_expected_stack_slots() {
    // ldp x19, x20, [sp, #16]
    let (mut a, mut m) = cpu_with_code(&[0xA941_53F3]);
    let orig_sp = a.sp_el1;
    m.load_u64(orig_sp + 16, 0x0123_4567_89AB_CDEF);
    m.load_u64(orig_sp + 24, 0x0FED_CBA9_8765_4321);

    step(&mut a, &mut m).unwrap();

    assert_eq!(a.x[19], 0x0123_4567_89AB_CDEF);
    assert_eq!(a.x[20], 0x0FED_CBA9_8765_4321);
    assert_eq!(a.sp_el1, orig_sp);
}
#[test]
fn exec_ldrb_zero_extends() {
    let (mut a, mut m) = cpu_with_code(&[0x3900_03E0, 0x3940_03E1]);
    a.x[0] = 0xFF;
    step(&mut a, &mut m).unwrap();
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[1], 0xFF);
}
#[test]
fn exec_mov_reg() {
    let (mut a, mut m) = cpu_with_code(&[0xAA01_03E0]);
    a.x[1] = 0x12345;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 0x12345);
}
#[test]
fn exec_mul() {
    let (mut a, mut m) = cpu_with_code(&[0x9B02_7C20]);
    a.x[1] = 7;
    a.x[2] = 6;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 42);
}
#[test]
fn exec_ldxr_stxr_succeeds() {
    let (mut a, mut m) = cpu_with_code(&[0xC85F_7C20, 0xC803_7C23]);
    m.load_u64(DATA_BASE, 42);
    a.x[1] = DATA_BASE;
    a.x[3] = 99;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[0], 42);
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[2], 0);
    assert_eq!(m.read_u64(DATA_BASE), 99);
}
#[ignore] // SWP atomic: not yet implemented in execute.rs
#[test]
fn exec_swp() {
    // SWP X0, X1, [X2]: size=11, opc=100, Rs=x0, Rn=x2, Rt=x1
    let (mut a, mut m) = cpu_with_code(&[0xF820_4041]);
    m.load_u64(DATA_BASE, 100);
    a.x[0] = 200;
    a.x[2] = DATA_BASE;
    step(&mut a, &mut m).unwrap();
    assert_eq!(a.x[1], 100);
    assert_eq!(m.read_u64(DATA_BASE), 200);
}
