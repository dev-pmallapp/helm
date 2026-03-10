//! Tests for EL2/EL3 exception routing, HVC, SMC, VHE, and stage-2 translation.

use crate::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;

const BASE: u64 = 0x40_0000;

fn cpu_with_code(insns: &[u32]) -> (Aarch64Cpu, AddressSpace) {
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();
    let size = (insns.len() * 4 + 0x1000) as u64;
    mem.map(BASE, size, (true, true, true));
    for (i, &insn) in insns.iter().enumerate() {
        let addr = BASE + (i as u64) * 4;
        mem.write(addr, &insn.to_le_bytes()).unwrap();
    }
    cpu.regs.pc = BASE;
    cpu.regs.sp = BASE + size - 8;
    (cpu, mem)
}

// ── HVC from EL1 ────────────────────────────────────────────────────────

#[test]
fn hvc_from_el1_takes_exception_to_el2() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0002]); // HVC #0
    c.regs.current_el = 1;
    c.regs.vbar_el2 = 0x8_0000;
    // Map VBAR region
    m.map(0x8_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    assert_eq!(c.regs.current_el, 2);
    assert_eq!(c.regs.pc, 0x8_0000 + 0x400); // from lower EL, AArch64
    assert_eq!(c.regs.elr_el2, BASE + 4); // preferred return address (PC+4 for HVC)
    assert_eq!(c.regs.esr_el2 >> 26, 0x16); // EC = HVC
}

// ── HVC disabled (HCD=1) ────────────────────────────────────────────────

#[test]
fn hvc_undefined_when_hcd_set() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0002]); // HVC #0
    c.regs.current_el = 1;
    c.regs.hcr_el2 = 1 << 29; // HCD=1
    c.regs.vbar_el1 = 0x8_0000;
    m.map(0x8_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    // Should trap to EL1 as UNDEFINED (EC=0x00)
    assert_eq!(c.regs.current_el, 1);
    assert_eq!(c.regs.esr_el1 >> 26, 0x00);
}

// ── SMC from EL1 → EL3 ─────────────────────────────────────────────────

#[test]
fn smc_from_el1_takes_exception_to_el3() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0003]); // SMC #0
    c.regs.current_el = 1;
    c.regs.scr_el3 = 0; // SMD=0 (SMC enabled)
    c.regs.vbar_el3 = 0xC_0000;
    m.map(0xC_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    assert_eq!(c.regs.current_el, 3);
    assert_eq!(c.regs.pc, 0xC_0000 + 0x400);
    assert_eq!(c.regs.elr_el3, BASE + 4); // preferred return address (PC+4 for SMC)
    assert_eq!(c.regs.esr_el3 >> 26, 0x17); // EC = SMC
}

// ── SMC trapped to EL2 (TSC) ───────────────────────────────────────────

#[test]
fn smc_from_el1_traps_to_el2_when_tsc() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0003]); // SMC #0
    c.regs.current_el = 1;
    c.regs.hcr_el2 = 1 << 19; // TSC=1
    c.regs.vbar_el2 = 0x8_0000;
    m.map(0x8_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    assert_eq!(c.regs.current_el, 2);
    assert_eq!(c.regs.pc, 0x8_0000 + 0x400);
    assert_eq!(c.regs.esr_el2 >> 26, 0x17); // EC = SMC
}

// ── SMC disabled (SMD=1) ────────────────────────────────────────────────

#[test]
fn smc_undefined_when_smd_set() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0003]); // SMC #0
    c.regs.current_el = 1;
    c.regs.scr_el3 = 1 << 7; // SMD=1
    c.regs.vbar_el1 = 0x8_0000;
    m.map(0x8_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    assert_eq!(c.regs.current_el, 1);
    assert_eq!(c.regs.esr_el1 >> 26, 0x00); // UNDEFINED
}

// ── ERET from EL3 → EL1 ────────────────────────────────────────────────

