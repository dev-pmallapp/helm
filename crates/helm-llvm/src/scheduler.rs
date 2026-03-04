//! Instruction scheduler for LLVM IR execution
//!
//! This module implements dynamic instruction scheduling similar to gem5-SALAM's
//! LLVMInterface. It handles dependency tracking, functional unit allocation,
//! and maintains in-flight instruction queues.

use crate::error::{Error, Result};
use crate::functional_units::{FunctionalUnitPool, FunctionalUnitType};
use crate::ir::{LLVMBasicBlock, LLVMInstruction, LLVMValue};
use crate::micro_op::{MicroOp, PhysReg};
use std::collections::{HashMap, VecDeque};

/// Scheduling configuration (gem5-SALAM inspired)
#[derive(Debug, Clone)]
pub struct SchedulingConfig {
    /// TRUE: Stall entire datapath if any operation stalls
    /// FALSE: Only stall dependent operations
    pub lockstep_mode: bool,

    /// Maximum scheduling window size (prevents explosion during high ILP)
    pub scheduling_threshold: usize,

    /// Whether functional units are pipelined
    pub pipelined: bool,
}

impl Default for SchedulingConfig {
    fn default() -> Self {
        Self {
            lockstep_mode: true,
            scheduling_threshold: 10000,
            pipelined: true,
        }
    }
}

/// Instruction in the reservation table
#[derive(Debug, Clone)]
pub struct ReservedInstruction {
    pub micro_op: MicroOp,
    pub active_parents: usize,
    pub cycle_issued: u64,
    pub cycles_remaining: u32,
}

/// Instruction scheduler implementing SALAM-style dynamic scheduling
pub struct InstructionScheduler {
    config: SchedulingConfig,
    functional_units: FunctionalUnitPool,

    /// Reservation table - instructions waiting to execute
    reservation_table: Vec<ReservedInstruction>,

    /// In-flight compute queue
    compute_queue: VecDeque<ReservedInstruction>,

    /// In-flight load queue
    load_queue: VecDeque<ReservedInstruction>,

    /// In-flight store queue
    store_queue: VecDeque<ReservedInstruction>,

    /// Current cycle
    cycle: u64,

    /// Dependency tracking: register -> instructions that depend on it
    dependencies: HashMap<PhysReg, Vec<usize>>,
}

impl InstructionScheduler {
    /// Create a new instruction scheduler
    pub fn new(config: SchedulingConfig, functional_units: FunctionalUnitPool) -> Self {
        Self {
            config,
            functional_units,
            reservation_table: Vec::new(),
            compute_queue: VecDeque::new(),
            load_queue: VecDeque::new(),
            store_queue: VecDeque::new(),
            cycle: 0,
            dependencies: HashMap::new(),
        }
    }

    /// Schedule a basic block of LLVM instructions
    pub fn schedule_basic_block(&mut self, bb: &LLVMBasicBlock) -> Result<()> {
        // Convert LLVM instructions to MicroOps and add to reservation table
        for inst in &bb.instructions {
            let micro_ops = crate::micro_op::llvm_to_micro_ops(
                inst,
                &mut crate::micro_op::ConversionContext::new(),
            );

            for micro_op in micro_ops {
                self.add_to_reservation_table(micro_op);
            }
        }

        // Handle terminator
        if let Some(term) = &bb.terminator {
            let micro_ops = crate::micro_op::llvm_to_micro_ops(
                term,
                &mut crate::micro_op::ConversionContext::new(),
            );
            for micro_op in micro_ops {
                self.add_to_reservation_table(micro_op);
            }
        }

        Ok(())
    }

    /// Add instruction to reservation table
    fn add_to_reservation_table(&mut self, micro_op: MicroOp) {
        let sources = micro_op.sources();
        let active_parents = sources.len();

        let reserved = ReservedInstruction {
            micro_op,
            active_parents,
            cycle_issued: 0,
            cycles_remaining: 0,
        };

        let idx = self.reservation_table.len();
        self.reservation_table.push(reserved);

        // Track dependencies
        for src_reg in sources {
            self.dependencies.entry(src_reg).or_default().push(idx);
        }
    }

