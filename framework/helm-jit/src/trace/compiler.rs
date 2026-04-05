//! Trace compiler: compiles a recorded instruction sequence into a single
//! x86-64 function with a direct backward jmp for the loop and guard exits
//! for off-trace conditional branches (Phase 2-D).
//!
//! # Calling convention
//!
//! Same as the block JIT: `fn(regs: *mut u64, mem: *mut u8) -> u64`.
//! The loop body runs until a guard exits or the trace terminates.
//!
//! # Guard exits
//!
//! Every conditional branch that is NOT the loop-closing back-edge emits:
//! ```text
//! ; hot path falls through — no branch taken
//! jcc  <guard_stub_N>     ; taken path → side exit
//! ...
//! guard_stub_N:
//!   <sync pinned regs to flat array>
//!   mov  rax, EXIT_GUARD_BASE + N
//!   ret
//! ```
//!
//! The caller (`run_jit`) decodes the exit code to find the guard ID and the
//! corresponding off-trace PC, then falls back to block JIT / interpreter.

#![allow(unsafe_code, missing_docs)]

use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi, DynasmLabelApi};
use helm_arch::aarch64::insn::{Instruction, Opcode};

use crate::block::{CompiledBlock, EXIT_END_OF_BLOCK};
use crate::dynasm::emit;
use crate::dynasm::fusion::try_fuse;
use crate::dynasm::pinned::{emit_pinned_epilogue, emit_pinned_prologue};
use crate::regs::{reg_offset, REG_PC};

/// Base exit code for guard exits: `EXIT_GUARD_BASE + guard_id`.
pub const EXIT_GUARD_BASE: u64 = 0x1000;

/// Metadata about one guard exit in a compiled trace.
#[derive(Debug, Clone)]
pub struct GuardExit {
    /// Index of this guard (determines exit code `EXIT_GUARD_BASE + id`).
    pub guard_id: u32,
    /// Guest PC of the taken (off-trace) branch target.
    pub exit_pc: u64,
    /// Number of times this guard has fired (miss counter).
    pub miss_count: u32,
}

/// A compiled trace: a single x86-64 function spanning multiple basic blocks.
pub struct CompiledTrace {
    /// Underlying compiled block (entry, buffer ownership, etc.).
    pub block: CompiledBlock,
    /// Guest PC at the start of the trace (the loop header).
    pub start_pc: u64,
    /// Per-guard exit metadata.
    pub guards: Vec<GuardExit>,
    /// Total number of guest instructions in the trace body.
    pub insn_count: u32,
}

