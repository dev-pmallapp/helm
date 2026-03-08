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

#[test]
fn test_mem_size_from_bits_unknown_defaults_to_word() {
    // Any unrecognised bit-width should fall back to Word
    assert_eq!(MemSize::from_bits(128), MemSize::Word);
    assert_eq!(MemSize::from_bits(0), MemSize::Word);
}

#[test]
fn test_int_sub_conversion() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("d".to_string(), 0);
    let lhs = LLVMValue::register("l".to_string(), 1);
    let rhs = LLVMValue::register("r".to_string(), 2);

    let inst = LLVMInstruction::Sub {
        dest,
        lhs,
        rhs,
        ty: LLVMType::Integer { bits: 32 },
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], MicroOp::IntSub { .. }));
}

#[test]
fn test_int_mul_conversion() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("d".to_string(), 0);
    let lhs = LLVMValue::register("l".to_string(), 1);
    let rhs = LLVMValue::register("r".to_string(), 2);

    let inst = LLVMInstruction::Mul {
        dest,
        lhs,
        rhs,
        ty: LLVMType::Integer { bits: 64 },
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], MicroOp::IntMul { .. }));
    assert_eq!(ops[0].functional_unit(), FunctionalUnitType::IntMultiplier);
    assert_eq!(ops[0].default_latency(), 3);
}

#[test]
fn test_int_div_conversion() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("d".to_string(), 0);
    let lhs = LLVMValue::register("l".to_string(), 1);
    let rhs = LLVMValue::register("r".to_string(), 2);

    let inst = LLVMInstruction::Div {
        dest,
        lhs,
        rhs,
        ty: LLVMType::Integer { bits: 32 },
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], MicroOp::IntDiv { .. }));
    assert_eq!(ops[0].functional_unit(), FunctionalUnitType::IntDivider);
    assert_eq!(ops[0].default_latency(), 10);
}

#[test]
fn test_fp_mul_single_precision() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("d".to_string(), 0);
    let lhs = LLVMValue::register("l".to_string(), 1);
    let rhs = LLVMValue::register("r".to_string(), 2);

    let inst = LLVMInstruction::FMul {
        dest,
        lhs,
        rhs,
        ty: LLVMType::Float,
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        MicroOp::FPMul { double_precision, .. } => {
            assert!(!double_precision, "Float should be single precision");
        }
        _ => panic!("Expected FPMul"),
    }
    assert_eq!(ops[0].functional_unit(), FunctionalUnitType::FPSPMultiplier);
}

#[test]
fn test_fp_mul_double_precision() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("d".to_string(), 0);
    let lhs = LLVMValue::register("l".to_string(), 1);
    let rhs = LLVMValue::register("r".to_string(), 2);

    let inst = LLVMInstruction::FMul {
        dest,
        lhs,
        rhs,
        ty: LLVMType::Double,
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        MicroOp::FPMul { double_precision, .. } => {
            assert!(*double_precision, "Double should be double precision");
        }
        _ => panic!("Expected FPMul"),
    }
    assert_eq!(ops[0].functional_unit(), FunctionalUnitType::FPDPMultiplier);
}

#[test]
fn test_fp_add_single_precision() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("d".to_string(), 0);
    let lhs = LLVMValue::register("l".to_string(), 1);
    let rhs = LLVMValue::register("r".to_string(), 2);

    let inst = LLVMInstruction::FAdd {
        dest,
        lhs,
        rhs,
        ty: LLVMType::Float,
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        MicroOp::FPAdd { double_precision, .. } => {
            assert!(!double_precision);
        }
        _ => panic!("Expected FPAdd"),
    }
    assert_eq!(ops[0].functional_unit(), FunctionalUnitType::FPSPAdder);
}

#[test]
fn test_store_conversion() {
    let mut ctx = ConversionContext::new();
    let value = LLVMValue::register("v".to_string(), 0);
    let ptr = LLVMValue::register("p".to_string(), 1);

    let inst = LLVMInstruction::Store { value, ptr };
    let ops = llvm_to_micro_ops(&inst, &mut ctx);

    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], MicroOp::Store { .. }));
    assert_eq!(ops[0].functional_unit(), FunctionalUnitType::LoadStore);
    assert_eq!(ops[0].default_latency(), 1);
}

#[test]
fn test_load_double_word() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("d".to_string(), 0);
    let ptr = LLVMValue::register("p".to_string(), 1);

    let inst = LLVMInstruction::Load {
        dest,
        ptr,
        ty: LLVMType::Double, // 64-bit = DoubleWord
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        MicroOp::Load { size, .. } => {
            assert_eq!(*size, MemSize::DoubleWord);
        }
        _ => panic!("Expected Load"),
    }
}

