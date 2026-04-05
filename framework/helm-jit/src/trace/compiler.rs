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
use helm_stats::JitPerfStats;

use crate::block::{CompiledBlock, EXIT_END_OF_BLOCK};
use crate::dynasm::emit;
use crate::dynasm::fusion::try_fuse;
use crate::dynasm::pinned::load_guest_to_rax;
use crate::dynasm::pinned::{emit_pinned_epilogue, emit_pinned_prologue};
use crate::regs::{reg_offset, REG_PC, REG_X0, REG_XZR};

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

/// Record that a compiled trace was produced.
pub fn note_trace_compiled(stats: &mut JitPerfStats, trace: &CompiledTrace) {
    stats.traces_compiled = stats.traces_compiled.saturating_add(1);
    stats.trace_guest_insns = stats
        .trace_guest_insns
        .saturating_add(u64::from(trace.insn_count));
}

/// Record that a trace was executed once.
pub fn note_trace_executed(stats: &mut JitPerfStats) {
    stats.traces_executed = stats.traces_executed.saturating_add(1);
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
    match insn.opcode {
        Opcode::BCond => emit_guard_bcond(ops, insn.cond, exit_label),
        Opcode::Cbz => emit_guard_cbz(ops, insn, exit_label, true),
        Opcode::Cbnz => emit_guard_cbz(ops, insn, exit_label, false),
        Opcode::Tbz => emit_guard_tbz(ops, insn, exit_label, true),
        Opcode::Tbnz => emit_guard_tbz(ops, insn, exit_label, false),
        _ => unreachable!("guard emitter only supports conditional branches"),
    }
}

#[inline]
fn src_slot(reg: u32) -> usize {
    if reg == 31 {
        REG_XZR
    } else {
        REG_X0 + reg as usize
    }
}

fn emit_guard_bcond(ops: &mut Assembler, cond: u32, exit_label: dynasmrt::DynamicLabel) {
    use crate::dynasm::lazy_nzcv::emit_materialize_nzcv;
    emit_materialize_nzcv(ops);

    match cond {
        0 => dynasm!(ops ; bt ebp, 30 ; jc  =>exit_label), // EQ: Z==1
        1 => dynasm!(ops ; bt ebp, 30 ; jnc =>exit_label), // NE: Z==0
        2 => dynasm!(ops ; bt ebp, 29 ; jc  =>exit_label), // CS/HS: C==1
        3 => dynasm!(ops ; bt ebp, 29 ; jnc =>exit_label), // CC/LO: C==0
        4 => dynasm!(ops ; bt ebp, 31 ; jc  =>exit_label), // MI: N==1
        5 => dynasm!(ops ; bt ebp, 31 ; jnc =>exit_label), // PL: N==0
        6 => dynasm!(ops ; bt ebp, 28 ; jc  =>exit_label), // VS: V==1
        7 => dynasm!(ops ; bt ebp, 28 ; jnc =>exit_label), // VC: V==0
        8 => dynasm!(ops ; bt ebp, 29 ; jnc >skip_hi ; bt ebp, 30 ; jnc =>exit_label ; skip_hi:),
        9 => dynasm!(ops ; bt ebp, 29 ; jnc =>exit_label ; bt ebp, 30 ; jc =>exit_label),
        10 => dynasm!(ops
            ; mov eax, ebp
            ; shr eax, 31
            ; mov ecx, ebp
            ; shr ecx, 28
            ; xor eax, ecx
            ; test eax, 1
            ; jz =>exit_label
        ),
        11 => dynasm!(ops
            ; mov eax, ebp
            ; shr eax, 31
            ; mov ecx, ebp
            ; shr ecx, 28
            ; xor eax, ecx
            ; test eax, 1
            ; jnz =>exit_label
        ),
        12 => dynasm!(ops
            ; bt ebp, 30 ; jc >skip_gt
            ; mov eax, ebp
            ; shr eax, 31
            ; mov ecx, ebp
            ; shr ecx, 28
            ; xor eax, ecx
            ; test eax, 1
            ; jz =>exit_label
            ; skip_gt:
        ),
        13 => dynasm!(ops
            ; bt ebp, 30 ; jc =>exit_label
            ; mov eax, ebp
            ; shr eax, 31
            ; mov ecx, ebp
            ; shr ecx, 28
            ; xor eax, ecx
            ; test eax, 1
            ; jnz =>exit_label
        ),
        14 | 15 => dynasm!(ops ; jmp =>exit_label),
        _ => unreachable!(),
    }
}

