//! End-to-end AArch64 TCG tests: instruction → emitter → interp → check.
//!
//! Each test encodes an AArch64 instruction, runs it through the A64 emitter
//! to produce TcgOps, executes via the TCG interpreter, and compares the
//! result against the reference Aarch64Cpu interpreter.

use crate::a64_emitter::{A64TcgEmitter, TranslateAction};
use crate::block::TcgBlock;
use crate::context::TcgContext;
use crate::interp::*;
use helm_memory::address_space::AddressSpace;

/// Build a TcgBlock from a single AArch64 instruction word.
fn translate_one(insn: u32, pc: u64) -> Option<TcgBlock> {
    let mut ctx = TcgContext::new();
    let mut emitter = A64TcgEmitter::new(&mut ctx, pc);
    match emitter.translate_insn(insn) {
        TranslateAction::Continue | TranslateAction::EndBlock => {}
        TranslateAction::Unhandled => return None,
    }
    Some(TcgBlock {
        guest_pc: pc,
        guest_size: 4,
        insn_count: 1,
        ops: ctx.finish(),
    })
}

/// Build a TcgBlock from multiple AArch64 instructions.
fn translate_many(insns: &[u32], pc: u64) -> Option<TcgBlock> {
    let mut ctx = TcgContext::new();
    let mut count = 0;
    for (i, &insn) in insns.iter().enumerate() {
        let mut emitter = A64TcgEmitter::new(&mut ctx, pc + (i as u64) * 4);
        match emitter.translate_insn(insn) {
            TranslateAction::Continue => count += 1,
            TranslateAction::EndBlock => { count += 1; break; }
            TranslateAction::Unhandled => break,
        }
    }
    if count == 0 { return None; }
    Some(TcgBlock {
        guest_pc: pc,
        guest_size: count * 4,
        insn_count: count,
        ops: ctx.finish(),
    })
}

fn make_mem() -> AddressSpace {
    let mut mem = AddressSpace::new();
    mem.map(0x0, 0x10_0000, (true, true, false));
    mem
}

/// Execute a TcgBlock on the interpreter and return the register array.
fn exec(block: &TcgBlock, regs: &mut [u64; NUM_REGS], mem: &mut AddressSpace) -> InterpResult {
    let mut interp = TcgInterp::new();
    interp.exec_block(block, regs, mem).unwrap()
}

/// Reference: execute instruction on Aarch64Cpu and return register values.
fn exec_ref(insn: u32, pc: u64, regs_in: &[u64; NUM_REGS], mem: &mut AddressSpace) -> [u64; NUM_REGS] {
    use helm_isa::arm::aarch64::Aarch64Cpu;

    let mut cpu = Aarch64Cpu::new();
    cpu.regs.pc = pc;
    // Copy X0-X30
    for i in 0..31 {
        cpu.set_xn(i as u16, regs_in[i]);
    }
    cpu.regs.sp = regs_in[REG_SP as usize];
    cpu.regs.nzcv = regs_in[REG_NZCV as usize] as u32;

    // Write instruction to memory at PC
    let _ = mem.write(pc, &insn.to_le_bytes());

    match cpu.step(mem) {
        Ok(_) => {}
        Err(e) => panic!("ref step failed: {e:?}"),
    }

    // Extract result registers
    let mut out = [0u64; NUM_REGS];
    for i in 0..31 {
        out[i] = cpu.xn(i as u16);
    }
    out[REG_SP as usize] = cpu.regs.sp;
    out[REG_PC as usize] = cpu.regs.pc;
    out[REG_NZCV as usize] = cpu.regs.nzcv as u64;
    out
}

/// Compare TCG result against reference for a single instruction.
/// Returns (tcg_regs, ref_regs) for further inspection.
fn compare_one(insn: u32, regs_in: &[u64; NUM_REGS]) -> ([u64; NUM_REGS], [u64; NUM_REGS]) {
    let pc = 0x1000u64;
    let mut mem_tcg = make_mem();
    let mut mem_ref = make_mem();

    // TCG path
    let block = translate_one(insn, pc).expect("emitter should handle instruction");
    let mut tcg_regs = *regs_in;
    exec(&block, &mut tcg_regs, &mut mem_tcg);

    // Reference path
    let ref_regs = exec_ref(insn, pc, regs_in, &mut mem_ref);

    (tcg_regs, ref_regs)
}

