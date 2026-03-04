//! The core trait that every ISA frontend must implement.

use helm_core::ir::MicroOp;
use helm_core::types::Addr;
use helm_core::HelmResult;

/// An ISA frontend decodes a stream of guest bytes into micro-ops.
pub trait IsaFrontend: Send + Sync {
    /// Human-readable ISA name (e.g. "riscv64", "x86_64").
    fn name(&self) -> &str;

    /// Decode the instruction at `pc` from the given byte slice.
    /// Returns the decoded micro-ops and the number of bytes consumed.
    fn decode(&self, pc: Addr, bytes: &[u8]) -> HelmResult<(Vec<MicroOp>, usize)>;

    /// Return the natural instruction alignment for this ISA (1 for x86, 2/4
    /// for RISC-V compressed / standard, 4 for ARM).
    fn min_insn_align(&self) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies the trait is object-safe (can be used as dyn).
    #[test]
    fn trait_is_object_safe() {
        fn _accepts_dyn(_f: &dyn IsaFrontend) {}
    }
}
