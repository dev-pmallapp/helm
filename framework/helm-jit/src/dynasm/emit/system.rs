//! System instruction emitters — all unsupported, force interpreter fallback.
//!
//! System instructions (SVC, MRS, MSR, ERET, WFI, etc.) interact with
//! privileged state that the JIT cannot handle. Returning `None` from
//! `emit_system` causes the block compiler to stop and let the interpreter
//! handle the instruction.

#![allow(missing_docs)]

use dynasmrt::x64::Assembler;
use helm_arch::aarch64::insn::{Instruction, Opcode};

/// Attempt to emit a system instruction.
///
/// NOP is the only system instruction we handle (by doing nothing).
/// All others return `None` to terminate block compilation.
pub fn emit_system(_ops: &mut Assembler, insn: &Instruction) -> Option<bool> {
    match insn.opcode {
        // NOP: emit nothing, block continues
        Opcode::Nop => Some(false),
        // All other system instructions: unsupported in JIT
        _ => None,
    }
}
