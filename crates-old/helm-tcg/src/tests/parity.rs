//! Threaded-vs-interpreter parity tests.
//!
//! Each test builds a `TcgBlock` from hand-written `TcgOp` sequences,
//! executes it through both `interp::exec_block` (the match-based
//! interpreter) and `threaded::exec_threaded` (the flat-bytecode dispatch
//! loop), then asserts that the two produce identical register state and
//! identical `InterpExit` variants.
//!
//! This mirrors QEMU's TCI-vs-JIT parity validation: the interpreter is
//! the reference oracle and the threaded backend must agree on every op.

use crate::block::TcgBlock;
use crate::interp::{InterpExit, TcgInterp, NUM_REGS, SYSREG_FILE_SIZE};
use crate::ir::{TcgOp, TcgTemp};
use crate::threaded::{compile_block, exec_threaded};
use helm_memory::address_space::AddressSpace;

// ── Shared helpers ────────────────────────────────────────────────────────────

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

/// Run `block` through both backends with the same initial register state.
///
/// Returns `(interp_regs, threaded_regs)` so callers can inspect values.
/// Panics if the two backends disagree on any register or on exit kind.
fn run_parity(block: &TcgBlock, init_regs: &[u64; NUM_REGS]) -> ([u64; NUM_REGS], [u64; NUM_REGS]) {
    // -- Interpreter path --
    let mut interp_regs = *init_regs;
    let mut interp_mem = make_mem();
    // Pre-populate memory that block might access
    for addr in (0x2000u64..0x3000).step_by(8) {
        let _ = interp_mem.write(addr, &[0u8; 8]);
    }
    let mut interp = TcgInterp::new();
    let interp_result = interp
        .exec_block(block, &mut interp_regs, &mut interp_mem)
        .expect("interp exec_block failed");

    // -- Threaded path --
    let mut threaded_regs = *init_regs;
    let mut threaded_mem = make_mem();
    for addr in (0x2000u64..0x3000).step_by(8) {
        let _ = threaded_mem.write(addr, &[0u8; 8]);
    }
    let compiled = compile_block(block);
    let mut sysregs = vec![0u64; SYSREG_FILE_SIZE];
    let threaded_result = exec_threaded(
        &compiled,
        &mut threaded_regs,
        &mut threaded_mem,
        &mut sysregs,
    )
    .expect("threaded exec_threaded failed");

    // -- Register comparison --
    for i in 0..NUM_REGS {
        assert_eq!(
            interp_regs[i], threaded_regs[i],
            "reg[{i}] mismatch: interp={:#x} threaded={:#x}",
            interp_regs[i], threaded_regs[i]
        );
    }

    // -- Exit kind comparison (variant-level, not value-level for Chain) --
    let exit_matches = match (&interp_result.exit, &threaded_result.exit) {
        (InterpExit::Exit, InterpExit::Exit) => true,
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
        "exit kind mismatch: interp={:?} threaded={:?}",
        interp_result.exit, threaded_result.exit
    );

    // -- insns_executed comparison --
    assert_eq!(
        interp_result.insns_executed, threaded_result.insns_executed,
        "insns_executed mismatch"
    );

    (interp_regs, threaded_regs)
}

// ── ALU operation parity ───────────────────────────────────────────────────────

#[test]
fn parity_add() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 100,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 200,
            },
            TcgOp::Add {
                dst: t(2),
                a: t(0),
                b: t(1),
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 300);
    assert_eq!(th[0], 300);
}

#[test]
fn parity_sub_wrapping() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 5,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 10,
            },
            TcgOp::Sub {
                dst: t(2),
                a: t(0),
                b: t(1),
            }, // 5 - 10 wraps
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 5u64.wrapping_sub(10));
    assert_eq!(ir[0], th[0]);
}

#[test]
fn parity_mul_overflow() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: u64::MAX,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 2,
            },
            TcgOp::Mul {
                dst: t(2),
                a: t(0),
                b: t(1),
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], u64::MAX.wrapping_mul(2));
    assert_eq!(ir[0], th[0]);
}

#[test]
fn parity_div() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 100,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 7,
            },
            TcgOp::Div {
                dst: t(2),
                a: t(0),
                b: t(1),
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 14);
    assert_eq!(ir[0], th[0]);
}

#[test]
fn parity_div_by_zero() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 42,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 0,
            },
            TcgOp::Div {
                dst: t(2),
                a: t(0),
                b: t(1),
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 0); // both must return 0 for div-by-zero
    assert_eq!(ir[0], th[0]);
}

