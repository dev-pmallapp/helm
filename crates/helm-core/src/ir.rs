//! HELM Intermediate Representation (IR).
//!
//! ISA frontends decode guest instructions into `MicroOp`s — a uniform,
//! ISA-agnostic representation consumed by the pipeline backend.

use crate::types::{Addr, RegId};

/// A single micro-operation produced by an ISA frontend.
#[derive(Debug, Clone)]
pub struct MicroOp {
    /// Guest PC that produced this uop.
    pub guest_pc: Addr,
    /// Opcode tag for the pipeline.
    pub opcode: Opcode,
    /// Source register operands.
    pub sources: Vec<RegId>,
    /// Destination register operand (if any).
    pub dest: Option<RegId>,
    /// Immediate value (if any).
    pub immediate: Option<u64>,
    /// Flags and annotations.
    pub flags: MicroOpFlags,
}

/// High-level opcode categories recognised by the microarchitectural backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    IntAlu,
    IntMul,
    IntDiv,
    FpAlu,
    FpMul,
    FpDiv,
    Load,
    Store,
    Branch,
    CondBranch,
    Syscall,
    Nop,
    Fence,
    /// Catch-all for ISA-specific ops not yet categorised.
    Other(u16),
}

/// Bit-flags describing micro-op properties.
#[derive(Debug, Clone, Copy, Default)]
pub struct MicroOpFlags {
    pub is_serialising: bool,
    pub is_memory_barrier: bool,
    pub is_branch: bool,
    pub is_call: bool,
    pub is_return: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn micro_op_default_flags_are_false() {
        let flags = MicroOpFlags::default();
        assert!(!flags.is_branch);
        assert!(!flags.is_call);
        assert!(!flags.is_return);
        assert!(!flags.is_serialising);
        assert!(!flags.is_memory_barrier);
    }

    #[test]
    fn micro_op_can_be_constructed() {
        let uop = MicroOp {
            guest_pc: 0x1000,
            opcode: Opcode::IntAlu,
            sources: vec![1, 2],
            dest: Some(3),
            immediate: None,
            flags: MicroOpFlags::default(),
        };
        assert_eq!(uop.guest_pc, 0x1000);
        assert_eq!(uop.sources.len(), 2);
        assert_eq!(uop.dest, Some(3));
    }

    #[test]
    fn opcode_equality() {
        assert_eq!(Opcode::Load, Opcode::Load);
        assert_ne!(Opcode::Load, Opcode::Store);
        assert_eq!(Opcode::Other(42), Opcode::Other(42));
        assert_ne!(Opcode::Other(1), Opcode::Other(2));
    }
}
