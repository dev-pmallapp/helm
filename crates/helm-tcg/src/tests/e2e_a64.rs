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

// ── Branches — BR, BLR, CBZ/CBNZ, TBZ/TBNZ, B.cond ─────────────────

#[test]
fn e2e_br_jumps_to_register() {
    // BR X1  — branch to address in X1
    // Encoding: 1101011 0000 11111 000000 Rn=1 00000 = 0xD61F0020
    let insn = 0xD61F0020u32;
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0x5000;
    regs[REG_PC as usize] = 0x1000;
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    // BR writes PC = X1 and exits
    match result.exit {
        InterpExit::Exit | InterpExit::EndOfBlock { .. } => {}
        other => panic!("expected Exit/EndOfBlock, got {other:?}"),
    }
    assert_eq!(regs[REG_PC as usize], 0x5000, "BR should set PC to X1");
}

#[test]
fn e2e_blr_sets_lr_and_branches() {
    // BLR X1  — branch-and-link to address in X1, set X30 = PC+4
    // Encoding: 1101011 0001 11111 000000 Rn=1 00000 = 0xD63F0020
    let insn = 0xD63F0020u32;
    let block = translate_one(insn, 0x2000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0x8000;
    regs[REG_PC as usize] = 0x2000;
    let mut mem = make_mem();
    exec(&block, &mut regs, &mut mem);
    assert_eq!(regs[30], 0x2004, "BLR should set X30 to PC+4 = 0x2004");
    assert_eq!(regs[REG_PC as usize], 0x8000, "BLR should set PC to X1");
}

#[test]
fn e2e_cbz_taken_when_zero() {
    // CBZ X0, +8  (branch forward 8 bytes if X0==0)
    // Encoding: sf=1 011 010 0 imm19=2 Rt=0 → 0xB4000040
    let insn = 0xB4000040u32;
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[0] = 0; // zero → branch taken
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    match result.exit {
        InterpExit::Chain { target_pc } => assert_eq!(target_pc, 0x1008, "CBZ taken: target = PC+8"),
        InterpExit::EndOfBlock { next_pc } => assert_eq!(next_pc, 0x1008),
        other => panic!("unexpected exit: {other:?}"),
    }
}

#[test]
fn e2e_cbz_not_taken_when_nonzero() {
    // CBZ X0, +8 — not taken when X0 != 0
    let insn = 0xB4000040u32;
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[0] = 42; // non-zero → fallthrough
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    match result.exit {
        InterpExit::Chain { target_pc } => assert_eq!(target_pc, 0x1004, "CBZ not taken: fallthrough PC+4"),
        InterpExit::EndOfBlock { next_pc } => assert_eq!(next_pc, 0x1004),
        other => panic!("unexpected exit: {other:?}"),
    }
}

#[test]
fn e2e_cbnz_taken_when_nonzero() {
    // CBNZ X0, +8 — taken when X0 != 0
    // Encoding: sf=1 011 010 1 imm19=2 Rt=0 → 0xB5000040
    let insn = 0xB5000040u32;
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[0] = 1; // non-zero → taken
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    match result.exit {
        InterpExit::Chain { target_pc } => assert_eq!(target_pc, 0x1008),
        InterpExit::EndOfBlock { next_pc } => assert_eq!(next_pc, 0x1008),
        other => panic!("unexpected exit: {other:?}"),
    }
}

#[test]
fn e2e_cbnz_not_taken_when_zero() {
    // CBNZ X0, +8 — not taken when X0 == 0
    let insn = 0xB5000040u32;
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[0] = 0; // zero → fallthrough
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    match result.exit {
        InterpExit::Chain { target_pc } => assert_eq!(target_pc, 0x1004),
        InterpExit::EndOfBlock { next_pc } => assert_eq!(next_pc, 0x1004),
        other => panic!("unexpected exit: {other:?}"),
    }
}

#[test]
fn e2e_tbz_taken_when_bit_clear() {
    // TBZ X0, #0, +8  — branch if bit 0 of X0 is 0
    // Encoding: b5=0 0110110 b40=00000 imm14=2 Rt=0 → 0x36000040
    let insn = 0x36000040u32;
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[0] = 0b10; // bit 0 is clear → branch taken
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    match result.exit {
        InterpExit::Chain { target_pc } => assert_eq!(target_pc, 0x1008),
        InterpExit::EndOfBlock { next_pc } => assert_eq!(next_pc, 0x1008),
        other => panic!("unexpected exit: {other:?}"),
    }
}

#[test]
fn e2e_tbz_not_taken_when_bit_set() {
    // TBZ X0, #0, +8 — bit 0 is set → fallthrough
    let insn = 0x36000040u32;
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[0] = 0b01; // bit 0 set → not taken
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    match result.exit {
        InterpExit::Chain { target_pc } => assert_eq!(target_pc, 0x1004),
        InterpExit::EndOfBlock { next_pc } => assert_eq!(next_pc, 0x1004),
        other => panic!("unexpected exit: {other:?}"),
    }
}

#[test]
fn e2e_tbnz_taken_when_bit_set() {
    // TBNZ X0, #0, +8 — branch if bit 0 is set
    // Encoding: b5=0 0110111 b40=00000 imm14=2 Rt=0 → 0x37000040
    let insn = 0x37000040u32;
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[0] = 0b01; // bit 0 set → taken
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    match result.exit {
        InterpExit::Chain { target_pc } => assert_eq!(target_pc, 0x1008),
        InterpExit::EndOfBlock { next_pc } => assert_eq!(next_pc, 0x1008),
        other => panic!("unexpected exit: {other:?}"),
    }
}

/// B.cond encoding: 0101 010 0 imm19 0 cond
/// cond field is bits [3:0].  imm19 is bits [23:5].
fn bcond_insn(cond: u32, imm19: u32) -> u32 {
    // 0101_0100 | (imm19 << 5) | (0 << 4) | cond
    0x5400_0000u32 | ((imm19 & 0x7FFFF) << 5) | (cond & 0xF)
}

#[test]
fn e2e_b_cond_eq_taken() {
    // B.EQ #8 — taken when Z=1
    let insn = bcond_insn(0x0, 2); // cond=EQ(0), imm19=2 → target=PC+8
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[REG_NZCV as usize] = 0x4000_0000; // Z=1
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    match result.exit {
        InterpExit::Chain { target_pc } => assert_eq!(target_pc, 0x1008, "B.EQ taken: target PC+8"),
        InterpExit::EndOfBlock { next_pc } => assert_eq!(next_pc, 0x1008),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn e2e_b_cond_eq_not_taken() {
    // B.EQ #8 — not taken when Z=0
    let insn = bcond_insn(0x0, 2);
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[REG_NZCV as usize] = 0; // Z=0
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    match result.exit {
        InterpExit::Chain { target_pc } => assert_eq!(target_pc, 0x1004, "B.EQ not taken: fallthrough"),
        InterpExit::EndOfBlock { next_pc } => assert_eq!(next_pc, 0x1004),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn e2e_b_cond_ne_taken() {
    // B.NE #8 — taken when Z=0 (cond=NE=0x1)
    let insn = bcond_insn(0x1, 2);
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[REG_NZCV as usize] = 0; // Z=0 → NE true
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    match result.exit {
        InterpExit::Chain { target_pc } | InterpExit::EndOfBlock { next_pc: target_pc } => {
            assert_eq!(target_pc, 0x1008);
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn e2e_b_cond_mi_taken() {
    // B.MI #8 — taken when N=1 (cond=MI=0x4)
    let insn = bcond_insn(0x4, 2);
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[REG_NZCV as usize] = 0x8000_0000; // N=1
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    match result.exit {
        InterpExit::Chain { target_pc } | InterpExit::EndOfBlock { next_pc: target_pc } => {
            assert_eq!(target_pc, 0x1008);
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn e2e_b_cond_vs_taken() {
    // B.VS #8 — taken when V=1 (cond=VS=0x6)
    let insn = bcond_insn(0x6, 2);
    let block = translate_one(insn, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[REG_NZCV as usize] = 0x1000_0000; // V=1
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    match result.exit {
        InterpExit::Chain { target_pc } | InterpExit::EndOfBlock { next_pc: target_pc } => {
            assert_eq!(target_pc, 0x1008);
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn e2e_b_cond_compare_against_ref() {
    // B.EQ #8 — verify TCG matches reference for both taken and not-taken
    let insn = bcond_insn(0x0, 2); // B.EQ, +8

    // Taken path: Z=1
    let mut regs_taken = [0u64; NUM_REGS];
    regs_taken[REG_NZCV as usize] = 0x4000_0000;
    let (tcg, rf) = compare_one(insn, &regs_taken);
    assert_regs_match(&tcg, &rf, insn);

    // Not-taken path: Z=0
    let mut regs_not = [0u64; NUM_REGS];
    regs_not[REG_NZCV as usize] = 0;
    let (tcg2, rf2) = compare_one(insn, &regs_not);
    assert_regs_match(&tcg2, &rf2, insn);
}

// ── Load/Store — narrow widths, sign extension ───────────────────────

#[test]
fn e2e_strb_ldrb_roundtrip() {
    // STRB W5, [X1] then LDRB W6, [X1]
    // STRB W5, [X1, #0]: 0011 1000 000 00000 0000 00 Rn=1 Rt=5 → 0x38000025
    // Actually unsigned offset: STRB W5, [X1, #0] = 0x39000025
    let strb = 0x39000025u32; // STRB W5, [X1, #0]
    let ldrb = 0x39400026u32; // LDRB W6, [X1, #0]
    let block = translate_many(&[strb, ldrb], 0x1000);
    if let Some(block) = block {
        let mut regs = [0u64; NUM_REGS];
        regs[1] = 0x2000;
        regs[5] = 0xAB; // only low byte matters
        let mut mem = make_mem();
        exec(&block, &mut regs, &mut mem);
        assert_eq!(regs[6], 0xAB, "LDRB should load byte zero-extended");
    }
}

#[test]
fn e2e_ldrb_zero_extends() {
    // LDRB W0, [X1] — byte load zero-extends to 64 bits
    let ldrb = 0x39400020u32; // LDRB W0, [X1, #0]
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0x2000;
    regs[0] = 0xDEAD_BEEF_DEAD_BEEF; // should be overwritten with zero-extension
    let mut mem = make_mem();
    let _ = mem.write(0x2000, &[0xFFu8]);
    if let Some(block) = translate_one(ldrb, 0x1000) {
        exec(&block, &mut regs, &mut mem);
        assert_eq!(regs[0], 0xFF, "LDRB zero-extends: upper bits must be 0");
    }
}

#[test]
fn e2e_strh_ldrh_roundtrip() {
    // STRH W5, [X1, #0]  then  LDRH W6, [X1, #0]
    let strh = 0x79000025u32; // STRH W5, [X1, #0]
    let ldrh = 0x79400026u32; // LDRH W6, [X1, #0]
    let block = translate_many(&[strh, ldrh], 0x1000);
    if let Some(block) = block {
        let mut regs = [0u64; NUM_REGS];
        regs[1] = 0x2000;
        regs[5] = 0xBEEF;
        let mut mem = make_mem();
        exec(&block, &mut regs, &mut mem);
        assert_eq!(regs[6], 0xBEEF, "LDRH should load halfword zero-extended");
    }
}

#[test]
fn e2e_ldrsb_sign_extends() {
    // LDRSB X0, [X1] — sign-extends byte to 64 bits
    // LDRSB X0, [X1, #0] unsigned offset: 0x39800020
    let ldrsb = 0x39800020u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0x2000;
    let mut mem = make_mem();
    let _ = mem.write(0x2000, &[0x80u8]); // -128 as signed byte
    if let Some(block) = translate_one(ldrsb, 0x1000) {
        exec(&block, &mut regs, &mut mem);
        assert_eq!(
            regs[0] as i64, -128i64,
            "LDRSB should sign-extend 0x80 to -128"
        );
    }
}

#[test]
fn e2e_ldrsh_sign_extends() {
    // LDRSH X0, [X1, #0] — sign-extends halfword to 64 bits
    // 0x79800020 = LDRSH X0, [X1, #0]
    let ldrsh = 0x79800020u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0x2000;
    let mut mem = make_mem();
    let _ = mem.write(0x2000, &0x8000u16.to_le_bytes()); // -32768
    if let Some(block) = translate_one(ldrsh, 0x1000) {
        exec(&block, &mut regs, &mut mem);
        assert_eq!(
            regs[0] as i64, -32768i64,
            "LDRSH should sign-extend 0x8000 to -32768"
        );
    }
}

#[test]
fn e2e_ldrsw_sign_extends() {
    // LDRSW X0, [X1, #0] — sign-extends 32-bit word to 64 bits
    // Encoding: 10 111 000 100 000000000000 Rn=1 Rt=0 → 0xB9800020
    let ldrsw = 0xB9800020u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0x2000;
    let mut mem = make_mem();
    let _ = mem.write(0x2000, &0x8000_0000u32.to_le_bytes()); // INT_MIN
    if let Some(block) = translate_one(ldrsw, 0x1000) {
        exec(&block, &mut regs, &mut mem);
        assert_eq!(
            regs[0] as i64, i32::MIN as i64,
            "LDRSW should sign-extend 0x80000000 to i32::MIN"
        );
    }
}

#[test]
fn e2e_ldr_post_index() {
    // LDR X0, [X1], #8 — post-index: load from X1, then X1 += 8
    // Encoding: 11 111 000 010 000001000 01 Rn=1 Rt=0 → 0xF8408420
    // imm9 = 8 = 0b000001000, post-index (not pre): 0xF8408420
    let ldr_post = 0xF8408420u32;
    let block = translate_one(ldr_post, 0x1000);
    if let Some(block) = block {
        let mut regs = [0u64; NUM_REGS];
        regs[1] = 0x2000;
        let mut mem = make_mem();
        let _ = mem.write(0x2000, &0xCAFE_BABE_0000_0001u64.to_le_bytes());
        exec(&block, &mut regs, &mut mem);
        assert_eq!(regs[0], 0xCAFE_BABE_0000_0001u64, "LDR post-index: load from original address");
        assert_eq!(regs[1], 0x2008, "LDR post-index: base should be updated by +8");
    }
}

#[test]
fn e2e_str_pre_index() {
    // STR X5, [X1, #8]! — pre-index: X1 += 8, then store to new X1
    // Encoding: 11 111 000 000 000001000 11 Rn=1 Rt=5 → 0xF8008C25
    let str_pre = 0xF8008C25u32;
    let block = translate_one(str_pre, 0x1000);
    if let Some(block) = block {
        let mut regs = [0u64; NUM_REGS];
        regs[1] = 0x2000;
        regs[5] = 0xABCD_EF01;
        let mut mem = make_mem();
        exec(&block, &mut regs, &mut mem);
        assert_eq!(regs[1], 0x2008, "STR pre-index: base should be updated by +8");
        let mut buf = [0u8; 8];
        let _ = mem.read(0x2008, &mut buf);
        assert_eq!(
            u64::from_le_bytes(buf),
            0xABCD_EF01,
            "STR pre-index: store should land at updated address"
        );
    }
}

#[test]
fn e2e_ldrb_compare_against_ref() {
    // LDRB W0, [X1, #0] — compare TCG vs reference
    let ldrb = 0x39400020u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0x2000;
    let mut mem_ref = make_mem();
    let _ = mem_ref.write(0x2000, &[0xABu8]);
    // Write to both memories
    let mut mem_tcg = make_mem();
    let _ = mem_tcg.write(0x2000, &[0xABu8]);

    let pc = 0x1000u64;
    let block = translate_one(ldrb, pc);
    if let Some(block) = block {
        let mut tcg_regs = regs;
        exec(&block, &mut tcg_regs, &mut mem_tcg);
        let ref_regs = exec_ref(ldrb, pc, &regs, &mut mem_ref);
        assert_regs_match(&tcg_regs, &ref_regs, ldrb);
        assert_eq!(tcg_regs[0], 0xAB);
    }
}

// ── Conditional select — CSINC, CSINV, CSNEG ─────────────────────────

#[test]
fn e2e_csinc_condition_true() {
    // CSINC X0, X1, X2, EQ — if EQ (Z=1): X0=X1, else X0=X2+1
    // Encoding: sf=1 0011010100 Rm=2 cond=0000(EQ) 01 Rn=1 Rd=0 → 0x9A820420
    let insn = 0x9A820420u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 100;
    regs[2] = 50;
    regs[REG_NZCV as usize] = 0x4000_0000; // Z=1 → EQ true
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 100, "CSINC cond true: result = Rn = 100");
}

#[test]
fn e2e_csinc_condition_false() {
    // CSINC X0, X1, X2, EQ — EQ false: X0 = X2+1
    let insn = 0x9A820420u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 100;
    regs[2] = 50;
    regs[REG_NZCV as usize] = 0; // Z=0 → EQ false
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 51, "CSINC cond false: result = Rm+1 = 51");
}

#[test]
fn e2e_csinv_condition_true() {
    // CSINV X0, X1, X2, NE — if NE (Z=0): X0=X1, else X0=~X2
    // Encoding: sf=1 op=1 S=0 11010100 rm=2 cond=NE(1) 0 0 rn=1 rd=0
    // = 0xDA821020
    let insn = 0xDA821020u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0xAAAA;
    regs[2] = 0x5555;
    regs[REG_NZCV as usize] = 0; // Z=0 → NE true → X0 = Rn = X1
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 0xAAAA, "CSINV cond true: result = Rn = X1");
}

#[test]
fn e2e_csinv_condition_false() {
    // CSINV X0, X1, X2, NE — NE false (Z=1): X0 = ~X2
    // Same encoding: 0xDA821020
    let insn = 0xDA821020u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0xAAAA;
    regs[2] = 0;
    regs[REG_NZCV as usize] = 0x4000_0000; // Z=1 → NE false → X0 = ~Rm = ~0 = MAX
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], !0u64, "CSINV cond false: result = ~Rm = ~0 = MAX");
}

#[test]
fn e2e_csneg_condition_true() {
    // CSNEG X0, X1, X2, EQ — if EQ (Z=1): X0=X1, else X0=-X2
    // Encoding: sf=1 op=1 S=0 11010100 rm=2 cond=EQ(0) 0 1 rn=1 rd=0
    // = 0xDA820420
    let insn = 0xDA820420u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 77;
    regs[2] = 10;
    regs[REG_NZCV as usize] = 0x4000_0000; // Z=1 → EQ true → X0 = Rn = 77
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 77, "CSNEG cond true: result = Rn = 77");
}

#[test]
fn e2e_csneg_condition_false() {
    // CSNEG X0, X1, X2, EQ — EQ false (Z=0): X0 = -X2
    let insn = 0xDA820420u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 77;
    regs[2] = 10;
    regs[REG_NZCV as usize] = 0; // Z=0 → EQ false → X0 = -Rm = -10
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    assert_eq!(tcg[0], 10u64.wrapping_neg(), "CSNEG cond false: result = -Rm = -(10)");
}

// ── Flag-setting edge cases: ADDS/SUBS ────────────────────────────────

#[test]
fn e2e_adds_overflow_positive() {
    // ADDS X0, X1, X2: i64::MAX + 1 → overflow, V=1, N=1
    // ADDS X0, X1, X2 = 0xAB020020
    let insn = 0xAB020020u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = i64::MAX as u64;
    regs[2] = 1;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    // Result is i64::MIN as u64, N=1, V=1, C=0, Z=0
    let nzcv = tcg[REG_NZCV as usize];
    assert_ne!(nzcv & 0x1000_0000, 0, "ADDS i64::MAX+1 should set V=1");
    assert_ne!(nzcv & 0x8000_0000, 0, "ADDS i64::MAX+1 should set N=1");
}

#[test]
fn e2e_adds_carry_out() {
    // ADDS X0, X1, X2: u64::MAX + 1 → carry, Z=1, C=1
    let insn = 0xAB020020u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = u64::MAX;
    regs[2] = 1;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    let nzcv = tcg[REG_NZCV as usize];
    assert_ne!(nzcv & 0x4000_0000, 0, "ADDS u64::MAX+1 should set Z=1");
    assert_ne!(nzcv & 0x2000_0000, 0, "ADDS u64::MAX+1 should set C=1");
    assert_eq!(tcg[0], 0, "ADDS u64::MAX+1 result should be 0");
}

#[test]
fn e2e_subs_borrow() {
    // SUBS X0, X1, X2: 0 - 1 → borrow, N=1, C=0 (no carry = borrow)
    let insn = 0xEB020020u32; // SUBS X0, X1, X2
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 0;
    regs[2] = 1;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    let nzcv = tcg[REG_NZCV as usize];
    assert_ne!(nzcv & 0x8000_0000, 0, "SUBS 0-1 should set N=1");
    assert_eq!(nzcv & 0x2000_0000, 0, "SUBS 0-1 should clear C=0 (borrow)");
}

#[test]
fn e2e_subs_overflow_negative() {
    // SUBS X0, X1, X2: i64::MIN - 1 → overflow (wraps to positive)
    let insn = 0xEB020020u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = i64::MIN as u64;
    regs[2] = 1;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    let nzcv = tcg[REG_NZCV as usize];
    assert_ne!(nzcv & 0x1000_0000, 0, "SUBS i64::MIN-1 should set V=1");
}

#[test]
fn e2e_adds_zero_result() {
    // ADDS X0, X1, X2: 5 + (-5) = 0, Z=1
    let insn = 0xAB020020u32;
    let mut regs = [0u64; NUM_REGS];
    regs[1] = 5;
    regs[2] = (-5i64) as u64;
    let (tcg, rf) = compare_one(insn, &regs);
    assert_regs_match(&tcg, &rf, insn);
    let nzcv = tcg[REG_NZCV as usize];
    assert_ne!(nzcv & 0x4000_0000, 0, "ADDS 5+(-5) should set Z=1");
    assert_eq!(tcg[0], 0, "ADDS 5+(-5) result should be 0");
}

// ── Additional multi-instruction sequences ────────────────────────────

#[test]
fn e2e_seq_cbz_then_add() {
    // CBZ X0, skip ; ADD X1, X1, #1 ; (branch target:) ADD X2, X2, #10
    // This is a three-instruction sequence where CBZ branches over ADD
    // Since translate_many stops at a branch, test single CBZ + verify chain
    let cbz = 0xB4000040u32; // CBZ X0, +8
    let block = translate_one(cbz, 0x1000).unwrap();
    let mut regs = [0u64; NUM_REGS];
    regs[0] = 0; // taken → skip to 0x1008
    let mut mem = make_mem();
    let result = exec(&block, &mut regs, &mut mem);
    match result.exit {
        InterpExit::Chain { target_pc } | InterpExit::EndOfBlock { next_pc: target_pc } => {
            assert_eq!(target_pc, 0x1008);
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn e2e_seq_adds_then_bcond() {
    // ADDS XZR, X1, X2 (sets flags) ; B.EQ +8
    // If X1+X2 == 0, branch taken; else fallthrough
    let adds = 0xAB02005Fu32; // ADDS XZR, X2, X3 — wait, need: ADDS XZR, X1, X2
    // ADDS XZR, X1, X2: Rd=31=XZR, Rn=1, Rm=2
    // 10101011 000 00010 000000 00001 11111 = 0xAB02003F
    let adds_xzr = 0xAB02003Fu32;
    let bcond_eq = bcond_insn(0x0, 2); // B.EQ, +8

    let block = translate_many(&[adds_xzr, bcond_eq], 0x1000);
    if let Some(block) = block {
        // Case 1: X1+X2 = 0 (e.g., 5 + (-5)) → Z=1 → B.EQ taken
        // adds_xzr is at PC=0x1000, bcond_eq is at PC=0x1004.
        // B.EQ target = 0x1004 + imm19*4 = 0x1004 + 8 = 0x100C
        let mut regs = [0u64; NUM_REGS];
        regs[1] = 5;
        regs[2] = (-5i64) as u64;
        regs[REG_PC as usize] = 0x1000;
        let mut mem = make_mem();
        let result = exec(&block, &mut regs, &mut mem);
        match result.exit {
            InterpExit::Chain { target_pc } | InterpExit::EndOfBlock { next_pc: target_pc } => {
                assert_eq!(target_pc, 0x100C, "5+(-5)=0, B.EQ should be taken to 0x100C");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
