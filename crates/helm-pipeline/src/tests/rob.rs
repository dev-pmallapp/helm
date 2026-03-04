use crate::rob::*;
use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};

fn make_uop(pc: u64) -> MicroOp {
    MicroOp {
        guest_pc: pc,
        opcode: Opcode::IntAlu,
        sources: vec![],
        dest: None,
        immediate: None,
        flags: MicroOpFlags::default(),
    }
}

#[test]
fn new_rob_is_empty() {
    let rob = ReorderBuffer::new(8);
    assert!(rob.is_empty());
    assert!(!rob.is_full());
}

#[test]
fn allocate_returns_index() {
    let mut rob = ReorderBuffer::new(4);
    let idx = rob.allocate(make_uop(0x100));
    assert_eq!(idx, Some(0));
    assert!(!rob.is_empty());
}

#[test]
fn full_rob_rejects_allocation() {
    let mut rob = ReorderBuffer::new(3); // capacity 3 means 2 usable slots
    rob.allocate(make_uop(0x100));
    rob.allocate(make_uop(0x104));
    assert!(rob.is_full());
    assert_eq!(rob.allocate(make_uop(0x108)), None);
}

#[test]
fn commit_in_order() {
    let mut rob = ReorderBuffer::new(8);
    let i0 = rob.allocate(make_uop(0x100)).unwrap();
    let i1 = rob.allocate(make_uop(0x104)).unwrap();

    // Complete second entry first — should not commit yet.
    rob.complete(i1);
    let committed = rob.try_commit();
    assert!(
        committed.is_empty(),
        "out-of-order complete should not commit"
    );

    // Complete first entry — both should commit now.
    rob.complete(i0);
    let committed = rob.try_commit();
    assert_eq!(committed.len(), 2);
    assert_eq!(committed[0].guest_pc, 0x100);
    assert_eq!(committed[1].guest_pc, 0x104);
}
