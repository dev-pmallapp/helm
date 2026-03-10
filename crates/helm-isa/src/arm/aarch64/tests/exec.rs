//! AArch64 executor tests — verify instruction behaviour.

use crate::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;

fn cpu_with_code(insns: &[u32]) -> (Aarch64Cpu, AddressSpace) {
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();
    let base = 0x40_0000u64;
    let size = (insns.len() * 4 + 0x1000) as u64;
    mem.map(base, size, (true, true, true));
    for (i, insn) in insns.iter().enumerate() {
        let addr = base + (i as u64 * 4);
        mem.write(addr, &insn.to_le_bytes()).unwrap();
    }
    cpu.regs.pc = base;
    // Map a stack
    mem.map(0x7FFF_0000, 0x10000, (true, true, false));
    cpu.regs.sp = 0x7FFF_8000;
    (cpu, mem)
}

// -- ALU immediate -------------------------------------------------------

#[test]
fn exec_add_imm() {
    // ADD X0, X1, #42
    let (mut cpu, mut mem) = cpu_with_code(&[0x91_00A8_20]);
    cpu.set_xn(1, 100);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 142);
}

#[test]
fn exec_sub_imm() {
    // SUB X0, X1, #10
    let (mut cpu, mut mem) = cpu_with_code(&[0xD1_0028_20]);
    cpu.set_xn(1, 50);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 40);
}

#[test]
fn exec_cmp_sets_flags() {
    // CMP X1, #0  (SUBS XZR, X1, #0)
    let (mut cpu, mut mem) = cpu_with_code(&[0xF100_003F]);
    cpu.set_xn(1, 0);
    cpu.step(&mut mem).unwrap();
    assert!(cpu.regs.z()); // zero flag set
}

#[test]
fn exec_movz() {
    // MOVZ X0, #0x1234
    let (mut cpu, mut mem) = cpu_with_code(&[0xD282_4680]);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x1234);
}

#[test]
fn exec_movz_movk_chain() {
    // MOVZ X0, #0x5678, LSL #16
    // MOVK X0, #0x1234
    let (mut cpu, mut mem) = cpu_with_code(&[0xD2AA_CF00, 0xF282_4680]);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x5678_0000, "MOVZ X0, #0x5678, LSL #16");
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x5678_1234, "MOVK X0, #0x1234");
}

#[test]
fn exec_adrp() {
    // ADRP X0, #0x1000 (1 page forward)
    let (mut cpu, mut mem) = cpu_with_code(&[0x9000_0020]);
    cpu.step(&mut mem).unwrap();
    // ADRP: base = PC & ~0xFFF, offset = immhi:immlo << 12
    // immhi=1, immlo=0 -> imm = 4, offset = 4 << 12 = 0x4000
    let expected = (0x40_0000u64 & !0xFFF) + 0x4000;
    assert_eq!(cpu.xn(0), expected);
}

// -- Branches ------------------------------------------------------------

#[test]
fn exec_b_forward() {
    // B #8 (skip one insn)
    let (mut cpu, mut mem) = cpu_with_code(&[0x1400_0002, 0xD503_201F]);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.pc, 0x40_0000 + 8);
}

#[test]
fn exec_bl_saves_lr() {
    // BL #8
    let (mut cpu, mut mem) = cpu_with_code(&[0x9400_0002, 0xD503_201F]);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(30), 0x40_0000 + 4); // LR = next insn
    assert_eq!(cpu.regs.pc, 0x40_0000 + 8);
}

#[test]
fn exec_ret() {
    // RET (BR X30)
    let (mut cpu, mut mem) = cpu_with_code(&[0xD65F_03C0]);
    cpu.set_xn(30, 0x50_0000);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.pc, 0x50_0000);
}

#[test]
fn exec_cbz_taken() {
    // CBZ X0, #8
    let (mut cpu, mut mem) = cpu_with_code(&[0xB400_0040, 0xD503_201F]);
    cpu.set_xn(0, 0);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.pc, 0x40_0000 + 8);
}

#[test]
fn exec_cbz_not_taken() {
    let (mut cpu, mut mem) = cpu_with_code(&[0xB400_0040, 0xD503_201F]);
    cpu.set_xn(0, 1);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.pc, 0x40_0000 + 4); // fallthrough
}

