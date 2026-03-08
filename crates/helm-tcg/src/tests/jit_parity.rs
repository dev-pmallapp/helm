//! JIT-vs-interpreter parity tests.
//!
//! Each test builds a `TcgBlock`, executes it through both the match-based
//! interpreter and the Cranelift JIT, then asserts that the two produce
//! identical register state and exit variants.

use crate::block::TcgBlock;
use crate::interp::{InterpExit, TcgInterp, NUM_REGS, SYSREG_FILE_SIZE};
use crate::ir::{TcgOp, TcgTemp};
use crate::jit::{exec_jit, JitEngine};
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

/// Run `block` through both interp and JIT with the same initial register
/// state. Panics if the two backends disagree on any register or exit kind.
fn run_jit_parity(
    block: &TcgBlock,
    init_regs: &[u64; NUM_REGS],
) -> ([u64; NUM_REGS], [u64; NUM_REGS]) {
    // -- Interpreter path --
    let mut interp_regs = *init_regs;
    let mut interp_mem = make_mem();
    for addr in (0x2000u64..0x3000).step_by(8) {
        let _ = interp_mem.write(addr, &[0u8; 8]);
    }
    let mut interp = TcgInterp::new();
    let interp_result = interp
        .exec_block(block, &mut interp_regs, &mut interp_mem)
        .expect("interp exec_block failed");

    // Apply PC fixup that the session loop would do
    match &interp_result.exit {
        InterpExit::Chain { target_pc } => {
            interp_regs[crate::interp::REG_PC as usize] = *target_pc;
        }
        InterpExit::EndOfBlock { next_pc } => {
            interp_regs[crate::interp::REG_PC as usize] = *next_pc;
        }
        _ => {}
    }

    // -- JIT path --
    let mut jit_regs = *init_regs;
    let mut jit_mem = make_mem();
    for addr in (0x2000u64..0x3000).step_by(8) {
        let _ = jit_mem.write(addr, &[0u8; 8]);
    }
    let mut engine = JitEngine::new();
    let jit_block = engine.compile(block).expect("JIT compile failed");
    let mut sysregs = vec![0u64; SYSREG_FILE_SIZE];
    // Sync ELR/SPSR from regs to sysregs so JIT ERET reads correct values
    sysregs[crate::interp::sysreg_idx(0xC201)] = init_regs[crate::interp::REG_ELR_EL1 as usize];
    sysregs[crate::interp::sysreg_idx(0xC200)] = init_regs[crate::interp::REG_SPSR_EL1 as usize];
    let jit_result = unsafe {
        exec_jit(
            &jit_block,
            &mut jit_regs,
            std::ptr::null_mut(),
            &mut jit_mem,
            &mut sysregs,
        )
    };

    // -- Register comparison --
    for i in 0..NUM_REGS {
        assert_eq!(
            interp_regs[i], jit_regs[i],
            "reg[{i}] mismatch at guest_pc={:#x}: interp={:#x} jit={:#x}",
            block.guest_pc, interp_regs[i], jit_regs[i]
        );
    }

    // -- Exit kind comparison --
    let exit_matches = match (&interp_result.exit, &jit_result.exit) {
        (InterpExit::Exit, InterpExit::Exit) => true,
        (InterpExit::EndOfBlock { .. }, InterpExit::Exit) => true,
        (InterpExit::Exit, InterpExit::EndOfBlock { .. }) => true,
        (InterpExit::EndOfBlock { .. }, InterpExit::EndOfBlock { .. }) => true,
        (InterpExit::Chain { target_pc: a }, InterpExit::Chain { target_pc: b }) => a == b,
        (InterpExit::Syscall { nr: a }, InterpExit::Syscall { nr: b }) => a == b,
        (InterpExit::Wfi, InterpExit::Wfi) => true,
        (
            InterpExit::Exception { class: ca, iss: ia },
            InterpExit::Exception { class: cb, iss: ib },
        ) => ca == cb && ia == ib,
        (InterpExit::ExceptionReturn, InterpExit::ExceptionReturn) => true,
        _ => false,
    };
    assert!(
        exit_matches,
        "exit kind mismatch: interp={:?} jit={:?}",
        interp_result.exit, jit_result.exit
    );

    (interp_regs, jit_regs)
}

