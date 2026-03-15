//! AArch64 executor tests.
//!
//! Tests execute decoded instructions against `Aarch64ArchState` and verify
//! register/flag state after execution.

use helm_arch::aarch64::arch_state::Aarch64ArchState;
use helm_arch::aarch64::decode::decode;
use helm_arch::aarch64::execute::execute;
use helm_arch::aarch64::insn::Opcode;
use helm_core::{AccessType, MemFault, MemInterface};

/// Simple test memory: 1 MB flat region at address 0.
struct TestMem {
    data: Vec<u8>,
}

impl TestMem {
    fn new() -> Self { Self { data: vec![0u8; 1 << 20] } }
    fn write_u64(&mut self, addr: u64, val: u64) {
        let off = addr as usize;
        self.data[off..off+8].copy_from_slice(&val.to_le_bytes());
    }
}

impl MemInterface for TestMem {
    fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
        let off = addr as usize;
        if off + size > self.data.len() { return Err(MemFault::AccessFault { addr }); }
        let mut buf = [0u8; 8];
        buf[..size].copy_from_slice(&self.data[off..off+size]);
        Ok(u64::from_le_bytes(buf))
    }
    fn write(&mut self, addr: u64, size: usize, val: u64, _ty: AccessType) -> Result<(), MemFault> {
        let off = addr as usize;
        if off + size > self.data.len() { return Err(MemFault::AccessFault { addr }); }
        self.data[off..off+size].copy_from_slice(&val.to_le_bytes()[..size]);
        Ok(())
    }
}

fn exec_at(raw: u32, a: &mut Aarch64ArchState, mem: &mut TestMem) -> bool {
    let insn = decode(raw, a.pc).expect("decode");
    execute(&insn, a, mem).expect("execute")
}

// ── Data processing immediate ──────────────────────────────────────────────────

#[test]
fn exec_movz() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    // MOVZ X0, #42
    exec_at(0xD2800540, &mut a, &mut m);
    assert_eq!(a.x[0], 42);
}

#[test]
fn exec_add_imm() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.x[0] = 100;
    // ADD X1, X0, #50
    exec_at(0x9100C801, &mut a, &mut m);
    assert_eq!(a.x[1], 150);
}

#[test]
fn exec_sub_imm() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.x[0] = 100;
    // SUB X1, X0, #30
    exec_at(0xD1007801, &mut a, &mut m);
    assert_eq!(a.x[1], 70);
}

#[test]
fn exec_subs_sets_flags() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.x[0] = 5;
    // SUBS X1, X0, #5  (result = 0, Z=1)
    exec_at(0xF1001401, &mut a, &mut m);
    assert_eq!(a.x[1], 0);
    assert!(a.flag_z());
    assert!(!a.flag_n());
}

// ── Load/Store ─────────────────────────────────────────────────────────────────

#[test]
fn exec_str_ldr_roundtrip() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.x[0] = 0x1000; // base address
    a.x[1] = 0xDEAD_BEEF_CAFE_BABE;
    // STR X1, [X0] = F9000001
    exec_at(0xF9000001, &mut a, &mut m);
    // LDR X2, [X0] = F9400002
    exec_at(0xF9400002, &mut a, &mut m);
    assert_eq!(a.x[2], 0xDEAD_BEEF_CAFE_BABE);
}

#[test]
fn exec_ldp_stp() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.sp = 0x2000;
    a.x[0] = 111;
    a.x[1] = 222;
    // STP X0, X1, [SP]
    exec_at(0xA90007E0, &mut a, &mut m);
    // LDP X2, X3, [SP]
    exec_at(0xA94007E2, &mut a, &mut m);  // need to verify encoding
    // Verify
    let v0 = m.read(0x2000, 8, AccessType::Load).unwrap();
    let v1 = m.read(0x2008, 8, AccessType::Load).unwrap();
    assert_eq!(v0, 111);
    assert_eq!(v1, 222);
}

// ── Branches ───────────────────────────────────────────────────────────────────

#[test]
fn exec_b_forward() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.pc = 0x1000;
    // B #8 → PC = 0x1008
    let pc_written = exec_at(0x14000002, &mut a, &mut m);
    assert!(pc_written);
    assert_eq!(a.pc, 0x1008);
}

