use crate::vcpu::{CoreRegs, SysRegs};

#[test]
fn core_regs_default_is_zero() {
    let regs = CoreRegs::default();
    assert_eq!(regs.pc, 0);
    assert_eq!(regs.sp, 0);
    assert_eq!(regs.pstate, 0);
    for x in &regs.xn {
        assert_eq!(*x, 0);
    }
}

#[test]
fn core_regs_clone() {
    let mut regs = CoreRegs::default();
    regs.pc = 0x4000_0000;
    regs.sp = 0x8000_0000;
    regs.xn[0] = 42;
    let cloned = regs.clone();
    assert_eq!(cloned.pc, 0x4000_0000);
    assert_eq!(cloned.sp, 0x8000_0000);
    assert_eq!(cloned.xn[0], 42);
}

#[test]
fn sys_regs_default_is_zero() {
    let regs = SysRegs::default();
    assert_eq!(regs.sctlr_el1, 0);
    assert_eq!(regs.tcr_el1, 0);
    assert_eq!(regs.ttbr0_el1, 0);
    assert_eq!(regs.ttbr1_el1, 0);
    assert_eq!(regs.mair_el1, 0);
    assert_eq!(regs.vbar_el1, 0);
    assert_eq!(regs.elr_el1, 0);
    assert_eq!(regs.spsr_el1, 0);
    assert_eq!(regs.esr_el1, 0);
    assert_eq!(regs.far_el1, 0);
}
