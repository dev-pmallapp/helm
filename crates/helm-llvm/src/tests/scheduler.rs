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

#[test]
fn test_cycle_increments_on_tick() {
    let config = SchedulingConfig::default();
    let fu_pool = FunctionalUnitPoolBuilder::new().build();
    let mut scheduler = InstructionScheduler::new(config, fu_pool);

    assert_eq!(scheduler.cycle(), 0);
    scheduler.tick().unwrap();
    assert_eq!(scheduler.cycle(), 1);
    scheduler.tick().unwrap();
    assert_eq!(scheduler.cycle(), 2);
}

#[test]
fn test_empty_bb_leaves_scheduler_idle() {
    let config = SchedulingConfig::default();
    let fu_pool = FunctionalUnitPoolBuilder::new().build();
    let mut scheduler = InstructionScheduler::new(config, fu_pool);

    let bb = LLVMBasicBlock::new("empty".to_string());
    scheduler.schedule_basic_block(&bb).unwrap();

    // No instructions were added, scheduler should be idle
    assert!(scheduler.is_idle());
}

#[test]
fn test_terminator_in_bb_is_scheduled() {
    let config = SchedulingConfig::default();
    let fu_pool = FunctionalUnitPoolBuilder::new().build();
    let mut scheduler = InstructionScheduler::new(config, fu_pool);

    let mut bb = LLVMBasicBlock::new("entry".to_string());
    bb.set_terminator(LLVMInstruction::Ret { value: None });

    scheduler.schedule_basic_block(&bb).unwrap();

    // The Ret → Nop has zero sources, so it is placed in the reservation
    // table with active_parents=0; the scheduler is not yet idle because
    // it has not been ticked yet.
    assert!(!scheduler.is_idle());
}

#[test]
fn test_scheduler_idles_after_ret_nop_drains() {
    // A Ret instruction converts to MicroOp::Nop (latency=0, no sources).
    // It should be dispatched and completed within a handful of ticks.
    let config = SchedulingConfig {
        lockstep_mode: false,
        scheduling_threshold: 100,
        pipelined: true,
    };
    let fu_pool = FunctionalUnitPoolBuilder::new().build();
    let mut scheduler = InstructionScheduler::new(config, fu_pool);

    let mut bb = LLVMBasicBlock::new("entry".to_string());
    // Ret → Nop: zero sources, zero latency — will drain quickly.
    bb.set_terminator(LLVMInstruction::Ret { value: None });

    scheduler.schedule_basic_block(&bb).unwrap();
    assert!(!scheduler.is_idle());

    for _ in 0..10 {
        scheduler.tick().unwrap();
        if scheduler.is_idle() {
            break;
        }
    }

    assert!(scheduler.is_idle(), "scheduler should have drained the Nop");
}

#[test]
fn test_load_instruction_scheduled_appears_in_reservation() {
    let config = SchedulingConfig {
        lockstep_mode: false,
        scheduling_threshold: 100,
        pipelined: true,
    };
    let fu_pool = FunctionalUnitPoolBuilder::new().build();
    let mut scheduler = InstructionScheduler::new(config, fu_pool);

    let mut bb = LLVMBasicBlock::new("entry".to_string());
    // A Load with a constant pointer has a source register dep; it lands in
    // the reservation table first.
    bb.add_instruction(LLVMInstruction::Load {
        dest: LLVMValue::register("v".to_string(), 0),
        ptr: LLVMValue::register("p".to_string(), 1),
        ty: LLVMType::Integer { bits: 32 },
    });

    scheduler.schedule_basic_block(&bb).unwrap();

    // The instruction is in the reservation table (has 1 unresolved source)
    let (res, comp, load, _store) = scheduler.queue_sizes();
    assert_eq!(res, 1, "load should be in reservation table");
    assert_eq!(comp, 0);
    assert_eq!(load, 0);
}

#[test]
fn test_store_instruction_scheduled_appears_in_reservation() {
    let config = SchedulingConfig {
        lockstep_mode: false,
        scheduling_threshold: 100,
        pipelined: true,
    };
    let fu_pool = FunctionalUnitPoolBuilder::new().build();
    let mut scheduler = InstructionScheduler::new(config, fu_pool);

    let mut bb = LLVMBasicBlock::new("entry".to_string());
    // A Store has two source deps (value + ptr), so it waits in reservation.
    bb.add_instruction(LLVMInstruction::Store {
        value: LLVMValue::register("val".to_string(), 0),
        ptr: LLVMValue::register("ptr".to_string(), 1),
    });

    scheduler.schedule_basic_block(&bb).unwrap();

    // Instruction is in the reservation table (2 unresolved sources)
    let (res, comp, load, store) = scheduler.queue_sizes();
    assert_eq!(res, 1, "store should be in reservation table");
    assert_eq!(comp, 0);
    assert_eq!(load, 0);
    assert_eq!(store, 0);
}

#[test]
fn test_scheduling_config_default_values() {
    let cfg = SchedulingConfig::default();
    assert!(cfg.lockstep_mode);
    assert_eq!(cfg.scheduling_threshold, 10000);
    assert!(cfg.pipelined);
}

#[test]
fn test_non_lockstep_schedules_source_free_ops_independently() {
    // Use two unconditional branches (no sources) to verify that non-lockstep
    // mode dispatches both into compute queue on the first tick.
    let config = SchedulingConfig {
        lockstep_mode: false,
        scheduling_threshold: 100,
        pipelined: true,
    };
    let fu_pool = FunctionalUnitPoolBuilder::new().build();
    let mut scheduler = InstructionScheduler::new(config, fu_pool);

    // Two Br instructions → two MicroOp::Branch (no sources, latency 1)
    let mut bb = LLVMBasicBlock::new("entry".to_string());
    bb.add_instruction(LLVMInstruction::Br {
        target: "a".to_string(),
    });
    bb.add_instruction(LLVMInstruction::Br {
        target: "b".to_string(),
    });

    scheduler.schedule_basic_block(&bb).unwrap();

    // Both are in reservation table before any tick
    let (res, _, _, _) = scheduler.queue_sizes();
    assert_eq!(res, 2);

    // After one tick both should move to compute queue (active_parents=0)
    scheduler.tick().unwrap();

    let (res, comp, _load, _store) = scheduler.queue_sizes();
    assert_eq!(
        res, 0,
        "reservation table must be empty after both dispatched"
    );
    assert_eq!(comp, 2, "both branches should be in compute queue");
}