#[test]
fn test_and_or_xor_shl_conversions() {
    let mut ctx = ConversionContext::new();

    let make_regs = || {
        (
            LLVMValue::register("d".to_string(), 0),
            LLVMValue::register("l".to_string(), 1),
            LLVMValue::register("r".to_string(), 2),
        )
    };

    let (d, l, r) = make_regs();
    let and_ops = llvm_to_micro_ops(&LLVMInstruction::And { dest: d, lhs: l, rhs: r }, &mut ctx);
    assert!(matches!(and_ops[0], MicroOp::And { .. }));
    assert_eq!(and_ops[0].functional_unit(), FunctionalUnitType::IntBit);

    let (d, l, r) = make_regs();
    let or_ops = llvm_to_micro_ops(&LLVMInstruction::Or { dest: d, lhs: l, rhs: r }, &mut ctx);
    assert!(matches!(or_ops[0], MicroOp::Or { .. }));
    assert_eq!(or_ops[0].functional_unit(), FunctionalUnitType::IntBit);

    let (d, l, r) = make_regs();
    let xor_ops = llvm_to_micro_ops(&LLVMInstruction::Xor { dest: d, lhs: l, rhs: r }, &mut ctx);
    assert!(matches!(xor_ops[0], MicroOp::Xor { .. }));
    assert_eq!(xor_ops[0].functional_unit(), FunctionalUnitType::IntBit);

    let (d, l, r) = make_regs();
    let shl_ops = llvm_to_micro_ops(&LLVMInstruction::Shl { dest: d, lhs: l, rhs: r }, &mut ctx);
    assert!(matches!(shl_ops[0], MicroOp::Shl { .. }));
    assert_eq!(shl_ops[0].functional_unit(), FunctionalUnitType::IntBit);
}

#[test]
fn test_cond_branch_conversion() {
    let mut ctx = ConversionContext::new();
    ctx.map_bb_label("then".to_string(), 2);
    ctx.map_bb_label("else".to_string(), 3);

    let condition = LLVMValue::register("cond".to_string(), 0);
    let inst = LLVMInstruction::CondBr {
        condition,
        true_target: "then".to_string(),
        false_target: "else".to_string(),
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        MicroOp::CondBranch { true_bb, false_bb, .. } => {
            assert_eq!(*true_bb, 2);
            assert_eq!(*false_bb, 3);
        }
        _ => panic!("Expected CondBranch"),
    }
    assert_eq!(ops[0].functional_unit(), FunctionalUnitType::Branch);
}

#[test]
fn test_ret_with_value_becomes_nop() {
    let mut ctx = ConversionContext::new();
    let val = LLVMValue::register("v".to_string(), 0);
    let inst = LLVMInstruction::Ret { value: Some(val) };
    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], MicroOp::Nop));
}

#[test]
fn test_ret_void_becomes_nop() {
    let mut ctx = ConversionContext::new();
    let inst = LLVMInstruction::Ret { value: None };
    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], MicroOp::Nop));
}

#[test]
fn test_phi_with_incoming_becomes_move() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("phi".to_string(), 0);
    let incoming_val = LLVMValue::register("v".to_string(), 1);

    let inst = LLVMInstruction::Phi {
        dest,
        incoming: vec![(incoming_val, "pred".to_string())],
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], MicroOp::Move { .. }));
}

#[test]
fn test_phi_empty_incoming_becomes_nop() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("phi".to_string(), 0);

    let inst = LLVMInstruction::Phi {
        dest,
        incoming: vec![],
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], MicroOp::Nop));
}

#[test]
fn test_zext_sext_trunc_become_move() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("d".to_string(), 0);
    let val = LLVMValue::register("v".to_string(), 1);

    let zext = LLVMInstruction::ZExt {
        dest: dest.clone(),
        value: val.clone(),
        ty: LLVMType::Integer { bits: 64 },
    };
    let ops = llvm_to_micro_ops(&zext, &mut ctx);
    assert!(matches!(ops[0], MicroOp::Move { .. }));

    let sext = LLVMInstruction::SExt {
        dest: dest.clone(),
        value: val.clone(),
        ty: LLVMType::Integer { bits: 64 },
    };
    let ops = llvm_to_micro_ops(&sext, &mut ctx);
    assert!(matches!(ops[0], MicroOp::Move { .. }));

    let trunc = LLVMInstruction::Trunc {
        dest,
        value: val,
        ty: LLVMType::Integer { bits: 8 },
    };
    let ops = llvm_to_micro_ops(&trunc, &mut ctx);
    assert!(matches!(ops[0], MicroOp::Move { .. }));
}

#[test]
fn test_call_with_dest_becomes_load_imm() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("ret".to_string(), 0);

    let inst = LLVMInstruction::Call {
        dest: Some(dest),
        function: "my_func".to_string(),
        args: vec![],
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], MicroOp::LoadImm { value: 0, .. }));
}

#[test]
fn test_call_without_dest_becomes_nop() {
    let mut ctx = ConversionContext::new();

    let inst = LLVMInstruction::Call {
        dest: None,
        function: "void_func".to_string(),
        args: vec![],
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], MicroOp::Nop));
}

