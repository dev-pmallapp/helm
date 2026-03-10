//! Hardware accelerator device model
//!
//! This module provides the main Accelerator device that executes LLVM IR
//! in a cycle-accurate manner, inspired by gem5-SALAM's LLVMInterface.

use crate::error::{Error, Result};
use crate::functional_units::FunctionalUnitPoolBuilder;
use crate::ir::LLVMModule;
use crate::scheduler::{InstructionScheduler, SchedulingConfig};
use std::path::Path;

/// Hardware accelerator configuration
#[derive(Debug, Clone)]
pub struct AcceleratorConfig {
    pub scheduling: SchedulingConfig,
    pub scratchpad_size: usize,
    pub clock_period_ns: u32,
}

impl Default for AcceleratorConfig {
    fn default() -> Self {
        Self {
            scheduling: SchedulingConfig::default(),
            scratchpad_size: 65536, // 64KB default
            clock_period_ns: 10,    // 10ns = 100MHz
        }
    }
}

/// Hardware accelerator device
///
/// Executes LLVM IR in a cycle-accurate manner with configurable
/// functional units and memory hierarchy.
///
/// # Example
///
/// ```rust,ignore
/// let accel = Accelerator::from_file("matmul.ll")
///     .with_int_adders(4)
///     .with_fp_multipliers(8)
///     .with_scratchpad_size(65536)
///     .build()?;
/// ```
pub struct Accelerator {
    module: LLVMModule,
    scheduler: InstructionScheduler,
    config: AcceleratorConfig,

    /// Statistics
    total_cycles: u64,
    memory_loads: u64,
    memory_stores: u64,
}

impl Accelerator {
    /// Create accelerator from LLVM IR file
    pub fn from_file<P: AsRef<Path>>(path: P) -> AcceleratorBuilder {
        AcceleratorBuilder::new().with_ir_file(path)
    }

    /// Create accelerator from LLVM IR string
    pub fn from_string(ir: &str) -> AcceleratorBuilder {
        AcceleratorBuilder::new().with_ir_string(ir)
    }

    /// Run the accelerator simulation
    pub fn run(&mut self) -> Result<()> {
        log::info!("Starting accelerator simulation");

        // Find main function or entry point
        let main_fn = self
            .module
            .get_function("main")
            .or_else(|| self.module.functions.first())
            .ok_or_else(|| Error::Other("No entry function found".to_string()))?;

        // Schedule entry basic block
        if let Some(entry_bb) = main_fn.entry_block() {
            self.scheduler.schedule_basic_block(entry_bb)?;
        }

        // Run simulation until idle
        while !self.scheduler.is_idle() {
            self.scheduler.tick()?;
            self.total_cycles += 1;

            if self.total_cycles % 10000 == 0 {
                let (res, comp, load, store) = self.scheduler.queue_sizes();
                log::debug!(
                    "Cycle {}: Reservation={}, Compute={}, Load={}, Store={}",
                    self.total_cycles,
                    res,
                    comp,
                    load,
                    store
                );
            }
        }

        log::info!("Simulation complete: {} cycles", self.total_cycles);
        Ok(())
    }

    /// Get total cycles executed
    pub fn total_cycles(&self) -> u64 {
        self.total_cycles
    }

    /// Return the accelerator configuration.
    pub fn config(&self) -> &AcceleratorConfig {
        &self.config
    }

    /// Get statistics
    pub fn stats(&self) -> AcceleratorStats {
        AcceleratorStats {
            total_cycles: self.total_cycles,
            memory_loads: self.memory_loads,
            memory_stores: self.memory_stores,
        }
    }
}

/// Accelerator statistics
#[derive(Debug, Clone)]
pub struct AcceleratorStats {
    pub total_cycles: u64,
    pub memory_loads: u64,
    pub memory_stores: u64,
}

/// Builder for creating accelerators with gem5-SALAM style configuration
pub struct AcceleratorBuilder {
    ir_source: Option<IRSource>,
    fu_builder: FunctionalUnitPoolBuilder,
    config: AcceleratorConfig,
}

enum IRSource {
    File(String),
    String(String),
}

impl AcceleratorBuilder {
    pub fn new() -> Self {
        Self {
            ir_source: None,
            fu_builder: FunctionalUnitPoolBuilder::new(),
            config: AcceleratorConfig::default(),
        }
    }

    /// Load LLVM IR from file
    pub fn with_ir_file<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.ir_source = Some(IRSource::File(path.as_ref().to_string_lossy().to_string()));
        self
    }

    /// Load LLVM IR from string
    pub fn with_ir_string(mut self, ir: &str) -> Self {
        self.ir_source = Some(IRSource::String(ir.to_string()));
        self
    }

    /// Configure integer adders (gem5-SALAM style: -1 = unlimited)
    pub fn with_int_adders(mut self, count: i32) -> Self {
        self.fu_builder = self.fu_builder.with_int_adders(count, 1, true);
        self
    }

    /// Configure integer multipliers
    pub fn with_int_multipliers(mut self, count: i32) -> Self {
        self.fu_builder = self.fu_builder.with_int_multipliers(count, 3, true);
        self
    }

    /// Configure FP multipliers (single precision)
    pub fn with_fp_sp_multipliers(mut self, count: i32) -> Self {
        self.fu_builder = self.fu_builder.with_fp_sp_multipliers(count, 5, true);
        self
    }

    /// Configure FP multipliers (double precision)
    pub fn with_fp_dp_multipliers(mut self, count: i32) -> Self {
        self.fu_builder = self.fu_builder.with_fp_dp_multipliers(count, 5, true);
        self
    }

    /// Configure load/store units
    pub fn with_load_store_units(mut self, count: i32) -> Self {
        self.fu_builder = self.fu_builder.with_load_store_units(count, 2);
        self
    }

    /// Set scratchpad memory size
    pub fn with_scratchpad_size(mut self, size: usize) -> Self {
        self.config.scratchpad_size = size;
        self
    }

    /// Set lockstep mode
    pub fn with_lockstep_mode(mut self, lockstep: bool) -> Self {
        self.config.scheduling.lockstep_mode = lockstep;
        self
    }

    /// Set scheduling threshold
    pub fn with_scheduling_threshold(mut self, threshold: usize) -> Self {
        self.config.scheduling.scheduling_threshold = threshold;
        self
    }

    /// Build the accelerator
    pub fn build(self) -> Result<Accelerator> {
        // Load LLVM IR
        let module = match self.ir_source {
            Some(IRSource::File(path)) => LLVMModule::from_file(path)?,
            Some(IRSource::String(ir)) => LLVMModule::from_string(&ir)?,
            None => return Err(Error::Other("No LLVM IR source provided".to_string())),
        };

        // Build functional unit pool
        let functional_units = self.fu_builder.build();

        // Create scheduler
        let scheduler = InstructionScheduler::new(self.config.scheduling.clone(), functional_units);

        Ok(Accelerator {
            module,
            scheduler,
            config: self.config,
            total_cycles: 0,
            memory_loads: 0,
            memory_stores: 0,
        })
    }
}

impl Default for AcceleratorBuilder {
    fn default() -> Self {
        Self::new()
    }
}
