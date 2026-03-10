//! Bulk load/store tests — all sizes, addressing modes, sign-extension.

use crate::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;

fn cpu_exec(insns: &[u32]) -> (Aarch64Cpu, AddressSpace) {
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();
    let base = 0x40_0000u64;
    let size = (insns.len() * 4 + 0x1000) as u64;
    mem.map(base, size, (true, true, true));
    for (i, insn) in insns.iter().enumerate() {
        mem.write(base + (i as u64 * 4), &insn.to_le_bytes())
            .unwrap();
    }
    cpu.regs.pc = base;
    mem.map(0x7FFF_0000, 0x10000, (true, true, false));
    cpu.regs.sp = 0x7FFF_8000;
    mem.map(0x10_0000, 0x4000, (true, true, false));
    (cpu, mem)
}

const D: u64 = 0x10_0000;

fn wr64(m: &mut AddressSpace, a: u64, v: u64) {
    m.write(a, &v.to_le_bytes()).unwrap();
}
fn wr32(m: &mut AddressSpace, a: u64, v: u32) {
    m.write(a, &v.to_le_bytes()).unwrap();
}
fn wr16(m: &mut AddressSpace, a: u64, v: u16) {
    m.write(a, &v.to_le_bytes()).unwrap();
}
fn wr8(m: &mut AddressSpace, a: u64, v: u8) {
    m.write(a, &[v]).unwrap();
}
fn rd64(m: &mut AddressSpace, a: u64) -> u64 {
    let mut b = [0u8; 8];
    m.read(a, &mut b).unwrap();
    u64::from_le_bytes(b)
}
fn rd32(m: &mut AddressSpace, a: u64) -> u32 {
    let mut b = [0u8; 4];
    m.read(a, &mut b).unwrap();
    u32::from_le_bytes(b)
}
fn rd16(m: &mut AddressSpace, a: u64) -> u16 {
    let mut b = [0u8; 2];
    m.read(a, &mut b).unwrap();
    u16::from_le_bytes(b)
}
fn rd8(m: &mut AddressSpace, a: u64) -> u8 {
    let mut b = [0u8; 1];
    m.read(a, &mut b).unwrap();
    b[0]
}

// Unsigned-offset encodings: size 111001 opc imm12 rn rt
fn str_uoff(sz: u32, opc: u32, imm12: u32, rn: u32, rt: u32) -> u32 {
    (sz << 30) | (0b111001 << 24) | (opc << 22) | (imm12 << 10) | (rn << 5) | rt
}
fn ldr_x(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b11, 0b01, i, rn, rt)
}
fn str_x(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b11, 0b00, i, rn, rt)
}
fn ldr_w(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b10, 0b01, i, rn, rt)
}
fn str_w(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b10, 0b00, i, rn, rt)
}
fn ldrh(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b01, 0b01, i, rn, rt)
}
fn strh(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b01, 0b00, i, rn, rt)
}
fn ldrb(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b00, 0b01, i, rn, rt)
}
fn strb(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b00, 0b00, i, rn, rt)
}
fn ldrsw(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b10, 0b10, i, rn, rt)
}
fn ldrsb_w(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b00, 0b11, i, rn, rt)
}
fn ldrsb_x(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b00, 0b10, i, rn, rt)
}
fn ldrsh_w(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b01, 0b11, i, rn, rt)
}
fn ldrsh_x(i: u32, rn: u32, rt: u32) -> u32 {
    str_uoff(0b01, 0b10, i, rn, rt)
}

