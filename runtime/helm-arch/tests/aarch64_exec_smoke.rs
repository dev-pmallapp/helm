//! AArch64 executor smoke tests — basic instruction coverage.
//!
//! Ported from the reference helm.git `exec.rs` test suite.
//! Covers: ADD, SUB, CMP, MOVZ, MOVK, ADRP, B, BL, RET, CBZ, STR+LDR,
//! STP+LDP, LDRB, MOV, MUL, LDXR+STXR, SWP.

use helm_arch::aarch64::arch_state::Aarch64ArchState;
use helm_core::{AccessType, MemFault, MemInterface};

// ── Test memory ────────────────────────────────────────────────────────────────

struct TestMem {
    data: Vec<u8>,
}

impl TestMem {
    fn new() -> Self {
        Self {
            data: vec![0u8; 16 * 1024 * 1024],
        } // 16 MB
    }
}

impl MemInterface for TestMem {
    fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
        let off = (addr & 0xFF_FFFF) as usize;
        if off + size > self.data.len() {
            return Err(MemFault::AccessFault { addr });
        }
        let mut buf = [0u8; 8];
        buf[..size].copy_from_slice(&self.data[off..off + size]);
        Ok(u64::from_le_bytes(buf))
    }
    fn write(&mut self, addr: u64, size: usize, val: u64, _ty: AccessType) -> Result<(), MemFault> {
        let off = (addr & 0xFF_FFFF) as usize;
        if off + size > self.data.len() {
            return Err(MemFault::AccessFault { addr });
        }
        self.data[off..off + size].copy_from_slice(&val.to_le_bytes()[..size]);
        Ok(())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

const BASE: u64 = 0x40_0000;

fn setup() -> (Aarch64ArchState, TestMem) {
    let mut a = Aarch64ArchState::new();
    let m = TestMem::new();
    a.pc = BASE;
    a.sp = 0x7F_8000; // within 16MB
    (a, m)
}

/// Execute one instruction at a.pc, advance PC by 4 if not a taken branch.
fn step(a: &mut Aarch64ArchState, mem: &mut TestMem, raw: u32) {
    let insn = helm_arch::aarch64::decode::decode(raw, a.pc).expect("decode");
    let pc_written = helm_arch::aarch64::execute::execute(&insn, a, mem).expect("execute");
    if !pc_written {
        a.pc += 4;
    }
}

fn write_u64(mem: &mut TestMem, addr: u64, val: u64) {
    mem.write(addr, 8, val, AccessType::Store).unwrap();
}

fn read_u64(mem: &mut TestMem, addr: u64) -> u64 {
    mem.read(addr, 8, AccessType::Load).unwrap()
}

// ── ALU immediate ──────────────────────────────────────────────────────────────

#[test]
fn exec_add_imm() {
    // ADD X0, X1, #42
    let (mut a, mut m) = setup();
    a.x[1] = 100;
    step(&mut a, &mut m, 0x91_00A8_20);
    assert_eq!(a.x[0], 142);
}

#[test]
fn exec_sub_imm() {
    // SUB X0, X1, #10
    let (mut a, mut m) = setup();
    a.x[1] = 50;
    step(&mut a, &mut m, 0xD1_0028_20);
    assert_eq!(a.x[0], 40);
}

#[test]
fn exec_cmp_sets_flags() {
    // CMP X1, #0  (SUBS XZR, X1, #0)
    let (mut a, mut m) = setup();
    a.x[1] = 0;
    step(&mut a, &mut m, 0xF100_003F);
    assert!(a.flag_z(), "zero flag set");
}

#[test]
fn exec_movz() {
    // MOVZ X0, #0x1234
    let (mut a, mut m) = setup();
    step(&mut a, &mut m, 0xD282_4680);
    assert_eq!(a.x[0], 0x1234);
}

#[test]
fn exec_movz_movk_chain() {
    // MOVZ X0, #0x5678, LSL #16
    // MOVK X0, #0x1234
    let (mut a, mut m) = setup();
    step(&mut a, &mut m, 0xD2AA_CF00);
    assert_eq!(a.x[0], 0x5678_0000, "MOVZ X0, #0x5678, LSL #16");
    step(&mut a, &mut m, 0xF282_4680);
    assert_eq!(a.x[0], 0x5678_1234, "MOVK X0, #0x1234");
}

#[test]
fn exec_adrp() {
    // ADRP X0, #0x1000 (1 page forward)
    let (mut a, mut m) = setup();
    step(&mut a, &mut m, 0x9000_0020);
    // ADRP: base = PC & ~0xFFF, offset = immhi:immlo << 12
    let expected = (BASE & !0xFFF) + 0x4000;
    assert_eq!(a.x[0], expected);
}

// ── Branches ───────────────────────────────────────────────────────────────────

#[test]
fn exec_b_forward() {
    // B #8 (skip one insn)
    let (mut a, mut m) = setup();
    step(&mut a, &mut m, 0x1400_0002);
    assert_eq!(a.pc, BASE + 8);
}

#[test]
fn exec_bl_saves_lr() {
    // BL #8
    let (mut a, mut m) = setup();
    step(&mut a, &mut m, 0x9400_0002);
    assert_eq!(a.x[30], BASE + 4); // LR = next insn
    assert_eq!(a.pc, BASE + 8);
}

#[test]
fn exec_ret() {
    // RET (BR X30)
    let (mut a, mut m) = setup();
    a.x[30] = 0x50_0000;
    step(&mut a, &mut m, 0xD65F_03C0);
    assert_eq!(a.pc, 0x50_0000);
}

#[test]
fn exec_cbz_taken() {
    // CBZ X0, #8
    let (mut a, mut m) = setup();
    a.x[0] = 0;
    step(&mut a, &mut m, 0xB400_0040);
    assert_eq!(a.pc, BASE + 8);
}

#[test]
fn exec_cbz_not_taken() {
    let (mut a, mut m) = setup();
    a.x[0] = 1;
    step(&mut a, &mut m, 0xB400_0040);
    assert_eq!(a.pc, BASE + 4); // fallthrough
}

// ── Load/Store ─────────────────────────────────────────────────────────────────

#[test]
fn exec_str_ldr_roundtrip() {
    // STR X0, [SP, #0]
    // LDR X1, [SP, #0]
    let (mut a, mut m) = setup();
    a.x[0] = 0xDEAD_BEEF_CAFE;
    step(&mut a, &mut m, 0xF900_03E0);
    step(&mut a, &mut m, 0xF940_03E1);
    assert_eq!(a.x[1], 0xDEAD_BEEF_CAFE);
}

#[test]
fn exec_stp_ldp_pair() {
    // STP X0, X1, [SP, #-16]!
    // LDP X2, X3, [SP], #16
    let (mut a, mut m) = setup();
    a.x[0] = 0xAAAA;
    a.x[1] = 0xBBBB;
    let orig_sp = a.sp;
    step(&mut a, &mut m, 0xA9BF_07E0); // STP pre-index
    assert_eq!(a.sp, orig_sp - 16);
    step(&mut a, &mut m, 0xA8C1_0FE2); // LDP post-index
    assert_eq!(a.x[2], 0xAAAA);
    assert_eq!(a.x[3], 0xBBBB);
    assert_eq!(a.sp, orig_sp);
}

#[test]
fn exec_ldrb_zero_extends() {
    // STRB W0, [SP]  then  LDRB W1, [SP]
    let (mut a, mut m) = setup();
    a.x[0] = 0xFF;
    step(&mut a, &mut m, 0x3900_03E0);
    step(&mut a, &mut m, 0x3940_03E1);
    assert_eq!(a.x[1], 0xFF); // zero-extended, not sign-extended
}

// ── Register ops ───────────────────────────────────────────────────────────────

#[test]
fn exec_mov_reg() {
    // MOV X0, X1  (ORR X0, XZR, X1)
    let (mut a, mut m) = setup();
    a.x[1] = 0x12345;
    step(&mut a, &mut m, 0xAA01_03E0);
    assert_eq!(a.x[0], 0x12345);
}

#[test]
fn exec_mul() {
    // MUL X0, X1, X2  (MADD X0, X1, X2, XZR)
    let (mut a, mut m) = setup();
    a.x[1] = 7;
    a.x[2] = 6;
    step(&mut a, &mut m, 0x9B02_7C20);
    assert_eq!(a.x[0], 42);
}

// ── Atomics ────────────────────────────────────────────────────────────────────

#[test]
fn exec_ldxr_stxr_succeeds() {
    // LDXR X0, [X1]
    // STXR W2, X3, [X1]
    let (mut a, mut m) = setup();
    let data_addr = 0x10_0000u64;
    write_u64(&mut m, data_addr, 42);
    a.x[1] = data_addr;
    a.x[3] = 99;

    step(&mut a, &mut m, 0xC85F_7C20); // LDXR
    assert_eq!(a.x[0], 42);

    step(&mut a, &mut m, 0xC803_7C23); // STXR
    assert_eq!(a.x[2], 0); // success

    assert_eq!(read_u64(&mut m, data_addr), 99);
}

#[test]
fn exec_swp() {
    // SWP X0, X1, [X2]
    let (mut a, mut m) = setup();
    let data_addr = 0x10_0000u64;
    write_u64(&mut m, data_addr, 100);
    a.x[0] = 200;
    a.x[2] = data_addr;

    step(&mut a, &mut m, 0xF820_4041); // SWP X0, X1, [X2]
    assert_eq!(a.x[1], 100); // old value
    assert_eq!(read_u64(&mut m, data_addr), 200); // new value
}