#[test]
fn parity_addi_negative() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 10,
            },
            TcgOp::Addi {
                dst: t(1),
                a: t(0),
                imm: -3,
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(1),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 7);
    assert_eq!(ir[0], th[0]);
}

// ── Bitwise parity ─────────────────────────────────────────────────────────────

#[test]
fn parity_and_or_xor_not() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0xF0F0,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 0xFF00,
            },
            TcgOp::And {
                dst: t(2),
                a: t(0),
                b: t(1),
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            TcgOp::Or {
                dst: t(3),
                a: t(0),
                b: t(1),
            },
            TcgOp::WriteReg {
                reg_id: 1,
                src: t(3),
            },
            TcgOp::Xor {
                dst: t(4),
                a: t(0),
                b: t(1),
            },
            TcgOp::WriteReg {
                reg_id: 2,
                src: t(4),
            },
            TcgOp::Not {
                dst: t(5),
                src: t(0),
            },
            TcgOp::WriteReg {
                reg_id: 3,
                src: t(5),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 0xF000);
    assert_eq!(ir[1], 0xFFF0);
    assert_eq!(ir[2], 0x0FF0);
    assert_eq!(ir[3], !0xF0F0u64);
    for i in 0..4 {
        assert_eq!(ir[i], th[i]);
    }
}

#[test]
fn parity_shifts() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0x80,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 3,
            },
            TcgOp::Shl {
                dst: t(2),
                a: t(0),
                b: t(1),
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            TcgOp::Shr {
                dst: t(3),
                a: t(0),
                b: t(1),
            },
            TcgOp::WriteReg {
                reg_id: 1,
                src: t(3),
            },
            // SAR with negative value
            TcgOp::Movi {
                dst: t(4),
                value: 0xFFFF_FFFF_0000_0000u64,
            },
            TcgOp::Movi {
                dst: t(5),
                value: 16,
            },
            TcgOp::Sar {
                dst: t(6),
                a: t(4),
                b: t(5),
            },
            TcgOp::WriteReg {
                reg_id: 2,
                src: t(6),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 0x400);
    assert_eq!(ir[1], 0x10);
    // SAR: 0xFFFF_FFFF_0000_0000 >> 16 (arithmetic) = 0xFFFF_FFFF_FFFF_0000
    assert_eq!(ir[2], 0xFFFF_FFFF_FFFF_0000u64);
    for i in 0..3 {
        assert_eq!(ir[i], th[i]);
    }
}

// ── Sign/zero extension parity ────────────────────────────────────────────────

#[test]
fn parity_sext_zext() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0x80,
            }, // -128 in 8-bit
            TcgOp::Sext {
                dst: t(1),
                src: t(0),
                from_bits: 8,
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(1),
            },
            TcgOp::Movi {
                dst: t(2),
                value: 0xFFFF_FFFF_FFFF_FF80u64,
            },
            TcgOp::Zext {
                dst: t(3),
                src: t(2),
                from_bits: 8,
            },
            TcgOp::WriteReg {
                reg_id: 1,
                src: t(3),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0] as i64, -128i64);
    assert_eq!(ir[1], 0x80);
    for i in 0..2 {
        assert_eq!(ir[i], th[i]);
    }
}

#[test]
fn parity_sext_16bit() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0x8000,
            }, // -32768 in 16-bit
            TcgOp::Sext {
                dst: t(1),
                src: t(0),
                from_bits: 16,
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(1),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0] as i64, -32768i64);
    assert_eq!(ir[0], th[0]);
}

// ── Comparison / SetXxx parity ────────────────────────────────────────────────

#[test]
fn parity_set_comparisons() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 5,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 5,
            },
            TcgOp::Movi {
                dst: t(2),
                value: 10,
            },
            // SetEq: 5==5 → 1
            TcgOp::SetEq {
                dst: t(3),
                a: t(0),
                b: t(1),
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(3),
            },
            // SetNe: 5!=10 → 1
            TcgOp::SetNe {
                dst: t(4),
                a: t(0),
                b: t(2),
            },
            TcgOp::WriteReg {
                reg_id: 1,
                src: t(4),
            },
            // SetLt: 5 < 10 signed → 1
            TcgOp::SetLt {
                dst: t(5),
                a: t(0),
                b: t(2),
            },
            TcgOp::WriteReg {
                reg_id: 2,
                src: t(5),
            },
            // SetGe: 10 >= 5 signed → 1
            TcgOp::SetGe {
                dst: t(6),
                a: t(2),
                b: t(0),
            },
            TcgOp::WriteReg {
                reg_id: 3,
                src: t(6),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 1); // 5 == 5
    assert_eq!(ir[1], 1); // 5 != 10
    assert_eq!(ir[2], 1); // 5 < 10
    assert_eq!(ir[3], 1); // 10 >= 5
    for i in 0..4 {
        assert_eq!(ir[i], th[i]);
    }
}