    /// Execute one cycle of simulation
    pub fn tick(&mut self) -> Result<()> {
        self.cycle += 1;
        log::debug!("Cycle {}", self.cycle);

        // Update functional units
        self.functional_units.tick();

        // Check compute queue for completed operations
        self.check_compute_queue();

        // Check if we can schedule new instructions
        if self.can_schedule() {
            self.schedule_ready_instructions()?;
        }

        Ok(())
    }

    /// Check if we can schedule new instructions
    fn can_schedule(&self) -> bool {
        if self.config.lockstep_mode {
            // In lockstep mode, must wait for all queues to be empty
            self.compute_queue.is_empty()
                && self.load_queue.is_empty()
                && self.store_queue.is_empty()
        } else {
            // In non-lockstep mode, can always try to schedule
            true
        }
    }

    /// Schedule ready instructions from reservation table
    fn schedule_ready_instructions(&mut self) -> Result<()> {
        let mut scheduled_count = 0;

        // Iterate through reservation table
        let mut i = 0;
        while i < self.reservation_table.len() {
            if scheduled_count >= self.config.scheduling_threshold {
                break;
            }

            let inst = &self.reservation_table[i];

            // Check if instruction is ready (no active parents)
            if inst.active_parents == 0 {
                // Try to allocate functional unit
                let fu_type = inst.micro_op.functional_unit();
                if self.functional_units.try_allocate(fu_type) {
                    // Remove from reservation table and add to appropriate queue
                    let mut inst = self.reservation_table.remove(i);
                    inst.cycle_issued = self.cycle;
                    inst.cycles_remaining = inst.micro_op.default_latency();

                    match &inst.micro_op {
                        MicroOp::Load { .. } => {
                            self.load_queue.push_back(inst);
                        }
                        MicroOp::Store { .. } => {
                            self.store_queue.push_back(inst);
                        }
                        _ => {
                            self.compute_queue.push_back(inst);
                        }
                    }

                    scheduled_count += 1;
                    continue; // Don't increment i since we removed an element
                }
            }

            i += 1;
        }

        Ok(())
    }

    /// Check compute queue for completed operations
    fn check_compute_queue(&mut self) {
        let mut i = 0;
        while i < self.compute_queue.len() {
            let inst = &mut self.compute_queue[i];

            if inst.cycles_remaining > 0 {
                inst.cycles_remaining -= 1;
            }

            // Check if instruction is complete
            if inst.cycles_remaining == 0 {
                // Remove from queue
                let inst = self.compute_queue.remove(i).unwrap();

                // Update dependencies
                if let Some(dest) = inst.micro_op.dest() {
                    self.notify_completion(dest);
                }

                continue; // Don't increment i
            }

            i += 1;
        }
    }

    /// Notify dependent instructions that a register is ready
    fn notify_completion(&mut self, reg: PhysReg) {
        if let Some(dependent_indices) = self.dependencies.get(&reg) {
            for &idx in dependent_indices {
                if idx < self.reservation_table.len() {
                    self.reservation_table[idx].active_parents =
                        self.reservation_table[idx].active_parents.saturating_sub(1);
                }
            }
        }
    }

    /// Check if scheduler is idle
    pub fn is_idle(&self) -> bool {
        self.reservation_table.is_empty()
            && self.compute_queue.is_empty()
            && self.load_queue.is_empty()
            && self.store_queue.is_empty()
    }

    /// Get current cycle count
    pub fn cycle(&self) -> u64 {
        self.cycle
    }

    /// Get queue sizes for debugging/statistics
    pub fn queue_sizes(&self) -> (usize, usize, usize, usize) {
        (
            self.reservation_table.len(),
            self.compute_queue.len(),
            self.load_queue.len(),
            self.store_queue.len(),
        )
    }
}