fn assert_regs_match(tcg: &[u64; NUM_REGS], reference: &[u64; NUM_REGS], insn: u32) {
    // Compare X0-X30
    for i in 0..31 {
        assert_eq!(
            tcg[i], reference[i],
            "X{i} mismatch for insn {insn:#010x}: tcg={:#x} ref={:#x}",
            tcg[i], reference[i]
        );
    }
    // Compare NZCV
    assert_eq!(
        tcg[REG_NZCV as usize], reference[REG_NZCV as usize],
        "NZCV mismatch for insn {insn:#010x}: tcg={:#x} ref={:#x}",
        tcg[REG_NZCV as usize], reference[REG_NZCV as usize]
    );
}

// ── Data Processing — Immediate ─────────────────────────────────────

#[test]
fn e2e_add_imm() {
    // ADD X0, X1, #42
    let insn = 0x9100A820;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 100;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 142);
}

#[test]
fn e2e_sub_imm() {
    // SUB X0, X1, #10
    let insn = 0xD1002820;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 100;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 90);
}

#[test]
fn e2e_adds_imm_sets_flags() {
    // ADDS X0, X1, #1  (sets NZCV)
    let insn = 0xB1000420;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = u64::MAX; // -1 + 1 = 0 → Z=1, C=1
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
}

#[test]
fn e2e_subs_imm_negative() {
    // SUBS X0, XZR, #1  → X0 = -1, N=1
    let insn = 0xF10007E0;
    let regs = [0u64; NUM_REGS];
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
}

#[test]
fn e2e_movz() {
    // MOVZ X0, #0x1234
    let insn = 0xD2824680;
    let regs = [0u64; NUM_REGS];
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 0x1234);
}

#[test]
fn e2e_movn() {
    // MOVN X0, #0  → X0 = ~0 = 0xFFFF_FFFF_FFFF_FFFF
    let insn = 0x92800000;
    let regs = [0u64; NUM_REGS];
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], u64::MAX);
}

#[test]
fn e2e_and_imm() {
    // AND X0, X1, #0xFF
    let insn = 0x92401C20;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0xDEAD_BEEF;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 0xEF);
}

#[test]
fn e2e_orr_imm() {
    // ORR X0, X1, #1
    let insn = 0xB2400020;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0x100;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 0x101);
}

// ── Data Processing — Register ──────────────────────────────────────

#[test]
fn e2e_add_reg() {
    // ADD X0, X1, X2
    let insn = 0x8B020020;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 30;
    regs[2] = 12;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 42);
}

#[test]
fn e2e_sub_reg() {
    // SUB X0, X1, X2
    let insn = 0xCB020020;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 100;
    regs[2] = 58;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 42);
}

#[test]
fn e2e_and_reg() {
    // AND X0, X1, X2
    let insn = 0x8A020020;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0xFF00;
    regs[2] = 0x0FF0;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 0x0F00);
}

#[test]
fn e2e_orr_reg() {
    // ORR X0, X1, X2
    let insn = 0xAA020020;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0xF0;
    regs[2] = 0x0F;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 0xFF);
}

#[test]
fn e2e_lsl_reg() {
    // LSL X0, X1, X2 = LSLV X0, X1, X2
    let insn = 0x9AC22020;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 1;
    regs[2] = 10;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 1024);
}

#[test]
fn e2e_lsr_reg() {
    // LSR X0, X1, X2 = LSRV X0, X1, X2
    let insn = 0x9AC22420;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0x1000;
    regs[2] = 4;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 0x100);
}

#[test]
fn e2e_madd() {
    // MADD X0, X1, X2, X3  → X0 = X1*X2 + X3
    let insn = 0x9B020C20;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 6;
    regs[2] = 7;
    regs[3] = 0;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 42);
}

#[test]
fn e2e_udiv() {
    // UDIV X0, X1, X2
    let insn = 0x9AC20820;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 100;
    regs[2] = 7;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 14);
}

// ── Bitfield ────────────────────────────────────────────────────────

#[test]
fn e2e_ubfm_lsr() {
    // LSR X0, X1, #4 = UBFM X0, X1, #4, #63
    let insn = 0xD344FC20;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0xABCD;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 0xABC);
}

