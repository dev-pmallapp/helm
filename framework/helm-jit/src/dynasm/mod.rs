//! Dynasm-rs JIT backend — translates decoded AArch64 instructions into
//! x86-64 machine code using dynasm-rs.
//!
//! # Calling convention
//!
//! The compiled block is called as:
//! ```text
//! extern "C" fn(regs: *mut u64, mem: *mut u8) -> u64
//! ```
//! - `rdi` = pointer to flat register array (`[u64; 48]`)
//! - `rsi` = pointer to `FlatMem` (passed through to memory helpers)
//! - Returns exit code in `rax` (`EXIT_*` constants from `block.rs`)

#![allow(missing_docs)]
#![allow(unsafe_code)]

use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi};
use helm_arch::aarch64::insn::Instruction;

use crate::backend::JitBackend;
use crate::block::{CompiledBlock, EXIT_END_OF_BLOCK};
use crate::regs::{reg_offset, REG_PC};

pub mod emit;
pub mod fusion;
pub mod lazy_nzcv;
pub mod pinned;
pub mod rv64;

use emit::fused::emit_fused_pair;
use fusion::try_fuse;
use pinned::{emit_pinned_epilogue, emit_pinned_prologue};

/// Maximum number of guest instructions per compiled block.
const MAX_BLOCK_INSNS: usize = 64;

/// Dynasm-rs JIT backend.
pub struct DynasmBackend;

impl DynasmBackend {
    /// Create a new dynasm backend instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for DynasmBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl JitBackend for DynasmBackend {
    fn compile_block(&mut self, pc: u64, insns: &[Instruction]) -> Option<CompiledBlock> {
        compile_block(pc, insns)
    }

    fn name(&self) -> &str {
        "dynasm"
    }
}

