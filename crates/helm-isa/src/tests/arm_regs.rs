//! TDD Stage 0 — AArch64 register file tests.

use crate::arm::regs::Aarch64Regs;

#[test]
fn x_regs_init_to_zero() {
    let regs = Aarch64Regs::default();
    for i in 0..31 {
        assert_eq!(regs.x[i], 0, "X{i} should init to 0");
    }
}

#[test]
fn sp_independent_of_x31() {
    let mut regs = Aarch64Regs::default();
    regs.sp = 0xFFFF;
    // X30 is the last GP register; SP is separate.
    assert_eq!(regs.x[30], 0);
    assert_eq!(regs.sp, 0xFFFF);
}

#[test]
fn pc_advances_by_four() {
    let mut regs = Aarch64Regs::default();
    regs.pc = 0x1000;
    regs.pc += 4;
    assert_eq!(regs.pc, 0x1004);
}

#[test]
fn nzcv_pack_unpack() {
    let mut regs = Aarch64Regs::default();
    regs.set_nzcv(true, false, true, false);
    assert!(regs.n());
    assert!(!regs.z());
    assert!(regs.c());
    assert!(!regs.v());

    regs.set_nzcv(false, true, false, true);
    assert!(!regs.n());
    assert!(regs.z());
    assert!(!regs.c());
    assert!(regs.v());
}

#[test]
fn nzcv_all_clear() {
    let regs = Aarch64Regs::default();
    assert!(!regs.n());
    assert!(!regs.z());
    assert!(!regs.c());
    assert!(!regs.v());
}

#[test]
fn simd_regs_init_to_zero() {
    let regs = Aarch64Regs::default();
    for i in 0..32 {
        assert_eq!(regs.v[i], 0, "V{i} should init to 0");
    }
}