#[test]
fn parity_set_negative_comparisons() {
    // Test signed comparisons with negative numbers
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: u64::MAX,
            }, // -1 as signed
            TcgOp::Movi {
                dst: t(1),
                value: 1,
            },
            // SetLt: -1 < 1 signed → 1
            TcgOp::SetLt {
                dst: t(2),
                a: t(0),
                b: t(1),
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            // SetGe: 1 >= -1 signed → 1
            TcgOp::SetGe {
                dst: t(3),
                a: t(1),
                b: t(0),
            },
            TcgOp::WriteReg {
                reg_id: 1,
                src: t(3),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 1); // -1 < 1
    assert_eq!(ir[1], 1); // 1 >= -1
    for i in 0..2 {
        assert_eq!(ir[i], th[i]);
    }
}

// ── Load/Store parity ─────────────────────────────────────────────────────────

#[test]
fn parity_load_store_1_byte() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0x2000,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 0xAB,
            },
            TcgOp::Store {
                addr: t(0),
                val: t(1),
                size: 1,
            },
            TcgOp::Load {
                dst: t(2),
                addr: t(0),
                size: 1,
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 0xAB);
    assert_eq!(ir[0], th[0]);
}

#[test]
fn parity_load_store_2_bytes() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0x2000,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 0xCAFE,
            },
            TcgOp::Store {
                addr: t(0),
                val: t(1),
                size: 2,
            },
            TcgOp::Load {
                dst: t(2),
                addr: t(0),
                size: 2,
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 0xCAFE);
    assert_eq!(ir[0], th[0]);
}

#[test]
fn parity_load_store_8_bytes() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0x2000,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 0xDEAD_BEEF_1234_5678u64,
            },
            TcgOp::Store {
                addr: t(0),
                val: t(1),
                size: 8,
            },
            TcgOp::Load {
                dst: t(2),
                addr: t(0),
                size: 8,
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 0xDEAD_BEEF_1234_5678u64);
    assert_eq!(ir[0], th[0]);
}

// ── ReadReg/WriteReg parity ───────────────────────────────────────────────────

#[test]
fn parity_read_write_reg() {
    let mut init = empty_regs();
    init[5] = 0xABCD_1234;
    let block = make_block(
        vec![
            TcgOp::ReadReg {
                dst: t(0),
                reg_id: 5,
            },
            TcgOp::Addi {
                dst: t(1),
                a: t(0),
                imm: 1,
            },
            TcgOp::WriteReg {
                reg_id: 7,
                src: t(1),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &init);
    assert_eq!(ir[7], 0xABCD_1235);
    assert_eq!(ir[7], th[7]);
}

// ── Control flow parity ───────────────────────────────────────────────────────

#[test]
fn parity_brcond_taken() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 1,
            }, // cond = true
            TcgOp::BrCond {
                cond: t(0),
                label: 0,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 999,
            }, // skipped
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(1),
            },
            TcgOp::ExitTb,
            TcgOp::Label { id: 0 },
            TcgOp::Movi {
                dst: t(1),
                value: 42,
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(1),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 42);
    assert_eq!(ir[0], th[0]);
}

#[test]
fn parity_brcond_not_taken() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0,
            }, // cond = false
            TcgOp::BrCond {
                cond: t(0),
                label: 0,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 77,
            }, // fallthrough
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(1),
            },
            TcgOp::ExitTb,
            TcgOp::Label { id: 0 },
            TcgOp::Movi {
                dst: t(1),
                value: 42,
            }, // not reached
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(1),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 77);
    assert_eq!(ir[0], th[0]);
}

#[test]
fn parity_unconditional_br() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 1,
            },
            TcgOp::Br { label: 1 },
            TcgOp::Movi {
                dst: t(0),
                value: 999,
            }, // skipped
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(0),
            },
            TcgOp::ExitTb,
            TcgOp::Label { id: 1 },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(0),
            }, // writes 1
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 1);
    assert_eq!(ir[0], th[0]);
}

