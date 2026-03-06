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
fn fe_model_always_one_cycle() {
    let mut m = FeModel;
    assert_eq!(m.instruction_latency(&nop_uop()), 1);
    assert_eq!(m.memory_latency(0x1000, 8, false), 0);
    assert_eq!(m.branch_misprediction_penalty(), 0);
    assert_eq!(m.accuracy(), AccuracyLevel::FE);
}

#[test]
fn ape_model_returns_l1() {
    let mut m = ApeModel::default();
    assert_eq!(m.memory_latency(0x1000, 8, false), 3);
    assert_eq!(m.accuracy(), AccuracyLevel::APE);
}

#[test]
fn accuracy_levels_are_distinct() {
    assert_ne!(AccuracyLevel::FE, AccuracyLevel::APE);
    assert_ne!(AccuracyLevel::APE, AccuracyLevel::CAE);
    assert_ne!(AccuracyLevel::FE, AccuracyLevel::CAE);
}

fn make_uop(opcode: helm_core::ir::Opcode) -> MicroOp {
    MicroOp {
        guest_pc: 0,
        opcode,
        sources: vec![],
        dest: None,
        immediate: None,
        flags: MicroOpFlags::default(),
    }
}

#[test]
fn fe_model_load_latency_is_one() {
    let mut m = FeModel;
    assert_eq!(m.instruction_latency(&make_uop(Opcode::Load)), 1);
}

#[test]
fn fe_model_branch_penalty_is_zero() {
    let mut m = FeModel;
    assert_eq!(m.branch_misprediction_penalty(), 0);
}

#[test]
fn ape_model_branch_penalty_is_positive() {
    let mut m = ApeModel::default();
    assert!(m.branch_misprediction_penalty() > 0);
}

#[test]
fn ape_model_instruction_latency_nop() {
    let mut m = ApeModel::default();
    assert_eq!(m.instruction_latency(&make_uop(Opcode::Nop)), 1);
}