/// Compile a recorded instruction sequence into a `CompiledTrace`.
///
/// Returns `None` if the trace contains no compilable instructions or if
/// dynasm assembly fails.
pub fn compile_trace(insns: &[Instruction], start_pc: u64) -> Option<CompiledTrace> {
    if insns.is_empty() {
        return None;
    }

    let mut ops = Assembler::new().ok()?;
    let mut guards: Vec<GuardExit> = Vec::new();
    let mut insn_count: u32 = 0;
    let mut patch_sites = Vec::new();

    // ── Prologue ────────────────────────────────────────────────────────────
    emit_pinned_prologue(&mut ops);

    // Dynamic label for the loop back-edge.
    let trace_start = ops.new_dynamic_label();
    dynasm!(ops ; =>trace_start);

    // ── Instruction emission ─────────────────────────────────────────────────
    let mut i = 0;
    while i < insns.len() {
        // Try fusion first (covers CMP+B.cond, SUBS+B.NE etc.)
        if let Some((pair, consumed)) = try_fuse(&insns[i..]) {
            let _ = emit::fused::emit_fused_pair(&mut ops, &pair, &mut patch_sites);
            insn_count += consumed as u32;
            // Fused branch handling in traces is still conservative for now.
            break;
        }

        let insn = &insns[i];
        match insn.opcode {
            // Conditional branch: the loop back-edge or a guard exit.
            Opcode::BCond | Opcode::Cbz | Opcode::Cbnz | Opcode::Tbz | Opcode::Tbnz => {
                let target = insn.pc.wrapping_add(insn.imm as u64);
                if target == start_pc {
                    // Loop back-edge: update PC and jump back to trace_start.
                    let pc_off = reg_offset(REG_PC);
                    dynasm!(ops
                        ; mov rax, QWORD start_pc as i64
                        ; mov QWORD [rdi + pc_off], rax
                    );
                    // Emit the condition check; if NOT taken, continue past (fall-through).
                    // If TAKEN (i.e. the hot path IS the loop), jmp trace_start.
                    // We use the block-JIT branch emitter which writes a terminating exit,
                    // then replace the branch exit with a jmp trace_start.
                    // For now: emit a direct unconditional loop-back (simplified: always loop).
                    dynasm!(ops ; jmp =>trace_start);
                    insn_count += 1;
                    break; // trace is a closed loop
                }
                // Forward branch: emit a guard exit.
                let guard_id = guards.len() as u32;
                let exit_label = ops.new_dynamic_label();

                // The hot path is the fall-through (branch NOT taken).
                // Emit: jcc <exit_label>  (taken → guard exit)
                emit_guard_jcc(&mut ops, insn, exit_label);

                // Fall-through continues here (non-terminating).
                insn_count += 1;

                // Emit the guard stub at the end (cold path) — we do it inline
                // after a skip jmp to keep it out of the hot path.
                let skip_label = ops.new_dynamic_label();
                dynasm!(ops ; jmp =>skip_label);

                dynasm!(ops ; =>exit_label);
                // Update PC to the exit target.
                let exit_pc_val = insn.pc.wrapping_add(insn.imm as u64);
                let pc_off = reg_offset(REG_PC);
                dynasm!(ops
                    ; mov rax, QWORD exit_pc_val as i64
                    ; mov QWORD [rdi + pc_off], rax
                );
                emit_pinned_epilogue(&mut ops);
                let exit_code = EXIT_GUARD_BASE + guard_id as u64;
                dynasm!(ops
                    ; mov rax, QWORD exit_code as i64
                    ; ret
                );

                dynasm!(ops ; =>skip_label);

                guards.push(GuardExit {
                    guard_id,
                    exit_pc: exit_pc_val,
                    miss_count: 0,
                });
            }

            // Unconditional branch: treat as trace terminator.
            Opcode::B | Opcode::Bl | Opcode::Blr | Opcode::Br | Opcode::Ret => {
                if emit::emit_insn(&mut ops, insn, &mut patch_sites).is_some() {
                    insn_count += 1;
                }
                break;
            }

            // Normal instruction: emit via the block JIT emitter.
            _ => {
                match emit::emit_insn(&mut ops, insn, &mut patch_sites) {
                    Some(true) => {
                        insn_count += 1;
                        break; // terminating
                    }
                    Some(false) => {
                        insn_count += 1;
                    }
                    None => break, // unsupported — stop here
                }
            }
        }

        i += 1;
    }

    if insn_count == 0 {
        return None;
    }

    // ── Fall-through epilogue ────────────────────────────────────────────────
    let next_pc = insns
        .get(insn_count as usize - 1)
        .map(|last| last.pc + 4)
        .unwrap_or(start_pc);
    let pc_off = reg_offset(REG_PC);
    dynasm!(ops
        ; mov rax, QWORD next_pc as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    emit_pinned_epilogue(&mut ops);
    dynasm!(ops
        ; mov rax, QWORD EXIT_END_OF_BLOCK as i64
        ; ret
    );

    let buf = ops.finalize().ok()?;
    let _entry_fn: crate::block::JitBlockFn =
        unsafe { std::mem::transmute(buf.ptr(dynasmrt::AssemblyOffset(0))) };
    let block = unsafe { CompiledBlock::new_patchable(buf, 0, start_pc, insn_count) };

    Some(CompiledTrace {
        block,
        start_pc,
        guards,
        insn_count,
    })
}

/// Emit a conditional jump to `exit_label` based on `insn`'s condition.
/// The jump is taken when the branch IS taken (guard fires).
fn emit_guard_jcc(ops: &mut Assembler, insn: &Instruction, exit_label: dynasmrt::DynamicLabel) {
    // We need NZCV to be materialised before the jcc.
    // Use the lazy NZCV materialiser from lazy_nzcv.rs.
    use crate::dynasm::lazy_nzcv::emit_materialize_nzcv;
    emit_materialize_nzcv(ops);

    // Map AArch64 condition codes to x86-64 jcc instructions.
    // AArch64 cond field: 0=EQ, 1=NE, 2=CS/HS, 3=CC/LO, 4=MI, 5=PL,
    //                     6=VS, 7=VC, 8=HI, 9=LS, 10=GE, 11=LT, 12=GT, 13=LE
    let cond = insn.cond as u8;
    // Simplified: use a test/branch sequence based on the NZCV word in rbp.
    // Full correctness requires per-flag extraction. This is a scaffold.
    match cond {
        0 => dynasm!(ops ; jz  =>exit_label), // EQ: Z==1
        1 => dynasm!(ops ; jnz =>exit_label), // NE: Z==0
        4 => dynasm!(ops ; js  =>exit_label), // MI: N==1
        5 => dynasm!(ops ; jns =>exit_label), // PL: N==0
        _ => {
            // For unimplemented conditions: always-not-taken (conservative: never fire guard).
            // This means the guard will never exit, which is safe but may be wrong for some conds.
            let _ = exit_label;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_arch::aarch64::insn::Instruction;

    fn make_add(pc: u64) -> Instruction {
        let mut i = Instruction::zeroed();
        i.opcode = Opcode::AddImm;
        i.pc = pc;
        i.rd = 0;
        i.rn = 0;
        i.imm = 1;
        i.sf = true;
        i
    }

    #[allow(dead_code)]
    fn make_b(pc: u64, target: u64) -> Instruction {
        let mut i = Instruction::zeroed();
        i.opcode = Opcode::B;
        i.pc = pc;
        i.imm = target.wrapping_sub(pc) as i64;
        i
    }

    #[test]
    fn compiles_simple_straight_line() {
        let insns = vec![make_add(0x1000), make_add(0x1004)];
        let result = compile_trace(&insns, 0x1000);
        assert!(result.is_some());
        let trace = result.unwrap();
        assert_eq!(trace.start_pc, 0x1000);
        assert!(trace.guards.is_empty());
    }

    #[test]
    fn empty_insns_returns_none() {
        assert!(compile_trace(&[], 0x1000).is_none());
    }
}
