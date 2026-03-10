//! Tests for system instruction support (Phases 1-4).

use crate::a64_emitter::{A64TcgEmitter, TranslateAction};
use crate::block::TcgBlock;
use crate::context::TcgContext;
use crate::interp::*;
use crate::ir::{TcgOp, TcgTemp};
use helm_memory::address_space::AddressSpace;

fn t(n: u32) -> TcgTemp {
    TcgTemp(n)
}

fn empty_regs() -> [u64; NUM_REGS] {
    [0u64; NUM_REGS]
}

fn make_block(ops: Vec<TcgOp>, insn_count: usize) -> TcgBlock {
    TcgBlock {
        guest_pc: 0x1000,
        guest_size: insn_count * 4,
        insn_count,
        ops,
    }
}

fn make_mem() -> AddressSpace {
    let mut mem = AddressSpace::new();
    mem.map(0x0, 0x10000, (true, true, false));
    mem
}

fn translate_one(insn: u32) -> (TranslateAction, Vec<TcgOp>) {
    let mut ctx = TcgContext::new();
    let mut emitter = A64TcgEmitter::new(&mut ctx, 0x1000);
    let action = emitter.translate_insn(insn);
    (action, ctx.finish())
}

// ── Phase 1: ReadSysReg / WriteSysReg in interpreter ────────────────────

#[test]
fn interp_read_sysreg() {
    let sysreg_id = 0xC082; // arbitrary
    let block = make_block(
        vec![
            TcgOp::ReadSysReg {
                dst: t(0),
                sysreg_id,
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(0),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.set_sysreg(sysreg_id, 0xDEAD_BEEF);
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[0], 0xDEAD_BEEF);
}

#[test]
fn interp_write_sysreg() {
    let sysreg_id = 0xC083;
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0x42,
            },
            TcgOp::WriteSysReg {
                sysreg_id,
                src: t(0),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(interp.get_sysreg(sysreg_id), 0x42);
}

#[test]
fn interp_unknown_sysreg_reads_zero() {
    let interp = TcgInterp::new();
    assert_eq!(interp.get_sysreg(0xFFFF), 0);
}

// ── Phase 2: MRS / MSR emitter ──────────────────────────────────────────

#[test]
fn emitter_mrs_produces_read_sysreg() {
    // MRS X1, SCTLR_EL1: op0=3(o0=1), op1=0, crn=1, crm=0, op2=0, rt=1
    // Encoding: 1101_0101_0011_1000_0001_0000_0000_0001
    let insn: u32 = 0xD538_1001;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::Continue);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TcgOp::ReadSysReg { sysreg_id, .. }
            if *sysreg_id == (3 << 14 | 0 << 11 | 1 << 7 | 0 << 3 | 0))),
        "expected ReadSysReg for SCTLR_EL1, got: {ops:?}"
    );
}

#[test]
fn emitter_msr_produces_write_sysreg() {
    // MSR VBAR_EL1, X2: op0=3(o0=1), op1=0, crn=12, crm=0, op2=0, rt=2
    // Encoding: 1101_0101_0001_1000_1100_0000_0000_0010
    let insn: u32 = 0xD518_C002;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::Continue);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TcgOp::WriteSysReg { sysreg_id, .. }
                if *sysreg_id == (3 << 14 | 0 << 11 | 12 << 7 | 0 << 3 | 0))),
        "expected WriteSysReg for VBAR_EL1, got: {ops:?}"
    );
}

#[test]
fn mrs_msr_roundtrip_through_interp() {
    let sysreg_id = 3 << 14 | 3 << 11 | 13 << 7 | 0 << 3 | 2; // TPIDR_EL0
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0xABCD_1234,
            },
            TcgOp::WriteSysReg {
                sysreg_id,
                src: t(0),
            },
            TcgOp::ReadSysReg {
                dst: t(1),
                sysreg_id,
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(1),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[0], 0xABCD_1234);
}

