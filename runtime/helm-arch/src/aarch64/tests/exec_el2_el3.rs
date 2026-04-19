//! Privileged EL2/EL3 exception-routing tests adapted from ../helm.git.

use super::harness::*;

const BASE: u64 = CODE_BASE;
const ERET: u32 = 0xD69F_03E0;

fn encode_hvc(imm16: u32) -> u32 {
    0xD400_0002 | ((imm16 & 0xFFFF) << 5)
}
fn encode_smc(imm16: u32) -> u32 {
    0xD400_0003 | ((imm16 & 0xFFFF) << 5)
}
fn encode_svc(imm16: u32) -> u32 {
    0xD400_0001 | ((imm16 & 0xFFFF) << 5)
}
fn encode_msr(rt: u32, o0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    0xD500_0000 | (1 << 20) | (o0 << 19) | (op1 << 16) | (crn << 12) | (crm << 8) | (op2 << 5) | rt
}

#[test]
fn hvc_from_el1_takes_exception_to_el2() {
    let (mut c, mut m) = cpu_with_code(&[encode_hvc(0)]);
    c.current_el = 1;
    c.vbar_el2 = 0x8_0000;

    step(&mut c, &mut m).unwrap();

    assert_eq!(c.current_el, 2);
    assert_eq!(c.pc, 0x8_0000 + 0x400);
    assert_eq!(c.elr_el2, BASE + 4);
    assert_eq!(c.esr_el2 >> 26, 0x16);
}

#[test]
fn hvc_undefined_when_hcd_set() {
    let (mut c, mut m) = cpu_with_code(&[encode_hvc(0)]);
    c.current_el = 1;
    c.hcr_el2 = 1 << 29;
    c.vbar_el1 = 0x8_0000;

    step(&mut c, &mut m).unwrap();

    assert_eq!(c.current_el, 1);
    assert_eq!(c.esr_el1 >> 26, 0x00);
    assert_eq!(c.pc, 0x8_0000 + 0x200);
}

#[test]
fn smc_from_el1_takes_exception_to_el3() {
    let (mut c, mut m) = cpu_with_code(&[encode_smc(0)]);
    c.current_el = 1;
    c.vbar_el3 = 0xC_0000;

    step(&mut c, &mut m).unwrap();

    assert_eq!(c.current_el, 3);
    assert_eq!(c.pc, 0xC_0000 + 0x400);
    assert_eq!(c.elr_el3, BASE + 4);
    assert_eq!(c.esr_el3 >> 26, 0x17);
}

#[test]
fn smc_from_el1_traps_to_el2_when_tsc() {
    let (mut c, mut m) = cpu_with_code(&[encode_smc(0)]);
    c.current_el = 1;
    c.hcr_el2 = 1 << 19;
    c.vbar_el2 = 0x8_0000;

    step(&mut c, &mut m).unwrap();

    assert_eq!(c.current_el, 2);
    assert_eq!(c.pc, 0x8_0000 + 0x400);
    assert_eq!(c.esr_el2 >> 26, 0x17);
}

#[test]
fn smc_undefined_when_smd_set() {
    let (mut c, mut m) = cpu_with_code(&[encode_smc(0)]);
    c.current_el = 1;
    c.scr_el3 = 1 << 7;
    c.vbar_el1 = 0x8_0000;

    step(&mut c, &mut m).unwrap();

    assert_eq!(c.current_el, 1);
    assert_eq!(c.esr_el1 >> 26, 0x00);
    assert_eq!(c.pc, 0x8_0000 + 0x200);
}

#[test]
fn eret_from_el3_to_el1() {
    let (mut c, mut m) = cpu_with_code(&[ERET]);
    c.current_el = 3;
    c.elr_el3 = 0x10_0000;
    c.spsr_el3 = (1 << 2) | 1;

    step(&mut c, &mut m).unwrap();

    assert_eq!(c.current_el, 1);
    assert!(c.spsel);
    assert_eq!(c.pc, 0x10_0000);
}

#[test]
fn eret_from_el2_to_el1() {
    let (mut c, mut m) = cpu_with_code(&[ERET]);
    c.current_el = 2;
    c.elr_el2 = 0x20_0000;
    c.spsr_el2 = (1 << 2) | 1;

    step(&mut c, &mut m).unwrap();

    assert_eq!(c.current_el, 1);
    assert_eq!(c.pc, 0x20_0000);
}

#[test]
fn vhe_redirects_sctlr_el1_to_sctlr_el2() {
    let insn = encode_msr(0, 3, 0, 1, 0, 0);
    let (mut c, mut m) = cpu_with_code(&[insn]);
    c.current_el = 2;
    c.hcr_el2 = 1u64 << 34;
    c.x[0] = 0xDEAD_BEEF;

    step(&mut c, &mut m).unwrap();

    assert_ne!(c.sctlr_el1, 0xDEAD_BEEF);
    assert_eq!(c.sctlr_el2, 0xDEAD_BEEF);
}

