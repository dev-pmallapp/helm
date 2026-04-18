//! AArch64 system register and exception tests. Ported from exec_sysreg.rs.
use super::harness::*;

const NOP: u32 = 0xD503_201F;
const ERET: u32 = 0xD69F_03E0;

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
fn encode_msr(rt: u32, o0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    0xD500_0000
        | (0 << 21)
        | (1 << 20)
        | (o0 << 19)
        | (op1 << 16)
        | (crn << 12)
        | (crm << 8)
        | (op2 << 5)
        | rt
}
fn encode_msr_daifset(imm: u32) -> u32 {
    0xD503_40DF | ((imm & 0xF) << 8)
}
fn encode_msr_daifclr(imm: u32) -> u32 {
    0xD503_40FF | ((imm & 0xF) << 8)
}
fn encode_msr_spsel(imm: u32) -> u32 {
    0xD500_40BF | ((imm & 0xF) << 8)
}
fn encode_sys(rt: u32, op0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    0xD508_0000 | (op0 << 19) | (op1 << 16) | (crn << 12) | (crm << 8) | (op2 << 5) | rt
}

#[test]
fn mrs_current_el() {
    let (mut c, mut m) = cpu_with_code(&[encode_mrs(0, 1, 0, 4, 2, 2)]);
    c.current_el = 1;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 1 << 2);
}
#[test]
fn msr_mrs_vbar_el1() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 1, 0, 12, 0, 0), encode_mrs(2, 1, 0, 12, 0, 0)]);
    c.x[1] = 0xFFFF_0000_1000_0000;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], 0xFFFF_0000_1000_0000);
    assert_eq!(c.vbar_el1, 0xFFFF_0000_1000_0000);
}
#[test]
fn msr_mrs_sctlr_el1() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 1, 0, 1, 0, 0), encode_mrs(2, 1, 0, 1, 0, 0)]);
    c.x[1] = 0xDEAD_BEEE;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], 0xDEAD_BEEE);
}
#[test]
fn msr_mrs_ttbr0_el1() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 1, 0, 2, 0, 0), encode_mrs(2, 1, 0, 2, 0, 0)]);
    c.x[1] = 0x4000_0000;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], 0x4000_0000);
}
#[test]
fn msr_mrs_tcr_el1() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(3, 1, 0, 2, 0, 2), encode_mrs(4, 1, 0, 2, 0, 2)]);
    c.x[3] = 0x0000_0000_B510_1510;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[4], 0x0000_0000_B510_1510);
}

#[test]
fn msr_mrs_vttbr_el2() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 3, 4, 2, 1, 0), encode_mrs(2, 3, 4, 2, 1, 0)]);
    c.current_el = 2;
    c.x[1] = 0x1234_5000;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.vttbr_el2, 0x1234_5000);
    assert_eq!(c.x[2], 0x1234_5000);
}

#[test]
fn msr_mrs_vtcr_el2() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 3, 4, 2, 1, 2), encode_mrs(2, 3, 4, 2, 1, 2)]);
    c.current_el = 2;
    c.x[1] = 0x0000_0000_0020_0040;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.vtcr_el2, 0x0000_0000_0020_0040);
    assert_eq!(c.x[2], 0x0000_0000_0020_0040);
}

#[test]
fn mrs_hpfar_el2() {
    let (mut c, mut m) = cpu_with_code(&[encode_mrs(0, 3, 4, 6, 0, 4)]);
    c.current_el = 2;
    c.hpfar_el2 = 0x1234_0000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x1234_0000);
}

#[test]
fn msr_mrs_mdcr_el2() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 3, 4, 1, 1, 1), encode_mrs(2, 3, 4, 1, 1, 1)]);
    c.current_el = 2;
    c.x[1] = 0x84E60;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.mdcr_el2, 0x84E60);
    assert_eq!(c.x[2], 0x84E60);
}

#[test]
fn msr_mrs_cptr_el2() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 3, 4, 1, 1, 2), encode_mrs(2, 3, 4, 1, 1, 2)]);
    c.current_el = 2;
    c.x[1] = 0x1033FF;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.cptr_el2, 0x1033FF);
    assert_eq!(c.x[2], 0x1033FF);
}