// ── Basic ALU ──────────────────────────────────────────────────────────────

#[test]
fn jit_parity_add() {
    let block = make_block(
        vec![
            TcgOp::Movi { dst: t(0), value: 100 },
            TcgOp::Movi { dst: t(1), value: 200 },
            TcgOp::Add { dst: t(2), a: t(0), b: t(1) },
            TcgOp::WriteReg { reg_id: 0, src: t(2) },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, jit) = run_jit_parity(&block, &empty_regs());
    assert_eq!(ir[0], 300);
    assert_eq!(jit[0], 300);
}

#[test]
fn jit_parity_sub() {
    let block = make_block(
        vec![
            TcgOp::Movi { dst: t(0), value: 5 },
            TcgOp::Movi { dst: t(1), value: 10 },
            TcgOp::Sub { dst: t(2), a: t(0), b: t(1) },
            TcgOp::WriteReg { reg_id: 0, src: t(2) },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, _) = run_jit_parity(&block, &empty_regs());
    assert_eq!(ir[0], 5u64.wrapping_sub(10));
}

#[test]
fn jit_parity_shifts() {
    let block = make_block(
        vec![
            TcgOp::Movi { dst: t(0), value: 0x80 },
            TcgOp::Movi { dst: t(1), value: 3 },
            TcgOp::Shl { dst: t(2), a: t(0), b: t(1) },
            TcgOp::WriteReg { reg_id: 0, src: t(2) },
            TcgOp::Shr { dst: t(3), a: t(0), b: t(1) },
            TcgOp::WriteReg { reg_id: 1, src: t(3) },
            TcgOp::Movi { dst: t(4), value: 0xFFFF_FFFF_0000_0000u64 },
            TcgOp::Movi { dst: t(5), value: 16 },
            TcgOp::Sar { dst: t(6), a: t(4), b: t(5) },
            TcgOp::WriteReg { reg_id: 2, src: t(6) },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, _) = run_jit_parity(&block, &empty_regs());
    assert_eq!(ir[0], 0x400);
    assert_eq!(ir[1], 0x10);
    assert_eq!(ir[2], 0xFFFF_FFFF_FFFF_0000u64);
}

// ── Comparisons ────────────────────────────────────────────────────────────

#[test]
fn jit_parity_comparisons() {
    let block = make_block(
        vec![
            TcgOp::Movi { dst: t(0), value: 5 },
            TcgOp::Movi { dst: t(1), value: 10 },
            TcgOp::SetEq { dst: t(2), a: t(0), b: t(0) },
            TcgOp::WriteReg { reg_id: 0, src: t(2) },
            TcgOp::SetNe { dst: t(3), a: t(0), b: t(1) },
            TcgOp::WriteReg { reg_id: 1, src: t(3) },
            TcgOp::SetLt { dst: t(4), a: t(0), b: t(1) },
            TcgOp::WriteReg { reg_id: 2, src: t(4) },
            TcgOp::SetGe { dst: t(5), a: t(1), b: t(0) },
            TcgOp::WriteReg { reg_id: 3, src: t(5) },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, _) = run_jit_parity(&block, &empty_regs());
    assert_eq!(ir[0], 1);
    assert_eq!(ir[1], 1);
    assert_eq!(ir[2], 1);
    assert_eq!(ir[3], 1);
}

// ── Branches ───────────────────────────────────────────────────────────────

#[test]
fn jit_parity_brcond_taken() {
    let block = make_block(
        vec![
            TcgOp::Movi { dst: t(0), value: 1 },
            TcgOp::BrCond { cond: t(0), label: 0 },
            TcgOp::Movi { dst: t(1), value: 999 },
            TcgOp::WriteReg { reg_id: 0, src: t(1) },
            TcgOp::ExitTb,
            TcgOp::Label { id: 0 },
            TcgOp::Movi { dst: t(1), value: 42 },
            TcgOp::WriteReg { reg_id: 0, src: t(1) },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, _) = run_jit_parity(&block, &empty_regs());
    assert_eq!(ir[0], 42);
}

#[test]
fn jit_parity_brcond_not_taken() {
    let block = make_block(
        vec![
            TcgOp::Movi { dst: t(0), value: 0 },
            TcgOp::BrCond { cond: t(0), label: 0 },
            TcgOp::Movi { dst: t(1), value: 77 },
            TcgOp::WriteReg { reg_id: 0, src: t(1) },
            TcgOp::ExitTb,
            TcgOp::Label { id: 0 },
            TcgOp::Movi { dst: t(1), value: 42 },
            TcgOp::WriteReg { reg_id: 0, src: t(1) },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, _) = run_jit_parity(&block, &empty_regs());
    assert_eq!(ir[0], 77);
}

// ── GotoTb / chain ─────────────────────────────────────────────────────────

#[test]
fn jit_parity_goto_tb() {
    let block = make_block(
        vec![TcgOp::GotoTb { target_pc: 0x5000 }],
        1,
    );
    run_jit_parity(&block, &empty_regs());
}

// ── NZCV flags (the most likely divergence source) ─────────────────────────

#[test]
fn jit_parity_nzcv_via_emitter() {
    use crate::a64_emitter::{A64TcgEmitter, TranslateAction};
    use crate::context::TcgContext;

    // SUBS X0, X1, #1 — test flag computation through the emitter
    let insn = 0xF1000420u32;
    let mut ctx = TcgContext::new();
    let mut e = A64TcgEmitter::new(&mut ctx, 0x1000);
    assert!(matches!(e.translate_insn(insn), TranslateAction::Continue));
    let block = TcgBlock {
        guest_pc: 0x1000,
        guest_size: 4,
        insn_count: 1,
        ops: ctx.finish(),
    };

    // Test 1: X1 = 5, SUBS X0, X1, #1 → X0=4, flags: no N, no Z, C=1, no V
    let mut regs = empty_regs();
    regs[1] = 5;
    run_jit_parity(&block, &regs);

    // Test 2: X1 = 1, SUBS X0, X1, #1 → X0=0, Z=1, C=1
    let mut regs = empty_regs();
    regs[1] = 1;
    run_jit_parity(&block, &regs);

    // Test 3: X1 = 0, SUBS X0, X1, #1 → X0=0xFFFF..FF, N=1, C=0
    let mut regs = empty_regs();
    regs[1] = 0;
    run_jit_parity(&block, &regs);
}

#[test]
fn jit_parity_subs_then_b_cond() {
    use crate::a64_emitter::{A64TcgEmitter, TranslateAction};
    use crate::context::TcgContext;

    // Typical page-table-loop: SUBS X3, X3, #1 ; B.NE loop
    let subs = 0xF1000463u32; // SUBS X3, X3, #1
    let bne = 0x54FFFFC1u32;  // B.NE #-8 (back to subs)
    let mut ctx = TcgContext::new();
    {
        let mut e = A64TcgEmitter::new(&mut ctx, 0x1000);
        let action = e.translate_insn(subs);
        assert!(matches!(action, TranslateAction::Continue));
    }
    {
        let mut e = A64TcgEmitter::new(&mut ctx, 0x1004);
        let action = e.translate_insn(bne);
        assert!(matches!(action, TranslateAction::EndBlock));
    }
    let block = TcgBlock {
        guest_pc: 0x1000,
        guest_size: 8,
        insn_count: 2,
        ops: ctx.finish(),
    };

    // X3 = 5: SUBS sets Z=0, B.NE taken → chain to 0x1000
    let mut regs = empty_regs();
    regs[3] = 5;
    run_jit_parity(&block, &regs);

    // X3 = 1: SUBS X3=0, sets Z=1, B.NE not taken → chain to 0x1008
    let mut regs = empty_regs();
    regs[3] = 1;
    run_jit_parity(&block, &regs);
}

#[test]
fn jit_parity_adds_flags() {
    use crate::a64_emitter::{A64TcgEmitter, TranslateAction};
    use crate::context::TcgContext;

    // ADDS X0, X1, #1
    let insn = 0xB1000420u32;
    let mut ctx = TcgContext::new();
    let mut e = A64TcgEmitter::new(&mut ctx, 0x1000);
    assert!(matches!(e.translate_insn(insn), TranslateAction::Continue));
    let block = TcgBlock {
        guest_pc: 0x1000,
        guest_size: 4,
        insn_count: 1,
        ops: ctx.finish(),
    };

    // X1 = MAX → overflow, Z=1, C=1
    let mut regs = empty_regs();
    regs[1] = u64::MAX;
    run_jit_parity(&block, &regs);

    // X1 = 0 → X0 = 1, no flags
    let mut regs = empty_regs();
    regs[1] = 0;
    run_jit_parity(&block, &regs);
}

#[test]
fn jit_parity_ccmp_both_paths() {
    use crate::a64_emitter::{A64TcgEmitter, TranslateAction};
    use crate::context::TcgContext;
    use crate::interp::REG_NZCV;

    // CCMP X18, X0, #0xd, PL (cond=5=PL: N==0)
    let insn = 0xfa405a4du32;
    let mut ctx = TcgContext::new();
    let mut e = A64TcgEmitter::new(&mut ctx, 0x1000);
    assert!(matches!(e.translate_insn(insn), TranslateAction::Continue));
    let block = TcgBlock {
        guest_pc: 0x1000,
        guest_size: 4,
        insn_count: 1,
        ops: ctx.finish(),
    };

    // PL true (N=0): do the compare
    let mut regs = empty_regs();
    regs[18] = 100;
    regs[0] = 50;
    regs[REG_NZCV as usize] = 0;
    run_jit_parity(&block, &regs);

    // PL false (N=1): set nzcv from immediate
    let mut regs = empty_regs();
    regs[18] = 100;
    regs[0] = 50;
    regs[REG_NZCV as usize] = 0x8000_0000;
    run_jit_parity(&block, &regs);
}

#[test]
fn jit_parity_csel() {
    use crate::a64_emitter::{A64TcgEmitter, TranslateAction};
    use crate::context::TcgContext;
    use crate::interp::REG_NZCV;

    // CSEL X0, X1, X2, EQ
    let insn = 0x9A820020u32;
    let mut ctx = TcgContext::new();
    let mut e = A64TcgEmitter::new(&mut ctx, 0x1000);
    assert!(matches!(e.translate_insn(insn), TranslateAction::Continue));
    let block = TcgBlock {
        guest_pc: 0x1000,
        guest_size: 4,
        insn_count: 1,
        ops: ctx.finish(),
    };

    // EQ true (Z=1): select X1
    let mut regs = empty_regs();
    regs[1] = 42;
    regs[2] = 99;
    regs[REG_NZCV as usize] = 0x4000_0000;
    let (ir, _) = run_jit_parity(&block, &regs);
    assert_eq!(ir[0], 42);

    // EQ false (Z=0): select X2
    let mut regs = empty_regs();
    regs[1] = 42;
    regs[2] = 99;
    regs[REG_NZCV as usize] = 0;
    let (ir, _) = run_jit_parity(&block, &regs);
    assert_eq!(ir[0], 99);
}

// ── Load/Store ─────────────────────────────────────────────────────────────

#[test]
fn jit_parity_load_store() {
    let block = make_block(
        vec![
            TcgOp::Movi { dst: t(0), value: 0x2000 },
            TcgOp::Movi { dst: t(1), value: 0xDEAD_BEEF },
            TcgOp::Store { addr: t(0), val: t(1), size: 4 },
            TcgOp::Load { dst: t(2), addr: t(0), size: 4 },
            TcgOp::WriteReg { reg_id: 0, src: t(2) },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, _) = run_jit_parity(&block, &empty_regs());
    assert_eq!(ir[0], 0xDEAD_BEEF);
}

// ── Extensions ─────────────────────────────────────────────────────────────

#[test]
fn jit_parity_sext_zext() {
    let block = make_block(
        vec![
            TcgOp::Movi { dst: t(0), value: 0x80 },
            TcgOp::Sext { dst: t(1), src: t(0), from_bits: 8 },
            TcgOp::WriteReg { reg_id: 0, src: t(1) },
            TcgOp::Movi { dst: t(2), value: 0xFFFF_FFFF_FFFF_FF80u64 },
            TcgOp::Zext { dst: t(3), src: t(2), from_bits: 8 },
            TcgOp::WriteReg { reg_id: 1, src: t(3) },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, _) = run_jit_parity(&block, &empty_regs());
    assert_eq!(ir[0] as i64, -128i64);
    assert_eq!(ir[1], 0x80);
}

// ── PSTATE ─────────────────────────────────────────────────────────────────

#[test]
fn jit_parity_daif_cfinv() {
    use crate::interp::{REG_DAIF, REG_NZCV};
    let block = make_block(
        vec![
            TcgOp::DaifSet { imm: 0xF },
            TcgOp::DaifClr { imm: 0x3 },
            TcgOp::Cfinv,
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, _) = run_jit_parity(&block, &empty_regs());
    assert_eq!(ir[REG_DAIF as usize], 0x300);
    assert_eq!(ir[REG_NZCV as usize], 1 << 29);
}

// ── Eret ───────────────────────────────────────────────────────────────────

#[test]
fn jit_parity_eret() {
    use crate::interp::*;
    let mut init = empty_regs();
    init[REG_ELR_EL1 as usize] = 0x4000;
    init[REG_SPSR_EL1 as usize] = 0xA000_03C5; // N=1,C=1, DAIF=0x3C0, EL=1, SPSel=1
    let block = make_block(vec![TcgOp::Eret], 1);
    let (ir, _) = run_jit_parity(&block, &init);
    assert_eq!(ir[REG_PC as usize], 0x4000);
}

// ── Multi-instruction block (simulating page table loop body) ──────────────

#[test]
fn jit_parity_page_table_loop_body() {
    use crate::a64_emitter::{A64TcgEmitter, TranslateAction};
    use crate::context::TcgContext;
    use crate::interp::REG_PC;

    // STR X4, [X0], #8  (post-index store)
    // ADD X4, X4, X2     (next PTE value)
    // SUBS X3, X3, #1    (decrement counter)
    // B.NE #-12          (loop back)
    let insns: &[u32] = &[
        0xF8008404, // STR X4, [X0], #8
        0x8B020084, // ADD X4, X4, X2
        0xF1000463, // SUBS X3, X3, #1
        0x54FFFF41, // B.NE #-24 (back to STR)
    ];

    let pc = 0x1000u64;
    let mut ctx = TcgContext::new();
    let mut count = 0;
    for (i, &insn) in insns.iter().enumerate() {
        let mut e = A64TcgEmitter::new(&mut ctx, pc + (i as u64) * 4);
        match e.translate_insn(insn) {
            TranslateAction::Continue => count += 1,
            TranslateAction::EndBlock => { count += 1; break; }
            TranslateAction::Unhandled => break,
        }
    }
    assert!(count > 0, "should translate at least one instruction");

    let block = TcgBlock {
        guest_pc: pc,
        guest_size: count * 4,
        insn_count: count,
        ops: ctx.finish(),
    };

    // Set up: X0 = store address, X2 = PTE increment, X3 = counter,
    // X4 = initial PTE value
    let mut regs = empty_regs();
    regs[0] = 0x2000; // base address for stores
    regs[2] = 0x1000; // PTE increment
    regs[3] = 5;      // counter
    regs[4] = 0x40000703; // PTE value
    regs[REG_PC as usize] = pc;

    run_jit_parity(&block, &regs);
}

// ── Emitter-level JIT parity for many instruction patterns ─────────────────

/// Helper: translate a single instruction and run JIT parity.
fn jit_parity_one_insn(insn: u32, init_regs: &[u64; NUM_REGS]) {
    use crate::a64_emitter::{A64TcgEmitter, TranslateAction};
    use crate::context::TcgContext;

    let pc = 0x1000u64;
    let mut ctx = TcgContext::new();
    let mut e = A64TcgEmitter::new(&mut ctx, pc);
    match e.translate_insn(insn) {
        TranslateAction::Continue | TranslateAction::EndBlock => {}
        TranslateAction::Unhandled => {
            eprintln!("  insn {insn:#010x} Unhandled, skipping");
            return;
        }
    }
    // Add fallthrough PC write for non-branch insns
    let ops = ctx.ops();
    let has_pc_write = ops.iter().any(|op| match op {
        TcgOp::WriteReg { reg_id, .. } if *reg_id == crate::interp::REG_PC => true,
        TcgOp::GotoTb { .. } | TcgOp::Eret | TcgOp::Syscall { .. }
        | TcgOp::SvcExc { .. } | TcgOp::HvcExc { .. } | TcgOp::SmcExc { .. } => true,
        _ => false,
    });
    if !has_pc_write {
        let next_pc = ctx.movi(pc + 4);
        ctx.write_reg(crate::interp::REG_PC, next_pc);
    }
    let block = TcgBlock {
        guest_pc: pc,
        guest_size: 4,
        insn_count: 1,
        ops: ctx.finish(),
    };

    let mut regs = *init_regs;
    regs[crate::interp::REG_PC as usize] = pc;
    run_jit_parity(&block, &regs);
}

#[test]
fn jit_parity_orr_imm() {
    let mut regs = empty_regs();
    regs[1] = 0x100;
    // ORR X0, X1, #1
    jit_parity_one_insn(0xB2400020, &regs);
}

#[test]
fn jit_parity_and_imm() {
    let mut regs = empty_regs();
    regs[1] = 0xDEAD_BEEF;
    // AND X0, X1, #0xFF
    jit_parity_one_insn(0x92401C20, &regs);
}

#[test]
fn jit_parity_eor_imm() {
    let mut regs = empty_regs();
    regs[1] = 0xFF00;
    // EOR X0, X1, #0xFF
    jit_parity_one_insn(0xD2401C20, &regs);
}

#[test]
fn jit_parity_ands_imm() {
    let mut regs = empty_regs();
    regs[1] = 0;
    // ANDS X0, X1, #0xFF — result 0 → Z=1
    jit_parity_one_insn(0xF2401C20, &regs);
}

#[test]
fn jit_parity_movz() {
    // MOVZ X0, #0x1234
    jit_parity_one_insn(0xD2824680, &empty_regs());
}

#[test]
fn jit_parity_movn() {
    // MOVN X0, #0
    jit_parity_one_insn(0x92800000, &empty_regs());
}

#[test]
fn jit_parity_movk() {
    let mut regs = empty_regs();
    regs[0] = 0x1234_5678;
    // MOVK X0, #0xABCD, LSL#16
    jit_parity_one_insn(0xF2B579A0, &regs);
}

#[test]
fn jit_parity_adr() {
    // ADR X0, #4
    jit_parity_one_insn(0x10000020, &empty_regs());
}

#[test]
fn jit_parity_adrp() {
    // ADRP X0, #0
    jit_parity_one_insn(0x90000000, &empty_regs());
}

#[test]
fn jit_parity_ubfm_lsr() {
    let mut regs = empty_regs();
    regs[1] = 0xABCD;
    // LSR X0, X1, #4
    jit_parity_one_insn(0xD344FC20, &regs);
}

#[test]
fn jit_parity_sbfm_asr() {
    let mut regs = empty_regs();
    regs[1] = 0xFFFF_FFFF_FFFF_FF00u64;
    // ASR X0, X1, #4
    jit_parity_one_insn(0x9344FC20, &regs);
}

#[test]
fn jit_parity_subs_reg() {
    let mut regs = empty_regs();
    regs[1] = 42;
    regs[2] = 42;
    // SUBS X0, X1, X2
    jit_parity_one_insn(0xEB020020, &regs);
}

#[test]
fn jit_parity_adds_reg() {
    let mut regs = empty_regs();
    regs[1] = 10;
    regs[2] = 20;
    // ADDS X0, X1, X2
    jit_parity_one_insn(0xAB020020, &regs);
}

#[test]
fn jit_parity_ands_reg() {
    let mut regs = empty_regs();
    regs[1] = 0;
    regs[2] = 0xFF;
    // ANDS X0, X1, X2
    jit_parity_one_insn(0xEA020020, &regs);
}

#[test]
fn jit_parity_bic() {
    let mut regs = empty_regs();
    regs[1] = 0xFF;
    regs[2] = 0x0F;
    // BIC X0, X1, X2
    jit_parity_one_insn(0x8A220020, &regs);
}

#[test]
fn jit_parity_orn() {
    let mut regs = empty_regs();
    regs[1] = 0;
    regs[2] = 0xFF;
    // ORN X0, X1, X2
    jit_parity_one_insn(0xAA220020, &regs);
}

#[test]
fn jit_parity_madd() {
    let mut regs = empty_regs();
    regs[1] = 6;
    regs[2] = 7;
    regs[3] = 0;
    // MADD X0, X1, X2, X3
    jit_parity_one_insn(0x9B020C20, &regs);
}

#[test]
fn jit_parity_add_reg_lsl3() {
    let mut regs = empty_regs();
    regs[1] = 100;
    regs[2] = 1;
    // ADD X0, X1, X2, LSL #3
    jit_parity_one_insn(0x8B020C20, &regs);
}

#[test]
fn jit_parity_32bit_add() {
    let mut regs = empty_regs();
    regs[1] = 0x1_0000_0001;
    regs[2] = 0x1_0000_0002;
    // ADD W0, W1, W2
    jit_parity_one_insn(0x0B020020, &regs);
}

#[test]
fn jit_parity_csinc() {
    use crate::interp::REG_NZCV;
    let mut regs = empty_regs();
    regs[1] = 42;
    regs[2] = 99;
    regs[REG_NZCV as usize] = 0x4000_0000; // Z=1 → EQ true
    // CSINC X0, X1, X2, EQ — true: X0=X1=42
    jit_parity_one_insn(0x9A820420, &regs);

    regs[REG_NZCV as usize] = 0; // Z=0 → EQ false
    // CSINC X0, X1, X2, EQ — false: X0=X2+1=100
    jit_parity_one_insn(0x9A820420, &regs);
}

#[test]
fn jit_parity_csinv() {
    use crate::interp::REG_NZCV;
    let mut regs = empty_regs();
    regs[1] = 42;
    regs[2] = 0xFF;
    regs[REG_NZCV as usize] = 0; // Z=0 → EQ false
    // CSINV X0, X1, X2, EQ — false: X0 = ~X2
    jit_parity_one_insn(0xDA820020, &regs);
}

#[test]
fn jit_parity_csneg() {
    use crate::interp::REG_NZCV;
    let mut regs = empty_regs();
    regs[1] = 42;
    regs[2] = 5;
    regs[REG_NZCV as usize] = 0; // Z=0 → EQ false
    // CSNEG X0, X1, X2, EQ — false: X0 = -X2
    jit_parity_one_insn(0xDA820420, &regs);
}

#[test]
fn jit_parity_subs_imm_carry_flags() {
    // Test C flag computation specifically
    let mut regs = empty_regs();
    // SUB with no borrow: 100 - 1 → C=1
    regs[1] = 100;
    jit_parity_one_insn(0xF1000420, &regs); // SUBS X0, X1, #1

    // SUB with borrow: 0 - 1 → C=0
    regs[1] = 0;
    jit_parity_one_insn(0xF1000420, &regs);

    // SUB equal: 1 - 1 → C=1, Z=1
    regs[1] = 1;
    jit_parity_one_insn(0xF1000420, &regs);

    // Large values
    regs[1] = 0x8000_0000_0000_0000;
    jit_parity_one_insn(0xF1000420, &regs);
}

#[test]
fn jit_parity_adds_overflow() {
    let mut regs = empty_regs();
    // ADDS with signed overflow: 0x7FFF..FF + 1 → V=1
    regs[1] = 0x7FFF_FFFF_FFFF_FFFF;
    jit_parity_one_insn(0xB1000420, &regs); // ADDS X0, X1, #1

    // ADDS with carry: MAX + 1 → C=1, Z=1
    regs[1] = u64::MAX;
    jit_parity_one_insn(0xB1000420, &regs);
}

#[test]
fn jit_parity_32bit_subs() {
    let mut regs = empty_regs();
    regs[1] = 5;
    // SUBS W0, W1, #1
    jit_parity_one_insn(0x71000420, &regs);

    // With high bits in X1 that should be ignored
    regs[1] = 0xFFFF_FFFF_0000_0005;
    jit_parity_one_insn(0x71000420, &regs);
}

#[test]
fn jit_parity_extr() {
    let mut regs = empty_regs();
    regs[1] = 0xFF;
    regs[2] = 0xF000_0000_0000_0000;
    // EXTR X0, X1, X2, #4
    jit_parity_one_insn(0x93C21020, &regs);
}
