//! LLVM IR Frontend for HELM Accelerator Simulation
//!
//! This crate provides LLVM IR-based hardware accelerator modeling for HELM,
//! inspired by gem5-SALAM. It enables cycle-accurate simulation of custom
//! hardware accelerators specified in high-level languages (C/C++) and compiled
//! to LLVM IR.
//!
//! # Architecture
//!
//! ```text
//! Accelerator C/C++ → LLVM IR → MicroOps → Unified Pipeline
//! ```
//!
//! # Key Components
//!
//! - **LLVM IR Parser**: Parses LLVM IR from files or memory
//! - **Instruction Scheduler**: Schedules LLVM instructions based on dependencies
//! - **Functional Unit Pool**: Configurable hardware resources
//! - **Accelerator Device**: Hardware accelerator device model
//!
//! # Example
//!
//! ```rust,ignore
//! use helm_llvm::Accelerator;
//!
//! // Create accelerator from LLVM IR file
//! let mut accel = Accelerator::from_file("matmul.ll")
//!     .with_int_adders(4)
//!     .with_fp_multipliers(8)
//!     .with_scratchpad_size(65536)
//!     .build()?;
//!
//! // Execute simulation
//! accel.run()?;
//! ```

pub mod accelerator;
pub mod device_bridge;
pub mod error;
pub mod ir;
pub mod memory;
pub mod micro_op;
pub mod parser;
pub mod scheduler;
pub mod functional_units;
pub mod scratchpad;

pub use accelerator::Accelerator;
pub use device_bridge::AcceleratorDevice;
pub use error::{Error, Result};
pub use ir::{LLVMModule, LLVMInstruction, LLVMBasicBlock, LLVMValue, LLVMType};
pub use memory::{MemoryBackend, SimpleMemory, HybridMemory};
pub use micro_op::MicroOp;
pub use scheduler::{InstructionScheduler, SchedulingConfig};
pub use functional_units::{FunctionalUnitPool, FunctionalUnitType, FunctionalUnit};
pub use scratchpad::{ScratchpadMemory, ScratchpadConfig, ScratchpadStats};

/// LLVM IR frontend version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests;