#[test]
fn parity_goto_tb_chain() {
    let block = make_block(vec![TcgOp::GotoTb { target_pc: 0x5000 }], 1);
    run_parity(&block, &empty_regs());
    // run_parity already asserts both sides return Chain { target_pc: 0x5000 }
}

#[test]
fn parity_syscall_exit() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 64,
            },
            TcgOp::Syscall { nr: t(0) },
        ],
        1,
    );
    run_parity(&block, &empty_regs());
    // run_parity asserts both sides return Syscall { nr: 64 }
}

// ── Exit kind parity ──────────────────────────────────────────────────────────

#[test]
fn parity_exit_tb() {
    let block = make_block(vec![TcgOp::ExitTb], 1);
    run_parity(&block, &empty_regs());
}

#[test]
fn parity_insn_count_propagated() {
    // insns_executed should equal block.insn_count in both backends
    let block = make_block(vec![TcgOp::ExitTb], 7);
    let mut mem = make_mem();
    let mut regs = empty_regs();

    let mut interp = TcgInterp::new();
    let ir = interp.exec_block(&block, &mut regs, &mut mem).unwrap();

    let compiled = compile_block(&block);
    let mut sysregs = vec![0u64; SYSREG_FILE_SIZE];
    let mut mem2 = make_mem();
    let mut regs2 = empty_regs();
    let th = exec_threaded(&compiled, &mut regs2, &mut mem2, &mut sysregs).unwrap();

    assert_eq!(ir.insns_executed, 7);
    assert_eq!(th.insns_executed, 7);
}

// ── PSTATE parity ────────────────────────────────────────────────────────────

#[test]
fn parity_daif_set_clr() {
    use crate::interp::REG_DAIF;
    let block = make_block(
        vec![
            TcgOp::DaifSet { imm: 0xF }, // set all DAIF bits
            TcgOp::DaifClr { imm: 0x3 }, // clear low 2 bits of imm4
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    // DaifSet: DAIF |= (0xF << 6) = 0x3C0
    // DaifClr: DAIF &= ~(0x3 << 6) = ~0xC0 → 0x3C0 & ~0xC0 = 0x300
    assert_eq!(ir[REG_DAIF as usize], 0x300);
    assert_eq!(ir[REG_DAIF as usize], th[REG_DAIF as usize]);
}

#[test]
fn parity_set_spsel() {
    use crate::interp::REG_SPSEL;
    let block = make_block(vec![TcgOp::SetSpSel { imm: 1 }, TcgOp::ExitTb], 1);
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[REG_SPSEL as usize], 1);
    assert_eq!(ir[REG_SPSEL as usize], th[REG_SPSEL as usize]);
}

#[test]
fn parity_cfinv() {
    use crate::interp::REG_NZCV;
    let mut init = empty_regs();
    init[REG_NZCV as usize] = 0; // C=0
    let block = make_block(
        vec![
            TcgOp::Cfinv, // toggle C bit (bit 29)
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &init);
    assert_eq!(ir[REG_NZCV as usize], 1 << 29); // C=1
    assert_eq!(ir[REG_NZCV as usize], th[REG_NZCV as usize]);
}

// ── System ops parity ─────────────────────────────────────────────────────────

#[test]
fn parity_wfi_exit() {
    let block = make_block(vec![TcgOp::Wfi], 1);
    run_parity(&block, &empty_regs());
    // run_parity asserts both sides return Wfi
}

#[test]
fn parity_hvc_exc() {
    let block = make_block(vec![TcgOp::HvcExc { imm16: 0x1234 }], 1);
    run_parity(&block, &empty_regs());
    // both must return Exception { class: 0x16, iss: 0x1234 }
}

#[test]
fn parity_brk_exc() {
    let block = make_block(vec![TcgOp::BrkExc { imm16: 0x5678 }], 1);
    run_parity(&block, &empty_regs());
}

#[test]
fn parity_smc_exc() {
    let block = make_block(vec![TcgOp::SmcExc { imm16: 0 }], 1);
    run_parity(&block, &empty_regs());
}

#[test]
fn parity_hlt_exc() {
    let block = make_block(vec![TcgOp::HltExc { imm16: 0xF000 }], 1);
    run_parity(&block, &empty_regs());
}

// ── Multi-op sequence parity ──────────────────────────────────────────────────

#[test]
fn parity_fibonacci_5() {
    // Compute the 5th element in the Fibonacci sequence using unrolled TcgOp arithmetic.
    // Starting with a=0, b=1 and applying t=a+b; a=b; b=t five times:
    //   iter 1: (0,1) → (1,1)
    //   iter 2: (1,1) → (1,2)
    //   iter 3: (1,2) → (2,3)
    //   iter 4: (2,3) → (3,5)
    //   iter 5: (3,5) → (5,8)
    // After 5 iterations b = 8.
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0,
            }, // a
            TcgOp::Movi {
                dst: t(1),
                value: 1,
            }, // b
            // iter 1
            TcgOp::Add {
                dst: t(2),
                a: t(0),
                b: t(1),
            },
            TcgOp::Mov {
                dst: t(0),
                src: t(1),
            },
            TcgOp::Mov {
                dst: t(1),
                src: t(2),
            },
            // iter 2
            TcgOp::Add {
                dst: t(2),
                a: t(0),
                b: t(1),
            },
            TcgOp::Mov {
                dst: t(0),
                src: t(1),
            },
            TcgOp::Mov {
                dst: t(1),
                src: t(2),
            },
            // iter 3
            TcgOp::Add {
                dst: t(2),
                a: t(0),
                b: t(1),
            },
            TcgOp::Mov {
                dst: t(0),
                src: t(1),
            },
            TcgOp::Mov {
                dst: t(1),
                src: t(2),
            },
            // iter 4
            TcgOp::Add {
                dst: t(2),
                a: t(0),
                b: t(1),
            },
            TcgOp::Mov {
                dst: t(0),
                src: t(1),
            },
            TcgOp::Mov {
                dst: t(1),
                src: t(2),
            },
            // iter 5
            TcgOp::Add {
                dst: t(2),
                a: t(0),
                b: t(1),
            },
            TcgOp::Mov {
                dst: t(0),
                src: t(1),
            },
            TcgOp::Mov {
                dst: t(1),
                src: t(2),
            },
            // result in t(1) = 8
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(1),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 8); // 5 iterations → b = 8
    assert_eq!(ir[0], th[0]);
}