#[test]
fn test_nop_latency_is_zero() {
    let nop = MicroOp::Nop;
    assert_eq!(nop.default_latency(), 0);
    assert_eq!(nop.sources(), Vec::<usize>::new());
    assert_eq!(nop.dest(), None);
    assert_eq!(nop.functional_unit(), FunctionalUnitType::IntAdder);
}

#[test]
fn test_load_imm_properties() {
    let op = MicroOp::LoadImm { dest: 5, value: 42 };
    assert_eq!(op.default_latency(), 1);
    assert_eq!(op.sources(), Vec::<usize>::new());
    assert_eq!(op.dest(), Some(5));
    assert_eq!(op.functional_unit(), FunctionalUnitType::IntAdder);
}

#[test]
fn test_move_properties() {
    let op = MicroOp::Move { dest: 3, src: 7 };
    assert_eq!(op.default_latency(), 1);
    assert_eq!(op.sources(), vec![7]);
    assert_eq!(op.dest(), Some(3));
}

#[test]
fn test_store_sources_include_src_and_addr() {
    let op = MicroOp::Store { src: 1, addr: 2, size: MemSize::Word };
    assert_eq!(op.sources(), vec![1, 2]);
    assert_eq!(op.dest(), None);
}

#[test]
fn test_load_sources_include_addr_only() {
    let op = MicroOp::Load { dest: 0, addr: 3, size: MemSize::HalfWord };
    assert_eq!(op.sources(), vec![3]);
    assert_eq!(op.dest(), Some(0));
    assert_eq!(op.default_latency(), 2);
}

#[test]
fn test_branch_properties() {
    let op = MicroOp::Branch { target_bb: 10 };
    assert_eq!(op.default_latency(), 1);
    assert_eq!(op.sources(), Vec::<usize>::new());
    assert_eq!(op.dest(), None);
}

#[test]
fn test_cond_branch_sources() {
    let op = MicroOp::CondBranch { condition: 4, true_bb: 1, false_bb: 2 };
    assert_eq!(op.sources(), vec![4]);
    assert_eq!(op.dest(), None);
}

#[test]
fn test_icmp_ne_predicate() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("d".to_string(), 0);
    let lhs = LLVMValue::register("l".to_string(), 1);
    let rhs = LLVMValue::register("r".to_string(), 2);

    let inst = LLVMInstruction::ICmp {
        dest,
        predicate: ICmpPredicate::NE,
        lhs,
        rhs,
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    match &ops[0] {
        MicroOp::Compare { op, .. } => assert_eq!(*op, CompareOp::NE),
        _ => panic!("Expected Compare"),
    }
}

#[test]
fn test_icmp_slt_maps_to_lt() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("d".to_string(), 0);
    let lhs = LLVMValue::register("l".to_string(), 1);
    let rhs = LLVMValue::register("r".to_string(), 2);

    let inst = LLVMInstruction::ICmp {
        dest,
        predicate: ICmpPredicate::SLT,
        lhs,
        rhs,
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    match &ops[0] {
        MicroOp::Compare { op, .. } => assert_eq!(*op, CompareOp::LT),
        _ => panic!("Expected Compare"),
    }
}

#[test]
fn test_icmp_sge_maps_to_ge() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("d".to_string(), 0);
    let lhs = LLVMValue::register("l".to_string(), 1);
    let rhs = LLVMValue::register("r".to_string(), 2);

    let inst = LLVMInstruction::ICmp {
        dest,
        predicate: ICmpPredicate::SGE,
        lhs,
        rhs,
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    match &ops[0] {
        MicroOp::Compare { op, .. } => assert_eq!(*op, CompareOp::GE),
        _ => panic!("Expected Compare"),
    }
}

#[test]
fn test_conversion_context_default() {
    let ctx = ConversionContext::default();
    // An unknown label should return None
    assert_eq!(ctx.get_bb_id("nonexistent"), None);
}

#[test]
fn test_conversion_context_reuses_register() {
    let mut ctx = ConversionContext::new();
    let val = LLVMValue::register("x".to_string(), 0);
    let r1 = ctx.get_or_alloc_reg(&val);
    let r2 = ctx.get_or_alloc_reg(&val);
    assert_eq!(r1, r2);
}

#[test]
fn test_conversion_context_allocates_distinct_registers() {
    let mut ctx = ConversionContext::new();
    let v1 = LLVMValue::register("a".to_string(), 1);
    let v2 = LLVMValue::register("b".to_string(), 2);
    let r1 = ctx.get_or_alloc_reg(&v1);
    let r2 = ctx.get_or_alloc_reg(&v2);
    assert_ne!(r1, r2);
}

#[test]
fn test_gep_empty_indices_becomes_move() {
    let mut ctx = ConversionContext::new();
    let dest = LLVMValue::register("d".to_string(), 0);
    let base = LLVMValue::register("b".to_string(), 1);

    let inst = LLVMInstruction::GetElementPtr {
        dest,
        base,
        indices: vec![],
    };

    let ops = llvm_to_micro_ops(&inst, &mut ctx);
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], MicroOp::Move { .. }));
}