#[test]
fn msr_mrs_hstr_el2() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 3, 4, 1, 1, 3), encode_mrs(2, 3, 4, 1, 1, 3)]);
    c.current_el = 2;
    c.x[1] = 0x9F6F;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.hstr_el2, 0x9F6F);
    assert_eq!(c.x[2], 0x9F6F);
}
#[test]
fn msr_mrs_mair_el1() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 1, 0, 10, 2, 0), encode_mrs(2, 1, 0, 10, 2, 0)]);
    c.x[1] = 0xFF44_00BB_0400_FFCC;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], 0xFF44_00BB_0400_FFCC);
}
#[test]
fn mrs_midr_el1() {
    let (mut c, mut m) = cpu_with_code(&[encode_mrs(0, 1, 0, 0, 0, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x4810_D050); // non-ARM implementer 0x48, A55 part number
}
#[test]
fn mrs_cntfrq_el0() {
    let (mut c, mut m) = cpu_with_code(&[encode_mrs(0, 1, 3, 14, 0, 0)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 62_500_000);
}

#[test]
fn msr_mrs_tpidrro_el0() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 3, 3, 13, 0, 3), encode_mrs(2, 3, 3, 13, 0, 3)]);
    c.current_el = 1;
    c.x[1] = 0xCAFE_1000;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.tpidrro_el0, 0xCAFE_1000);
    assert_eq!(c.x[2], 0xCAFE_1000);
}

#[test]
fn msr_mrs_cnthctl_el2() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 3, 4, 14, 1, 0), encode_mrs(2, 3, 4, 14, 1, 0)]);
    c.current_el = 2;
    c.x[1] = 0x123;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.cnthctl_el2, 0x123);
    assert_eq!(c.x[2], 0x123);
}

#[test]
fn msr_mrs_cnthp_ctl_el2() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 3, 4, 14, 2, 1), encode_mrs(2, 3, 4, 14, 2, 1)]);
    c.current_el = 2;
    c.x[1] = 0x5;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.cnthp_ctl_el2, 0x5);
    assert_eq!(c.x[2], 0x5);
}

#[test]
fn msr_mrs_cnthp_cval_el2() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 3, 4, 14, 2, 2), encode_mrs(2, 3, 4, 14, 2, 2)]);
    c.current_el = 2;
    c.x[1] = 0x4186_04D;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.cnthp_cval_el2, 0x4186_04D);
    assert_eq!(c.x[2], 0x4186_04D);
}

#[test]
fn msr_mrs_cntvoff_el2() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 3, 4, 14, 0, 3), encode_mrs(2, 3, 4, 14, 0, 3)]);
    c.current_el = 2;
    c.x[1] = 0x55AA;
    step(&mut c, &mut m).unwrap();
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.cntvoff_el2, 0x55AA);
    assert_eq!(c.x[2], 0x55AA);
}

#[test]
fn msr_cnthp_tval_el2_sets_cval_relative_to_counter() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr(1, 3, 4, 14, 2, 0)]);
    c.current_el = 2;
    c.cntvct_el0 = 1000;
    c.x[1] = 250;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.cnthp_cval_el2, 1250);
}

#[test]
fn mrs_cnthp_tval_el2_reports_remaining_ticks() {
    let (mut c, mut m) = cpu_with_code(&[encode_mrs(0, 3, 4, 14, 2, 0)]);
    c.current_el = 2;
    c.cntvct_el0 = 1000;
    c.cnthp_cval_el2 = 1250;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 250);
}
#[test]
fn daifset_masks_interrupts() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr_daifset(0xF)]);
    c.daif = 0;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.daif, 0xF);
}
#[test]
fn daifclr_unmasks_interrupts() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr_daifclr(0xF)]);
    c.daif = 0xF;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.daif, 0);
}
#[test]
fn spsel_switches_sp() {
    let (mut c, mut m) = cpu_with_code(&[encode_msr_spsel(1), NOP]);
    c.current_el = 1;
    c.sp = 0x1000;
    c.sp_el1 = 0x2000;
    c.spsel = false;
    step(&mut c, &mut m).unwrap();
    assert!(c.spsel);
    assert_eq!(c.sp_el1, 0x2000);
}
#[test]
fn mrs_dczid_el0_reports_64b_zva() {
    let (mut c, mut m) = cpu_with_code(&[encode_mrs(0, 1, 3, 0, 0, 7)]);
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[0], 0x4);
}
#[test]
fn tlbi_sets_tlb_flush_pending() {
    // TLBI VMALLE1IS, X0 (Rt ignored for all-entry form)
    let (mut c, mut m) = cpu_with_code(&[encode_sys(0, 1, 0, 8, 3, 0)]);
    assert!(!c.tlb_flush_pending);
    step(&mut c, &mut m).unwrap();
    assert!(c.tlb_flush_pending);
}

