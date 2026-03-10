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
fn ite_model_returns_l1() {
    let mut m = IteModel::default();
    assert_eq!(m.memory_latency(0x1000, 8, false), 3);
    assert_eq!(m.accuracy(), AccuracyLevel::ITE);
}

#[test]
fn accuracy_levels_are_distinct() {
    assert_ne!(AccuracyLevel::FE, AccuracyLevel::ITE);
    assert_ne!(AccuracyLevel::ITE, AccuracyLevel::CAE);
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
fn ite_model_branch_penalty_is_zero() {
    let mut m = IteModel::default();
    // Basic IteModel does not model branch misprediction.
    assert_eq!(m.branch_misprediction_penalty(), 0);
}

#[test]
fn ite_model_instruction_latency_nop() {
    let mut m = IteModel::default();
    assert_eq!(m.instruction_latency(&make_uop(Opcode::Nop)), 1);
}

// ---------------------------------------------------------------------------
// IteModelDetailed tests
// ---------------------------------------------------------------------------

#[test]
fn detailed_model_is_ape() {
    let m = IteModelDetailed::default();
    assert_eq!(m.accuracy(), AccuracyLevel::ITE);
}

#[test]
fn detailed_model_int_alu_is_one() {
    let mut m = IteModelDetailed::default();
    assert_eq!(m.instruction_latency_for_class(InsnClass::IntAlu), 1);
}

#[test]
fn detailed_model_int_mul_latency() {
    let mut m = IteModelDetailed::default();
    assert_eq!(m.instruction_latency_for_class(InsnClass::IntMul), 3);
}

#[test]
fn detailed_model_int_div_latency() {
    let mut m = IteModelDetailed::default();
    assert_eq!(m.instruction_latency_for_class(InsnClass::IntDiv), 12);
}

#[test]
fn detailed_model_fp_latencies() {
    let mut m = IteModelDetailed::default();
    assert_eq!(m.instruction_latency_for_class(InsnClass::FpAlu), 4);
    assert_eq!(m.instruction_latency_for_class(InsnClass::FpMul), 5);
    assert_eq!(m.instruction_latency_for_class(InsnClass::FpDiv), 15);
}

#[test]
fn detailed_model_load_store_latencies() {
    let mut m = IteModelDetailed::default();
    assert_eq!(m.instruction_latency_for_class(InsnClass::Load), 4);
    assert_eq!(m.instruction_latency_for_class(InsnClass::Store), 1);
}

#[test]
fn detailed_model_branch_penalty() {
    let mut m = IteModelDetailed::default();
    assert_eq!(m.branch_misprediction_penalty(), 10);
}

#[test]
fn detailed_model_memory_latency_varies_by_addr() {
    let mut m = IteModelDetailed::default();
    // Deterministic: same address always gives same result
    let lat1 = m.memory_latency(0x1000, 8, false);
    let lat2 = m.memory_latency(0x1000, 8, false);
    assert_eq!(lat1, lat2);
    // Different addresses may give different latencies
    // (but all should be valid cache-level values)
    let valid_latencies = [m.l1_latency, m.l2_latency, m.l3_latency, m.dram_latency];
    for addr_shift in 0..100u64 {
        let lat = m.memory_latency(addr_shift << 6, 8, false);
        assert!(
            valid_latencies.contains(&lat),
            "unexpected latency {lat} for addr {:#x}",
            addr_shift << 6
        );
    }
}

#[test]
fn detailed_model_simd_uses_fp_alu() {
    let mut m = IteModelDetailed::default();
    assert_eq!(
        m.instruction_latency_for_class(InsnClass::Simd),
        m.fp_alu_latency
    );
}

#[test]
fn detailed_model_custom_latencies() {
    let mut m = IteModelDetailed {
        int_mul_latency: 7,
        branch_penalty: 20,
        ..IteModelDetailed::default()
    };
    assert_eq!(m.instruction_latency_for_class(InsnClass::IntMul), 7);
    assert_eq!(m.branch_misprediction_penalty(), 20);
}

#[test]
fn insn_class_variants_are_distinct() {
    use std::collections::HashSet;
    let classes = [
        InsnClass::IntAlu,
        InsnClass::IntMul,
        InsnClass::IntDiv,
        InsnClass::FpAlu,
        InsnClass::FpMul,
        InsnClass::FpDiv,
        InsnClass::Load,
        InsnClass::Store,
        InsnClass::Branch,
        InsnClass::CondBranch,
        InsnClass::Syscall,
        InsnClass::Nop,
        InsnClass::Simd,
        InsnClass::Fence,
    ];
    let set: HashSet<InsnClass> = classes.iter().copied().collect();
    assert_eq!(set.len(), 14);
}

#[test]
fn default_trait_instruction_latency_for_class_returns_one() {
    // FeModel uses the default impl which returns 1 for all classes
    let mut m = FeModel;
    assert_eq!(m.instruction_latency_for_class(InsnClass::IntMul), 1);
    assert_eq!(m.instruction_latency_for_class(InsnClass::Load), 1);
}
