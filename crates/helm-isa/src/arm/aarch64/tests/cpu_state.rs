use crate::arm::aarch64::cpu_state::Aarch64CpuState;
use crate::arm::aarch64::sysreg;
use helm_core::cpu::CpuState;

#[test]
fn gpr_round_trip() {
    let mut cpu = Aarch64CpuState::new();
    for i in 0..31u16 {
        cpu.set_gpr(i, (i as u64) * 1000 + 42);
        assert_eq!(cpu.gpr(i), (i as u64) * 1000 + 42, "X{i} mismatch");
    }
}

#[test]
fn sp_via_gpr_31() {
    let mut cpu = Aarch64CpuState::new();
    cpu.set_gpr(31, 0xDEAD_BEEF);
    assert_eq!(cpu.gpr(31), 0xDEAD_BEEF);
    assert_eq!(cpu.regs.sp, 0xDEAD_BEEF);
}

#[test]
fn pc_round_trip() {
    let mut cpu = Aarch64CpuState::new();
    cpu.set_pc(0xFFFF_0000_0000_1000);
    assert_eq!(cpu.pc(), 0xFFFF_0000_0000_1000);
}

#[test]
fn sysreg_sctlr_el1() {
    let mut cpu = Aarch64CpuState::new();
    cpu.set_sysreg(sysreg::SCTLR_EL1, 0x1234_5678);
    assert_eq!(cpu.sysreg(sysreg::SCTLR_EL1), 0x1234_5678);
    assert_eq!(cpu.regs.sctlr_el1, 0x1234_5678);
}

#[test]
fn sysreg_tpidr_el0() {
    let mut cpu = Aarch64CpuState::new();
    cpu.set_sysreg(sysreg::TPIDR_EL0, 0xCAFE);
    assert_eq!(cpu.sysreg(sysreg::TPIDR_EL0), 0xCAFE);
}

#[test]
fn sysreg_unknown_returns_zero() {
    let cpu = Aarch64CpuState::new();
    assert_eq!(cpu.sysreg(0xFFFF), 0);
}

#[test]
fn flags_nzcv_round_trip() {
    let mut cpu = Aarch64CpuState::new();
    // Set N=1, Z=0, C=1, V=0 → bits 31,29 set
    cpu.regs.nzcv = (1 << 31) | (1 << 29);
    let flags = cpu.flags();
    assert_eq!(flags & 0xF000_0000, (1u64 << 31) | (1u64 << 29));
}

#[test]
fn flags_current_el_round_trip() {
    let mut cpu = Aarch64CpuState::new();
    cpu.regs.current_el = 2;
    assert_eq!(cpu.privilege_level(), 2);
    let flags = cpu.flags();
    assert_eq!((flags >> 2) & 3, 2);
}

#[test]
fn set_flags_round_trip() {
    let mut cpu = Aarch64CpuState::new();
    // N=1, Z=1, C=0, V=0, DAIF=0xF, EL=1, SPSel=1
    let flags = (0b1100u64 << 28) | (0xF << 6) | (1 << 2) | 1;
    cpu.set_flags(flags);
    assert!(cpu.regs.nzcv & (1 << 31) != 0, "N should be set");
    assert!(cpu.regs.nzcv & (1 << 30) != 0, "Z should be set");
    assert_eq!(cpu.regs.daif, 0xF);
    assert_eq!(cpu.regs.current_el, 1);
    assert_eq!(cpu.regs.sp_sel, 1);
}

#[test]
fn wide_reg_v0_round_trip() {
    let mut cpu = Aarch64CpuState::new();
    let data: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    // V0 is reg id 32
    cpu.set_gpr_wide(32, &data);
    let mut out = [0u8; 16];
    let n = cpu.gpr_wide(32, &mut out);
    assert_eq!(n, 16);
    assert_eq!(out, data);
}

#[test]
fn wide_reg_invalid_returns_zero() {
    let cpu = Aarch64CpuState::new();
    let mut out = [0u8; 16];
    assert_eq!(cpu.gpr_wide(0, &mut out), 0); // 0 is not a vreg
}

#[test]
fn default_id_registers() {
    let cpu = Aarch64CpuState::new();
    assert_eq!(cpu.sysreg(sysreg::MIDR_EL1), 0x410F_D034); // Cortex-A53
    assert_eq!(cpu.sysreg(sysreg::CNTFRQ_EL0), 62_500_000);
}