#[test]
fn tvm_traps_sctlr_el1_write_to_el2() {
    let insn = encode_msr(0, 3, 0, 1, 0, 0);
    let (mut c, mut m) = cpu_with_code(&[insn]);
    c.current_el = 1;
    c.hcr_el2 = 1 << 26;
    c.vbar_el2 = 0x8_0000;
    c.x[0] = 0x1234_5678;

    step(&mut c, &mut m).unwrap();

    assert_eq!(c.current_el, 2);
    assert_eq!(c.esr_el2 >> 26, 0x18);
    assert_eq!(c.sctlr_el1, 0x0000_0800);
}

#[test]
fn svc_el0_routes_to_el2_with_tge() {
    let (mut c, mut m) = cpu_with_code(&[encode_svc(0)]);
    c.current_el = 0;
    c.spsel = false;
    c.hcr_el2 = 1 << 27;
    c.vbar_el2 = 0x8_0000;

    step(&mut c, &mut m).unwrap();

    assert_eq!(c.current_el, 2);
    assert_eq!(c.pc, 0x8_0000 + 0x400);
    assert_eq!(c.esr_el2 >> 26, 0x15);
}

#[test]
fn el3_exception_entry_saves_full_state() {
    let (mut c, mut m) = cpu_with_code(&[encode_smc(0)]);
    c.current_el = 2;
    c.spsel = true;
    c.daif = 0x8;
    c.nzcv = 0x6000_0000;
    c.vbar_el3 = 0xC_0000;

    step(&mut c, &mut m).unwrap();

    assert_eq!(c.current_el, 3);
    let spsr = c.spsr_el3;
    assert_eq!(spsr & 0xF000_0000, 0x6000_0000);
    assert_eq!(spsr & 0x3C0, 0x200);
    assert_eq!((spsr >> 2) & 3, 2);
    assert_eq!(spsr & 1, 1);
    assert_eq!(c.daif, 0xF);
    assert!(c.spsel);
}

#[test]
fn smc_from_el2_routes_to_el3() {
    let (mut c, mut m) = cpu_with_code(&[encode_smc(0)]);
    c.current_el = 2;
    c.spsel = true;
    c.vbar_el3 = 0xC_0000;

    step(&mut c, &mut m).unwrap();

    assert_eq!(c.current_el, 3);
    assert_eq!(c.pc, 0xC_0000 + 0x400);
    assert_eq!(c.esr_el3 >> 26, 0x17);
}

#[test]
fn eret_from_el3_to_el2() {
    let (mut c, mut m) = cpu_with_code(&[ERET]);
    c.current_el = 3;
    c.elr_el3 = 0x30_0000;
    c.spsr_el3 = (2 << 2) | 1;

    step(&mut c, &mut m).unwrap();

    assert_eq!(c.current_el, 2);
    assert!(c.spsel);
    assert_eq!(c.pc, 0x30_0000);
}

fn encode_mrs(rt: u32, o0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    0xD500_0000
        | (1 << 21)
        | (1 << 20)
        | (o0 << 19)
        | (op1 << 16)
        | (crn << 12)
        | (crm << 8)
        | (op2 << 5)
        | rt
}

#[test]
fn vhe_redirects_esr_el1_read_to_esr_el2() {
    // MRS X0, ESR_EL1 (3, 0, 5, 2, 0)
    let insn = encode_mrs(0, 1, 0, 5, 2, 0);
    let (mut c, mut m) = cpu_with_code(&[insn]);
    c.current_el = 2;
    c.hcr_el2 = 1u64 << 34; // E2H
    c.esr_el1 = 0xAAAA_BBBB;
    c.esr_el2 = 0xDEAD_BEEF;

    step(&mut c, &mut m).unwrap();

    // With VHE, ESR_EL1 read should return ESR_EL2 value
    assert_eq!(c.x[0], 0xDEAD_BEEF);
}

#[test]
fn vhe_redirects_cpacr_el1_write_to_cptr_el2() {
    // MSR CPACR_EL1, X0 (3, 0, 1, 0, 2)
    let insn = encode_msr(0, 3, 0, 1, 0, 2);
    let (mut c, mut m) = cpu_with_code(&[insn]);
    c.current_el = 2;
    c.hcr_el2 = 1u64 << 34; // E2H
    c.cpacr_el1 = 0; // clear default
    c.cptr_el2 = 0;
    c.x[0] = 0x0030_0000; // FPEN=3

    step(&mut c, &mut m).unwrap();

    // With VHE, CPACR_EL1 write should go to CPTR_EL2
    assert_eq!(c.cptr_el2, 0x0030_0000);
    // cpacr_el1 should remain untouched
    assert_eq!(c.cpacr_el1, 0);
}