// ── Phase 3a: MSR immediate — DAIFSet / DAIFClr / SPSel ────────────────

#[test]
fn interp_daifset() {
    let block = make_block(vec![TcgOp::DaifSet { imm: 0xF }, TcgOp::ExitTb], 1);
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[REG_DAIF as usize], 0x3C0); // all DAIF bits set
}

#[test]
fn interp_daifclr() {
    let block = make_block(
        vec![
            TcgOp::DaifSet { imm: 0xF },
            TcgOp::DaifClr { imm: 0x3 }, // clear D and A
            TcgOp::ExitTb,
        ],
        1,
    );
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[REG_DAIF as usize], 0x300); // I and F still set
}

#[test]
fn interp_set_spsel() {
    let block = make_block(vec![TcgOp::SetSpSel { imm: 1 }, TcgOp::ExitTb], 1);
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[REG_SPSEL as usize], 1);
}

#[test]
fn emitter_msr_i_daifset() {
    // MSR DAIFSet, #0xF → 1101_0101_0000_0011_0100_1111_1101_1111
    let insn: u32 = 0xD503_4FDF;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::Continue);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TcgOp::DaifSet { imm: 0xF })),
        "expected DaifSet {{ imm: 0xF }}, got: {ops:?}"
    );
}

#[test]
fn emitter_msr_i_daifclear() {
    // MSR DAIFClr, #0x3 → 1101_0101_0000_0011_0100_0011_1111_1111
    let insn: u32 = 0xD503_43FF;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::Continue);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TcgOp::DaifClr { imm: 0x3 })),
        "expected DaifClr {{ imm: 0x3 }}, got: {ops:?}"
    );
}

#[test]
fn emitter_msr_i_spsel() {
    // MSR SPSel, #1 → 1101_0101_0000_0000_0100_0001_1011_1111
    let insn: u32 = 0xD500_41BF;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::Continue);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TcgOp::SetSpSel { imm: 1 })),
        "expected SetSpSel {{ imm: 1 }}, got: {ops:?}"
    );
}

// ── Phase 3b: SVC exception entry ──────────────────────────────────────

#[test]
fn interp_svc_exc_sets_elr_esr_spsr() {
    let block = make_block(vec![TcgOp::SvcExc { imm16: 0 }], 1);
    let mut regs = empty_regs();
    regs[REG_PC as usize] = 0x8_0000;
    regs[REG_VBAR_EL1 as usize] = 0x4_0000;
    regs[REG_CURRENT_EL as usize] = 0 << 2; // EL0
    regs[REG_NZCV as usize] = 0x4000_0000; // Z flag set
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    let result = interp.exec_block(&block, &mut regs, &mut mem).unwrap();

    // ELR_EL1 = PC + 4
    assert_eq!(regs[REG_ELR_EL1 as usize], 0x8_0004);
    // ESR_EL1: EC=0x15 (SVC64), IL=1, ISS=0
    assert_eq!(regs[REG_ESR_EL1 as usize], (0x15 << 26) | (1 << 25));
    // PC = VBAR + 0x400 (from EL0)
    assert_eq!(regs[REG_PC as usize], 0x4_0400);
    // DAIF masked
    assert_eq!(regs[REG_DAIF as usize], 0x3C0);
    // CurrentEL = 1
    assert_eq!(regs[REG_CURRENT_EL as usize], 1 << 2);
    // Exit type
    assert!(matches!(
        result.exit,
        InterpExit::Exception {
            class: 0x15,
            iss: 0
        }
    ));
}

#[test]
fn interp_svc_from_el1_uses_offset_0x200() {
    let block = make_block(vec![TcgOp::SvcExc { imm16: 42 }], 1);
    let mut regs = empty_regs();
    regs[REG_PC as usize] = 0x1000;
    regs[REG_VBAR_EL1 as usize] = 0x2_0000;
    regs[REG_CURRENT_EL as usize] = 1 << 2; // EL1
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[REG_PC as usize], 0x2_0200);
    // ISS encodes imm16
    assert_eq!(regs[REG_ESR_EL1 as usize] & 0xFFFF, 42);
}