#[test]
fn eret_from_el3_to_el1() {
    let (mut c, mut m) = cpu_with_code(&[0xD69F_03E0]); // ERET
    c.regs.current_el = 3;
    c.regs.elr_el3 = 0x10_0000;
    // SPSR: EL1h (current_el=1, sp_sel=1)
    c.regs.spsr_el3 = (1 << 2) | 1; // M[3:2]=01 (EL1), M[0]=1 (SPh)
    m.map(0x10_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    assert_eq!(c.regs.current_el, 1);
    assert_eq!(c.regs.sp_sel, 1);
    assert_eq!(c.regs.pc, 0x10_0000);
}

// ── ERET from EL2 → EL1 ────────────────────────────────────────────────

#[test]
fn eret_from_el2_to_el1() {
    let (mut c, mut m) = cpu_with_code(&[0xD69F_03E0]); // ERET
    c.regs.current_el = 2;
    c.regs.elr_el2 = 0x20_0000;
    c.regs.spsr_el2 = (1 << 2) | 1; // EL1h
    m.map(0x20_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    assert_eq!(c.regs.current_el, 1);
    assert_eq!(c.regs.pc, 0x20_0000);
}

// ── VHE register redirection ────────────────────────────────────────────

#[test]
fn vhe_redirects_sctlr_el1_to_sctlr_el2() {
    // MSR SCTLR_EL1, X0 at EL2 with E2H=1 should write SCTLR_EL2
    // SCTLR_EL1 encoding: 1101_0101_00_0_11_000_0001_0000_000_00000 = 0xD518_1000
    // MSR SCTLR_EL1, X0: op0=3(11) L=0 op1=000 CRn=0001 CRm=0000 op2=000 Rt=00000
    // insn = 1101_0101_00_0_11_000_0001_0000_000_00000 = 0xD518_1000
    let insn = 0xD518_1000u32; // MSR SCTLR_EL1, X0
    let (mut c, mut m) = cpu_with_code(&[insn]);
    c.regs.current_el = 2;
    c.regs.hcr_el2 = 1u64 << 34; // E2H=1 (VHE)
    c.set_xn(0, 0xDEAD_BEEF);

    c.step(&mut m).unwrap();

    // Should have written to SCTLR_EL2, not SCTLR_EL1
    assert_ne!(c.regs.sctlr_el1, 0xDEAD_BEEF);
    // SCTLR_EL2 was written (TLB flush may zero some bits)
    assert_eq!(c.regs.sctlr_el2, 0xDEAD_BEEF);
}

// ── TVM trap ────────────────────────────────────────────────────────────

#[test]
fn tvm_traps_sctlr_el1_write_to_el2() {
    let insn = 0xD518_1000u32; // MSR SCTLR_EL1, X0
    let (mut c, mut m) = cpu_with_code(&[insn]);
    c.regs.current_el = 1;
    c.regs.hcr_el2 = 1 << 26; // TVM=1
    c.regs.vbar_el2 = 0x8_0000;
    m.map(0x8_0000, 0x1000, (true, true, true));
    c.set_xn(0, 0x12345678);

    c.step(&mut m).unwrap();

    // Should have trapped to EL2
    assert_eq!(c.regs.current_el, 2);
    assert_eq!(c.regs.esr_el2 >> 26, 0x18); // EC = MSR/MRS trap
                                            // SCTLR_EL1 should NOT have been modified
    assert_eq!(c.regs.sctlr_el1, 0x0080_0800); // default RES1 value
}

// ── SVC from EL0 with TGE → routes to EL2 ──────────────────────────────

#[test]
fn svc_el0_routes_to_el2_with_tge() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0001]); // SVC #0
    c.regs.current_el = 0;
    c.regs.hcr_el2 = 1 << 27; // TGE=1
    c.regs.vbar_el2 = 0x8_0000;
    m.map(0x8_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    assert_eq!(c.regs.current_el, 2);
    assert_eq!(c.regs.pc, 0x8_0000 + 0x400);
    assert_eq!(c.regs.esr_el2 >> 26, 0x15); // EC = SVC
}

// ── Exception entry to EL3 saves state correctly ────────────────────────

#[test]
fn el3_exception_entry_saves_full_state() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0003]); // SMC #0
    c.regs.current_el = 2;
    c.regs.sp_sel = 1;
    c.regs.daif = 0x80; // only I masked
    c.regs.nzcv = 0x6000_0000; // Z and C set
    c.regs.vbar_el3 = 0xC_0000;
    m.map(0xC_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    assert_eq!(c.regs.current_el, 3);
    // Verify SPSR_EL3 captured the source state
    let spsr = c.regs.spsr_el3;
    assert_eq!(spsr & 0xF000_0000, 0x6000_0000); // NZCV
    assert_eq!(spsr & 0x3C0, 0x80); // DAIF
    assert_eq!((spsr >> 2) & 3, 2); // source EL = 2
    assert_eq!(spsr & 1, 1); // SP_ELx
                             // EL3 state: all masked
    assert_eq!(c.regs.daif, 0x3C0);
    assert_eq!(c.regs.sp_sel, 1);
}

// ── SMC from EL2 → EL3 ───────────────────────────────────────────────

#[test]
fn smc_from_el2_routes_to_el3() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0003]); // SMC #0
    c.regs.current_el = 2;
    c.regs.sp_sel = 1;
    c.regs.scr_el3 = 0; // SMD=0
    c.regs.vbar_el3 = 0xC_0000;
    m.map(0xC_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    assert_eq!(c.regs.current_el, 3);
    assert_eq!(c.regs.pc, 0xC_0000 + 0x400); // from lower EL (EL2 < EL3)
    assert_eq!(c.regs.esr_el3 >> 26, 0x17); // EC = SMC
}

// ── ERET from EL3 → EL2 ────────────────────────────────────────────

