use crate::block::TranslatedBlock;
use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};

#[test]
fn empty_block() {
    let block = TranslatedBlock {
        start_pc: 0x1000,
        guest_size: 0,
        uops: vec![],
    };
    assert_eq!(block.start_pc, 0x1000);
    assert!(block.uops.is_empty());
    assert_eq!(block.guest_size, 0);
}

#[test]
fn block_with_uops() {
    let uop = MicroOp {
        guest_pc: 0x1000,
        opcode: Opcode::Nop,
        sources: vec![],
        dest: None,
        immediate: None,
        flags: MicroOpFlags::default(),
    };
    let block = TranslatedBlock {
        start_pc: 0x1000,
        guest_size: 4,
        uops: vec![uop],
    };
    assert_eq!(block.uops.len(), 1);
    assert_eq!(block.uops[0].guest_pc, 0x1000);
}

#[test]
fn block_clone() {
    let uop = MicroOp {
        guest_pc: 0x2000,
        opcode: Opcode::IntAlu,
        sources: vec![1, 2],
        dest: Some(0),
        immediate: Some(42),
        flags: MicroOpFlags::default(),
    };
    let block = TranslatedBlock {
        start_pc: 0x2000,
        guest_size: 8,
        uops: vec![uop],
    };
    let cloned = block.clone();
    assert_eq!(cloned.start_pc, 0x2000);
    assert_eq!(cloned.guest_size, 8);
    assert_eq!(cloned.uops.len(), 1);
    assert_eq!(cloned.uops[0].immediate, Some(42));
}

#[test]
fn block_guest_size_matches_instructions() {
    let block = TranslatedBlock {
        start_pc: 0x3000,
        guest_size: 16,
        uops: vec![
            MicroOp {
                guest_pc: 0x3000,
                opcode: Opcode::Nop,
                sources: vec![],
                dest: None,
                immediate: None,
                flags: MicroOpFlags::default(),
            },
            MicroOp {
                guest_pc: 0x3004,
                opcode: Opcode::Nop,
                sources: vec![],
                dest: None,
                immediate: None,
                flags: MicroOpFlags::default(),
            },
            MicroOp {
                guest_pc: 0x3008,
                opcode: Opcode::Nop,
                sources: vec![],
                dest: None,
                immediate: None,
                flags: MicroOpFlags::default(),
            },
            MicroOp {
                guest_pc: 0x300C,
                opcode: Opcode::Branch,
                sources: vec![],
                dest: None,
                immediate: None,
                flags: MicroOpFlags {
                    is_branch: true,
                    ..Default::default()
                },
            },
        ],
    };
    assert_eq!(block.guest_size, 16);
    assert_eq!(block.uops.len(), 4);
}