#[test]
fn emitter_svc_produces_svc_exc() {
    // SVC #0 → 1101_0100_0000_0000_0000_0000_0000_0001
    let insn: u32 = 0xD400_0001;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::EndBlock);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TcgOp::SvcExc { imm16: 0 })),
        "expected SvcExc {{ imm16: 0 }}, got: {ops:?}"
    );
}

// ── Phase 3c: ERET ──────────────────────────────────────────────────────

#[test]
fn interp_eret_restores_pstate() {
    let block = make_block(vec![TcgOp::Eret], 1);
    let mut regs = empty_regs();
    regs[REG_ELR_EL1 as usize] = 0xCAFE_0000;
    // SPSR: N=1 (bit 31), DAIF=0x3C0, EL=0, SPSel=0
    regs[REG_SPSR_EL1 as usize] = 0x8000_03C0;
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    let result = interp.exec_block(&block, &mut regs, &mut mem).unwrap();

    assert_eq!(regs[REG_PC as usize], 0xCAFE_0000);
    assert_eq!(regs[REG_NZCV as usize], 0x8000_0000); // N flag
    assert_eq!(regs[REG_DAIF as usize], 0x3C0);
    assert_eq!(regs[REG_CURRENT_EL as usize], 0); // EL0
    assert_eq!(regs[REG_SPSEL as usize], 0);
    assert!(matches!(result.exit, InterpExit::ExceptionReturn));
}

#[test]
fn emitter_eret_produces_eret_op() {
    // ERET → 1101_0110_1001_1111_0000_0011_1110_0000
    let insn: u32 = 0xD69F_03E0;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::EndBlock);
    assert!(
        ops.iter().any(|op| matches!(op, TcgOp::Eret)),
        "expected Eret op, got: {ops:?}"
    );
}

#[test]
fn svc_then_eret_roundtrip() {
    // SVC from EL0 → ERET back to EL0
    let mut regs = empty_regs();
    regs[REG_PC as usize] = 0x8_0000;
    regs[REG_VBAR_EL1 as usize] = 0x4_0000;
    regs[REG_CURRENT_EL as usize] = 0;
    regs[REG_NZCV as usize] = 0x2000_0000; // C flag
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();

    // Step 1: SVC
    let svc_block = make_block(vec![TcgOp::SvcExc { imm16: 0 }], 1);
    interp.exec_block(&svc_block, &mut regs, &mut mem).unwrap();
    let saved_el = regs[REG_CURRENT_EL as usize];
    assert_eq!(saved_el, 1 << 2); // now at EL1

    // Step 2: ERET
    let eret_block = make_block(vec![TcgOp::Eret], 1);
    interp.exec_block(&eret_block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[REG_PC as usize], 0x8_0004); // returned
    assert_eq!(regs[REG_CURRENT_EL as usize], 0); // back to EL0
    assert_eq!(regs[REG_NZCV as usize], 0x2000_0000); // C flag restored
}

// ── Phase 4: WFI ────────────────────────────────────────────────────────

#[test]
fn interp_wfi_exits_with_wfi() {
    let block = make_block(vec![TcgOp::Wfi], 1);
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    let result = interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert!(matches!(result.exit, InterpExit::Wfi));
}