// STP/LDP encodings
fn stp_x(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_10_0u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn ldp_x(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_10_1u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn stp_x_pre(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_11_0u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn ldp_x_pre(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_11_1u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn stp_x_post(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_01_0u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn ldp_x_post(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b10_101_0_0_01_1u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn stp_w(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b00_101_0_0_10_0u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}
fn ldp_w(imm7: i32, rt2: u32, rn: u32, rt: u32) -> u32 {
    (0b00_101_0_0_10_1u32 << 22) | (((imm7 as u32) & 0x7F) << 15) | (rt2 << 10) | (rn << 5) | rt
}

// ── Macros ──

macro_rules! test_str_ldr {
    ($name:ident, $str_fn:ident, $ldr_fn:ident, $write_val:expr, $expected:expr, $offset:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_exec(&[$str_fn($offset, 3, 0), $ldr_fn($offset, 3, 1)]);
            c.set_xn(0, $write_val);
            c.set_xn(3, D);
            c.step(&mut m).unwrap();
            c.step(&mut m).unwrap();
            assert_eq!(c.xn(1), $expected);
        }
    };
}

macro_rules! test_ldrs {
    ($name:ident, $ldr_fn:ident, $wr_fn:ident, $mem_val:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_exec(&[$ldr_fn(0, 3, 0)]);
            $wr_fn(&mut m, D, $mem_val);
            c.set_xn(3, D);
            c.step(&mut m).unwrap();
            assert_eq!(c.xn(0), $expected);
        }
    };
}

macro_rules! test_stp_ldp {
    ($name:ident, $stp:ident, $ldp:ident, $off:expr, $a:expr, $b:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_exec(&[$stp($off, 1, 3, 0), $ldp($off, 5, 3, 4)]);
            c.set_xn(0, $a);
            c.set_xn(1, $b);
            c.set_xn(3, D + 0x100);
            c.step(&mut m).unwrap();
            c.step(&mut m).unwrap();
            assert_eq!(c.xn(4), $a);
            assert_eq!(c.xn(5), $b);
        }
    };
}

// ===================================================================
//  STR / LDR — X (64-bit) at various offsets
// ===================================================================
test_str_ldr!(
    str_ldr_x_off0,
    str_x,
    ldr_x,
    0xDEAD_BEEF_CAFE_1234u64,
    0xDEAD_BEEF_CAFE_1234u64,
    0
);
test_str_ldr!(str_ldr_x_off1, str_x, ldr_x, 0x42u64, 0x42u64, 1);
test_str_ldr!(str_ldr_x_off2, str_x, ldr_x, 0x1234u64, 0x1234u64, 2);
test_str_ldr!(str_ldr_x_off10, str_x, ldr_x, u64::MAX, u64::MAX, 10);
test_str_ldr!(
    str_ldr_x_off100,
    str_x,
    ldr_x,
    0xAABBCCDDu64,
    0xAABBCCDDu64,
    100
);

// ===================================================================
//  STR / LDR — W (32-bit) at various offsets
// ===================================================================
test_str_ldr!(
    str_ldr_w_off0,
    str_w,
    ldr_w,
    0x1_FFFF_FFFFu64,
    0xFFFF_FFFFu64,
    0
);
test_str_ldr!(
    str_ldr_w_off1,
    str_w,
    ldr_w,
    0x12345678u64,
    0x12345678u64,
    1
);
test_str_ldr!(str_ldr_w_off5, str_w, ldr_w, 0u64, 0u64, 5);
test_str_ldr!(
    str_ldr_w_off50,
    str_w,
    ldr_w,
    0xDEADBEEFu64,
    0xDEADBEEFu64,
    50
);

// ===================================================================
//  STRH / LDRH — halfword
// ===================================================================
test_str_ldr!(strh_ldrh_off0, strh, ldrh, 0xABCDu64, 0xABCDu64, 0);
test_str_ldr!(strh_ldrh_off1, strh, ldrh, 0x1234u64, 0x1234u64, 1);
test_str_ldr!(strh_ldrh_off10, strh, ldrh, 0xFFFFu64, 0xFFFFu64, 10);
test_str_ldr!(strh_ldrh_trunc, strh, ldrh, 0x1FFFFu64, 0xFFFFu64, 0);

// ===================================================================
//  STRB / LDRB — byte
// ===================================================================
test_str_ldr!(strb_ldrb_off0, strb, ldrb, 0xFFu64, 0xFFu64, 0);
test_str_ldr!(strb_ldrb_off1, strb, ldrb, 0x42u64, 0x42u64, 1);
test_str_ldr!(strb_ldrb_off100, strb, ldrb, 0xABu64, 0xABu64, 100);
test_str_ldr!(strb_ldrb_trunc, strb, ldrb, 0x1FFu64, 0xFFu64, 0);
test_str_ldr!(strb_ldrb_zero, strb, ldrb, 0u64, 0u64, 0);

// ===================================================================
//  Sign-extending loads — LDRSW
// ===================================================================
test_ldrs!(ldrsw_pos_0, ldrsw, wr32, 0x7FFF_FFFFu32, 0x7FFF_FFFFu64);
test_ldrs!(
    ldrsw_neg_msb,
    ldrsw,
    wr32,
    0x8000_0000u32,
    0xFFFF_FFFF_8000_0000u64
);
test_ldrs!(
    ldrsw_neg_1,
    ldrsw,
    wr32,
    0xFFFF_FFFFu32,
    0xFFFF_FFFF_FFFF_FFFFu64
);
test_ldrs!(ldrsw_zero, ldrsw, wr32, 0u32, 0u64);
test_ldrs!(
    ldrsw_small_neg,
    ldrsw,
    wr32,
    0xFFFF_FFF0u32,
    0xFFFF_FFFF_FFFF_FFF0u64
);

// ===================================================================
//  Sign-extending loads — LDRSB to W (32-bit dest)
// ===================================================================
test_ldrs!(ldrsb_w_pos, ldrsb_w, wr8, 0x7Fu8, 0x7Fu64);
test_ldrs!(ldrsb_w_neg, ldrsb_w, wr8, 0x80u8, 0xFFFF_FF80u64);
test_ldrs!(ldrsb_w_neg1, ldrsb_w, wr8, 0xFFu8, 0xFFFF_FFFFu64);
test_ldrs!(ldrsb_w_zero, ldrsb_w, wr8, 0u8, 0u64);
test_ldrs!(ldrsb_w_1, ldrsb_w, wr8, 1u8, 1u64);

// ===================================================================
//  Sign-extending loads — LDRSB to X (64-bit dest)
// ===================================================================
test_ldrs!(ldrsb_x_pos, ldrsb_x, wr8, 0x7Fu8, 0x7Fu64);
test_ldrs!(ldrsb_x_neg, ldrsb_x, wr8, 0x80u8, 0xFFFF_FFFF_FFFF_FF80u64);
test_ldrs!(ldrsb_x_neg1, ldrsb_x, wr8, 0xFFu8, 0xFFFF_FFFF_FFFF_FFFFu64);
test_ldrs!(ldrsb_x_zero, ldrsb_x, wr8, 0u8, 0u64);

// ===================================================================
//  Sign-extending loads — LDRSH to W and X
// ===================================================================
test_ldrs!(ldrsh_w_pos, ldrsh_w, wr16, 0x7FFFu16, 0x7FFFu64);
test_ldrs!(ldrsh_w_neg, ldrsh_w, wr16, 0x8000u16, 0xFFFF_8000u64);
test_ldrs!(ldrsh_w_neg1, ldrsh_w, wr16, 0xFFFFu16, 0xFFFF_FFFFu64);
test_ldrs!(ldrsh_w_zero, ldrsh_w, wr16, 0u16, 0u64);
test_ldrs!(ldrsh_x_pos, ldrsh_x, wr16, 0x7FFFu16, 0x7FFFu64);
test_ldrs!(
    ldrsh_x_neg,
    ldrsh_x,
    wr16,
    0x8000u16,
    0xFFFF_FFFF_FFFF_8000u64
);
test_ldrs!(
    ldrsh_x_neg1,
    ldrsh_x,
    wr16,
    0xFFFFu16,
    0xFFFF_FFFF_FFFF_FFFFu64
);
test_ldrs!(ldrsh_x_zero, ldrsh_x, wr16, 0u16, 0u64);

// ===================================================================
//  STP / LDP — 64-bit pairs, various offsets
// ===================================================================
test_stp_ldp!(stp_ldp_x_off0, stp_x, ldp_x, 0, 0xAAAAu64, 0xBBBBu64);
test_stp_ldp!(stp_ldp_x_off2, stp_x, ldp_x, 2, 0x1111u64, 0x2222u64);
test_stp_ldp!(stp_ldp_x_neg2, stp_x, ldp_x, -2, 0xCCCCu64, 0xDDDDu64);
test_stp_ldp!(stp_ldp_x_off10, stp_x, ldp_x, 10, u64::MAX, 0u64);

// ===================================================================
//  STP / LDP — 32-bit pairs
// ===================================================================
test_stp_ldp!(stp_ldp_w_off0, stp_w, ldp_w, 0, 0xAAAAu64, 0xBBBBu64);
test_stp_ldp!(stp_ldp_w_off4, stp_w, ldp_w, 4, 0x1111u64, 0x2222u64);

// ===================================================================
//  STP / LDP — pre-index
// ===================================================================

#[test]
fn stp_pre_neg4() {
    let (mut c, mut m) = cpu_exec(&[stp_x_pre(-4, 1, 3, 0)]);
    c.set_xn(0, 0xAA);
    c.set_xn(1, 0xBB);
    c.set_xn(3, D + 0x100);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(3), D + 0x100 - 32, "pre-index decrements");
    assert_eq!(rd64(&mut m, D + 0x100 - 32), 0xAA);
    assert_eq!(rd64(&mut m, D + 0x100 - 24), 0xBB);
}

#[test]
fn ldp_pre_pos2() {
    let (mut c, mut m) = cpu_exec(&[ldp_x_pre(2, 1, 3, 0)]);
    wr64(&mut m, D + 0x110, 0x1111);
    wr64(&mut m, D + 0x118, 0x2222);
    c.set_xn(3, D + 0x100);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(3), D + 0x110, "pre-index increments");
    assert_eq!(c.xn(0), 0x1111);
    assert_eq!(c.xn(1), 0x2222);
}

// ===================================================================
//  STP / LDP — post-index
// ===================================================================

#[test]
fn stp_post_pos2() {
    let (mut c, mut m) = cpu_exec(&[stp_x_post(2, 1, 3, 0)]);
    c.set_xn(0, 0xCC);
    c.set_xn(1, 0xDD);
    c.set_xn(3, D + 0x100);
    c.step(&mut m).unwrap();
    assert_eq!(rd64(&mut m, D + 0x100), 0xCC, "stores at original base");
    assert_eq!(rd64(&mut m, D + 0x108), 0xDD);
    assert_eq!(c.xn(3), D + 0x110, "post-index increments after");
}

#[test]
fn ldp_post_neg2() {
    let (mut c, mut m) = cpu_exec(&[ldp_x_post(-2, 1, 3, 0)]);
    wr64(&mut m, D + 0x100, 0x5555);
    wr64(&mut m, D + 0x108, 0x6666);
    c.set_xn(3, D + 0x100);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x5555);
    assert_eq!(c.xn(1), 0x6666);
    assert_eq!(c.xn(3), D + 0xF0, "post-index decrements");
}