/// Compile a block of decoded AArch64 instructions into x86-64 machine code.
///
/// # Arguments
/// - `pc`: guest PC at the start of the block
/// - `insns`: slice of pre-decoded instructions starting at `pc`
///   (must be at least 1 instruction; the slice may be longer than needed)
///
/// # Returns
/// - `Some(CompiledBlock)` on success (at least one instruction compiled)
/// - `None` if the first instruction is unsupported
pub fn compile_block(pc: u64, insns: &[Instruction]) -> Option<CompiledBlock> {
    if insns.is_empty() {
        return None;
    }

    let mut ops = Assembler::new().ok()?;
    let mut insn_count: u32 = 0;
    let mut patch_sites = Vec::new();

    // ── Prologue: save callee-saved regs, load pinned guest regs ───────────
    emit_pinned_prologue(&mut ops);

    // ── Instruction emission ────────────────────────────────────────────────
    let mut i = 0;
    while i < insns.len().min(MAX_BLOCK_INSNS) {
        // Try instruction fusion before single-instruction emit.
        if let Some((pair, consumed)) = try_fuse(&insns[i..]) {
            let terminates = emit_fused_pair(&mut ops, &pair, &mut patch_sites);
            insn_count += consumed as u32;
            if terminates {
                break;
            }
            i += consumed;
            continue;
        }

        let insn = &insns[i];
        match emit::emit_insn(&mut ops, insn, &mut patch_sites) {
            Some(true) => {
                // Block-terminating instruction (branch). The emitter already
                // wrote the PC update, exit code, and `ret`.
                insn_count += 1;
                break;
            }
            Some(false) => {
                // Non-terminating instruction.
                insn_count += 1;
            }
            None => {
                // Unsupported opcode — stop compilation here.
                break;
            }
        }
        i += 1;
    }

    if insn_count == 0 {
        return None;
    }

    // ── Epilogue (fall-through case) ────────────────────────────────────────
    // Update PC to the instruction after the last compiled instruction.
    let next_pc = pc + u64::from(insn_count) * 4;
    let pc_off = reg_offset(REG_PC);

    dynasm!(ops
        ; mov rax, QWORD next_pc as i64
        ; mov QWORD [rdi + pc_off], rax
    );
    // Flush pinned regs and pop callee-saved before returning.
    emit_pinned_epilogue(&mut ops);
    dynasm!(ops
        ; mov rax, QWORD EXIT_END_OF_BLOCK as i64
    );

    // ── 5-byte patch site for block chaining (Phase 2-B) ───────────────────
    // Emit `ret + nop×4` as the default unlinked exit.
    // When the target block (guest PC = next_pc) is compiled, JitCache will
    // overwrite these 5 bytes with `jmp rel32` pointing at the target.
    let patch_offset = ops.offset().0; // byte offset of the patch site
    dynasm!(ops
        ; ret
        ; nop
        ; nop
        ; nop
        ; nop
    );

    let buf = ops.finalize().ok()?;
    let mut block = unsafe { CompiledBlock::new_patchable(buf, 0, pc, insn_count) };

    patch_sites.push(crate::block::PatchSite {
        byte_offset: patch_offset,
        target_pc: next_pc,
        linked: false,
    });
    block.patch_sites = patch_sites;

    Some(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_arch::aarch64::insn::{Instruction, Opcode};

    fn make_nop(pc: u64) -> Instruction {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::Nop;
        insn.pc = pc;
        insn
    }

    fn make_add_imm(pc: u64, rd: u32, rn: u32, imm: i64) -> Instruction {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::AddImm;
        insn.pc = pc;
        insn.rd = rd;
        insn.rn = rn;
        insn.imm = imm;
        insn.sf = true;
        insn
    }

    fn make_subs_imm(pc: u64, rd: u32, rn: u32, imm: i64) -> Instruction {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::SubsImm;
        insn.pc = pc;
        insn.rd = rd;
        insn.rn = rn;
        insn.imm = imm;
        insn.sf = true;
        insn
    }

    fn make_bcond(pc: u64, cond: u32, imm: i64) -> Instruction {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::BCond;
        insn.pc = pc;
        insn.cond = cond;
        insn.imm = imm;
        insn
    }

    fn make_cbnz(pc: u64, rt: u32, imm: i64) -> Instruction {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::Cbnz;
        insn.pc = pc;
        insn.rd = rt;
        insn.imm = imm;
        insn.sf = true;
        insn
    }

    #[test]
    fn compile_single_nop() {
        let insns = [make_nop(0x1000)];
        let block = compile_block(0x1000, &insns);
        assert!(block.is_some());
        let block = block.unwrap();
        assert_eq!(block.guest_pc, 0x1000);
        assert_eq!(block.insn_count, 1);
    }

    #[test]
    fn compile_add_sequence() {
        let insns = [
            make_add_imm(0x2000, 0, 0, 1),
            make_add_imm(0x2004, 1, 1, 2),
            make_add_imm(0x2008, 2, 2, 3),
        ];
        let block = compile_block(0x2000, &insns);
        assert!(block.is_some());
        let block = block.unwrap();
        assert_eq!(block.insn_count, 3);
    }

    #[test]
    fn unsupported_first_insn_returns_none() {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::Mrs; // System instruction → unsupported
        insn.pc = 0x3000;
        let insns = [insn];
        assert!(compile_block(0x3000, &insns).is_none());
    }

    #[test]
    fn empty_insns_returns_none() {
        assert!(compile_block(0x4000, &[]).is_none());
    }

    #[test]
    fn execute_movz_64() {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::Movz;
        insn.pc = 0x1000;
        insn.rd = 0;
        insn.imm = (0x1234_u64 << 16) as i64;
        insn.imm2 = 1;
        insn.sf = true;

        let block = compile_block(0x1000, &[insn]).unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[0], 0x1234_0000);
        assert_eq!(regs[crate::regs::REG_PC], 0x1004);
    }

    #[test]
    fn execute_movn_32() {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::Movn;
        insn.pc = 0x1000;
        insn.rd = 0;
        insn.imm = -1;
        insn.imm2 = 0;
        insn.sf = false;

        let block = compile_block(0x1000, &[insn]).unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(
            regs[0], 0xFFFF_FFFF,
            "MOVN W0, #0 should give 0xFFFFFFFF, got {:#x}",
            regs[0]
        );
    }

    #[test]
    fn execute_add_imm_modifies_reg() {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::AddImm;
        insn.pc = 0x1000;
        insn.rd = 1;
        insn.rn = 0;
        insn.imm = 42;
        insn.sf = true;

        let block = compile_block(0x1000, &[insn]).unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[0] = 100;
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[1], 142);
    }

    #[test]
    fn execute_subs_imm_sets_nzcv() {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::SubsImm;
        insn.pc = 0x1000;
        insn.rd = 0;
        insn.rn = 0;
        insn.imm = 100;
        insn.sf = true;

        let block = compile_block(0x1000, &[insn]).unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[0] = 100;
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[0], 0);
        let nzcv = regs[crate::regs::REG_NZCV] as u32;
        assert!(
            nzcv & (1 << 30) != 0,
            "Z flag should be set, nzcv={nzcv:#x}"
        );
        assert!(
            nzcv & (1 << 29) != 0,
            "C flag should be set (no borrow), nzcv={nzcv:#x}"
        );
        assert!(
            nzcv & (1 << 31) == 0,
            "N flag should be clear, nzcv={nzcv:#x}"
        );
        assert!(
            nzcv & (1 << 28) == 0,
            "V flag should be clear, nzcv={nzcv:#x}"
        );
    }

    #[test]
    fn conditional_branch_fallthrough_keeps_following_insns_in_block() {
        let insns = [make_cbnz(0x1000, 0, 0x10), make_add_imm(0x1004, 1, 1, 1)];
        let block = compile_block(0x1000, &insns).unwrap();
        assert_eq!(block.insn_count, 2);

        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[1] = 5;
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, EXIT_END_OF_BLOCK);
        assert_eq!(regs[1], 6);
        assert_eq!(regs[crate::regs::REG_PC], 0x1008);

        let mut regs_taken = [0u64; crate::regs::REG_COUNT];
        regs_taken[0] = 1;
        regs_taken[1] = 5;
        let exit = unsafe { (block.entry)(regs_taken.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, EXIT_END_OF_BLOCK);
        assert_eq!(regs_taken[1], 5);
        assert_eq!(regs_taken[crate::regs::REG_PC], 0x1010);
    }

    #[test]
    fn fused_subs_bne_fallthrough_keeps_following_insns_in_block() {
        let insns = [
            make_subs_imm(0x2000, 0, 0, 1),
            make_bcond(0x2004, 1, 0x10),
            make_add_imm(0x2008, 1, 1, 1),
        ];
        let block = compile_block(0x2000, &insns).unwrap();
        assert_eq!(block.insn_count, 3);

        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[0] = 1;
        regs[1] = 7;
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, EXIT_END_OF_BLOCK);
        assert_eq!(regs[0], 0);
        assert_eq!(regs[1], 8);
        assert_eq!(regs[crate::regs::REG_PC], 0x200c);

        let mut regs_taken = [0u64; crate::regs::REG_COUNT];
        regs_taken[0] = 2;
        regs_taken[1] = 7;
        let exit = unsafe { (block.entry)(regs_taken.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, EXIT_END_OF_BLOCK);
        assert_eq!(regs_taken[0], 1);
        assert_eq!(regs_taken[1], 7);
        assert_eq!(regs_taken[crate::regs::REG_PC], 0x2014);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // JIT vs Interpreter differential correctness tests
    // ═══════════════════════════════════════════════════════════════════════

    use crate::regs::{arch_to_flat, flat_to_arch};
    use helm_arch::aarch64::arch_state::Aarch64ArchState;
    use helm_arch::aarch64::decode::decode as aarch64_decode;
    use helm_arch::aarch64::execute::execute as aarch64_execute;
    use helm_core::{AccessType, MemFault, MemInterface};

    struct NullMem;

    impl MemInterface for NullMem {
        fn read(&mut self, _addr: u64, _size: usize, _ty: AccessType) -> Result<u64, MemFault> {
            Ok(0)
        }
        fn write(
            &mut self,
            _addr: u64,
            _size: usize,
            _val: u64,
            _ty: AccessType,
        ) -> Result<(), MemFault> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct InitState {
        x: [u64; 31],
        sp: u64,
        nzcv: u32,
    }

    impl Default for InitState {
        fn default() -> Self {
            Self {
                x: [0u64; 31],
                sp: 0x0000_7FFF_FFF0_0000,
                nzcv: 0,
            }
        }
    }

    fn assert_jit_matches_interpreter(raw: u32, pc: u64, init: &InitState, label: &str) {
        let insn = aarch64_decode(raw, pc)
            .unwrap_or_else(|e| panic!("[{label}] decode failed for {raw:#010x}: {e}"));

        let Some(block) = compile_block(pc, &[insn]) else {
            eprintln!(
                "[{label}] JIT unsupported opcode {:?}, skipping",
                insn.opcode
            );
            return;
        };

        let mut interp_state = Aarch64ArchState::default();
        for i in 0..31 {
            interp_state.x[i] = init.x[i];
        }
        interp_state.sp = init.sp;
        interp_state.pc = pc;
        interp_state.nzcv = init.nzcv;

        let mut interp_mem = NullMem;
        let pc_written = aarch64_execute(&insn, &mut interp_state, &mut interp_mem)
            .unwrap_or_else(|e| panic!("[{label}] interpreter execute failed: {e}"));
        if !pc_written {
            interp_state.pc = pc + 4;
        }

        let mut jit_state = Aarch64ArchState::default();
        for i in 0..31 {
            jit_state.x[i] = init.x[i];
        }
        jit_state.sp = init.sp;
        jit_state.pc = pc;
        jit_state.nzcv = init.nzcv;
        let mut flat = arch_to_flat(&jit_state);

        let exit = unsafe { (block.entry)(flat.as_mut_ptr(), std::ptr::null_mut()) };

        flat_to_arch(&mut flat, &mut jit_state);

        let mut mismatches = Vec::new();

        for i in 0..31 {
            if jit_state.x[i] != interp_state.x[i] {
                mismatches.push(format!(
                    "  X{i}: JIT={:#018x}  interp={:#018x}",
                    jit_state.x[i], interp_state.x[i]
                ));
            }
        }

        if jit_state.sp != interp_state.sp {
            mismatches.push(format!(
                "  SP: JIT={:#018x}  interp={:#018x}",
                jit_state.sp, interp_state.sp
            ));
        }

        if jit_state.pc != interp_state.pc {
            mismatches.push(format!(
                "  PC: JIT={:#018x}  interp={:#018x}",
                jit_state.pc, interp_state.pc
            ));
        }

        if jit_state.nzcv != interp_state.nzcv {
            let jn = (jit_state.nzcv >> 31) & 1;
            let jz = (jit_state.nzcv >> 30) & 1;
            let jc = (jit_state.nzcv >> 29) & 1;
            let jv = (jit_state.nzcv >> 28) & 1;
            let in_ = (interp_state.nzcv >> 31) & 1;
            let iz = (interp_state.nzcv >> 30) & 1;
            let ic = (interp_state.nzcv >> 29) & 1;
            let iv = (interp_state.nzcv >> 28) & 1;
            mismatches.push(format!(
                "  NZCV: JIT={:#010x} (N={jn} Z={jz} C={jc} V={jv})  \
                 interp={:#010x} (N={in_} Z={iz} C={ic} V={iv})",
                jit_state.nzcv, interp_state.nzcv
            ));
        }

        assert!(
            mismatches.is_empty(),
            "\n\n[{label}] JIT vs interpreter MISMATCH\n\
             Instruction: {raw:#010x} at pc={pc:#x}\n\
             Opcode: {:?}\n\
             JIT exit code: {exit}\n\
             Mismatched registers:\n{}\n",
            insn.opcode,
            mismatches.join("\n")
        );
    }

    #[test]
    fn jit_vs_interp_movz_x12_0x1b00() {
        assert_jit_matches_interpreter(
            0xd283600c,
            0x400560,
            &InitState::default(),
            "MOVZ X12, #0x1B00",
        );
    }

    #[test]
    fn jit_vs_interp_add_x0_x0_0xcf0() {
        let mut init = InitState::default();
        init.x[0] = 0x0040_0000;
        assert_jit_matches_interpreter(0x9133c000, 0x400564, &init, "ADD X0, X0, #0xCF0");
    }

    #[test]
    fn jit_vs_interp_movz_w3_0() {
        let mut init = InitState::default();
        init.x[3] = 0xDEAD_BEEF_CAFE_BABE;
        assert_jit_matches_interpreter(0x52800003, 0x400568, &init, "MOVZ W3, #0");
    }

    #[test]
    fn jit_vs_interp_mov_x29_sp() {
        let init = InitState::default();
        assert_jit_matches_interpreter(
            0x910003fd,
            0x40056c,
            &init,
            "MOV X29, SP (ADD X29, SP, #0)",
        );
    }

    #[test]
    fn jit_vs_interp_movn_w0_neg1() {
        assert_jit_matches_interpreter(
            0x12800000,
            0x400570,
            &InitState::default(),
            "MOVN W0, #0 (MOV W0, #-1)",
        );
    }

    #[test]
    fn jit_vs_interp_add_imm_0x91032ac0() {
        let mut init = InitState::default();
        init.x[22] = 0x1000;
        assert_jit_matches_interpreter(
            0x91032ac0,
            0x400574,
            &init,
            "ADD (0x91032AC0) with X22 preset",
        );
    }

    #[test]
    fn jit_vs_interp_cmp_x0_0() {
        let mut init = InitState::default();
        init.x[0] = 42;
        assert_jit_matches_interpreter(
            0xF100001F,
            0x400578,
            &init,
            "CMP X0, #0 (SUBS XZR, X0, #0) with X0=42",
        );
    }

    #[test]
    fn jit_vs_interp_cmp_x0_0_zero_result() {
        let init = InitState::default();
        assert_jit_matches_interpreter(
            0xF100001F,
            0x40057c,
            &init,
            "CMP X0, #0 (SUBS XZR, X0, #0) with X0=0 => Z=1",
        );
    }

    #[test]
    fn jit_vs_interp_subs_x0_x0_1_underflow() {
        let init = InitState::default();
        assert_jit_matches_interpreter(
            0xF1000400,
            0x400580,
            &init,
            "SUBS X0, X0, #1 with X0=0 => negative wrap",
        );
    }

    #[test]
    fn jit_vs_interp_cmp_w1_5() {
        let mut init = InitState::default();
        init.x[1] = 10;
        assert_jit_matches_interpreter(
            0x7100143F,
            0x400584,
            &init,
            "CMP W1, #5 (32-bit SUBS) with W1=10",
        );
    }

    #[test]
    fn jit_vs_interp_adds_x0_x1_1_max() {
        let mut init = InitState::default();
        init.x[1] = u64::MAX;
        assert_jit_matches_interpreter(
            0xB1000420,
            0x400588,
            &init,
            "ADDS X0, X1, #1 with X1=MAX => carry+zero",
        );
    }

    #[test]
    fn jit_vs_interp_sub_x0_x0_0x10() {
        let mut init = InitState::default();
        init.x[0] = 0x100;
        assert_jit_matches_interpreter(0xD1004000, 0x40058c, &init, "SUB X0, X0, #0x10");
    }

    #[test]
    fn jit_vs_interp_orr_w0_wzr_1() {
        assert_jit_matches_interpreter(
            0x320003E0,
            0x400590,
            &InitState::default(),
            "ORR W0, WZR, #1 (MOV W0, #1)",
        );
    }

    #[test]
    fn jit_vs_interp_and_x0_x0_0xff() {
        let mut init = InitState::default();
        init.x[0] = 0xDEAD_BEEF_CAFE_BABE;
        assert_jit_matches_interpreter(0x92401C00, 0x400594, &init, "AND X0, X0, #0xFF");
    }

    #[test]
    fn jit_vs_interp_movk_x0_0x1234_lsl16() {
        let mut init = InitState::default();
        init.x[0] = 0x0000_0000_0000_5678;
        assert_jit_matches_interpreter(0xF2A24680, 0x400598, &init, "MOVK X0, #0x1234, LSL#16");
    }

    #[test]
    fn jit_vs_interp_add_x0_x1_x2() {
        let mut init = InitState::default();
        init.x[1] = 100;
        init.x[2] = 200;
        assert_jit_matches_interpreter(0x8B020020, 0x40059c, &init, "ADD X0, X1, X2");
    }

    #[test]
    fn jit_vs_interp_subs_x0_x1_x2() {
        let mut init = InitState::default();
        init.x[1] = 50;
        init.x[2] = 50;
        assert_jit_matches_interpreter(
            0xEB020020,
            0x4005a0,
            &init,
            "SUBS X0, X1, X2 (equal operands => Z=1)",
        );
    }

    #[test]
    fn jit_vs_interp_add_x0_x1_x2_lsl4() {
        let mut init = InitState::default();
        init.x[1] = 0x100;
        init.x[2] = 0x10;
        assert_jit_matches_interpreter(0x8B021020, 0x4005a4, &init, "ADD X0, X1, X2, LSL #4");
    }

    #[test]
    fn jit_vs_interp_eor_x0_all_ones() {
        let mut init = InitState::default();
        init.x[0] = 0xAAAA_BBBB_CCCC_DDDD;
        assert_jit_matches_interpreter(
            0xD2400000,
            0x4005a8,
            &init,
            "EOR X0, X0, #0xFFFFFFFFFFFFFFFF (bitwise NOT)",
        );
    }

    #[test]
    fn jit_vs_interp_ands_x0_x1_0xff() {
        let mut init = InitState::default();
        init.x[1] = 0x0000_0000_0000_0000;
        assert_jit_matches_interpreter(
            0xF2401C20,
            0x4005ac,
            &init,
            "ANDS X0, X1, #0xFF with X1=0 => Z=1",
        );
    }

    #[test]
    fn jit_vs_interp_ands_x0_x1_0xff_nonzero() {
        let mut init = InitState::default();
        init.x[1] = 0x80000000_0000_00AB;
        assert_jit_matches_interpreter(
            0xF2401C20,
            0x4005b0,
            &init,
            "ANDS X0, X1, #0xFF with X1=0xAB => Z=0",
        );
    }

    #[test]
    fn jit_vs_interp_add_w0_w1_0x10() {
        let mut init = InitState::default();
        init.x[0] = 0xFFFF_FFFF_FFFF_FFFF;
        init.x[1] = 0xFFFF_FFFF_0000_0100;
        assert_jit_matches_interpreter(
            0x11004020,
            0x4005b4,
            &init,
            "ADD W0, W1, #0x10 (32-bit, verify zero-extension)",
        );
    }

    #[test]
    fn jit_vs_interp_movz_x5_abcd_lsl48() {
        assert_jit_matches_interpreter(
            0xD2F579A5,
            0x4005b8,
            &InitState::default(),
            "MOVZ X5, #0xABCD, LSL #48",
        );
    }

    #[test]
    fn jit_vs_interp_movn_x1_0() {
        assert_jit_matches_interpreter(
            0x92800001,
            0x4005bc,
            &InitState::default(),
            "MOVN X1, #0 (MOV X1, #-1)",
        );
    }

    #[test]
    fn jit_vs_interp_subs_signed_overflow() {
        let mut init = InitState::default();
        init.x[0] = 0x8000_0000_0000_0000;
        assert_jit_matches_interpreter(
            0xF1000400,
            0x4005c0,
            &init,
            "SUBS X0, X0, #1 with X0=MIN_SIGNED => V=1",
        );
    }

    #[test]
    fn jit_vs_interp_adds_signed_overflow() {
        let mut init = InitState::default();
        init.x[0] = 0x7FFF_FFFF_FFFF_FFFF;
        assert_jit_matches_interpreter(
            0xB1000400,
            0x4005c4,
            &init,
            "ADDS X0, X0, #1 with X0=MAX_SIGNED => V=1,N=1",
        );
    }

    #[test]
    fn jit_vs_interp_sub_x0_x1_x2_lsr8() {
        let mut init = InitState::default();
        init.x[1] = 0x1000;
        init.x[2] = 0xFF00;
        assert_jit_matches_interpreter(0xCB422020, 0x4005c8, &init, "SUB X0, X1, X2, LSR #8");
    }

    #[test]
    fn jit_vs_interp_subs_sweep() {
        let test_values: &[(u64, &str)] = &[
            (0, "zero"),
            (1, "one"),
            (0xFFFF_FFFF, "u32_max"),
            (0xFFFF_FFFF_FFFF_FFFF, "u64_max"),
            (0x8000_0000_0000_0000, "min_signed_64"),
            (0x7FFF_FFFF_FFFF_FFFF, "max_signed_64"),
            (100, "hundred"),
            (0xDEAD_BEEF, "deadbeef"),
        ];
        for &(val, name) in test_values {
            let mut init = InitState::default();
            init.x[0] = val;
            assert_jit_matches_interpreter(
                0xF1000000,
                0x500000,
                &init,
                &format!("SUBS X0, X0, #0 with X0={name}"),
            );
        }
    }
}
