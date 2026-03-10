use crate::block::TcgBlock;
use crate::ir::TcgOp;

#[test]
fn empty_block() {
    let block = TcgBlock {
        guest_pc: 0x1000,
        guest_size: 0,
        insn_count: 0,
        ops: vec![],
    };
    assert_eq!(block.guest_pc, 0x1000);
    assert!(block.ops.is_empty());
    assert_eq!(block.insn_count, 0);
}

#[test]
fn block_with_ops() {
    let block = TcgBlock {
        guest_pc: 0x2000,
        guest_size: 8,
        insn_count: 2,
        ops: vec![TcgOp::ExitTb, TcgOp::ExitTb],
    };
    assert_eq!(block.ops.len(), 2);
    assert_eq!(block.guest_size, 8);
    assert_eq!(block.insn_count, 2);
}

#[test]
fn block_clone() {
    let block = TcgBlock {
        guest_pc: 0x3000,
        guest_size: 4,
        insn_count: 1,
        ops: vec![TcgOp::ExitTb],
    };
    let cloned = block.clone();
    assert_eq!(cloned.guest_pc, block.guest_pc);
    assert_eq!(cloned.ops.len(), block.ops.len());
}
