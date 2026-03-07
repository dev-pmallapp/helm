use crate::arm::aarch64::sysreg::*;

#[test]
fn sysreg_encoding_sctlr_el1() {
    assert_eq!(SCTLR_EL1, sysreg(3, 0, 1, 0, 0));
}

#[test]
fn sysreg_encoding_ttbr0_el1() {
    assert_eq!(TTBR0_EL1, sysreg(3, 0, 2, 0, 0));
}

#[test]
fn sysreg_encoding_ttbr1_el1() {
    assert_eq!(TTBR1_EL1, sysreg(3, 0, 2, 0, 1));
}

#[test]
fn sysreg_encoding_tcr_el1() {
    assert_eq!(TCR_EL1, sysreg(3, 0, 2, 0, 2));
}

#[test]
fn sysreg_encoding_tpidr_el0() {
    assert_eq!(TPIDR_EL0, sysreg(3, 3, 13, 0, 2));
}

#[test]
fn sysreg_encoding_nzcv() {
    assert_eq!(NZCV, sysreg(3, 3, 4, 2, 0));
}

#[test]
fn sysreg_encoding_spsel() {
    assert_eq!(SPSEL, sysreg(3, 0, 4, 2, 0));
}

#[test]
fn sysreg_encoding_daif() {
    assert_eq!(DAIF, sysreg(3, 3, 4, 2, 1));
}

#[test]
fn sysreg_encoding_current_el() {
    assert_eq!(CURRENT_EL, sysreg(3, 0, 4, 2, 2));
}

#[test]
fn sysreg_all_fields_contribute() {
    let a = sysreg(3, 0, 0, 0, 0);
    let b = sysreg(3, 0, 0, 0, 1);
    let c = sysreg(3, 0, 0, 1, 0);
    let d = sysreg(3, 0, 1, 0, 0);
    let e = sysreg(3, 1, 0, 0, 0);
    assert_ne!(a, b);
    assert_ne!(a, c);
    assert_ne!(a, d);
    assert_ne!(a, e);
}

#[test]
fn sysreg_encoding_vbar_el1() {
    assert_eq!(VBAR_EL1, sysreg(3, 0, 12, 0, 0));
}

#[test]
fn sysreg_encoding_esr_el1() {
    assert_eq!(ESR_EL1, sysreg(3, 0, 5, 2, 0));
}

#[test]
fn sysreg_encoding_far_el1() {
    assert_eq!(FAR_EL1, sysreg(3, 0, 6, 0, 0));
}