// ===================================================================
//  Store then verify memory contents directly
// ===================================================================

macro_rules! test_str_verify {
    ($name:ident, $str_fn:ident, $rd_fn:ident, $val:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let (mut c, mut m) = cpu_exec(&[$str_fn(0, 3, 0)]);
            c.set_xn(0, $val);
            c.set_xn(3, D);
            c.step(&mut m).unwrap();
            assert_eq!($rd_fn(&mut m, D), $expected);
        }
    };
}

test_str_verify!(str_x_verify_max, str_x, rd64, u64::MAX, u64::MAX);
test_str_verify!(str_x_verify_zero, str_x, rd64, 0u64, 0u64);
test_str_verify!(
    str_w_verify_max,
    str_w,
    rd32,
    0xFFFF_FFFFu64,
    0xFFFF_FFFFu32
);
test_str_verify!(
    str_w_verify_trunc,
    str_w,
    rd32,
    0x1_2345_6789u64,
    0x2345_6789u32
);
test_str_verify!(strh_verify_max, strh, rd16, 0xFFFFu64, 0xFFFFu16);
test_str_verify!(strh_verify_trunc, strh, rd16, 0x1FFFFu64, 0xFFFFu16);
test_str_verify!(strb_verify_max, strb, rd8, 0xFFu64, 0xFFu8);
test_str_verify!(strb_verify_trunc, strb, rd8, 0x1FFu64, 0xFFu8);

