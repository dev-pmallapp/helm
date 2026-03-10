//! Executor trait — functionally executes a decoded instruction.

use crate::cpu::CpuState;
use crate::insn::{DecodedInsn, ExecOutcome};
use crate::mem::MemoryAccess;

/// Functionally executes a decoded instruction, mutating CPU and memory state.
///
/// For CISC instructions with REP prefix, a single `execute()` call performs
/// ONE iteration. The caller checks `outcome.rep_ongoing` and re-calls until
/// it returns `false`.
pub trait Executor: Send {
    fn execute(
        &mut self,
        insn: &DecodedInsn,
        cpu: &mut dyn CpuState,
        mem: &mut dyn MemoryAccess,
    ) -> ExecOutcome;
}