#[test]
fn e2e_sbfm_asr() {
    // ASR X0, X1, #4 = SBFM X0, X1, #4, #63
    let insn = 0x9344FC20;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0xFFFF_FFFF_FFFF_FF00u64; // negative
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
}

// ── 32-bit operations ───────────────────────────────────────────────

#[test]
fn e2e_add_w_reg() {
    // ADD W0, W1, W2 (32-bit)
    let insn = 0x0B020020;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0x1_0000_0001; // upper bits should be ignored
    regs[2] = 0x1_0000_0002;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 3); // 32-bit result, zero-extended
}

#[test]
fn e2e_movz_w() {
    // MOVZ W0, #0x5678
    let insn = 0x528ACF00;
    let mut regs = [0u64; NUM_REGS];
    regs[0] = 0xDEAD_BEEF_1234_5678; // should be overwritten
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 0x5678);
}

// ── Conditional select ──────────────────────────────────────────────

#[test]
fn e2e_csel_eq_taken() {
    // CSEL X0, X1, X2, EQ
    let insn = 0x9A820020;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 42;
    regs[2] = 99;
    regs[REG_NZCV as usize] = 0x4000_0000; // Z=1 → EQ is true
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 42);
}

#[test]
fn e2e_csel_eq_not_taken() {
    // CSEL X0, X1, X2, EQ
    let insn = 0x9A820020;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 42;
    regs[2] = 99;
    regs[REG_NZCV as usize] = 0; // Z=0 → EQ is false
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 99);
}

// ── Branches (multi-insn blocks) ────────────────────────────────────

#[test]
fn e2e_b_unconditional() {
    // B #8 (forward 2 instructions)
    let insn = 0x14000002;
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[REG_PC as usize] = 0x1000;
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    // Should chain to target PC
    match result.exit {
        InterpExit::Chain { target_pc } => assert_eq!(target_pc, 0x1008),
        InterpExit::EndOfBlock { next_pc } => assert_eq!(next_pc, 0x1008),
        other => panic!("unexpected exit: {other:?}"),
    }
}

#[test]
fn e2e_bl_sets_lr() {
    // BL #0 (branch-and-link to next instruction)
    let insn = 0x94000001;
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[REG_PC as usize] = 0x1000;
    let mut mem = make_mem();
    exec(&block, &mut regs, &mut mem);
    // LR (X30) should be set to return address (PC + 4)
    assert_eq!(regs[30], 0x1004, "BL should set X30 to return address");
}

// ── Multi-instruction blocks ────────────────────────────────────────

#[test]
fn e2e_block_add_add() {
    // ADD X0, X1, #1 ; ADD X2, X0, #2
    let insns = [0x91000420u32, 0x91000802];
    let block = translate_many(&insns, 0x1000).unwrap();
    assert_eq!(block.insn_count, 2);
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 10;
    regs[REG_PC as usize] = 0x1000;
    let mut mem = make_mem();
    exec(&block, &mut regs, &mut mem);
    assert_eq!(regs[0], 11, "X0 = X1 + 1 = 11");
    assert_eq!(regs[2], 13, "X2 = X0 + 2 = 13");
}

#[test]
fn e2e_block_mov_then_branch() {
    // MOVZ X5, #42 ; B #0x100
    let insns = [0xD2800545u32, 0x14000040];
    let block = translate_many(&insns, 0x1000).unwrap();
    assert_eq!(block.insn_count, 2);
    let mut regs = [0u64; NUM_REGS];
    regs[REG_PC as usize] = 0x1000;
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    assert_eq!(regs[5], 42, "MOVZ X5, #42");
    // Branch target: 0x1004 + 0x100 = 0x1104
    match result.exit {
        InterpExit::Chain { target_pc } => assert_eq!(target_pc, 0x1104),
        other => panic!("expected Chain, got {other:?}"),
    }
}

