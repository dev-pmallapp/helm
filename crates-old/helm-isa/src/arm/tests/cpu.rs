use crate::arm::aarch64::exec::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;

fn make_cpu() -> Aarch64Cpu {
    Aarch64Cpu::new()
}

#[test]
fn new_cpu_regs_are_zeroed() {
    let cpu = make_cpu();
    for i in 0..31u16 {
        assert_eq!(cpu.xn(i), 0, "X{i} should be 0");
    }
    assert_eq!(cpu.current_sp(), 0);
}

#[test]
fn xn_out_of_range_returns_zero() {
    let cpu = make_cpu();
    assert_eq!(cpu.xn(31), 0);
    assert_eq!(cpu.xn(100), 0);
}

#[test]
fn set_xn_round_trip() {
    let mut cpu = make_cpu();
    cpu.set_xn(0, 0xDEAD_BEEF);
    assert_eq!(cpu.xn(0), 0xDEAD_BEEF);
    cpu.set_xn(30, 0x1234);
    assert_eq!(cpu.xn(30), 0x1234);
}

#[test]
fn set_xn_31_is_noop() {
    let mut cpu = make_cpu();
    cpu.set_xn(31, 0xFFFF);
    assert_eq!(cpu.xn(31), 0);
}

#[test]
fn xn_sp_31_reads_sp() {
    let mut cpu = make_cpu();
    cpu.set_current_sp(0xABCD_0000);
    assert_eq!(cpu.xn_sp(31), 0xABCD_0000);
}

#[test]
fn xn_sp_reg_reads_gpr() {
    let mut cpu = make_cpu();
    cpu.set_xn(5, 42);
    assert_eq!(cpu.xn_sp(5), 42);
}

#[test]
fn set_xn_sp_31_writes_sp() {
    let mut cpu = make_cpu();
    cpu.set_xn_sp(31, 0x8000);
    assert_eq!(cpu.current_sp(), 0x8000);
}

#[test]
fn set_xn_sp_reg_writes_gpr() {
    let mut cpu = make_cpu();
    cpu.set_xn_sp(10, 99);
    assert_eq!(cpu.xn(10), 99);
}

#[test]
fn wn_truncates_to_32_bits() {
    let mut cpu = make_cpu();
    cpu.set_xn(7, 0x1_FFFF_FFFF);
    assert_eq!(cpu.wn(7), 0xFFFF_FFFF);
}

#[test]
fn set_wn_zero_extends() {
    let mut cpu = make_cpu();
    cpu.set_xn(3, 0xFFFF_FFFF_FFFF_FFFF);
    cpu.set_wn(3, 0x0000_0001);
    assert_eq!(cpu.xn(3), 1);
}

#[test]
fn current_sp_el0_uses_sp_field() {
    let mut cpu = make_cpu();
    cpu.regs.current_el = 0;
    cpu.regs.sp = 0x1000;
    assert_eq!(cpu.current_sp(), 0x1000);
}

#[test]
fn current_sp_el1_spsel1_uses_sp_el1() {
    let mut cpu = make_cpu();
    cpu.regs.current_el = 1;
    cpu.regs.sp_sel = 1;
    cpu.regs.sp_el1 = 0x2000;
    assert_eq!(cpu.current_sp(), 0x2000);
}

#[test]
fn current_sp_el1_spsel0_uses_sp_el0() {
    let mut cpu = make_cpu();
    cpu.regs.current_el = 1;
    cpu.regs.sp_sel = 0;
    cpu.regs.sp = 0x3000;
    assert_eq!(cpu.current_sp(), 0x3000);
}

#[test]
fn current_sp_el2_spsel1_uses_sp_el2() {
    let mut cpu = make_cpu();
    cpu.regs.current_el = 2;
    cpu.regs.sp_sel = 1;
    cpu.regs.sp_el2 = 0x4000;
    assert_eq!(cpu.current_sp(), 0x4000);
}

#[test]
fn current_sp_el3_spsel1_uses_sp_el3() {
    let mut cpu = make_cpu();
    cpu.regs.current_el = 3;
    cpu.regs.sp_sel = 1;
    cpu.regs.sp_el3 = 0x5000;
    assert_eq!(cpu.current_sp(), 0x5000);
}

#[test]
fn set_current_sp_el1_spsel1_writes_sp_el1() {
    let mut cpu = make_cpu();
    cpu.regs.current_el = 1;
    cpu.regs.sp_sel = 1;
    cpu.set_current_sp(0x6000);
    assert_eq!(cpu.regs.sp_el1, 0x6000);
}

#[test]
fn set_se_mode_toggles() {
    let mut cpu = make_cpu();
    cpu.set_se_mode(true);
    cpu.set_se_mode(false);
}

#[test]
fn new_cpu_not_halted() {
    let cpu = make_cpu();
    assert!(!cpu.halted);
    assert_eq!(cpu.exit_code, 0);
}

#[test]
fn new_cpu_insn_count_zero() {
    let cpu = make_cpu();
    assert_eq!(cpu.insn_count, 0);
}

#[test]
fn step_nop_advances_pc() {
    let mut cpu = make_cpu();
    cpu.set_se_mode(true);
    let mut mem = AddressSpace::new();
    mem.map(0x1000, 0x1000, (true, true, true));
    let nop = 0xD503201Fu32.to_le_bytes();
    mem.write(0x1000, &nop).unwrap();
    cpu.regs.pc = 0x1000;
    let trace = cpu.step(&mut mem).unwrap();
    assert_eq!(trace.pc, 0x1000);
    assert_eq!(cpu.regs.pc, 0x1004);
    assert_eq!(cpu.insn_count, 1);
}

#[test]
fn step_add_imm_computes_correctly() {
    let mut cpu = make_cpu();
    cpu.set_se_mode(true);
    let mut mem = AddressSpace::new();
    mem.map(0x1000, 0x1000, (true, true, true));
    // ADD X1, X0, #5  →  0x91001401
    let add_insn = 0x91001401u32.to_le_bytes();
    mem.write(0x1000, &add_insn).unwrap();
    cpu.set_xn(0, 10);
    cpu.regs.pc = 0x1000;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(1), 15);
}