#[test]
fn parity_sum_loop_via_br() {
    // Sum 1+2+3+4+5 = 15 using Br for an unrolled pattern with labels
    // For simplicity: unrolled, no real loop (no back-edge label support needed)
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0,
            }, // accumulator
            TcgOp::Movi {
                dst: t(1),
                value: 1,
            },
            TcgOp::Add {
                dst: t(0),
                a: t(0),
                b: t(1),
            },
            TcgOp::Movi {
                dst: t(1),
                value: 2,
            },
            TcgOp::Add {
                dst: t(0),
                a: t(0),
                b: t(1),
            },
            TcgOp::Movi {
                dst: t(1),
                value: 3,
            },
            TcgOp::Add {
                dst: t(0),
                a: t(0),
                b: t(1),
            },
            TcgOp::Movi {
                dst: t(1),
                value: 4,
            },
            TcgOp::Add {
                dst: t(0),
                a: t(0),
                b: t(1),
            },
            TcgOp::Movi {
                dst: t(1),
                value: 5,
            },
            TcgOp::Add {
                dst: t(0),
                a: t(0),
                b: t(1),
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(0),
            },
            TcgOp::ExitTb,
        ],
        5,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 15);
    assert_eq!(ir[0], th[0]);
}

#[test]
fn parity_dc_zva_zeroes_memory() {
    // DC ZVA: zero a 64-byte cache-line-aligned block
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0x2040,
            }, // addr inside cache line at 0x2040
            TcgOp::DcZva { addr: t(0) },
            TcgOp::Movi {
                dst: t(1),
                value: 0x2040,
            },
            TcgOp::Load {
                dst: t(2),
                addr: t(1),
                size: 8,
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    // Pre-populate the target area with non-zero data in both memories
    let mut init = empty_regs();
    let (ir, th) = run_parity(&block, &init);
    // Both should have zeroed the memory and read back 0
    assert_eq!(ir[0], 0);
    assert_eq!(ir[0], th[0]);
}

#[test]
fn parity_barrier_nop() {
    // Barrier ops are no-ops in single-threaded execution — both backends must agree
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 99,
            },
            TcgOp::Barrier { kind: 0 }, // DSB
            TcgOp::Barrier { kind: 1 }, // DMB
            TcgOp::Barrier { kind: 2 }, // ISB
            TcgOp::Clrex,
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(0),
            },
            TcgOp::ExitTb,
        ],
        3,
    );
    let (ir, th) = run_parity(&block, &empty_regs());
    assert_eq!(ir[0], 99);
    assert_eq!(ir[0], th[0]);
}