#[test]
fn emitter_wfi_produces_wfi_op() {
    // WFI → 1101_0101_0000_0011_0010_0000_0111_1111
    let insn: u32 = 0xD503_207F;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::EndBlock);
    assert!(
        ops.iter().any(|op| matches!(op, TcgOp::Wfi)),
        "expected Wfi op, got: {ops:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Phase 5 — DC ZVA, TLBI, AT, barriers, CLREX
// ═══════════════════════════════════════════════════════════════════

#[test]
fn interp_dc_zva_zeroes_memory() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0x2010,
            },
            TcgOp::DcZva { addr: t(0) },
            TcgOp::ExitTb,
        ],
        1,
    );
    let mut regs = empty_regs();
    let mut mem = make_mem();
    mem.write(0x2000, &[0xFFu8; 64]).unwrap();
    let mut interp = TcgInterp::new();
    let result = interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    let mut buf = [0xFFu8; 64];
    mem.read(0x2000, &mut buf).unwrap();
    assert!(buf.iter().all(|&b| b == 0), "DC ZVA should zero the block");
    assert!(result
        .mem_accesses
        .iter()
        .any(|a| a.is_write && a.addr == 0x2000 && a.size == 64));
}

#[test]
fn emitter_sys_dc_zva() {
    // SYS #3, C7, C4, #1, X5
    // Encoding: 1101_0101_0000_1011_0111_0100_0010_0101
    let insn: u32 = 0xD50B_7425;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::Continue);
    assert!(
        ops.iter().any(|op| matches!(op, TcgOp::DcZva { .. })),
        "expected DcZva, got: {ops:?}"
    );
}

#[test]
fn emitter_sys_tlbi() {
    // TLBI VMALLE1IS: SYS #0, C8, C3, #0, XZR
    // Encoding: 1101_0101_0000_1000_1000_0011_0001_1111
    let insn: u32 = 0xD508_831F;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::Continue);
    assert!(
        ops.iter().any(|op| matches!(op, TcgOp::Tlbi { .. })),
        "expected Tlbi, got: {ops:?}"
    );
}

#[test]
fn interp_tlbi_is_nop() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0,
            },
            TcgOp::Tlbi {
                op: 0x030,
                addr: t(0),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    let result = interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert!(matches!(result.exit, InterpExit::Exit));
}

#[test]
fn interp_at_writes_par() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0x1234_5000,
            },
            TcgOp::At {
                op: 0x30, // S1E1R
                addr: t(0),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(interp.get_sysreg(0xC3A0), 0x1234_5000); // PAR_EL1
}

#[test]
fn emitter_dsb_produces_barrier() {
    // DSB SY → 1101_0101_0000_0011_0011_1111_1001_1111
    let insn: u32 = 0xD503_3F9F;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::Continue);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TcgOp::Barrier { kind: 0 })),
        "expected Barrier(DSB), got: {ops:?}"
    );
}

#[test]
fn emitter_dmb_produces_barrier() {
    // DMB SY → 1101_0101_0000_0011_0011_1111_1011_1111
    let insn: u32 = 0xD503_3FBF;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::Continue);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TcgOp::Barrier { kind: 1 })),
        "expected Barrier(DMB), got: {ops:?}"
    );
}

#[test]
fn emitter_isb_ends_block() {
    // ISB → 1101_0101_0000_0011_0011_1111_1101_1111
    let insn: u32 = 0xD503_3FDF;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::EndBlock);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TcgOp::Barrier { kind: 2 })),
        "expected Barrier(ISB), got: {ops:?}"
    );
}

#[test]
fn emitter_clrex_produces_clrex() {
    // CLREX → 1101_0101_0000_0011_0011_1111_0101_1111
    let insn: u32 = 0xD503_3F5F;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::Continue);
    assert!(
        ops.iter().any(|op| matches!(op, TcgOp::Clrex)),
        "expected Clrex, got: {ops:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Phase 6 — HVC, SMC, BRK, HLT
// ═══════════════════════════════════════════════════════════════════

#[test]
fn emitter_hvc_produces_hvc_exc() {
    // HVC #0x42 → 1101_0100_0000_0000_0000_1000_0100_0010
    let insn: u32 = 0xD400_0842;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::EndBlock);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TcgOp::HvcExc { imm16: 0x42 })),
        "expected HvcExc {{ imm16: 0x42 }}, got: {ops:?}"
    );
}

