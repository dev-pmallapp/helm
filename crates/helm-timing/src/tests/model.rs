use crate::model::*;
use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};

fn nop_uop() -> MicroOp {
    MicroOp {
        guest_pc: 0,
        opcode: Opcode::Nop,
        sources: vec![],
        dest: None,
        immediate: None,
        flags: MicroOpFlags::default(),
    }
}

#[test]
fn functional_model_always_one_cycle() {
    let mut m = FunctionalModel;
    assert_eq!(m.instruction_latency(&nop_uop()), 1);
    assert_eq!(m.memory_latency(0x1000, 8, false), 0);
    assert_eq!(m.branch_misprediction_penalty(), 0);
    assert_eq!(m.accuracy(), AccuracyLevel::Functional);
}

#[test]
fn stall_annotated_model_returns_l1() {
    let mut m = StallAnnotatedModel::default();
    assert_eq!(m.memory_latency(0x1000, 8, false), 3);
    assert_eq!(m.accuracy(), AccuracyLevel::StallAnnotated);
}
