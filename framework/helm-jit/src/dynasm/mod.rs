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
use dynasmrt::{x64::Assembler, DynasmApi, DynasmLabelApi};
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
            let terminates = emit_fused_pair(&mut ops, &pair, &mut patch_sites, insn_count);
            insn_count += consumed as u32;
            if terminates {
                break;
            }
            i += consumed;
            continue;
        }

        let insn = &insns[i];
        match emit::emit_insn(&mut ops, insn, &mut patch_sites, insn_count) {
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
    // Write actual retired count for the fall-through path.
    let retired_off = reg_offset(crate::regs::REG_JIT_RETIRED);
    dynasm!(ops
        ; add QWORD [rdi + retired_off], insn_count as i32
    );
    // Guard against infinite chained loops on the fall-through path.
    dynasm!(ops
        ; mov rax, QWORD [rdi + retired_off]
        ; cmp rax, crate::block::MAX_CHAIN_BUDGET
        ; jge >bail
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
    // Budget exceeded: return to runtime for a proper budget check.
    dynasm!(ops
        ; bail:
        ; mov rax, QWORD EXIT_END_OF_BLOCK as i64
        ; ret
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

    fn make_adr(pc: u64, rd: u32, imm: i64) -> Instruction {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::Adr;
        insn.pc = pc;
        insn.rd = rd;
        insn.imm = imm;
        insn
    }

    fn make_adrp(pc: u64, rd: u32, imm: i64) -> Instruction {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::Adrp;
        insn.pc = pc;
        insn.rd = rd;
        insn.imm = imm;
        insn
    }

    fn make_sub_ext(
        pc: u64,
        rd: u32,
        rn: u32,
        rm: u32,
        extend_type: u32,
        extend_amt: u32,
    ) -> Instruction {
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::SubExt;
        insn.pc = pc;
        insn.rd = rd;
        insn.rn = rn;
        insn.rm = rm;
        insn.extend_type = extend_type;
        insn.extend_amt = extend_amt;
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
        insn.opcode = Opcode::Ldxr; // Exclusive load → unsupported in JIT
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
    fn execute_adr_writes_pc_relative_address() {
        let insn = make_adr(0x1234, 2, 0x28);
        let block = compile_block(0x1234, &[insn]).unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[2], 0x125c);
        assert_eq!(regs[crate::regs::REG_PC], 0x1238);
    }

    #[test]
    fn execute_adrp_writes_page_relative_address() {
        let insn = make_adrp(0x4abc, 3, 2);
        let block = compile_block(0x4abc, &[insn]).unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[3], 0x6000);
        assert_eq!(regs[crate::regs::REG_PC], 0x4ac0);
    }

    #[test]
    fn execute_sub_ext_sxtw_modifies_reg() {
        let insn = make_sub_ext(0x2000, 0, 1, 2, 0b110, 0);
        let block = compile_block(0x2000, &[insn]).unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[1] = 100;
        regs[2] = 0xFFFF_FFFF;
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[0], 101);
        assert_eq!(regs[crate::regs::REG_PC], 0x2004);
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
    fn execute_ldrb_reg_offset_reads_guest_byte() {
        let insn = aarch64_decode(0x3862_6820, 0x1000).expect("decode LDRB W0, [X1, X2]");
        let block = compile_block(0x1000, &[insn]).expect("compile block");
        let mut regs = [0u64; crate::regs::REG_COUNT];
        let mut mem = helm_memory::FlatMem::new(0x1000, 0x1000);
        let mut tlb = crate::helpers::JitSeTlb::new();
        regs[1] = 0x1000;
        regs[2] = 3;
        regs[crate::regs::REG_JIT_SE_TLB] = tlb.entries.as_mut_ptr() as u64;
        mem.load_bytes(0x1003, &[0xAB]);

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), (&mut mem as *mut _) as *mut u8) };
        assert_eq!(exit, 0);
        assert_eq!(regs[0], 0xAB);
        assert_eq!(regs[crate::regs::REG_PC], 0x1004);
    }

    #[test]
    fn execute_ldrb_reg_offset_sxtw_reads_guest_byte() {
        let insn = aarch64_decode(0x3860_CA41, 0x1000).expect("decode LDRB W1, [X18, W0, SXTW]");
        let block = compile_block(0x1000, &[insn]).expect("compile block");
        let mut regs = [0u64; crate::regs::REG_COUNT];
        let mut mem = helm_memory::FlatMem::new(0x2000, 0x1000);
        let mut tlb = crate::helpers::JitSeTlb::new();
        regs[0] = 5;
        regs[18] = 0x2000;
        regs[crate::regs::REG_JIT_SE_TLB] = tlb.entries.as_mut_ptr() as u64;
        mem.load_bytes(0x2005, &[0x7F]);

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), (&mut mem as *mut _) as *mut u8) };
        assert_eq!(exit, 0);
        assert_eq!(regs[1], 0x7F);
        assert_eq!(regs[crate::regs::REG_PC], 0x1004);
    }

    #[test]
    fn execute_strb_reg_offset_writes_guest_byte() {
        let insn = aarch64_decode(0x3820_6981, 0x1000).expect("decode STRB W1, [X12, X0]");
        let block = compile_block(0x1000, &[insn]).expect("compile block");
        let mut regs = [0u64; crate::regs::REG_COUNT];
        let mut mem = helm_memory::FlatMem::new(0x3000, 0x1000);
        let mut tlb = crate::helpers::JitSeTlb::new();
        regs[0] = 2;
        regs[1] = 0xAB;
        regs[12] = 0x3000;
        regs[crate::regs::REG_JIT_SE_TLB] = tlb.entries.as_mut_ptr() as u64;

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), (&mut mem as *mut _) as *mut u8) };
        assert_eq!(exit, 0);
        assert_eq!(
            mem.read(0x3002, 1, helm_core::AccessType::Load)
                .expect("guest byte"),
            0xAB
        );
        assert_eq!(regs[crate::regs::REG_PC], 0x1004);
    }

    #[test]
    fn execute_ubfm_lsr_shifts_value() {
        let insn = aarch64_decode(0xD344_FC00, 0x1000).expect("decode LSR X0, X0, #4");
        let block = compile_block(0x1000, &[insn]).expect("compile block");
        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[0] = 0xFF00;

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[0], 0x0FF0);
        assert_eq!(regs[crate::regs::REG_PC], 0x1004);
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
        let pc_written = aarch64_execute(&insn, &mut interp_state, &mut interp_mem, None)
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
    fn jit_vs_interp_adr_x0_plus_0() {
        assert_jit_matches_interpreter(0x10000000, 0x400120, &InitState::default(), "ADR X0, #0");
    }

    #[test]
    fn jit_vs_interp_adrp_x0_page_base() {
        assert_jit_matches_interpreter(0x90000000, 0x400123, &InitState::default(), "ADRP X0, #0");
    }

    #[test]
    fn jit_vs_interp_sub_ext_x0_x1_w2_sxtw() {
        let mut init = InitState::default();
        init.x[1] = 100;
        init.x[2] = 0xFFFF_FFFF;
        assert_jit_matches_interpreter(0xCB22C020, 0x400126, &init, "SUB X0, X1, W2, SXTW");
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
    fn jit_vs_interp_ccmp_imm_taken() {
        let mut init = InitState::default();
        init.x[0] = 2;
        init.nzcv = 0;
        assert_jit_matches_interpreter(0x7A42_1800, 0x40057e, &init, "CCMP W0, #2, #0, NE");
    }

    #[test]
    fn jit_vs_interp_ccmp_imm_cond_false_uses_nzcv_literal() {
        let mut init = InitState::default();
        init.x[0] = 5;
        init.nzcv = 0x4000_0000; // Z=1, so NE is false.
        assert_jit_matches_interpreter(0x7A42_180A, 0x400582, &init, "CCMP W0, #2, #0xA, NE");
    }

    #[test]
    fn jit_vs_interp_ccmp_reg_taken() {
        let mut init = InitState::default();
        init.x[0] = 10;
        init.x[2] = 10;
        init.nzcv = 0;
        assert_jit_matches_interpreter(0x7A42_1000, 0x400586, &init, "CCMP W0, W2, #0, NE");
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
    fn jit_vs_interp_orr_x0_x1_x2_lsl4() {
        let mut init = InitState::default();
        init.x[1] = 0x100;
        init.x[2] = 0x10;
        assert_jit_matches_interpreter(0xAA021020, 0x4005a6, &init, "ORR X0, X1, X2, LSL #4");
    }

    #[test]
    fn jit_vs_interp_ubfm_lsr_x0_x0_4() {
        let mut init = InitState::default();
        init.x[0] = 0xFF00;
        assert_jit_matches_interpreter(0xD344FC00, 0x4005a7, &init, "LSR X0, X0, #4");
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

    // ═══════════════════════════════════════════════════════════════════════
    // Phase 2: MRS DCZID_EL0 + DC ZVA inline JIT tests
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn jit_mrs_dczid_el0_loads_constant() {
        // MRS X5, DCZID_EL0 = 0xD53B00E5
        let raw = 0xD53B00E5u32;
        let pc = 0x1000u64;
        let insn =
            aarch64_decode(raw, pc).unwrap_or_else(|e| panic!("decode MRS DCZID_EL0 failed: {e}"));
        assert_eq!(insn.opcode, Opcode::Mrs);

        let block = compile_block(pc, &[insn]);
        assert!(block.is_some(), "MRS DCZID_EL0 should compile in JIT");

        let block = block.unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[5] = 0xDEAD; // X5 should be overwritten
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0, "block should exit normally");
        assert_eq!(regs[5], 0x4, "DCZID_EL0 = 0x4 (64-byte block)");
        assert_eq!(regs[crate::regs::REG_PC], pc + 4);
    }

    #[test]
    fn jit_mrs_dczid_el0_to_xzr_is_nop() {
        // MRS XZR, DCZID_EL0 = 0xD53B00FF
        let raw = 0xD53B00FFu32;
        let pc = 0x2000u64;
        let insn = aarch64_decode(raw, pc)
            .unwrap_or_else(|e| panic!("decode MRS DCZID_EL0 to XZR failed: {e}"));

        let block = compile_block(pc, &[insn]);
        assert!(block.is_some(), "MRS DCZID_EL0 (XZR dest) should compile");

        let block = block.unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[crate::regs::REG_PC], pc + 4);
    }

    #[test]
    fn jit_dc_zva_zeros_64_bytes() {
        // DC ZVA, X0 = 0xD50B7420
        let raw = 0xD50B7420u32;
        let pc = 0x3000u64;
        let insn = aarch64_decode(raw, pc).unwrap_or_else(|e| panic!("decode DC ZVA failed: {e}"));
        assert_eq!(insn.opcode, Opcode::DcZva);

        let block = compile_block(pc, &[insn]).expect("DC ZVA should compile in JIT");

        let mut mem = helm_memory::FlatMem::new(0, 0x2000);
        // Fill target region with non-zero pattern.
        for i in 0..64u64 {
            mem.write(0x1000 + i, 1, 0xAA, AccessType::Store).unwrap();
        }
        // Sentinel bytes before and after the 64-byte block.
        mem.write(0x0FFF, 1, 0xBB, AccessType::Store).unwrap();
        mem.write(0x1040, 1, 0xCC, AccessType::Store).unwrap();

        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[0] = 0x1010; // unaligned; DC ZVA should align down to 0x1000
        regs[crate::regs::REG_JIT_MEM_WRITE] = crate::helpers::jit_mem_write as *const () as u64;

        let exit = unsafe {
            (block.entry)(
                regs.as_mut_ptr(),
                (&mut mem as *mut helm_memory::FlatMem).cast::<u8>(),
            )
        };
        assert_eq!(exit, 0, "DC ZVA should succeed");
        assert_eq!(regs[crate::regs::REG_PC], pc + 4);

        // All 64 bytes at 0x1000 should be zero.
        for off in (0..64).step_by(8) {
            let val = mem.read(0x1000 + off, 8, AccessType::Load).unwrap();
            assert_eq!(val, 0, "byte offset {off} not zeroed");
        }
        // Sentinel before the block should be untouched.
        assert_eq!(
            mem.read(0x0FFF, 1, AccessType::Load).unwrap(),
            0xBB,
            "byte before DC ZVA block modified"
        );
        // Sentinel after the block should be untouched.
        assert_eq!(
            mem.read(0x1040, 1, AccessType::Load).unwrap(),
            0xCC,
            "byte after DC ZVA block modified"
        );
    }

    #[test]
    fn jit_dc_zva_aligned_address() {
        // DC ZVA with already-aligned address.
        let raw = 0xD50B7420u32; // DC ZVA, X0
        let pc = 0x4000u64;
        let insn = aarch64_decode(raw, pc).unwrap();

        let block = compile_block(pc, &[insn]).unwrap();
        let mut mem = helm_memory::FlatMem::new(0, 0x2000);
        for i in 0..64u64 {
            mem.write(0x1000 + i, 1, 0xFF, AccessType::Store).unwrap();
        }

        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[0] = 0x1000; // already aligned
        regs[crate::regs::REG_JIT_MEM_WRITE] = crate::helpers::jit_mem_write as *const () as u64;

        let exit = unsafe {
            (block.entry)(
                regs.as_mut_ptr(),
                (&mut mem as *mut helm_memory::FlatMem).cast::<u8>(),
            )
        };
        assert_eq!(exit, 0);
        for off in (0..64).step_by(8) {
            assert_eq!(mem.read(0x1000 + off, 8, AccessType::Load).unwrap(), 0);
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Phase 3: SIMD instruction JIT tests (DUP, STR Q, STP Q)
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn jit_simd_dup_16b_replicates_byte() {
        // DUP V0.16B, W1 -- replicate low byte of X1 across all 16 bytes of V0
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::SimdDup;
        insn.pc = 0x1000;
        insn.rd = 0; // Vd = V0
        insn.rn = 1; // Wn = W1
        insn.imm = 0b00001; // imm5: byte
        insn.sf = true; // Q=1 -> 128-bit

        let block = compile_block(0x1000, &[insn]);
        assert!(block.is_some(), "DUP V0.16B should compile");

        let block = block.unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[1] = 0xAB; // X1 = 0xAB, low byte = 0xAB

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);

        // V0 lo = 0xABABABABABABABAB, V0 hi = 0xABABABABABABABAB
        let v0_lo = regs[crate::regs::REG_V_BASE];
        let v0_hi = regs[crate::regs::REG_V_BASE + 1];
        assert_eq!(v0_lo, 0xABABABABABABABAB, "V0 lo");
        assert_eq!(v0_hi, 0xABABABABABABABAB, "V0 hi");
    }

    #[test]
    fn jit_simd_dup_8b_zeros_upper() {
        // DUP V2.8B, W3 -- Q=0 -> 64-bit, upper 64 bits zeroed
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::SimdDup;
        insn.pc = 0x2000;
        insn.rd = 2;
        insn.rn = 3;
        insn.imm = 0b00001;
        insn.sf = false; // Q=0

        let block = compile_block(0x2000, &[insn]).unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[3] = 0x42;
        // Pre-fill V2 hi to verify it gets zeroed.
        regs[crate::regs::REG_V_BASE + 2 * 2 + 1] = 0xDEAD;

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);

        let v2_lo = regs[crate::regs::REG_V_BASE + 2 * 2];
        let v2_hi = regs[crate::regs::REG_V_BASE + 2 * 2 + 1];
        assert_eq!(v2_lo, 0x4242424242424242);
        assert_eq!(v2_hi, 0, "Q=0 upper must be zeroed");
    }

    #[test]
    fn jit_simd_dup_4s_word_replication() {
        // DUP V0.4S, W0 -- replicate 32-bit word
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::SimdDup;
        insn.pc = 0x3000;
        insn.rd = 0;
        insn.rn = 0;
        insn.imm = 0b00100; // imm5: word (bit 2 set)
        insn.sf = true;

        let block = compile_block(0x3000, &[insn]).unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[0] = 0xFFFF_FFFF_DEAD_BEEF; // only low 32 bits matter

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);

        let v0_lo = regs[crate::regs::REG_V_BASE];
        let v0_hi = regs[crate::regs::REG_V_BASE + 1];
        assert_eq!(v0_lo, 0xDEAD_BEEF_DEAD_BEEF);
        assert_eq!(v0_hi, 0xDEAD_BEEF_DEAD_BEEF);
    }

    #[test]
    fn jit_str_q_stores_128_bits() {
        // STR Q0, [X1, #0]
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::StrSimd;
        insn.pc = 0x4000;
        insn.rd = 0; // Vt = V0
        insn.rn = 1; // Xn = X1
        insn.ftype = 4; // Q-register (128-bit)
        insn.imm = 0;

        let block = compile_block(0x4000, &[insn]);
        assert!(block.is_some(), "STR Q0, [X1] should compile");

        let block = block.unwrap();
        let mut mem = helm_memory::FlatMem::new(0, 0x2000);
        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[1] = 0x1000; // base address
                          // V0 = 0x_FEDCBA9876543210_0123456789ABCDEF
        regs[crate::regs::REG_V_BASE] = 0x0123456789ABCDEF; // lo
        regs[crate::regs::REG_V_BASE + 1] = 0xFEDCBA9876543210; // hi
        regs[crate::regs::REG_JIT_MEM_WRITE] = crate::helpers::jit_mem_write as *const () as u64;

        let exit = unsafe {
            (block.entry)(
                regs.as_mut_ptr(),
                (&mut mem as *mut helm_memory::FlatMem).cast::<u8>(),
            )
        };
        assert_eq!(exit, 0);
        assert_eq!(
            mem.read(0x1000, 8, AccessType::Load).unwrap(),
            0x0123456789ABCDEF
        );
        assert_eq!(
            mem.read(0x1008, 8, AccessType::Load).unwrap(),
            0xFEDCBA9876543210
        );
    }

    #[test]
    fn jit_stp_q_stores_32_bytes() {
        // STP Q0, Q1, [X2, #0]
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::StpSimd;
        insn.pc = 0x5000;
        insn.rd = 0; // Vt1 = V0
        insn.pair_second = 1; // Vt2 = V1
        insn.rn = 2; // Xn = X2
        insn.ftype = 2; // Q-register pair
        insn.imm = 0;

        let block = compile_block(0x5000, &[insn]);
        assert!(block.is_some(), "STP Q0, Q1, [X2] should compile");

        let block = block.unwrap();
        let mut mem = helm_memory::FlatMem::new(0, 0x2000);
        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[2] = 0x1000;
        // V0 = 0x_1111111111111111_2222222222222222
        regs[crate::regs::REG_V_BASE] = 0x2222222222222222;
        regs[crate::regs::REG_V_BASE + 1] = 0x1111111111111111;
        // V1 = 0x_3333333333333333_4444444444444444
        regs[crate::regs::REG_V_BASE + 2] = 0x4444444444444444;
        regs[crate::regs::REG_V_BASE + 3] = 0x3333333333333333;
        regs[crate::regs::REG_JIT_MEM_WRITE] = crate::helpers::jit_mem_write as *const () as u64;

        let exit = unsafe {
            (block.entry)(
                regs.as_mut_ptr(),
                (&mut mem as *mut helm_memory::FlatMem).cast::<u8>(),
            )
        };
        assert_eq!(exit, 0);
        assert_eq!(
            mem.read(0x1000, 8, AccessType::Load).unwrap(),
            0x2222222222222222
        );
        assert_eq!(
            mem.read(0x1008, 8, AccessType::Load).unwrap(),
            0x1111111111111111
        );
        assert_eq!(
            mem.read(0x1010, 8, AccessType::Load).unwrap(),
            0x4444444444444444
        );
        assert_eq!(
            mem.read(0x1018, 8, AccessType::Load).unwrap(),
            0x3333333333333333
        );
    }
    // ═══════════════════════════════════════════════════════════════════════
    // Phase 4: Conditional select + SBFM JIT tests
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn jit_vs_interp_csel_eq_taken() {
        let mut init = InitState::default();
        init.x[1] = 100;
        init.x[2] = 200;
        init.nzcv = 0x4000_0000; // Z=1 -> EQ is true
        assert_jit_matches_interpreter(0x9a820020, 0x1000, &init, "CSEL X0,X1,X2,EQ (taken)");
    }

    #[test]
    fn jit_vs_interp_csel_eq_not_taken() {
        let mut init = InitState::default();
        init.x[1] = 100;
        init.x[2] = 200;
        init.nzcv = 0x0000_0000; // Z=0 -> EQ is false
        assert_jit_matches_interpreter(0x9a820020, 0x1000, &init, "CSEL X0,X1,X2,EQ (not taken)");
    }

    #[test]
    fn jit_vs_interp_csinc_ne() {
        let mut init = InitState::default();
        init.x[1] = 10;
        init.x[2] = 20;
        init.nzcv = 0x0000_0000; // Z=0 -> NE is true
        assert_jit_matches_interpreter(0x9a821420, 0x1000, &init, "CSINC X0,X1,X2,NE (cond true)");
    }

    #[test]
    fn jit_vs_interp_csinc_ne_false() {
        let mut init = InitState::default();
        init.x[1] = 10;
        init.x[2] = 20;
        init.nzcv = 0x4000_0000; // Z=1 -> NE is false
        assert_jit_matches_interpreter(0x9a821420, 0x1000, &init, "CSINC X0,X1,X2,NE (cond false)");
    }

    #[test]
    fn jit_vs_interp_sxtb() {
        let mut init = InitState::default();
        init.x[1] = 0x80; // -128 as signed byte
        assert_jit_matches_interpreter(0x93401c20, 0x2000, &init, "SXTB X0, X1");
    }

    #[test]
    fn jit_vs_interp_sxtw() {
        let mut init = InitState::default();
        init.x[1] = 0xFFFF_FFFF; // -1 as signed 32-bit
        assert_jit_matches_interpreter(0x93407c20, 0x2000, &init, "SXTW X0, W1");
    }

    #[test]
    fn jit_vs_interp_asr() {
        let mut init = InitState::default();
        init.x[1] = 0x8000_0000_0000_0000; // large negative
        assert_jit_matches_interpreter(0x9343fc20, 0x3000, &init, "ASR X0, X1, #3");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Phase 1: ANDS/MADD/MUL coverage tests
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn jit_vs_interp_ands_reg() {
        let mut init = InitState::default();
        init.x[1] = 0xFF00_FF00;
        init.x[2] = 0x00FF_00FF;
        assert_jit_matches_interpreter(0xea020020, 0x1000, &init, "ANDS X0, X1, X2");
    }

    #[test]
    fn jit_vs_interp_madd() {
        let mut init = InitState::default();
        init.x[1] = 7;
        init.x[2] = 8;
        init.x[3] = 100;
        assert_jit_matches_interpreter(0x9b020c20, 0x2000, &init, "MADD X0, X1, X2, X3");
    }

    #[test]
    fn jit_vs_interp_mul() {
        let mut init = InitState::default();
        init.x[1] = 12;
        init.x[2] = 13;
        assert_jit_matches_interpreter(0x9b027c20, 0x3000, &init, "MUL X0, X1, X2");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Batch: shifts, 1-source, logical-neg, div, extr
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn jit_vs_interp_lsl_reg() {
        let mut init = InitState::default();
        init.x[1] = 0xFF;
        init.x[2] = 4;
        assert_jit_matches_interpreter(0x9ac22020, 0x1000, &init, "LSL X0, X1, X2");
    }

    #[test]
    fn jit_vs_interp_clz() {
        let mut init = InitState::default();
        init.x[1] = 0x0000_0000_00FF_0000;
        assert_jit_matches_interpreter(0xdac01020, 0x2000, &init, "CLZ X0, X1");
    }

    #[test]
    fn jit_vs_interp_rev() {
        let mut init = InitState::default();
        init.x[1] = 0x0102030405060708;
        assert_jit_matches_interpreter(0xdac00c20, 0x3000, &init, "REV X0, X1");
    }

    #[test]
    fn jit_vs_interp_extr() {
        let mut init = InitState::default();
        init.x[1] = 0xAAAA_BBBB_CCCC_DDDD;
        init.x[2] = 0x1111_2222_3333_4444;
        assert_jit_matches_interpreter(0x93c22020, 0x4000, &init, "EXTR X0, X1, X2, #8");
    }

    #[test]
    fn jit_vs_interp_orn() {
        let mut init = InitState::default();
        init.x[1] = 0xFF00_FF00;
        init.x[2] = 0x0F0F_0F0F;
        assert_jit_matches_interpreter(0xaa220020, 0x5000, &init, "ORN X0, X1, X2");
    }

    #[test]
    fn jit_vs_interp_sdiv() {
        let mut init = InitState::default();
        init.x[1] = 100;
        init.x[2] = 7;
        assert_jit_matches_interpreter(0x9ac20c20, 0x6000, &init, "SDIV X0, X1, X2");
    }

    #[test]
    fn jit_vs_interp_sdiv_by_zero() {
        let mut init = InitState::default();
        init.x[1] = 42;
        init.x[2] = 0;
        assert_jit_matches_interpreter(0x9ac20c20, 0x6004, &init, "SDIV X0, X1, 0");
    }

    #[test]
    fn jit_vs_interp_udiv() {
        let mut init = InitState::default();
        init.x[1] = 200;
        init.x[2] = 3;
        assert_jit_matches_interpreter(0x9ac20820, 0x7000, &init, "UDIV X0, X1, X2");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Batch: ADC/SBC, SMULH/UMULH
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn jit_vs_interp_adc_with_carry() {
        let mut init = InitState::default();
        init.x[1] = u64::MAX;
        init.x[2] = 1;
        init.nzcv = 0x2000_0000; // C=1
        assert_jit_matches_interpreter(0x9a020020, 0x1000, &init, "ADC X0,X1,X2 (C=1)");
    }

    #[test]
    fn jit_vs_interp_adc_no_carry() {
        let mut init = InitState::default();
        init.x[1] = 10;
        init.x[2] = 20;
        init.nzcv = 0x0000_0000; // C=0
        assert_jit_matches_interpreter(0x9a020020, 0x1000, &init, "ADC X0,X1,X2 (C=0)");
    }

    #[test]
    fn jit_vs_interp_smulh() {
        let mut init = InitState::default();
        init.x[1] = 0x7FFF_FFFF_FFFF_FFFF;
        init.x[2] = 2;
        assert_jit_matches_interpreter(0x9b427c20, 0x2000, &init, "SMULH X0, X1, X2");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // System instructions: MRS, MSR, MsrImm, WFI/WFE, barriers
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn jit_mrs_tpidr_el0_reads_from_arch_state() {
        // MRS X0, TPIDR_EL0 = 0xD53BD040
        let raw = 0xD53BD040u32;
        let pc = 0x1000u64;
        let insn = aarch64_decode(raw, pc).expect("decode MRS TPIDR_EL0");
        assert_eq!(insn.opcode, Opcode::Mrs);

        let block = compile_block(pc, &[insn]).expect("MRS TPIDR_EL0 should compile");

        let mut arch = helm_arch::aarch64::Aarch64ArchState::new();
        arch.tpidr_el0 = 0xCAFE_BABE_1234_5678;

        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[crate::regs::REG_JIT_ARCH_STATE] = &mut arch as *mut _ as u64;

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[0], 0xCAFE_BABE_1234_5678, "X0 should hold TPIDR_EL0");
    }

    #[test]
    fn jit_msr_tpidr_el0_writes_to_arch_state() {
        // MSR TPIDR_EL0, X1 = 0xD51BD041
        let raw = 0xD51BD041u32;
        let pc = 0x2000u64;
        let insn = aarch64_decode(raw, pc).expect("decode MSR TPIDR_EL0");
        assert_eq!(insn.opcode, Opcode::Msr);

        let block = compile_block(pc, &[insn]).expect("MSR TPIDR_EL0 should compile");

        let mut arch = helm_arch::aarch64::Aarch64ArchState::new();
        arch.tpidr_el0 = 0;

        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[crate::regs::REG_JIT_ARCH_STATE] = &mut arch as *mut _ as u64;
        regs[1] = 0xDEAD_BEEF_CAFE_F00D; // X1

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(arch.tpidr_el0, 0xDEAD_BEEF_CAFE_F00D);
    }

    #[test]
    fn jit_mrs_msr_nzcv_uses_pinned_register() {
        // MRS X5, NZCV = 0xD53B4200  (encoding 3,3,4,2,0)
        // rd=5, imm = NZCV encoding
        let raw_mrs = 0xD53B4205u32;
        let pc = 0x3000u64;
        let insn = aarch64_decode(raw_mrs, pc).expect("decode MRS NZCV");
        let block = compile_block(pc, &[insn]).expect("MRS NZCV should compile");

        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[crate::regs::REG_NZCV] = 0x6000_0000; // Z=1, C=1

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[5], 0x6000_0000, "X5 should hold NZCV value");
    }

    #[test]
    fn jit_wfi_compiles_as_nop() {
        // WFI = 0xD503207F
        let raw = 0xD503207Fu32;
        let pc = 0x4000u64;
        let insn = aarch64_decode(raw, pc).expect("decode WFI");
        assert_eq!(insn.opcode, Opcode::Wfi);

        let block = compile_block(pc, &[insn]);
        assert!(block.is_some(), "WFI should compile in JIT");
    }

    #[test]
    fn jit_barriers_decode_as_sys_and_compile() {
        // DSB/DMB/ISB decode as Sys in this codebase (barrier check path
        // doesn't match standard encodings). They appear in blocks as Sys
        // instructions and are handled by the generic SYS fallback.
        // The JIT handles Sys for TLBI/AT/IC/DC ops by falling back to the
        // interpreter, but NOP/WFI/WFE/SEV/SEVL/YIELD/ESB/SB/BTI are
        // compiled as no-ops.
        //
        // Verify NOP, WFI, WFE compile:
        let nop = aarch64_decode(0xD503201F, 0x5000).expect("NOP");
        assert_eq!(nop.opcode, Opcode::Nop);
        assert!(compile_block(0x5000, &[nop]).is_some());

        let wfi = aarch64_decode(0xD503207F, 0x5004).expect("WFI");
        assert_eq!(wfi.opcode, Opcode::Wfi);
        assert!(compile_block(0x5004, &[wfi]).is_some());

        let wfe = aarch64_decode(0xD503205F, 0x5008).expect("WFE");
        // WFE decodes as Sys in this decoder (only NOP/WFI are special-cased).
        assert_eq!(wfe.opcode, Opcode::Sys);
        assert!(compile_block(0x5008, &[wfe]).is_some());
    }

    #[test]
    fn jit_mrs_hcr_el2_reads_from_arch_state() {
        // MRS X0, HCR_EL2 (op0=3,op1=4,CRn=1,CRm=1,op2=0)
        let raw = 0xD53C1100u32;
        let pc = 0x6000u64;
        let insn = aarch64_decode(raw, pc).expect("decode MRS HCR_EL2");
        assert_eq!(insn.opcode, Opcode::Mrs);

        let block = compile_block(pc, &[insn]).expect("MRS HCR_EL2 should compile");

        let mut arch = helm_arch::aarch64::Aarch64ArchState::new();
        arch.current_el = 2; // HCR_EL2 requires EL≥2 per ARM DDI 0487
        arch.hcr_el2 = 0x8000_0000_0000_003E;

        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[crate::regs::REG_JIT_ARCH_STATE] = &mut arch as *mut _ as u64;

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[0], 0x8000_0000_0000_003E);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Fused pair NZCV correctness (regression test for block-chaining bug)
    // ═══════════════════════════════════════════════════════════════════════

    /// Verify that a fused CMP+B.cond pair correctly defers NZCV so that
    /// the flat array holds the right flags after block exit.  Before the
    /// fix, the fused pair left REG_FLAG_OP stale, causing the epilogue to
    /// replay a prior flag-setting operation and write corrupt NZCV.
    #[test]
    fn fused_cmp_bcond_writes_correct_nzcv() {
        // Block: SUBS X5, X5, #100  (sets deferred NZCV to some value)
        //        CMP  X0, #37       (fused F1 pair with B.LE below)
        //        B.LE target
        //
        // With X0 = 50 (50 > 37), B.LE is not taken.  After the block,
        // NZCV should reflect CMP X0, #37 (50 - 37 = 13 > 0 → N=0,Z=0,C=1,V=0)
        // and NOT the earlier SUBS X5, X5, #100.
        let insns = [
            make_subs_imm(0x1000, 5, 5, 100), // SUBS X5, X5, #100
            // CMP X0, #37 = SUBS XZR, X0, #37
            make_subs_imm(0x1004, 31, 0, 37), // CMP (rd=31 → XZR)
            make_bcond(0x1008, 13, -0x20),    // B.LE (cond=13) backwards
        ];
        let block = compile_block(0x1000, &insns).unwrap();
        assert_eq!(block.insn_count, 3);

        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[0] = 50; // X0 = 50, so 50 > 37 → B.LE not taken
        regs[5] = 200; // X5 = 200

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, EXIT_END_OF_BLOCK);

        // Block fell through (B.LE not taken), PC = 0x100c
        assert_eq!(regs[crate::regs::REG_PC], 0x100c);

        // NZCV should reflect CMP X0, #37 (50 - 37):
        //   N=0 (result positive), Z=0 (non-zero), C=1 (no borrow), V=0
        let nzcv = regs[crate::regs::REG_NZCV] as u32;
        assert!(
            nzcv & (1 << 31) == 0,
            "N should be 0 for CMP 50,37; got nzcv={nzcv:#x}"
        );
        assert!(
            nzcv & (1 << 30) == 0,
            "Z should be 0 for CMP 50,37; got nzcv={nzcv:#x}"
        );
        assert!(
            nzcv & (1 << 29) != 0,
            "C should be 1 for CMP 50,37 (no borrow); got nzcv={nzcv:#x}"
        );
        assert!(
            nzcv & (1 << 28) == 0,
            "V should be 0 for CMP 50,37; got nzcv={nzcv:#x}"
        );
    }

    /// Same test but B.LE is taken — verify NZCV is correct on the taken
    /// exit path (which goes through emit_chainable_exit → epilogue).
    #[test]
    fn fused_cmp_bcond_taken_writes_correct_nzcv() {
        let insns = [
            make_subs_imm(0x1000, 5, 5, 100),
            make_subs_imm(0x1004, 31, 0, 37), // CMP X0, #37
            make_bcond(0x1008, 13, -0x20),    // B.LE backwards
        ];
        let block = compile_block(0x1000, &insns).unwrap();

        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[0] = 10; // X0 = 10 ≤ 37 → B.LE taken
        regs[5] = 200;

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, EXIT_END_OF_BLOCK);

        // Taken path: PC = 0x1008 + (-0x20) = 0xfe8
        assert_eq!(regs[crate::regs::REG_PC], 0xfe8);

        // NZCV from CMP X0, #37 (10 - 37 = -27):
        //   N=1 (negative), Z=0, C=0 (borrow), V=0
        let nzcv = regs[crate::regs::REG_NZCV] as u32;
        assert!(
            nzcv & (1 << 31) != 0,
            "N should be 1 for CMP 10,37; got nzcv={nzcv:#x}"
        );
        assert!(
            nzcv & (1 << 30) == 0,
            "Z should be 0 for CMP 10,37; got nzcv={nzcv:#x}"
        );
        assert!(
            nzcv & (1 << 29) == 0,
            "C should be 0 for CMP 10,37 (borrow); got nzcv={nzcv:#x}"
        );
        assert!(
            nzcv & (1 << 28) == 0,
            "V should be 0 for CMP 10,37; got nzcv={nzcv:#x}"
        );
    }

    /// Verify F2 fusion (SUBS+BNE) also writes correct deferred NZCV.
    #[test]
    fn fused_subs_bne_writes_correct_nzcv() {
        // SUBS X5, X5, #100 (poison prior flags)
        // SUBS X0, X0, #1   (fused with B.NE below)
        // B.NE target
        //
        // X0 = 1 → after SUBS X0,X0,#1 → X0 = 0, Z=1 → B.NE not taken.
        // NZCV should reflect the SUBS X0,X0,#1 result (1 - 1 = 0).
        let insns = [
            make_subs_imm(0x2000, 5, 5, 100),
            make_subs_imm(0x2004, 0, 0, 1),
            make_bcond(0x2008, 1, -0x20), // B.NE (cond=1)
        ];
        let block = compile_block(0x2000, &insns).unwrap();

        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[0] = 1;
        regs[5] = 200;

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, EXIT_END_OF_BLOCK);

        assert_eq!(regs[0], 0);
        assert_eq!(regs[crate::regs::REG_PC], 0x200c);

        // NZCV from SUBS X0,X0,#1 (1 - 1 = 0):
        //   N=0, Z=1, C=1 (no borrow), V=0
        let nzcv = regs[crate::regs::REG_NZCV] as u32;
        assert!(
            nzcv & (1 << 30) != 0,
            "Z should be 1 for 1-1=0; got nzcv={nzcv:#x}"
        );
        assert!(
            nzcv & (1 << 29) != 0,
            "C should be 1 for 1-1 (no borrow); got nzcv={nzcv:#x}"
        );
        assert!(nzcv & (1 << 31) == 0, "N should be 0; got nzcv={nzcv:#x}");
    }

    /// Verify that CMP (SUBS XZR, X1, X2) does NOT corrupt the XZR slot.
    /// This is a regression test for the BSS clearing loop bug where
    /// STP XZR, XZR would read a non-zero value from the XZR slot.
    #[test]
    fn cmp_reg_does_not_corrupt_xzr() {
        // CMP X1, X2 = SUBS XZR, X1, X2 = 0xeb02003f
        let insn = aarch64_decode(0xeb02003f, 0x1000).expect("decode CMP X1,X2");
        assert_eq!(insn.opcode, Opcode::SubsReg);
        assert_eq!(insn.rd, 31); // XZR

        let block = compile_block(0x1000, &[insn]).unwrap();

        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[1] = 100;
        regs[2] = 50;

        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, EXIT_END_OF_BLOCK);
        assert_eq!(
            regs[crate::regs::REG_XZR],
            0,
            "XZR must remain 0 after CMP; got {:#x}",
            regs[crate::regs::REG_XZR]
        );
    }
}
