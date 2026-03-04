//! Tests for instruction scheduler

use crate::functional_units::{FunctionalUnitPoolBuilder, FunctionalUnitType};
use crate::ir::{LLVMBasicBlock, LLVMInstruction, LLVMType, LLVMValue};
use crate::scheduler::{InstructionScheduler, SchedulingConfig};

#[test]
fn test_scheduler_basic() {
    let config = SchedulingConfig::default();
    let fu_pool = FunctionalUnitPoolBuilder::new().build();
    let scheduler = InstructionScheduler::new(config, fu_pool);

    assert_eq!(scheduler.cycle(), 0);
    assert!(scheduler.is_idle());
}

#[test]
fn test_schedule_simple_bb() {
    let config = SchedulingConfig::default();
    let fu_pool = FunctionalUnitPoolBuilder::new().build();
    let mut scheduler = InstructionScheduler::new(config, fu_pool);

    // Create simple basic block: r0 = r1 + r2
    let mut bb = LLVMBasicBlock::new("entry".to_string());
    bb.add_instruction(LLVMInstruction::Add {
        dest: LLVMValue::register("r0".to_string(), 0),
        lhs: LLVMValue::register("r1".to_string(), 1),
        rhs: LLVMValue::register("r2".to_string(), 2),
        ty: LLVMType::Integer { bits: 32 },
    });

    scheduler.schedule_basic_block(&bb).unwrap();

    // Should have instructions in reservation table
    let (res, _, _, _) = scheduler.queue_sizes();
    assert!(res > 0);
}

#[test]
fn test_limited_resources() {
    let config = SchedulingConfig {
        lockstep_mode: false,
        scheduling_threshold: 100,
        pipelined: false,
    };

    let fu_pool = FunctionalUnitPoolBuilder::new()
        .with_int_adders(1, 1, false) // Only 1 non-pipelined adder
        .build();

    let mut scheduler = InstructionScheduler::new(config, fu_pool);

    // Create BB with 3 add instructions
    let mut bb = LLVMBasicBlock::new("entry".to_string());
    for i in 0..3 {
        bb.add_instruction(LLVMInstruction::Add {
            dest: LLVMValue::register(format!("r{}", i), i),
            lhs: LLVMValue::register(format!("a{}", i), i + 10),
            rhs: LLVMValue::register(format!("b{}", i), i + 20),
            ty: LLVMType::Integer { bits: 32 },
        });
    }

    scheduler.schedule_basic_block(&bb).unwrap();

    // Tick and check that not all instructions can execute simultaneously
    scheduler.tick().unwrap();
    let (res, comp, _, _) = scheduler.queue_sizes();

    // With 1 adder, at most 1 should be in compute queue initially
    assert!(comp <= 1);
    assert!(res >= 2); // At least 2 should still be waiting
}

#[test]
fn test_lockstep_mode() {
    let config = SchedulingConfig {
        lockstep_mode: true,
        scheduling_threshold: 100,
        pipelined: true,
    };

    let fu_pool = FunctionalUnitPoolBuilder::new().build();
    let mut scheduler = InstructionScheduler::new(config, fu_pool);

    // Create simple BB
    let mut bb = LLVMBasicBlock::new("entry".to_string());
    bb.add_instruction(LLVMInstruction::Add {
        dest: LLVMValue::register("r0".to_string(), 0),
        lhs: LLVMValue::register("r1".to_string(), 1),
        rhs: LLVMValue::register("r2".to_string(), 2),
        ty: LLVMType::Integer { bits: 32 },
    });

    scheduler.schedule_basic_block(&bb).unwrap();

    // In lockstep mode, scheduler behavior is predictable
    assert!(!scheduler.is_idle());
}

#[test]
fn test_queue_sizes() {
    let config = SchedulingConfig::default();
    let fu_pool = FunctionalUnitPoolBuilder::new().build();
    let scheduler = InstructionScheduler::new(config, fu_pool);

    let (res, comp, load, store) = scheduler.queue_sizes();
    assert_eq!(res, 0);
    assert_eq!(comp, 0);
    assert_eq!(load, 0);
    assert_eq!(store, 0);
}
