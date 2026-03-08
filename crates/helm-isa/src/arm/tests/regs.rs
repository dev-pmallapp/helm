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

#[test]
fn nzcv_set_all_flags() {
    let mut regs = Aarch64Regs::default();
    regs.set_nzcv(true, true, true, true);
    assert!(regs.n());
    assert!(regs.z());
    assert!(regs.c());
    assert!(regs.v());
}

#[test]
fn nzcv_clear_all_flags() {
    let mut regs = Aarch64Regs::default();
    regs.set_nzcv(true, true, true, true);
    regs.set_nzcv(false, false, false, false);
    assert!(!regs.n());
    assert!(!regs.z());
    assert!(!regs.c());
    assert!(!regs.v());
}

#[test]
fn nzcv_raw_bits_only_upper_nibble() {
    let mut regs = Aarch64Regs::default();
    regs.set_nzcv(true, false, false, false); // N only
                                              // Bit 31 should be set, 30/29/28 clear
    assert_eq!(regs.nzcv & 0xF000_0000, 0x8000_0000);
}

#[test]
fn tpidr_el0_default_zero() {
    let regs = Aarch64Regs::default();
    assert_eq!(regs.tpidr_el0, 0);
}

#[test]
fn fpcr_fpsr_default_zero() {
    let regs = Aarch64Regs::default();
    assert_eq!(regs.fpcr, 0);
    assert_eq!(regs.fpsr, 0);
}

#[test]
fn aarch64_regs_clone() {
    let mut regs = Aarch64Regs::default();
    regs.x[0] = 0xCAFE;
    regs.pc = 0x1000;
    let cloned = regs.clone();
    assert_eq!(cloned.x[0], 0xCAFE);
    assert_eq!(cloned.pc, 0x1000);
}
