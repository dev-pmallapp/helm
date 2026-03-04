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
    SyscallEmulation,
    /// Cycle-accurate out-of-order microarchitectural simulation.
    Microarchitectural,
}

/// Supported ISA families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IsaKind {
    X86_64,
    RiscV64,
    Arm64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_mode_variants_are_distinct() {
        assert_ne!(ExecMode::SyscallEmulation, ExecMode::Microarchitectural);
    }

    #[test]
    fn isa_kind_variants_are_distinct() {
        assert_ne!(IsaKind::X86_64, IsaKind::RiscV64);
        assert_ne!(IsaKind::RiscV64, IsaKind::Arm64);
    }

    #[test]
    fn exec_mode_roundtrips_through_serde() {
        let mode = ExecMode::Microarchitectural;
        let json = serde_json::to_string(&mode).unwrap();
        let back: ExecMode = serde_json::from_str(&json).unwrap();
        assert_eq!(mode, back);
    }

    #[test]
    fn isa_kind_roundtrips_through_serde() {
        for isa in [IsaKind::X86_64, IsaKind::RiscV64, IsaKind::Arm64] {
            let json = serde_json::to_string(&isa).unwrap();
            let back: IsaKind = serde_json::from_str(&json).unwrap();
            assert_eq!(isa, back);
        }
    }
}