// ===================================================================
//  Load via SP (Rn=31)
// ===================================================================

#[test]
fn ldr_x_via_sp() {
    let (mut c, mut m) = cpu_exec(&[str_x(0, 31, 0), ldr_x(0, 31, 1)]);
    c.set_xn(0, 0xCAFE);
    c.step(&mut m).unwrap();
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xCAFE);
}
#[test]
fn str_w_via_sp() {
    let (mut c, mut m) = cpu_exec(&[str_w(0, 31, 0), ldr_w(0, 31, 1)]);
    c.set_xn(0, 0xBEEF);
    c.step(&mut m).unwrap();
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xBEEF);
}
#[test]
fn strb_via_sp() {
    let (mut c, mut m) = cpu_exec(&[strb(0, 31, 0), ldrb(0, 31, 1)]);
    c.set_xn(0, 0xAB);
    c.step(&mut m).unwrap();
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0xAB);
}
#[test]
fn strh_via_sp() {
    let (mut c, mut m) = cpu_exec(&[strh(0, 31, 0), ldrh(0, 31, 1)]);
    c.set_xn(0, 0x1234);
    c.step(&mut m).unwrap();
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(1), 0x1234);
}

// ===================================================================
//  Multi-register store then load (different registers)
// ===================================================================

#[test]
fn str_ldr_x_multiple_regs() {
    let (mut c, mut m) = cpu_exec(&[
        str_x(0, 10, 0),
        str_x(1, 10, 1),
        str_x(2, 10, 2),
        ldr_x(0, 10, 3),
        ldr_x(1, 10, 4),
        ldr_x(2, 10, 5),
    ]);
    c.set_xn(0, 100);
    c.set_xn(1, 200);
    c.set_xn(2, 300);
    c.set_xn(10, D);
    for _ in 0..6 {
        c.step(&mut m).unwrap();
    }
    assert_eq!(c.xn(3), 100);
    assert_eq!(c.xn(4), 200);
    assert_eq!(c.xn(5), 300);
}

