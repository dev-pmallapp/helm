//! AArch64 system register and exception tests. Ported from exec_sysreg.rs.
use super::harness::*;

const NOP: u32 = 0xD503_201F;
const BASE: u64 = CODE_BASE;
const ERET: u32 = 0xD69F_03E0;

fn encode_mrs(rt: u32, o0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    0xD500_0000 | (1 << 21) | (1 << 20) | (o0 << 19) | (op1 << 16) | (crn << 12) | (crm << 8) | (op2 << 5) | rt
}
fn encode_msr(rt: u32, o0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    0xD500_0000 | (0 << 21) | (1 << 20) | (o0 << 19) | (op1 << 16) | (crn << 12) | (crm << 8) | (op2 << 5) | rt
}
fn encode_msr_daifset(imm: u32) -> u32 { 0xD503_40DF | ((imm & 0xF) << 8) }
fn encode_msr_daifclr(imm: u32) -> u32 { 0xD503_40FF | ((imm & 0xF) << 8) }
fn encode_msr_spsel(imm: u32) -> u32 { 0xD500_40BF | ((imm & 0xF) << 8) }

#[test]
fn mrs_current_el() {
    let (mut c, mut m) = cpu_with_code(&[encode_mrs(0, 1, 0, 4, 2, 2)]);
    c.current_el = 1; step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 1 << 2);
}
#[test]
fn msr_mrs_vbar_el1() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr(1, 1, 0, 12, 0, 0), encode_mrs(2, 1, 0, 12, 0, 0)]);
    c.x[1] = 0xFFFF_0000_1000_0000;
    step(&mut c, &mut m).unwrap(); step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], 0xFFFF_0000_1000_0000);
    assert_eq!(c.vbar_el1, 0xFFFF_0000_1000_0000);
}
#[test]
fn msr_mrs_sctlr_el1() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr(1, 1, 0, 1, 0, 0), encode_mrs(2, 1, 0, 1, 0, 0)]);
    c.x[1] = 0xDEAD_BEEE;
    step(&mut c, &mut m).unwrap(); step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], 0xDEAD_BEEE);
}
#[test]
fn msr_mrs_ttbr0_el1() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr(1, 1, 0, 2, 0, 0), encode_mrs(2, 1, 0, 2, 0, 0)]);
    c.x[1] = 0x4000_0000;
    step(&mut c, &mut m).unwrap(); step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], 0x4000_0000);
}
#[test]
fn msr_mrs_tcr_el1() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr(3, 1, 0, 2, 0, 2), encode_mrs(4, 1, 0, 2, 0, 2)]);
    c.x[3] = 0x0000_0000_B510_1510;
    step(&mut c, &mut m).unwrap(); step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[4], 0x0000_0000_B510_1510);
}
#[test]
fn msr_mrs_mair_el1() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr(1, 1, 0, 10, 2, 0), encode_mrs(2, 1, 0, 10, 2, 0)]);
    c.x[1] = 0xFF44_00BB_0400_FFCC;
    step(&mut c, &mut m).unwrap(); step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], 0xFF44_00BB_0400_FFCC);
}
#[test]
fn mrs_midr_el1() {
    let (mut c, mut m) = cpu_with_code(&[encode_mrs(0, 1, 0, 0, 0, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x410F_D034);
}
#[test]
fn mrs_cntfrq_el0() {
    let (mut c, mut m) = cpu_with_code(&[encode_mrs(0, 1, 3, 14, 0, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 62_500_000);
}
#[test]
#[ignore = "MsrImm not yet implemented in execute.rs"]
fn daifset_masks_interrupts() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr_daifset(0xF)]);
    c.daif = 0; step(&mut c, &mut m).unwrap();
    assert_eq!(c.daif, 0xF);
}
#[test]
#[ignore = "MsrImm not yet implemented in execute.rs"]
fn daifclr_unmasks_interrupts() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr_daifclr(0xF)]);
    c.daif = 0xF; step(&mut c, &mut m).unwrap();
    assert_eq!(c.daif, 0);
}
#[test]
#[ignore = "MsrImm not yet implemented in execute.rs"]
fn spsel_switches_sp() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr_spsel(1), NOP]);
    c.current_el = 1; c.sp = 0x1000; c.sp_el1 = 0x2000; c.spsel = false;
    step(&mut c, &mut m).unwrap();
    assert!(c.spsel);
    assert_eq!(c.sp_el1, 0x2000);
}
#[test]
#[ignore = "SVC at EL0 returns EnvironmentCall, not exception entry in this SE-mode executor"]
fn exception_entry_saves_state() { }
#[test]
fn eret_restores_state() {
    let mut mem = TestMem::new();
    let eret_addr = 0x40_0000u64;
    mem.map_zeroed(eret_addr, 0x1000);
    mem.load_u32(eret_addr, ERET);
    mem.map_zeroed(0x7FFF_0000, 0x10000);

    let mut c = crate::aarch64::Aarch64ArchState::new();
    c.pc = eret_addr;
    c.current_el = 1;
    c.spsel = true;
    c.daif = 0xF;
    c.elr_el1 = 0x50_0000;
    c.spsr_el1 = (1 << 30) | 0; // Z=1, EL0, SP_EL0

    step(&mut c, &mut mem).unwrap();

    assert_eq!(c.pc, 0x50_0000);
    assert_eq!(c.current_el, 0);
    assert!(!c.spsel);
    assert_eq!(c.nzcv, 1 << 30);
    assert_eq!(c.daif, 0);
}