// -- Load/Store ----------------------------------------------------------

#[test]
fn exec_str_ldr_roundtrip() {
    // STR X0, [SP, #0]
    // LDR X1, [SP, #0]
    let (mut cpu, mut mem) = cpu_with_code(&[0xF900_03E0, 0xF940_03E1]);
    cpu.set_xn(0, 0xDEAD_BEEF_CAFE);
    cpu.step(&mut mem).unwrap();
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(1), 0xDEAD_BEEF_CAFE);
}

#[test]
fn exec_stp_ldp_pair() {
    // STP X0, X1, [SP, #-16]!
    // LDP X2, X3, [SP], #16
    let (mut cpu, mut mem) = cpu_with_code(&[0xA9BF_07E0, 0xA8C1_0FE2]);
    cpu.set_xn(0, 0xAAAA);
    cpu.set_xn(1, 0xBBBB);
    let orig_sp = cpu.regs.sp;
    cpu.step(&mut mem).unwrap(); // STP pre-index
    assert_eq!(cpu.regs.sp, orig_sp - 16);
    cpu.step(&mut mem).unwrap(); // LDP post-index
    assert_eq!(cpu.xn(2), 0xAAAA);
    assert_eq!(cpu.xn(3), 0xBBBB);
    assert_eq!(cpu.regs.sp, orig_sp);
}

#[test]
fn exec_ldrb_zero_extends() {
    // STRB W0, [SP]  then  LDRB W1, [SP]
    let (mut cpu, mut mem) = cpu_with_code(&[0x3900_03E0, 0x3940_03E1]);
    cpu.set_xn(0, 0xFF);
    cpu.step(&mut mem).unwrap();
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(1), 0xFF); // zero-extended, not sign-extended
}

// -- Register ops --------------------------------------------------------

#[test]
fn exec_mov_reg() {
    // MOV X0, X1  (ORR X0, XZR, X1)
    let (mut cpu, mut mem) = cpu_with_code(&[0xAA01_03E0]);
    cpu.set_xn(1, 0x12345);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x12345);
}

#[test]
fn exec_mul() {
    // MUL X0, X1, X2  (MADD X0, X1, X2, XZR)
    let (mut cpu, mut mem) = cpu_with_code(&[0x9B02_7C20]);
    cpu.set_xn(1, 7);
    cpu.set_xn(2, 6);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 42);
}

// -- Atomics -------------------------------------------------------------

#[test]
fn exec_ldxr_stxr_succeeds() {
    // LDXR X0, [X1]
    // STXR W2, X3, [X1]
    let (mut cpu, mut mem) = cpu_with_code(&[0xC85F_7C20, 0xC803_7C23]);
    mem.map(0x10_0000, 0x1000, (true, true, false));
    mem.write(0x10_0000, &42u64.to_le_bytes()).unwrap();
    cpu.set_xn(1, 0x10_0000);
    cpu.set_xn(3, 99);

    cpu.step(&mut mem).unwrap(); // LDXR
    assert_eq!(cpu.xn(0), 42);

    cpu.step(&mut mem).unwrap(); // STXR
    assert_eq!(cpu.xn(2), 0); // success

    let mut buf = [0u8; 8];
    mem.read(0x10_0000, &mut buf).unwrap();
    assert_eq!(u64::from_le_bytes(buf), 99);
}

#[test]
fn exec_swp() {
    // SWP X0, X1, [X2]  (size=11, o3=1)
    // SWP X0, X1, [X2]: size=11 111000 0 0 1 Rs=0 1 000 00 Rn=2 Rt=1
    let (mut cpu, mut mem) = cpu_with_code(&[0xF820_8041]);
    mem.map(0x10_0000, 0x1000, (true, true, false));
    mem.write(0x10_0000, &100u64.to_le_bytes()).unwrap();
    cpu.set_xn(0, 200);
    cpu.set_xn(2, 0x10_0000);

    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(1), 100); // old value
    let mut buf = [0u8; 8];
    mem.read(0x10_0000, &mut buf).unwrap();
    assert_eq!(u64::from_le_bytes(buf), 200); // new value
}