#[test]
fn stp_ldp_chain_4_regs() {
    let (mut c, mut m) = cpu_exec(&[
        stp_x(0, 1, 10, 0),
        stp_x(2, 3, 10, 2),
        ldp_x(0, 5, 10, 4),
        ldp_x(2, 7, 10, 6),
    ]);
    c.set_xn(0, 10);
    c.set_xn(1, 20);
    c.set_xn(2, 30);
    c.set_xn(3, 40);
    c.set_xn(10, D);
    for _ in 0..4 {
        c.step(&mut m).unwrap();
    }
    assert_eq!(c.xn(4), 10);
    assert_eq!(c.xn(5), 20);
    assert_eq!(c.xn(6), 30);
    assert_eq!(c.xn(7), 40);
}

// ===================================================================
//  STR/LDR X at many more offsets
// ===================================================================
test_str_ldr!(
    str_ldr_x_off3,
    str_x,
    ldr_x,
    0x1234_5678u64,
    0x1234_5678u64,
    3
);
test_str_ldr!(
    str_ldr_x_off4,
    str_x,
    ldr_x,
    0x1234_5678u64,
    0x1234_5678u64,
    4
);
test_str_ldr!(
    str_ldr_x_off5,
    str_x,
    ldr_x,
    0x1234_5678u64,
    0x1234_5678u64,
    5
);
test_str_ldr!(
    str_ldr_x_off8,
    str_x,
    ldr_x,
    0x1234_5678u64,
    0x1234_5678u64,
    8
);
test_str_ldr!(
    str_ldr_x_off16,
    str_x,
    ldr_x,
    0x1234_5678u64,
    0x1234_5678u64,
    16
);
test_str_ldr!(
    str_ldr_x_off32,
    str_x,
    ldr_x,
    0x1234_5678u64,
    0x1234_5678u64,
    32
);
test_str_ldr!(
    str_ldr_x_off64,
    str_x,
    ldr_x,
    0x1234_5678u64,
    0x1234_5678u64,
    64
);
test_str_ldr!(
    str_ldr_x_off128,
    str_x,
    ldr_x,
    0x1234_5678u64,
    0x1234_5678u64,
    128
);
test_str_ldr!(
    str_ldr_x_off255,
    str_x,
    ldr_x,
    0x1234_5678u64,
    0x1234_5678u64,
    255
);

