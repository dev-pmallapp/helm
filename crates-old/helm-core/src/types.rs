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
///
/// Orthogonal to [`AccuracyLevel`](helm_timing::AccuracyLevel) — any execution
/// mode can be combined with any timing fidelity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecMode {
    /// Syscall Emulation — user-space binary with Linux syscalls emulated.
    SE,
    /// Full System — boots a kernel image with devices, MMU, and interrupts.
    FS,
    /// Hardware-Assisted Emulation — near-native execution via KVM.
    HAE,
}

/// Supported ISA families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IsaKind {
    X86_64,
    RiscV64,
    Arm64,
}
