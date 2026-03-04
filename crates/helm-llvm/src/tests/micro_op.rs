//! Tests for MicroOp conversion

use crate::functional_units::FunctionalUnitType;
use crate::ir::{ICmpPredicate, LLVMInstruction, LLVMType, LLVMValue};
use crate::micro_op::{llvm_to_micro_ops, CompareOp, ConversionContext, MemSize, MicroOp};

#[test]
fn test_int_add_conversion() {
    let mut ctx = ConversionContext::new();

    let dest = LLVMValue::register("r0".to_string(), 0);
    let lhs = LLVMValue::register("r1".to_string(), 1);
    let rhs = LLVMValue::register("r2".to_string(), 2);

    let inst = LLVMInstruction::Add {
        dest: dest.clone(),
        lhs: lhs.clone(),
        rhs: rhs.clone(),
        ty: LLVMType::Integer { bits: 32 },
    };

    let micro_ops = llvm_to_micro_ops(&inst, &mut ctx);

    assert_eq!(micro_ops.len(), 1);
    match &micro_ops[0] {
        MicroOp::IntAdd { dest, src1, src2 } => {
            assert_eq!(*dest, 0);
            assert_eq!(*src1, 1);
            assert_eq!(*src2, 2);
        }
        _ => panic!("Expected IntAdd MicroOp"),
    }
}

#[test]
fn test_fp_add_conversion() {
    let mut ctx = ConversionContext::new();

    let dest = LLVMValue::register("f0".to_string(), 0);
    let lhs = LLVMValue::register("f1".to_string(), 1);
    let rhs = LLVMValue::register("f2".to_string(), 2);

    let inst = LLVMInstruction::FAdd {
        dest: dest.clone(),
        lhs: lhs.clone(),
        rhs: rhs.clone(),
        ty: LLVMType::Double,
    };

    let micro_ops = llvm_to_micro_ops(&inst, &mut ctx);

    assert_eq!(micro_ops.len(), 1);
    match &micro_ops[0] {
        MicroOp::FPAdd {
            dest,
            src1,
            src2,
            double_precision,
        } => {
            assert_eq!(*dest, 0);
            assert_eq!(*src1, 1);
            assert_eq!(*src2, 2);
            assert!(*double_precision);
        }
        _ => panic!("Expected FPAdd MicroOp"),
    }
}

#[test]
fn test_load_conversion() {
    let mut ctx = ConversionContext::new();

    let dest = LLVMValue::register("r0".to_string(), 0);
    let ptr = LLVMValue::register("ptr".to_string(), 1);

    let inst = LLVMInstruction::Load {
        dest: dest.clone(),
        ptr: ptr.clone(),
        ty: LLVMType::Integer { bits: 32 },
    };

    let micro_ops = llvm_to_micro_ops(&inst, &mut ctx);

    assert_eq!(micro_ops.len(), 1);
    match &micro_ops[0] {
        MicroOp::Load { dest, addr, size } => {
            assert_eq!(*dest, 0);
            assert_eq!(*addr, 1);
            assert_eq!(*size, MemSize::Word);
        }
        _ => panic!("Expected Load MicroOp"),
    }
}

#[test]
fn test_icmp_conversion() {
    let mut ctx = ConversionContext::new();

    let dest = LLVMValue::register("cmp".to_string(), 0);
    let lhs = LLVMValue::register("a".to_string(), 1);
    let rhs = LLVMValue::register("b".to_string(), 2);

    let inst = LLVMInstruction::ICmp {
        dest: dest.clone(),
        predicate: ICmpPredicate::EQ,
        lhs: lhs.clone(),
        rhs: rhs.clone(),
    };

    let micro_ops = llvm_to_micro_ops(&inst, &mut ctx);

    assert_eq!(micro_ops.len(), 1);
    match &micro_ops[0] {
        MicroOp::Compare {
            dest,
            src1,
            src2,
            op,
        } => {
            assert_eq!(*dest, 0);
            assert_eq!(*src1, 1);
            assert_eq!(*src2, 2);
            assert_eq!(*op, CompareOp::EQ);
        }
        _ => panic!("Expected Compare MicroOp"),
    }
}

#[test]
fn test_gep_expansion() {
    let mut ctx = ConversionContext::new();

    let dest = LLVMValue::register("ptr".to_string(), 0);
    let base = LLVMValue::register("array".to_string(), 1);
    let idx1 = LLVMValue::const_int(0, 32);
    let idx2 = LLVMValue::const_int(4, 32);

    let inst = LLVMInstruction::GetElementPtr {
        dest: dest.clone(),
        base: base.clone(),
        indices: vec![idx1, idx2],
    };

    let micro_ops = llvm_to_micro_ops(&inst, &mut ctx);

    // GEP should expand to multiple IntAdd operations
    assert!(micro_ops.len() >= 2);
    for op in &micro_ops {
        match op {
            MicroOp::IntAdd { .. } => {} // Expected
            MicroOp::Move { .. } => {}   // Also acceptable
            _ => panic!("GEP should only produce IntAdd or Move ops"),
        }
    }
}

#[test]
fn test_branch_conversion() {
    let mut ctx = ConversionContext::new();
    ctx.map_bb_label("target".to_string(), 5);

    let inst = LLVMInstruction::Br {
        target: "target".to_string(),
    };

    let micro_ops = llvm_to_micro_ops(&inst, &mut ctx);

    assert_eq!(micro_ops.len(), 1);
    match &micro_ops[0] {
        MicroOp::Branch { target_bb } => {
            assert_eq!(*target_bb, 5);
        }
        _ => panic!("Expected Branch MicroOp"),
    }
}

#[test]
fn test_micro_op_properties() {
    let op = MicroOp::IntMul {
        dest: 0,
        src1: 1,
        src2: 2,
    };

    // Test functional unit type
    assert_eq!(
        op.functional_unit(),
        FunctionalUnitType::IntMultiplier
    );

    // Test latency
    assert_eq!(op.default_latency(), 3);

    // Test sources
    assert_eq!(op.sources(), vec![1, 2]);

    // Test dest
    assert_eq!(op.dest(), Some(0));
}

#[test]
fn test_mem_size_from_bits() {
    assert_eq!(MemSize::from_bits(8), MemSize::Byte);
    assert_eq!(MemSize::from_bits(16), MemSize::HalfWord);
    assert_eq!(MemSize::from_bits(32), MemSize::Word);
    assert_eq!(MemSize::from_bits(64), MemSize::DoubleWord);
}