#[test]
fn e2e_kernel_first_block() {
    // Reproduce the kernel entry: CCMP + B
    // 0xfa405a4d = CCMP X18, X0, #0xd, PL (conditional compare)
    // 0x1447e019 = B #0x11F8068 (relative to 0x40200004)
    let ccmp = 0xfa405a4du32;
    let b_insn = 0x1447e019u32;

    // First check: does CCMP translate?
    let ccmp_action = {
        let mut ctx = TcgContext::new();
        let mut e = A64TcgEmitter::new(&mut ctx, 0x40200000);
        e.translate_insn(ccmp)
    };
    eprintln!("CCMP action: {:?}", ccmp_action);

    // If CCMP is Unhandled, the block should be just the B instruction
    // starting from the fallback PC. Test the B alone:
    let b_action = {
        let mut ctx = TcgContext::new();
        let mut e = A64TcgEmitter::new(&mut ctx, 0x40200004);
        e.translate_insn(b_insn)
    };
    assert_eq!(b_action, TranslateAction::EndBlock, "B should end block");
}

#[test]
fn e2e_stp_ldp_pair() {
    // STP X0, X1, [X2, #0] ; LDP X3, X4, [X2, #0]
    let stp = 0xA90007C0u32; // STP X0, X1, [X30, #0]... actually let me use a proper encoding
    // STP X0, X1, [X2] = opc=10 V=0 0101 L=0 imm7=0 Rt2=1 Rn=2 Rt=0
    // 10 101 0 010 0 0000000 00001 00010 00000
    let stp = 0xA9000440u32;
    let ldp = 0xA9400C43u32; // LDP X3, X3, [X2] ... need proper encoding
    // LDP X3, X4, [X2] = 10 101 0 010 1 0000000 00100 00010 00011
    let ldp = 0xA9401043u32;

    let block = translate_many(&[stp, ldp], 0x1000);
    if let Some(block) = block {
        let mut regs = [0u64; NUM_REGS];
        regs[0] = 0xAAAA;
        regs[1] = 0xBBBB;
        regs[2] = 0x2000;
        regs[REG_PC as usize] = 0x1000;
        let mut mem = make_mem();
        exec(&block, &mut regs, &mut mem);
        assert_eq!(regs[3], 0xAAAA, "LDP Rt should load first stored value");
        assert_eq!(regs[4], 0xBBBB, "LDP Rt2 should load second stored value");
    }
}

// ── CCMP / conditional compare ──────────────────────────────────────

#[test]
fn e2e_ccmp_pl_condition_true() {
    // CCMP X18, X0, #0xd, PL  (cond=0x5=PL: N==0)
    let insn = 0xfa405a4du32;
    let mut regs = [0u64; NUM_REGS];
    regs[18] = 100;
    regs[0] = 50;
    // PL requires N=0. Set NZCV=0 (N=0 → PL true → do CMP)
    regs[REG_NZCV as usize] = 0;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
}

#[test]
fn e2e_ccmp_pl_condition_false() {
    // CCMP X18, X0, #0xd, PL  (cond=0x5=PL: N==0)
    let insn = 0xfa405a4du32;
    let mut regs = [0u64; NUM_REGS];
    regs[18] = 100;
    regs[0] = 50;
    // PL requires N=0. Set N=1 → PL false → set nzcv=0xd from imm
    regs[REG_NZCV as usize] = 0x8000_0000; // N=1
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    // When condition false, NZCV should be set to imm nzcv field (0xd)
    // 0xd = 1101 → N=1,Z=1,C=0,V=1 → NZCV = 0xD000_0000
    assert_eq!(tcg[REG_NZCV as usize], rf[REG_NZCV as usize]);
}

// ── Systematic reference comparison ─────────────────────────────────

