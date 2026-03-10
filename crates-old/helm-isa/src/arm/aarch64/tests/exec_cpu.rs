//! Direct tests for `Aarch64Cpu` accessor methods and step() semantics.
//! These cover paths not exercised by instruction-level tests.

use crate::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;

fn mem_with_code(insns: &[u32]) -> (Aarch64Cpu, AddressSpace) {
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();
    let base = 0x40_0000u64;
    mem.map(base, (insns.len() as u64 * 4 + 0x1000), (true, true, true));
    for (i, insn) in insns.iter().enumerate() {
        mem.write(base + i as u64 * 4, &insn.to_le_bytes()).unwrap();
    }
    cpu.regs.pc = base;
    mem.map(0x7FFF_0000, 0x10000, (true, true, false));
    cpu.regs.sp = 0x7FFF_8000;
    (cpu, mem)
}

// NOP encoding
const NOP: u32 = 0xD503_201F;

#[test]
fn cpu_new_all_gp_regs_zero() {
    let cpu = Aarch64Cpu::new();
    for i in 0..31 {
        assert_eq!(cpu.xn(i), 0, "X{i} should start at 0");
    }
}

#[test]
fn cpu_new_not_halted() {
    let cpu = Aarch64Cpu::new();
    assert!(!cpu.halted);
}

#[test]
fn cpu_new_exit_code_zero() {
    let cpu = Aarch64Cpu::new();
    assert_eq!(cpu.exit_code, 0);
}

#[test]
fn xn_reg31_reads_as_zero() {
    let cpu = Aarch64Cpu::new();
    assert_eq!(cpu.xn(31), 0, "XZR must always read 0");
}

#[test]
fn set_xn_reg31_is_ignored() {
    let mut cpu = Aarch64Cpu::new();
    cpu.set_xn(31, 0xDEAD_BEEF);
    assert_eq!(cpu.xn(31), 0, "write to XZR should be discarded");
}

#[test]
fn xn_sp_reg31_returns_sp() {
    let mut cpu = Aarch64Cpu::new();
    cpu.regs.sp = 0x7FFF_8000;
    assert_eq!(cpu.xn_sp(31), 0x7FFF_8000);
}

#[test]
fn xn_sp_reg30_returns_lr() {
    let mut cpu = Aarch64Cpu::new();
    cpu.set_xn(30, 0x1234_5678);
    assert_eq!(cpu.xn_sp(30), 0x1234_5678);
}

#[test]
fn set_xn_sp_reg31_writes_sp() {
    let mut cpu = Aarch64Cpu::new();
    cpu.set_xn_sp(31, 0xABCD_0000);
    assert_eq!(cpu.regs.sp, 0xABCD_0000);
}

#[test]
fn set_xn_sp_reg30_writes_x30() {
    let mut cpu = Aarch64Cpu::new();
    cpu.set_xn_sp(30, 0x9999);
    assert_eq!(cpu.xn(30), 0x9999);
}

#[test]
fn wn_is_lower_32_bits() {
    let mut cpu = Aarch64Cpu::new();
    cpu.set_xn(5, 0xDEAD_BEEF_1234_5678);
    assert_eq!(cpu.wn(5), 0x1234_5678);
}

#[test]
fn set_wn_zero_extends_to_64_bits() {
    let mut cpu = Aarch64Cpu::new();
    cpu.set_xn(3, 0xFFFF_FFFF_FFFF_FFFF);
    cpu.set_wn(3, 0x1234_ABCD);
    // W register write zero-extends — upper 32 bits must be cleared
    assert_eq!(cpu.xn(3), 0x1234_ABCD);
}

#[test]
fn step_advances_pc_by_4_on_non_branch() {
    let (mut cpu, mut mem) = mem_with_code(&[NOP]);
    let initial_pc = cpu.regs.pc;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.pc, initial_pc + 4);
}

#[test]
fn step_unmapped_memory_returns_error() {
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();
    // PC points to unmapped memory
    cpu.regs.pc = 0xDEAD_0000;
    assert!(cpu.step(&mut mem).is_err());
}

#[test]
fn step_multiple_nops_advance_pc_sequentially() {
    let nops = [NOP; 4];
    let (mut cpu, mut mem) = mem_with_code(&nops);
    let base = cpu.regs.pc;
    for i in 0u64..4 {
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.regs.pc, base + (i + 1) * 4);
    }
}
