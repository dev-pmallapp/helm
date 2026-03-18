//! Corner-case tests for specific code paths. Ported from exec_corner_cases.rs.
use super::harness::*;

const D: u64 = DATA_BASE;

fn add_sub_imm(sf: u32, op: u32, s: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b10001 << 24) | (sh << 22) | (imm12 << 10) | (rn << 5) | rd
}
fn mov_wide(sf: u32, opc: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}
fn add_sub_ext(sf: u32, op: u32, s: u32, rm: u32, option: u32, imm3: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b01011 << 24) | (1 << 21) | (rm << 16) | (option << 13) | (imm3 << 10) | (rn << 5) | rd
}
fn log_reg(sf: u32, opc: u32, n: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b01010 << 24) | (shift << 22) | (n << 21) | (rm << 16) | (imm6 << 10) | (rn << 5) | rd
}
fn add_sub_reg(sf: u32, op: u32, s: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b01011 << 24) | (shift << 22) | (rm << 16) | (imm6 << 10) | (rn << 5) | rd
}
fn stur_x(imm9: i32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30) | (0b111000 << 24) | (0b00 << 22) | (((imm9 as u32) & 0x1FF) << 12) | (0b00 << 10) | (rn << 5) | rt
}
fn ldur_x(imm9: i32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30) | (0b111000 << 24) | (0b01 << 22) | (((imm9 as u32) & 0x1FF) << 12) | (0b00 << 10) | (rn << 5) | rt
}
fn str_x_pre(imm9: i32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30) | (0b111000 << 24) | (0b00 << 22) | (((imm9 as u32) & 0x1FF) << 12) | (0b11 << 10) | (rn << 5) | rt
}
fn ldr_x_pre(imm9: i32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30) | (0b111000 << 24) | (0b01 << 22) | (((imm9 as u32) & 0x1FF) << 12) | (0b11 << 10) | (rn << 5) | rt
}
fn str_x_post(imm9: i32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30) | (0b111000 << 24) | (0b00 << 22) | (((imm9 as u32) & 0x1FF) << 12) | (0b01 << 10) | (rn << 5) | rt
}
fn ldr_x_post(imm9: i32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30) | (0b111000 << 24) | (0b01 << 22) | (((imm9 as u32) & 0x1FF) << 12) | (0b01 << 10) | (rn << 5) | rt
}
fn ldr_x_reg(rm: u32, option: u32, s_flag: u32, rn: u32, rt: u32) -> u32 {
    (0b11 << 30) | (0b111000 << 24) | (0b01 << 22) | (1 << 21) | (rm << 16) | (option << 13) | (s_flag << 12) | (0b10 << 10) | (rn << 5) | rt
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
    c.x[1] = 5; step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 6);
    // Flags should be unchanged
    assert!(flag_n(&c)); assert!(flag_z(&c)); assert!(flag_c(&c)); assert!(flag_v(&c));
}

// --- Logical register shifts ---
#[test]
fn and_reg_lsl() {
    // AND X0, X1, X2, LSL #8
    let (mut c, mut m) = cpu_with_code(&[log_reg(1, 0b00, 0, 0b00, 2, 8, 1, 0)]);
    c.x[1] = 0xFFFF; c.x[2] = 0x01;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x100, "AND with LSL#8: X2<<8 = 0x100 & 0xFFFF = 0x100");
}
#[test]
fn orr_reg_lsr() {
    // ORR X0, X1, X2, LSR #4
    let (mut c, mut m) = cpu_with_code(&[log_reg(1, 0b01, 0, 0b01, 2, 4, 1, 0)]);
    c.x[1] = 0; c.x[2] = 0xF0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xF, "ORR with LSR#4: 0xF0>>4 = 0xF");
}

// --- STUR/LDUR negative offsets ---
#[test]
fn stur_x_negative_offset() {
    let (mut c, mut m) = cpu_with_code(&[stur_x(-8, 2, 0), ldur_x(-8, 2, 1)]);
    c.x[0] = 0xDEAD_CAFE; c.x[2] = D + 16;
    step(&mut c, &mut m).unwrap(); step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[1], 0xDEAD_CAFE);
}

// --- Pre/post indexed single loads ---
#[test]
fn str_x_pre_index() {
    let (mut c, mut m) = cpu_with_code(&[str_x_pre(8, 2, 0), ldr_x_post(8, 2, 1)]);
    c.x[0] = 0xABCD; c.x[2] = D;
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
    assert_eq!(c.x[0], 0xFFFF_FFFF, "MOVN W0, #0 must produce 0xFFFFFFFF not u64::MAX");
}

// --- Extended register addressing ---
#[test]
fn add_ext_uxtw() {
    // ADD X0, X1, W2, UXTW: zero-extends W2 to 64 bits
    let (mut c, mut m) = cpu_with_code(&[add_sub_ext(1, 0, 0, 2, 0b010, 0, 1, 0)]);
    c.x[1] = 100; c.x[2] = 0x1_0000_0032; // W2 = 50 (0x32)
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 150, "ADD X0, X1, W2 UXTW: 100 + UXTW(50) = 150");
}

// --- LDR register offset ---
#[test]
fn ldr_x_reg_lsl() {
    // LDR X0, [X1, X2, LSL #3]
    let (mut c, mut m) = cpu_with_code(&[ldr_x_reg(2, 0b111, 1, 1, 0)]);
    m.load_u64(D + 8 * 2, 0xBEEF);
    c.x[1] = D; c.x[2] = 2;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0xBEEF);
}

// --- Add/sub shifted register ---
#[test]
fn add_reg_asr_shift() {
    // ADD X0, X1, X2, ASR #4 (shift=10=0b10)
    let (mut c, mut m) = cpu_with_code(&[add_sub_reg(1, 0, 0, 0b10, 2, 4, 1, 0)]);
    c.x[1] = 100; c.x[2] = 0x8000_0000_0000_0000; // negative >> 4
    step(&mut c, &mut m).unwrap();
    let shifted = ((0x8000_0000_0000_0000u64 as i64) >> 4) as u64;
    assert_eq!(c.x[0], 100u64.wrapping_add(shifted));
}