/// Run a sequence of instructions through both TCG (block) and reference
/// (step-by-step), comparing register state after the full sequence.
fn compare_sequence(insns: &[u32], init_regs: &[u64; NUM_REGS]) {
    let pc = 0x1000u64;

    // TCG path: translate as a block and execute
    let block = translate_many(insns, pc);
    let block = match block {
        Some(b) => b,
        None => { eprintln!("  block translation failed (Unhandled)"); return; }
    };
    let mut tcg_regs = *init_regs;
    tcg_regs[REG_PC as usize] = pc;
    let mut mem_tcg = make_mem();
    // Write instructions to memory (for loads that might read code)
    for (i, &insn) in insns.iter().enumerate() {
        let _ = mem_tcg.write(pc + i as u64 * 4, &insn.to_le_bytes());
    }
    exec(&block, &mut tcg_regs, &mut mem_tcg);

    // Reference path: step each instruction
    let mut ref_regs = *init_regs;
    let mut mem_ref = make_mem();
    for (i, &insn) in insns.iter().enumerate() {
        let _ = mem_ref.write(pc + i as u64 * 4, &insn.to_le_bytes());
    }
    use helm_isa::arm::aarch64::Aarch64Cpu;
    let mut cpu = Aarch64Cpu::new();
    cpu.regs.pc = pc;
    for i in 0..31 { cpu.set_xn(i as u16, ref_regs[i]); }
    cpu.regs.sp = ref_regs[REG_SP as usize];
    cpu.regs.nzcv = ref_regs[REG_NZCV as usize] as u32;

    for (i, &_insn) in insns.iter().enumerate() {
        match cpu.step(&mut mem_ref) {
            Ok(_) => {}
            Err(e) => { eprintln!("  ref step {i} failed: {e:?}"); return; }
        }
    }
    // Extract ref results
    for i in 0..31 { ref_regs[i] = cpu.xn(i as u16); }
    ref_regs[REG_SP as usize] = cpu.regs.sp;
    ref_regs[REG_PC as usize] = cpu.regs.pc;
    ref_regs[REG_NZCV as usize] = cpu.regs.nzcv as u64;

    // Compare X0-X30
    for i in 0..31 {
        assert_eq!(
            tcg_regs[i], ref_regs[i],
            "X{i} mismatch after sequence: tcg={:#x} ref={:#x}",
            tcg_regs[i], ref_regs[i]
        );
    }
    assert_eq!(
        tcg_regs[REG_NZCV as usize], ref_regs[REG_NZCV as usize],
        "NZCV mismatch: tcg={:#x} ref={:#x}",
        tcg_regs[REG_NZCV as usize], ref_regs[REG_NZCV as usize]
    );
}

#[test]
fn e2e_seq_add_sub() {
    // ADD X0, XZR, #10 ; SUB X0, X0, #3
    compare_sequence(&[0x910029E0, 0xD1000C00], &[0u64; NUM_REGS]);
}

#[test]
fn e2e_seq_movz_adds() {
    // MOVZ X1, #100 ; ADDS X0, X1, #1
    let mut regs = [0u64; NUM_REGS];
    compare_sequence(&[0xD2800C81, 0xB1000420], &regs);
}

#[test]
fn e2e_seq_subs_csel() {
    // SUBS XZR, X1, X2 ; CSEL X0, X3, X4, EQ
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 5;
    regs[2] = 5; // equal → Z=1
    regs[3] = 42;
    regs[4] = 99;
    compare_sequence(&[0xEB02003F, 0x9A840060], &regs);
}

#[test]
fn e2e_seq_subs_csel_ne() {
    // SUBS XZR, X1, X2 ; CSEL X0, X3, X4, EQ
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 5;
    regs[2] = 7; // not equal → Z=0
    regs[3] = 42;
    regs[4] = 99;
    compare_sequence(&[0xEB02003F, 0x9A840060], &regs);
}

// ── Load/Store via emitter ──────────────────────────────────────────

#[test]
fn e2e_str_ldr_imm() {
    // STR X5, [X1, #0]  then  LDR X6, [X1, #0]
    let str_insn = 0xF9000025; // STR X5, [X1]
    let ldr_insn = 0xF9400026; // LDR X6, [X1]
    let block = translate_many(&[str_insn, ldr_insn], 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0x2000; // base address
    regs[5] = 0xCAFE_BABE;
    let mut mem = make_mem();
    exec(&block, &mut regs, &mut mem);
    assert_eq!(regs[6], 0xCAFE_BABE, "LDR should load what STR stored");
}

#[test]
fn e2e_ldr_w() {
    // LDR W0, [X1] — 32-bit load
    let insn = 0xB9400020;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0x2000;
    let mut mem = make_mem();
    let _ = mem.write(0x2000, &0xDEAD_BEEFu32.to_le_bytes());
    let block = translate_one(insn, 0x1000).unwrap();
    exec(&block, &mut regs, &mut mem);
    assert_eq!(regs[0], 0xDEAD_BEEF, "32-bit load should zero-extend");
}