// ===================================================================
//  STR/LDR W at many more offsets
// ===================================================================
test_str_ldr!(str_ldr_w_off2, str_w, ldr_w, 0xABCDu64, 0xABCDu64, 2);
test_str_ldr!(str_ldr_w_off3, str_w, ldr_w, 0xABCDu64, 0xABCDu64, 3);
test_str_ldr!(str_ldr_w_off4, str_w, ldr_w, 0xABCDu64, 0xABCDu64, 4);
test_str_ldr!(str_ldr_w_off8, str_w, ldr_w, 0xABCDu64, 0xABCDu64, 8);
test_str_ldr!(str_ldr_w_off16, str_w, ldr_w, 0xABCDu64, 0xABCDu64, 16);
test_str_ldr!(str_ldr_w_off100b, str_w, ldr_w, 0xABCDu64, 0xABCDu64, 100);
test_str_ldr!(str_ldr_w_off255, str_w, ldr_w, 0xABCDu64, 0xABCDu64, 255);

// ===================================================================
//  STRH/LDRH at many offsets
// ===================================================================
test_str_ldr!(strh_ldrh_off2, strh, ldrh, 0xBEEFu64, 0xBEEFu64, 2);
test_str_ldr!(strh_ldrh_off4, strh, ldrh, 0xBEEFu64, 0xBEEFu64, 4);
test_str_ldr!(strh_ldrh_off8, strh, ldrh, 0xBEEFu64, 0xBEEFu64, 8);
test_str_ldr!(strh_ldrh_off16, strh, ldrh, 0xBEEFu64, 0xBEEFu64, 16);
test_str_ldr!(strh_ldrh_off100b, strh, ldrh, 0xBEEFu64, 0xBEEFu64, 100);

// ===================================================================
//  STRB/LDRB at many offsets
// ===================================================================
test_str_ldr!(strb_ldrb_off2, strb, ldrb, 0x42u64, 0x42u64, 2);
test_str_ldr!(strb_ldrb_off3, strb, ldrb, 0x42u64, 0x42u64, 3);
test_str_ldr!(strb_ldrb_off4, strb, ldrb, 0x42u64, 0x42u64, 4);
test_str_ldr!(strb_ldrb_off8, strb, ldrb, 0x42u64, 0x42u64, 8);
test_str_ldr!(strb_ldrb_off16, strb, ldrb, 0x42u64, 0x42u64, 16);
test_str_ldr!(strb_ldrb_off32, strb, ldrb, 0x42u64, 0x42u64, 32);
test_str_ldr!(strb_ldrb_off64, strb, ldrb, 0x42u64, 0x42u64, 64);
test_str_ldr!(strb_ldrb_off128, strb, ldrb, 0x42u64, 0x42u64, 128);
test_str_ldr!(strb_ldrb_off255b, strb, ldrb, 0x42u64, 0x42u64, 255);