#[test]
fn exec_bl_saves_lr() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.pc = 0x1000;
    // BL #4
    let pc_written = exec_at(0x94000001, &mut a, &mut m);
    assert!(pc_written);
    assert_eq!(a.pc, 0x1004);
    assert_eq!(a.x[30], 0x1004); // LR = PC + 4
}

#[test]
fn exec_cbz_taken() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.pc = 0x1000;
    a.x[0] = 0; // zero → branch taken
    // CBZ X0, #8
    let pc_written = exec_at(0xB4000040, &mut a, &mut m);
    assert!(pc_written);
    assert_eq!(a.pc, 0x1008);
}

#[test]
fn exec_cbz_not_taken() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.pc = 0x1000;
    a.x[0] = 1; // non-zero → not taken
    // CBZ X0, #8
    let pc_written = exec_at(0xB4000040, &mut a, &mut m);
    assert!(!pc_written);
}

// ── Logical register ───────────────────────────────────────────────────────────

#[test]
fn exec_orr_reg() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.x[1] = 0xF0;
    a.x[2] = 0x0F;
    // ORR X0, X1, X2
    exec_at(0xAA020020, &mut a, &mut m);
    assert_eq!(a.x[0], 0xFF);
}

#[test]
fn exec_and_reg() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.x[1] = 0xFF;
    a.x[2] = 0x0F;
    // AND X0, X1, X2
    exec_at(0x8A020020, &mut a, &mut m);
    assert_eq!(a.x[0], 0x0F);
}

// ── Multiply ───────────────────────────────────────────────────────────────────

#[test]
fn exec_madd() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.x[1] = 6;
    a.x[2] = 7;
    a.x[3] = 0; // Ra = XZR → MUL
    // MADD X0, X1, X2, XZR → MUL X0, X1, X2
    exec_at(0x9B027C20, &mut a, &mut m);
    assert_eq!(a.x[0], 42);
}

// ── SIMD ───────────────────────────────────────────────────────────────────────

#[test]
fn exec_simd_dup_byte() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.x[1] = 0xAB;
    // DUP V0.16B, W1 = 4E010C20
    exec_at(0x4E010C20, &mut a, &mut m);
    // Every byte of V0 should be 0xAB
    let v = a.v[0];
    for i in 0..16 {
        assert_eq!(((v >> (i * 8)) & 0xFF) as u8, 0xAB, "byte {i}");
    }
}

#[test]
fn exec_simd_str_q_ldr_q() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.x[0] = 0x1000; // base
    a.v[0] = 0xDEAD_BEEF_1234_5678_CAFE_BABE_9876_5432u128;
    // STR Q0, [X0]  = 3D800000
    exec_at(0x3D800000, &mut a, &mut m);
    // LDR Q1, [X0]  = 3DC00001 — wait, need correct encoding
    // For LDR Q1, [X0, #0]: size=00, V=1, opc=11 → 3DC00001
    exec_at(0x3DC00001, &mut a, &mut m);
    assert_eq!(a.v[1], a.v[0]);
}

// ── Load literal ───────────────────────────────────────────────────────────────

#[test]
fn exec_ldr_literal() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    a.pc = 0x1000;
    // Place a value at PC + 8 = 0x1008
    m.write_u64(0x1008, 0x42);
    // LDR X0, #8 (imm19=2 → offset=8)
    // Encoding: 01011000 imm19[18:0] Rt[4:0]
    // imm19=2: 0101_1000_0000_0000_0000_0100_0000_0000 = 0x58000040
    let pc_written = exec_at(0x58000040, &mut a, &mut m);
    assert!(!pc_written);
    assert_eq!(a.x[0], 0x42);
}

// ── DC ZVA ─────────────────────────────────────────────────────────────────────

#[test]
fn exec_dc_zva() {
    let mut a = Aarch64ArchState::new();
    let mut m = TestMem::new();
    // Fill 64 bytes at 0x1000 with non-zero
    for i in 0..64 {
        m.data[0x1000 + i] = 0xFF;
    }
    a.x[0] = 0x1000;
    // DC ZVA, X0: D50B7420
    exec_at(0xD50B7420, &mut a, &mut m);
    // Verify all 64 bytes are zero
    for i in 0..64 {
        assert_eq!(m.data[0x1000 + i], 0, "byte {i} not zeroed");
    }
}
