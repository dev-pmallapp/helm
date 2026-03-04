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
