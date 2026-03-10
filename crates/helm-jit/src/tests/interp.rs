use crate::block::TcgBlock;
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

// -- Arithmetic --

#[test]
fn interp_add() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 10,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 20,
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
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    let result = interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[0], 30);
    assert!(matches!(result.exit, InterpExit::Exit));
}

#[test]
fn interp_sub() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 50,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 8,
            },
            TcgOp::Sub {
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
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[0], 42);
}

#[test]
fn interp_mul() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 7,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 6,
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
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[0], 42);
}

#[test]
fn interp_div_by_zero_returns_zero() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 100,
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
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[0], 0);
}

// -- Bitwise --

#[test]
fn interp_and_or_xor() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0xFF,
            },
            TcgOp::Movi {
                dst: t(1),
                value: 0x0F,
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
            TcgOp::ExitTb,
        ],
        1,
    );
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[0], 0x0F);
    assert_eq!(regs[1], 0xFF);
    assert_eq!(regs[2], 0xF0);
}

// -- Load/Store --

#[test]
fn interp_load_store_roundtrip() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0x2000,
            }, // addr
            TcgOp::Movi {
                dst: t(1),
                value: 0xDEADBEEF,
            }, // value
            TcgOp::Store {
                addr: t(0),
                val: t(1),
                size: 4,
            },
            TcgOp::Load {
                dst: t(2),
                addr: t(0),
                size: 4,
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    let result = interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[0], 0xDEADBEEF);
    assert_eq!(result.mem_accesses.len(), 2);
    assert!(result.mem_accesses[0].is_write); // store first
    assert!(!result.mem_accesses[1].is_write); // then load
}

// -- ReadReg/WriteReg --

#[test]
fn interp_read_write_reg() {
    let block = make_block(
        vec![
            TcgOp::ReadReg {
                dst: t(0),
                reg_id: 5,
            },
            TcgOp::Addi {
                dst: t(1),
                a: t(0),
                imm: 100,
            },
            TcgOp::WriteReg {
                reg_id: 6,
                src: t(1),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let mut regs = empty_regs();
    regs[5] = 42;
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[6], 142);
}

// -- Control flow --

#[test]
fn interp_brcond_taken() {
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
            }, // taken path
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
    assert_eq!(regs[0], 42);
}

#[test]
fn interp_brcond_not_taken() {
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
                value: 999,
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
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[0], 999);
}

// -- System ops --

#[test]
fn interp_syscall_exit() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 93,
            }, // exit syscall nr
            TcgOp::Syscall { nr: t(0) },
        ],
        1,
    );
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    let result = interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert!(matches!(result.exit, InterpExit::Syscall { nr: 93 }));
}

#[test]
fn interp_goto_tb_chain() {
    let block = make_block(vec![TcgOp::GotoTb { target_pc: 0x2000 }], 1);
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    let result = interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert!(matches!(
        result.exit,
        InterpExit::Chain { target_pc: 0x2000 }
    ));
}

// -- Sign/zero extension --

#[test]
fn interp_sext() {
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
            TcgOp::ExitTb,
        ],
        1,
    );
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[0] as i64, -128);
}

#[test]
fn interp_zext() {
    let block = make_block(
        vec![
            TcgOp::Movi {
                dst: t(0),
                value: 0xFFFF_FFFF_FFFF_FF80,
            },
            TcgOp::Zext {
                dst: t(1),
                src: t(0),
                from_bits: 8,
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
    assert_eq!(regs[0], 0x80);
}

// -- Comparisons --

#[test]
fn interp_set_eq() {
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
            TcgOp::SetEq {
                dst: t(2),
                a: t(0),
                b: t(1),
            },
            TcgOp::WriteReg {
                reg_id: 0,
                src: t(2),
            },
            TcgOp::Movi {
                dst: t(3),
                value: 6,
            },
            TcgOp::SetEq {
                dst: t(4),
                a: t(0),
                b: t(3),
            },
            TcgOp::WriteReg {
                reg_id: 1,
                src: t(4),
            },
            TcgOp::ExitTb,
        ],
        1,
    );
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(regs[0], 1); // 5 == 5
    assert_eq!(regs[1], 0); // 5 != 6
}

#[test]
fn interp_insn_count_reported() {
    let block = make_block(vec![TcgOp::ExitTb], 3);
    let mut regs = empty_regs();
    let mut mem = make_mem();
    let mut interp = TcgInterp::new();
    let result = interp.exec_block(&block, &mut regs, &mut mem).unwrap();
    assert_eq!(result.insns_executed, 3);
}
