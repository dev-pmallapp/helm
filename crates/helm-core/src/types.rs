//! Fundamental type aliases used throughout HELM.

use serde::{Deserialize, Serialize};

/// Virtual or physical address.
pub type Addr = u64;

/// Machine word (widest supported).
pub type Word = u64;

/// Logical register identifier.
pub type RegId = u16;

/// Cycle counter.
pub type Cycle = u64;

/// Execution mode selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecMode {
    /// Fast functional emulation via dynamic translation.
    SE,
    /// Cycle-accurate out-of-order microarchitectural simulation.
    CAE,
}

/// Supported ISA families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IsaKind {
    X86_64,
    RiscV64,
    Arm64,
}