#[test]
fn eret_from_el3_to_el2() {
    let (mut c, mut m) = cpu_with_code(&[0xD69F_03E0]); // ERET
    c.regs.current_el = 3;
    c.regs.elr_el3 = 0x30_0000;
    c.regs.spsr_el3 = (2 << 2) | 1; // EL2h
    m.map(0x30_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    assert_eq!(c.regs.current_el, 2);
    assert_eq!(c.regs.sp_sel, 1);
    assert_eq!(c.regs.pc, 0x30_0000);
}

// ── SVC from EL0 without TGE → routes to EL1 ───────────────────────

#[test]
fn svc_el0_routes_to_el1_without_tge() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0001]); // SVC #0
    c.regs.current_el = 0;
    c.regs.hcr_el2 = 0; // TGE=0
    c.regs.vbar_el1 = 0x8_0000;
    m.map(0x8_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    assert_eq!(c.regs.current_el, 1);
    assert_eq!(c.regs.pc, 0x8_0000 + 0x400);
    assert_eq!(c.regs.esr_el1 >> 26, 0x15); // EC = SVC
}

// ── AT S1E1R — address translate instruction ────────────────────────

#[test]
fn at_s1e1r_fault_sets_par_el1_f_bit() {
    // AT S1E1R, X0: SYS #0, C7, C8, #0, X0
    // Encoding: 1101_0101_00_0_01_000_0111_1000_000_00000 = 0xD508_7800
    let (mut c, mut m) = cpu_with_code(&[0xD508_7800]);
    c.set_xn(0, 0x1234_0000); // VA to translate
                              // TCR/TTBR are all zero → page tables invalid → translation fault
    c.regs.tcr_el1 = 25; // T0SZ=25

    c.step(&mut m).unwrap();

    // PAR_EL1.F (bit 0) should be set on fault
    assert_eq!(
        c.regs.par_el1 & 1,
        1,
        "PAR.F should be set on translation fault"
    );
}

// ── VHE: MRS TTBR0_EL1 at EL2 with E2H=1 reads TTBR0_EL2 ─────────

#[test]
fn vhe_redirects_ttbr0_el1_read_to_el2() {
    // MRS X0, TTBR0_EL1: op0=3(o0=1), op1=0, CRn=2, CRm=0, op2=0
    // Encoding: 0xD538_2000
    let insn = 0xD538_2000u32; // MRS X0, TTBR0_EL1
    let (mut c, mut m) = cpu_with_code(&[insn]);
    c.regs.current_el = 2;
    c.regs.hcr_el2 = 1u64 << 34; // E2H=1 (VHE)
    c.regs.ttbr0_el2 = 0xABCD_0000;
    c.regs.ttbr0_el1 = 0x1111_0000;

    c.step(&mut m).unwrap();

    // VHE should redirect to TTBR0_EL2
    assert_eq!(c.xn(0), 0xABCD_0000);
}

// ── TVM: MRS reads are NOT trapped ──────────────────────────────────

#[test]
fn tvm_does_not_trap_sysreg_reads() {
    // MRS X0, SCTLR_EL1 at EL1 with TVM=1 should NOT trap (TVM only traps writes)
    let insn = 0xD538_1000u32; // MRS X0, SCTLR_EL1
    let (mut c, mut m) = cpu_with_code(&[insn]);
    c.regs.current_el = 1;
    c.regs.hcr_el2 = 1 << 26; // TVM=1
    c.regs.sctlr_el1 = 0xDEAD_BEEE; // bit 0 clear → MMU off (avoids fetch fault)

    c.step(&mut m).unwrap();

    // Should have read SCTLR_EL1 successfully (no trap)
    assert_eq!(c.regs.current_el, 1, "should stay at EL1");
    assert_eq!(c.xn(0), 0xDEAD_BEEE);
}

// ── TVM: traps TTBR0_EL1 write ─────────────────────────────────────

#[test]
fn tvm_traps_ttbr0_el1_write_to_el2() {
    // MSR TTBR0_EL1, X0: op0=3(o0=1), op1=0, CRn=2, CRm=0, op2=0
    let insn = 0xD518_2000u32; // MSR TTBR0_EL1, X0
    let (mut c, mut m) = cpu_with_code(&[insn]);
    c.regs.current_el = 1;
    c.regs.hcr_el2 = 1 << 26; // TVM=1
    c.regs.vbar_el2 = 0x8_0000;
    m.map(0x8_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    assert_eq!(c.regs.current_el, 2);
    assert_eq!(c.regs.esr_el2 >> 26, 0x18); // EC = MSR/MRS trap
}

// ── HVC from EL0 → UNDEFINED ────────────────────────────────────────

#[test]
fn hvc_from_el0_is_undefined() {
    let (mut c, mut m) = cpu_with_code(&[0xD400_0002]); // HVC #0
    c.regs.current_el = 0;
    c.regs.vbar_el1 = 0x8_0000;
    m.map(0x8_0000, 0x1000, (true, true, true));

    c.step(&mut m).unwrap();

    // HVC at EL0 → UNDEFINED → EL1
    assert_eq!(c.regs.current_el, 1);
    assert_eq!(c.regs.esr_el1 >> 26, 0x00); // EC = Unknown/UNDEF
}
