use crate::ir::*;

#[test]
fn micro_op_default_flags_are_false() {
    let flags = MicroOpFlags::default();
    assert!(!flags.is_branch);
    assert!(!flags.is_call);
    assert!(!flags.is_return);
    assert!(!flags.is_serialising);
    assert!(!flags.is_memory_barrier);
}

#[test]
fn micro_op_can_be_constructed() {
    let uop = MicroOp {
        guest_pc: 0x1000,
        opcode: Opcode::IntAlu,
        sources: vec![1, 2],
        dest: Some(3),
        immediate: None,
        flags: MicroOpFlags::default(),
    };
    assert_eq!(uop.guest_pc, 0x1000);
    assert_eq!(uop.sources.len(), 2);
    assert_eq!(uop.dest, Some(3));
}

#[test]
fn opcode_equality() {
    assert_eq!(Opcode::Load, Opcode::Load);
    assert_ne!(Opcode::Load, Opcode::Store);
    assert_eq!(Opcode::Other(42), Opcode::Other(42));
    assert_ne!(Opcode::Other(1), Opcode::Other(2));
}

#[test]
fn micro_op_with_immediate_stores_value() {
    let uop = MicroOp {
        guest_pc: 0x2000,
        opcode: Opcode::IntAlu,
        sources: vec![],
        dest: Some(0),
        immediate: Some(0xFF),
        flags: MicroOpFlags::default(),
    };
    assert_eq!(uop.immediate, Some(0xFF));
}

#[test]
fn micro_op_no_dest_is_none() {
    let uop = MicroOp {
        guest_pc: 0x3000,
        opcode: Opcode::Store,
        sources: vec![1, 2],
        dest: None,
        immediate: None,
        flags: MicroOpFlags::default(),
    };
    assert!(uop.dest.is_none());
}

#[test]
fn micro_op_multiple_sources() {
    let uop = MicroOp {
        guest_pc: 0,
        opcode: Opcode::IntAlu,
        sources: vec![0, 1, 2, 3],
        dest: Some(4),
        immediate: None,
        flags: MicroOpFlags::default(),
    };
    assert_eq!(uop.sources.len(), 4);
    assert_eq!(uop.sources[2], 2);
}

#[test]
fn opcode_other_different_discriminants_are_not_equal() {
    assert_ne!(Opcode::Other(0), Opcode::Other(1));
    assert_ne!(Opcode::Other(100), Opcode::Other(200));
}

#[test]
fn all_named_opcodes_can_be_matched() {
    let ops = vec![
        Opcode::IntAlu,
        Opcode::IntMul,
        Opcode::IntDiv,
        Opcode::FpAlu,
        Opcode::FpMul,
        Opcode::FpDiv,
        Opcode::Load,
        Opcode::Store,
        Opcode::Branch,
        Opcode::CondBranch,
        Opcode::Syscall,
        Opcode::Nop,
        Opcode::Fence,
    ];
    for op in ops {
        // All opcodes equal themselves
        assert_eq!(op, op);
    }
}

#[test]
fn micro_op_flags_all_fields_settable() {
    let flags = MicroOpFlags {
        is_serialising: true,
        is_memory_barrier: true,
        is_branch: true,
        is_call: true,
        is_return: true,
    };
    assert!(flags.is_serialising);
    assert!(flags.is_memory_barrier);
    assert!(flags.is_branch);
    assert!(flags.is_call);
    assert!(flags.is_return);
}