#[test]
fn emitter_smc_produces_smc_exc() {
    // SMC #0 → 1101_0100_0000_0000_0000_0000_0110_0011
    let insn: u32 = 0xD400_0003;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::EndBlock);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TcgOp::SmcExc { imm16: 0 })),
        "expected SmcExc {{ imm16: 0 }}, got: {ops:?}"
    );
}

#[test]
fn emitter_brk_produces_brk_exc() {
    // BRK #1 → 1101_0100_0010_0000_0000_0000_0010_0000
    let insn: u32 = 0xD420_0020;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::EndBlock);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TcgOp::BrkExc { imm16: 1 })),
        "expected BrkExc {{ imm16: 1 }}, got: {ops:?}"
    );
}

#[test]
fn emitter_hlt_produces_hlt_exc() {
    // HLT #0 → 1101_0100_0100_0000_0000_0000_0000_0000
    let insn: u32 = 0xD440_0000;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::EndBlock);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TcgOp::HltExc { imm16: 0 })),
        "expected HltExc {{ imm16: 0 }}, got: {ops:?}"
    );
}

#[test]
fn interp_hvc_exits_with_ec_0x16() {
    let block = make_block(vec![TcgOp::HvcExc { imm16: 7 }], 1);
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    let result = interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert!(matches!(
        result.exit,
        InterpExit::Exception {
            class: 0x16,
            iss: 7
        }
    ));
}

#[test]
fn interp_smc_exits_with_ec_0x17() {
    let block = make_block(vec![TcgOp::SmcExc { imm16: 0 }], 1);
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    let result = interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert!(matches!(
        result.exit,
        InterpExit::Exception {
            class: 0x17,
            iss: 0
        }
    ));
}

#[test]
fn interp_brk_exits_with_ec_0x3c() {
    let block = make_block(vec![TcgOp::BrkExc { imm16: 99 }], 1);
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    let result = interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert!(matches!(
        result.exit,
        InterpExit::Exception {
            class: 0x3C,
            iss: 99
        }
    ));
}

// ═══════════════════════════════════════════════════════════════════
// Phase 7 — SYS dispatch (AT already tested above)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn emitter_sys_other_is_nop() {
    // SYS #0, C7, C5, #0, XZR  (IC IALLU — NOP in simulation)
    // Encoding: 1101_0101_0000_1000_0111_0101_0001_1111
    let insn: u32 = 0xD508_751F;
    let (action, _ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::Continue);
}

// ═══════════════════════════════════════════════════════════════════
// Phase 8 — CFINV
// ═══════════════════════════════════════════════════════════════════

#[test]
fn interp_cfinv_toggles_c_flag() {
    let block = make_block(vec![TcgOp::Cfinv, TcgOp::ExitTb], 1);
    let mut regs = empty_regs();
    regs[REG_NZCV as usize] = 0x2000_0000; // C=1
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[REG_NZCV as usize], 0); // C toggled off
}

#[test]
fn interp_cfinv_sets_c_when_clear() {
    let block = make_block(vec![TcgOp::Cfinv, TcgOp::ExitTb], 1);
    let mut regs = empty_regs();
    regs[REG_NZCV as usize] = 0x8000_0000; // N=1, C=0
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[REG_NZCV as usize], 0xA000_0000); // N=1, C=1
}

#[test]
fn emitter_cfinv_produces_cfinv_op() {
    // CFINV → 1101_0101_0000_0000_0100_0000_0001_1111
    let insn: u32 = 0xD500_401F;
    let (action, ops) = translate_one(insn);
    assert_eq!(action, TranslateAction::Continue);
    assert!(
        ops.iter().any(|op| matches!(op, TcgOp::Cfinv)),
        "expected Cfinv, got: {ops:?}"
    );
}