#[test]
fn tlbi_vae1is_sign_extends_high_kernel_va() {
    // TLBI VAE1IS, X0
    let (mut c, mut m) = cpu_with_code(&[encode_sys(0, 1, 0, 8, 3, 1)]);
    c.x[0] = 0xFFFF_0000_1234_5000u64 >> 12;
    step(&mut c, &mut m).unwrap();
    assert!(c.tlb_flush_pending);
    assert_eq!(c.tlb_flush_va, Some(0xFFFF_0000_1234_5000));
}

#[test]
fn tlbi_vale1is_records_per_va_flush() {
    // TLBI VALE1IS, X0
    let (mut c, mut m) = cpu_with_code(&[encode_sys(0, 1, 0, 8, 7, 1)]);
    c.x[0] = 0x12345;
    step(&mut c, &mut m).unwrap();
    assert!(c.tlb_flush_pending);
    assert_eq!(c.tlb_flush_va, Some(0x12345_000));
}

#[test]
fn tlbi_aside1is_records_asid_flush() {
    // TLBI ASIDE1IS, X0
    let (mut c, mut m) = cpu_with_code(&[encode_sys(0, 1, 0, 8, 3, 2)]);
    c.tcr_el1 |= 1u64 << 36;
    c.x[0] = 0x1234u64 << 48;
    step(&mut c, &mut m).unwrap();
    assert!(c.tlb_flush_pending);
    assert_eq!(c.tlb_flush_asid, Some(0x1234));
    assert_eq!(c.tlb_flush_va, None);
}

#[test]
fn tlbi_aside1is_masks_to_8bit_asid_when_tcr_as_clear() {
    // TLBI ASIDE1IS, X0
    let (mut c, mut m) = cpu_with_code(&[encode_sys(0, 1, 0, 8, 3, 2)]);
    c.tcr_el1 &= !(1u64 << 36);
    c.x[0] = 0x1234u64 << 48;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.tlb_flush_asid, Some(0x34));
}

#[test]
fn at_s1e1r_mm_off_sets_par_el1_identity() {
    // AT S1E1R, X0
    let (mut c, mut m) = cpu_with_code(&[encode_sys(0, 1, 0, 7, 8, 0)]);
    c.sctlr_el1 &= !1;
    c.x[0] = 0x4000_1234;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.par_el1, 0x4000_1000);
}
#[test]
#[ignore = "SVC at EL0 returns EnvironmentCall, not exception entry in this SE-mode executor"]
fn exception_entry_saves_state() {}
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

/// MSR/MRS for SP_EL1 must use op1=4 (not op1=6, which is SP_EL2).
///
/// Regression test: prior to this fix, both encodings aliased to
/// `sp_el2`, so an EL2 hypervisor (e.g. L4Re/Fiasco at EL2) seeding the
/// guest's SP_EL1 would silently zero its own SP_EL2 and crash on the
/// next stack access.
#[test]
fn msr_mrs_sp_el1_op1_is_4() {
    // MSR SP_EL1, X1 ; MRS X2, SP_EL1
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 1, 4, 4, 1, 0), encode_mrs(2, 1, 4, 4, 1, 0)]);
    c.current_el = 2;
    c.spsel = true;
    c.x[1] = 0xDEAD_BEEF_1000_0000;
    c.sp_el2 = 0xCAFE_0000_1000_0000;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.sp_el1, 0xDEAD_BEEF_1000_0000, "MSR SP_EL1 must hit sp_el1");
    assert_eq!(c.sp_el2, 0xCAFE_0000_1000_0000, "MSR SP_EL1 must not touch sp_el2");
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], 0xDEAD_BEEF_1000_0000, "MRS SP_EL1 must read sp_el1");
}

/// MSR/MRS for SP_EL2 uses op1=6 (only accessible from EL3).
#[test]
fn msr_mrs_sp_el2_op1_is_6() {
    let (mut c, mut m) =
        cpu_with_code(&[encode_msr(1, 1, 6, 4, 1, 0), encode_mrs(2, 1, 6, 4, 1, 0)]);
    c.current_el = 3;
    c.spsel = true;
    c.x[1] = 0xAAAA_BBBB_2000_0000;
    c.sp_el1 = 0x1111_2222_3333_4444;
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.sp_el2, 0xAAAA_BBBB_2000_0000, "MSR SP_EL2 must hit sp_el2");
    assert_eq!(c.sp_el1, 0x1111_2222_3333_4444, "MSR SP_EL2 must not touch sp_el1");
    step(&mut c, &mut m).unwrap();
    assert_eq!(c.x[2], 0xAAAA_BBBB_2000_0000, "MRS SP_EL2 must read sp_el2");
}