fn emit_guard_cbz(
    ops: &mut Assembler,
    insn: &Instruction,
    exit_label: dynasmrt::DynamicLabel,
    branch_on_zero: bool,
) {
    load_guest_to_rax(ops, src_slot(insn.rd));
    if insn.sf {
        if branch_on_zero {
            dynasm!(ops ; test rax, rax ; jz =>exit_label);
        } else {
            dynasm!(ops ; test rax, rax ; jnz =>exit_label);
        }
    } else if branch_on_zero {
        dynasm!(ops ; test eax, eax ; jz =>exit_label);
    } else {
        dynasm!(ops ; test eax, eax ; jnz =>exit_label);
    }
}

fn emit_guard_tbz(
    ops: &mut Assembler,
    insn: &Instruction,
    exit_label: dynasmrt::DynamicLabel,
    branch_on_zero: bool,
) {
    load_guest_to_rax(ops, src_slot(insn.rn));
    let bit_pos = insn.imm2 as i8;
    if branch_on_zero {
        dynasm!(ops ; bt rax, bit_pos ; jnc =>exit_label);
    } else {
        dynasm!(ops ; bt rax, bit_pos ; jc =>exit_label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regs::REG_COUNT;
    use helm_arch::aarch64::insn::Instruction;

    fn make_add(pc: u64) -> Instruction {
        let mut i = Instruction::zeroed();
        i.opcode = Opcode::AddImm;
        i.pc = pc;
        i.rd = 1;
        i.rn = 1;
        i.imm = 1;
        i.sf = true;
        i
    }

    fn make_cbnz(pc: u64, target: u64, rt: u32) -> Instruction {
        let mut i = Instruction::zeroed();
        i.opcode = Opcode::Cbnz;
        i.pc = pc;
        i.rd = rt;
        i.imm = target.wrapping_sub(pc) as i64;
        i.sf = true;
        i
    }

    fn make_subs_bcond(pc: u64, target: u64, rn: u32, imm: i64, cond: u32) -> [Instruction; 2] {
        let mut subs = Instruction::zeroed();
        subs.opcode = Opcode::SubsImm;
        subs.pc = pc;
        subs.rd = 2; // Deliberately avoid CMP fusion; trace guards still handle fused pairs conservatively.
        subs.rn = rn;
        subs.imm = imm;
        subs.sf = true;

        let mut bcond = Instruction::zeroed();
        bcond.opcode = Opcode::BCond;
        bcond.pc = pc + 4;
        bcond.imm = target.wrapping_sub(pc + 4) as i64;
        bcond.cond = cond;

        [subs, bcond]
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

    #[test]
    fn note_trace_compiled_records_length() {
        let insns = vec![make_add(0x1000), make_add(0x1004)];
        let trace = compile_trace(&insns, 0x1000).expect("trace should compile");
        let mut stats = JitPerfStats::default();

        note_trace_compiled(&mut stats, &trace);
        note_trace_executed(&mut stats);

        assert_eq!(stats.traces_compiled, 1);
        assert_eq!(stats.trace_guest_insns, 2);
        assert_eq!(stats.traces_executed, 1);
    }

    #[test]
    fn forward_cbnz_guard_exits_on_taken_path() {
        let insns = vec![
            make_add(0x1000),
            make_cbnz(0x1004, 0x1010, 0),
            make_add(0x1008),
        ];
        let trace = compile_trace(&insns, 0x1000).expect("trace should compile");

        let mut regs = [0u64; REG_COUNT];
        regs[1] = 5;
        let exit = unsafe { (trace.block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, EXIT_END_OF_BLOCK);
        assert_eq!(regs[1], 7);
        assert_eq!(regs[REG_PC], 0x100c);

        let mut regs_taken = [0u64; REG_COUNT];
        regs_taken[0] = 1;
        regs_taken[1] = 5;
        let exit = unsafe { (trace.block.entry)(regs_taken.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, EXIT_GUARD_BASE);
        assert_eq!(regs_taken[1], 6);
        assert_eq!(regs_taken[REG_PC], 0x1010);
    }

    #[test]
    fn forward_bcond_guard_exits_on_taken_path() {
        let [subs, bcond] = make_subs_bcond(0x2000, 0x2010, 0, 1, 0);
        let insns = vec![subs, bcond, make_add(0x2008)];
        let trace = compile_trace(&insns, 0x2000).expect("trace should compile");

        let mut regs = [0u64; REG_COUNT];
        regs[0] = 2;
        regs[1] = 9;
        let exit = unsafe { (trace.block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, EXIT_END_OF_BLOCK);
        assert_eq!(regs[1], 10);
        assert_eq!(regs[REG_PC], 0x200c);

        let mut regs_taken = [0u64; REG_COUNT];
        regs_taken[0] = 1;
        regs_taken[1] = 9;
        let exit = unsafe { (trace.block.entry)(regs_taken.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, EXIT_GUARD_BASE);
        assert_eq!(regs_taken[1], 9);
        assert_eq!(regs_taken[REG_PC], 0x2010);
    }
}