// ===================================================================
//  More sign-extending load variants
// ===================================================================
test_ldrs!(ldrsw_1, ldrsw, wr32, 1u32, 1u64);
test_ldrs!(ldrsw_max_pos, ldrsw, wr32, 0x7FFF_FFFFu32, 0x7FFF_FFFFu64);
test_ldrs!(ldrsw_127, ldrsw, wr32, 127u32, 127u64);
test_ldrs!(ldrsw_128, ldrsw, wr32, 128u32, 128u64);
test_ldrs!(ldrsb_w_127, ldrsb_w, wr8, 127u8, 127u64);
test_ldrs!(ldrsb_w_128, ldrsb_w, wr8, 128u8, 0xFFFF_FF80u64);
test_ldrs!(ldrsb_x_127, ldrsb_x, wr8, 127u8, 127u64);
test_ldrs!(ldrsb_x_128, ldrsb_x, wr8, 128u8, 0xFFFF_FFFF_FFFF_FF80u64);
test_ldrs!(ldrsh_w_32767, ldrsh_w, wr16, 0x7FFFu16, 0x7FFFu64);
test_ldrs!(ldrsh_w_32768, ldrsh_w, wr16, 0x8000u16, 0xFFFF_8000u64);
test_ldrs!(ldrsh_x_32767, ldrsh_x, wr16, 0x7FFFu16, 0x7FFFu64);
test_ldrs!(
    ldrsh_x_32768,
    ldrsh_x,
    wr16,
    0x8000u16,
    0xFFFF_FFFF_FFFF_8000u64
);

// ===================================================================
//  STP/LDP with more offset values
// ===================================================================
test_stp_ldp!(stp_ldp_x_off1, stp_x, ldp_x, 1, 0x100u64, 0x200u64);
test_stp_ldp!(stp_ldp_x_off3, stp_x, ldp_x, 3, 0x300u64, 0x400u64);
test_stp_ldp!(stp_ldp_x_off4, stp_x, ldp_x, 4, 0x500u64, 0x600u64);
test_stp_ldp!(stp_ldp_x_off5, stp_x, ldp_x, 5, 0x700u64, 0x800u64);
test_stp_ldp!(stp_ldp_x_neg1, stp_x, ldp_x, -1, 0x900u64, 0xA00u64);
test_stp_ldp!(stp_ldp_x_neg3, stp_x, ldp_x, -3, 0xB00u64, 0xC00u64);
test_stp_ldp!(stp_ldp_x_neg4, stp_x, ldp_x, -4, 0xD00u64, 0xE00u64);
test_stp_ldp!(stp_ldp_w_off1, stp_w, ldp_w, 1, 0x100u64, 0x200u64);
test_stp_ldp!(stp_ldp_w_off2, stp_w, ldp_w, 2, 0x300u64, 0x400u64);
test_stp_ldp!(stp_ldp_w_neg1, stp_w, ldp_w, -1, 0x500u64, 0x600u64);
test_stp_ldp!(stp_ldp_w_neg2, stp_w, ldp_w, -2, 0x700u64, 0x800u64);

// ===================================================================
//  Store then read different sizes from same address
// ===================================================================

#[test]
fn str_x_read_w_h_b() {
    let (mut c, mut m) = cpu_exec(&[str_x(0, 3, 0), ldr_w(0, 3, 1), ldrh(0, 3, 2), ldrb(0, 3, 4)]);
    c.set_xn(0, 0x0102_0304_0506_0708);
    c.set_xn(3, D);
    for _ in 0..4 {
        c.step(&mut m).unwrap();
    }
    assert_eq!(c.xn(1), 0x0506_0708, "LDR W from low 32");
    assert_eq!(c.xn(2), 0x0708, "LDRH from low 16");
    assert_eq!(c.xn(4), 0x08, "LDRB from low 8");
}
