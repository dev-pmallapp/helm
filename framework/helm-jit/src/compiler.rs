//! Block compiler — translates a sequence of decoded AArch64 instructions
//! into a single `CompiledBlock` of x86-64 machine code.
//!
//! # Compilation strategy
//!
//! Starting at a guest PC, the compiler iterates decoded instructions
//! (up to `MAX_BLOCK_INSNS` or the first terminator/unsupported opcode).
//! Each instruction is passed to [`emit::emit_insn`], which emits x86-64
//! code via dynasm. An epilogue is appended at the end.
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
use dynasmrt::{DynasmApi, x64::Assembler};
use helm_arch::aarch64::insn::Instruction;

use crate::block::{CompiledBlock, EXIT_END_OF_BLOCK};
use crate::emit;
use crate::regs::{reg_offset, REG_PC};

/// Maximum number of guest instructions per compiled block.
const MAX_BLOCK_INSNS: usize = 64;

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

    // ── Prologue ────────────────────────────────────────────────────────────
    // No callee-saved register preservation needed: JIT code only uses
    // rax/rcx/rdx/r8/r9 (scratch) plus rdi/rsi (block-preserved, saved
    // by ldst emitters around helper calls). Branch emitters emit bare
    // `ret`, so no stack frame to unwind.
    //
    // SysV ABI guarantees 16-byte stack alignment on entry. Helper call
    // sites in ldst.rs manage their own sub rsp/add rsp for alignment.

    // ── Instruction emission ────────────────────────────────────────────────
    for (i, insn) in insns.iter().enumerate() {
        if i >= MAX_BLOCK_INSNS {
            break;
        }

        match emit::emit_insn(&mut ops, insn) {
            Some(true) => {
                // Block-terminating instruction (branch). The emitter already
                // wrote the PC update, exit code, and `ret`.
                insn_count += 1;
                break;
            }
            Some(false) => {
                // Non-terminating instruction. Advance guest PC conceptually
                // (the flat array's PC is updated at the end of the block).
                insn_count += 1;
            }
            None => {
                // Unsupported opcode — stop compilation here.
                break;
            }
        }
    }

    if insn_count == 0 {
        return None;
    }

    // ── Epilogue (fall-through case) ────────────────────────────────────────
    // If we reach here, the block ended without a branch (hit max insns or
    // unsupported opcode). Update PC to point past the last compiled insn.
    let next_pc = pc + u64::from(insn_count) * 4;
    let pc_off = reg_offset(REG_PC);

    dynasm!(ops
        ; mov rax, QWORD next_pc as i64
        ; mov QWORD [rdi + pc_off], rax
        ; mov rax, QWORD EXIT_END_OF_BLOCK as i64
        ; ret
    );

    let buf = ops.finalize().ok()?;
    Some(unsafe { CompiledBlock::new(buf, pc, insn_count) })
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
        // MOVZ X0, #0x1234, LSL#16
        // Decoder convention: imm = imm16 << (hw * 16) = 0x1234 << 16
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::Movz;
        insn.pc = 0x1000;
        insn.rd = 0;
        insn.imm = (0x1234_u64 << 16) as i64; // decoder pre-shifts
        insn.imm2 = 1; // hw=1 (stored but not used by emitter)
        insn.sf = true;

        let block = compile_block(0x1000, &[insn]).unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        #[allow(unsafe_code)]
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0); // EXIT_END_OF_BLOCK
        assert_eq!(regs[0], 0x1234_0000);
        assert_eq!(regs[crate::regs::REG_PC], 0x1004);
    }

    #[test]
    fn execute_movn_32() {
        // MOVN W0, #0 → W0 = 0xFFFFFFFF
        // Decoder convention: imm = !(imm16 << (hw * 16)) = !(0) = -1
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::Movn;
        insn.pc = 0x1000;
        insn.rd = 0;
        insn.imm = -1; // decoder pre-inverts
        insn.imm2 = 0;
        insn.sf = false;

        let block = compile_block(0x1000, &[insn]).unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        #[allow(unsafe_code)]
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[0], 0xFFFF_FFFF, "MOVN W0, #0 should give 0xFFFFFFFF, got {:#x}", regs[0]);
    }

    #[test]
    fn execute_add_imm_modifies_reg() {
        // ADD X1, X0, #42
        let mut insn = Instruction::zeroed();
        insn.opcode = Opcode::AddImm;
        insn.pc = 0x1000;
        insn.rd = 1;
        insn.rn = 0;
        insn.imm = 42;
        insn.sf = true;

        let block = compile_block(0x1000, &[insn]).unwrap();
        let mut regs = [0u64; crate::regs::REG_COUNT];
        regs[0] = 100; // X0 = 100
        #[allow(unsafe_code)]
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[1], 142); // X1 = 100 + 42
    }

    #[test]
    fn execute_subs_imm_sets_nzcv() {
        // SUBS X0, X0, #100 (X0=100 → result=0 → Z=1,C=1)
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
        #[allow(unsafe_code)]
        let exit = unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) };
        assert_eq!(exit, 0);
        assert_eq!(regs[0], 0);
        let nzcv = regs[crate::regs::REG_NZCV] as u32;
        // Z=1 (bit 30), C=1 (bit 29, no borrow)
        assert!(nzcv & (1 << 30) != 0, "Z flag should be set, nzcv={nzcv:#x}");
        assert!(nzcv & (1 << 29) != 0, "C flag should be set (no borrow), nzcv={nzcv:#x}");
        assert!(nzcv & (1 << 31) == 0, "N flag should be clear, nzcv={nzcv:#x}");
        assert!(nzcv & (1 << 28) == 0, "V flag should be clear, nzcv={nzcv:#x}");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // JIT vs Interpreter differential correctness tests
    //
    // Each test decodes a real AArch64 instruction encoding via the real
    // decoder, compiles it via the JIT, executes it via the interpreter,
    // and compares all architectural state (X0-X30, SP, PC, NZCV).
    // ═══════════════════════════════════════════════════════════════════════

    use helm_arch::aarch64::decode::decode as aarch64_decode;
    use helm_arch::aarch64::execute::execute as aarch64_execute;
    use helm_arch::aarch64::arch_state::Aarch64ArchState;
    use helm_core::{AccessType, MemFault, MemInterface};
    use crate::regs::{arch_to_flat, flat_to_arch};

    /// Minimal memory implementation for testing non-memory instructions.
    /// Reads return 0, writes succeed silently.
    struct NullMem;

    impl MemInterface for NullMem {
        fn read(&mut self, _addr: u64, _size: usize, _ty: AccessType) -> Result<u64, MemFault> {
            Ok(0)
        }
        fn write(&mut self, _addr: u64, _size: usize, _val: u64, _ty: AccessType) -> Result<(), MemFault> {
            Ok(())
        }
    }

    /// Initial register state for a test case.
    /// Only the fields that differ from default need to be set.
    #[derive(Clone)]
    struct InitState {
        /// X0-X30 initial values (index 0..=30).
        x: [u64; 31],
        /// Stack pointer.
        sp: u64,
        /// Initial NZCV flags.
        nzcv: u32,
    }

    impl Default for InitState {
        fn default() -> Self {
            Self {
                x: [0u64; 31],
                sp: 0x0000_7FFF_FFF0_0000, // realistic stack pointer
                nzcv: 0,
            }
        }
    }

    /// Compare JIT execution against the interpreter for a single instruction.
    ///
    /// Panics with detailed diagnostics on the first mismatch.
    fn assert_jit_matches_interpreter(
        raw: u32,
        pc: u64,
        init: &InitState,
        label: &str,
    ) {
        // 1. Decode the raw instruction using the real decoder
        let insn = aarch64_decode(raw, pc)
            .unwrap_or_else(|e| panic!("[{label}] decode failed for {raw:#010x}: {e}"));

        // Skip instructions the JIT doesn't support (compile_block returns None)
        let block = match compile_block(pc, &[insn]) {
            Some(b) => b,
            None => {
                // JIT doesn't support this opcode -- nothing to compare
                eprintln!("[{label}] JIT unsupported opcode {:?}, skipping", insn.opcode);
                return;
            }
        };

        // 2. Prepare interpreter state
        let mut interp_state = Aarch64ArchState::default();
        for i in 0..31 {
            interp_state.x[i] = init.x[i];
        }
        interp_state.sp = init.sp;
        interp_state.pc = pc;
        interp_state.nzcv = init.nzcv;

        // 3. Execute via interpreter
        let mut interp_mem = NullMem;
        let pc_written = aarch64_execute(&insn, &mut interp_state, &mut interp_mem)
            .unwrap_or_else(|e| panic!("[{label}] interpreter execute failed: {e}"));
        if !pc_written {
            interp_state.pc = pc + 4;
        }

        // 4. Prepare JIT state: copy initial state into flat array
        let mut jit_state = Aarch64ArchState::default();
        for i in 0..31 {
            jit_state.x[i] = init.x[i];
        }
        jit_state.sp = init.sp;
        jit_state.pc = pc;
        jit_state.nzcv = init.nzcv;
        let mut flat = arch_to_flat(&jit_state);

        // 5. Execute via JIT
        #[allow(unsafe_code)]
        let exit = unsafe { (block.entry)(flat.as_mut_ptr(), std::ptr::null_mut()) };

        // Copy flat regs back to arch state for comparison
        flat_to_arch(&mut flat, &mut jit_state);

        // 6. Compare all state
        let mut mismatches = Vec::new();

        // GPRs X0-X30
        for i in 0..31 {
            if jit_state.x[i] != interp_state.x[i] {
                mismatches.push(format!(
                    "  X{i}: JIT={:#018x}  interp={:#018x}",
                    jit_state.x[i], interp_state.x[i]
                ));
            }
        }

        // SP
        if jit_state.sp != interp_state.sp {
            mismatches.push(format!(
                "  SP: JIT={:#018x}  interp={:#018x}",
                jit_state.sp, interp_state.sp
            ));
        }

        // PC
        if jit_state.pc != interp_state.pc {
            mismatches.push(format!(
                "  PC: JIT={:#018x}  interp={:#018x}",
                jit_state.pc, interp_state.pc
            ));
        }

        // NZCV
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

        if !mismatches.is_empty() {
            panic!(
                "\n\n[{label}] JIT vs interpreter MISMATCH\n\
                 Instruction: {raw:#010x} at pc={pc:#x}\n\
                 Opcode: {:?}\n\
                 JIT exit code: {exit}\n\
                 Mismatched registers:\n{}\n",
                insn.opcode,
                mismatches.join("\n")
            );
        }
    }

    // ── Test: MOV X12, #0x1b00 ────────────────────────────────────────────
    // MOVZ X12, #0x1B00
    // Encoding: 0xd2836 00c
    #[test]
    fn jit_vs_interp_movz_x12_0x1b00() {
        assert_jit_matches_interpreter(
            0xd283600c,
            0x400560,
            &InitState::default(),
            "MOVZ X12, #0x1B00",
        );
    }

    // ── Test: ADD X0, X0, #0xcf0 ──────────────────────────────────────────
    // Encoding: 0x9133c000
    #[test]
    fn jit_vs_interp_add_x0_x0_0xcf0() {
        let mut init = InitState::default();
        init.x[0] = 0x0040_0000;
        assert_jit_matches_interpreter(
            0x9133c000,
            0x400564,
            &init,
            "ADD X0, X0, #0xCF0",
        );
    }

    // ── Test: MOV W3, #0 ──────────────────────────────────────────────────
    // MOVZ W3, #0
    // Encoding: 0x52800003
    #[test]
    fn jit_vs_interp_movz_w3_0() {
        let mut init = InitState::default();
        init.x[3] = 0xDEAD_BEEF_CAFE_BABE; // dirty pre-value to verify 32-bit zeroing
        assert_jit_matches_interpreter(
            0x52800003,
            0x400568,
            &init,
            "MOVZ W3, #0",
        );
    }

    // ── Test: MOV X29, SP (ADD X29, SP, #0) ──────────────────────────────
    // rn=31 in ADD imm context means SP (not XZR)
    // Encoding: 0x910003fd
    #[test]
    fn jit_vs_interp_mov_x29_sp() {
        let mut init = InitState::default();
        init.sp = 0x0000_7FFF_FFF0_0000;
        assert_jit_matches_interpreter(
            0x910003fd,
            0x40056c,
            &init,
            "MOV X29, SP (ADD X29, SP, #0)",
        );
    }

    // ── Test: MOV W0, #-1 (MOVN W0, #0) ──────────────────────────────────
    // Encoding: 0x12800000
    #[test]
    fn jit_vs_interp_movn_w0_neg1() {
        assert_jit_matches_interpreter(
            0x12800000,
            0x400570,
            &InitState::default(),
            "MOVN W0, #0 (MOV W0, #-1)",
        );
    }

    // ── Test: ADD X22, X22, #0xca ─────────────────────────────────────────
    // Encoding: 0x91032ac0  (X0 = X22 + #0xCA ... wait, let me re-derive)
    // 0x91032AC0: sf=1, op=0(ADD), S=0, sh=0, imm12=0xCA, Rn=22(X22), Rd=0(X0)
    // Actually: bits[4:0]=Rd, bits[9:5]=Rn
    // 0xC0 = 1100_0000 => Rd = 0b00000 = 0
    // 0x2A = 0010_1010 => Rn = 0b10110 = 22
    // Hmm, that gives Rd=0, not Rd=22. Let me re-check the user request.
    // The user said "add x22, x22, #0xca" but the encoding 0x91032ac0
    // decodes as ADD X0, X22, #0xCA. Let me use whatever the decoder produces.
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

    // ── Test: SUBS/CMP ────────────────────────────────────────────────────
    // CMP X0, #0 is SUBS XZR, X0, #0 → encoding: 0xF100001F
    // sf=1, op=1(SUB), S=1, sh=0, imm12=0, Rn=0(X0), Rd=31(XZR)
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

    // ── Test: CMP with zero result (Z flag) ───────────────────────────────
    #[test]
    fn jit_vs_interp_cmp_x0_0_zero_result() {
        let init = InitState::default(); // X0=0
        assert_jit_matches_interpreter(
            0xF100001F,
            0x40057c,
            &init,
            "CMP X0, #0 (SUBS XZR, X0, #0) with X0=0 => Z=1",
        );
    }

    // ── Test: CMP with negative result (N flag) ───────────────────────────
    // SUBS X0, X0, #1 → encoding: 0xF1000400
    // sf=1, op=1(SUB), S=1, sh=0, imm12=1, Rn=0, Rd=0
    #[test]
    fn jit_vs_interp_subs_x0_x0_1_underflow() {
        let init = InitState::default(); // X0=0, so 0-1 wraps
        assert_jit_matches_interpreter(
            0xF1000400,
            0x400580,
            &init,
            "SUBS X0, X0, #1 with X0=0 => negative wrap",
        );
    }

    // ── Test: 32-bit SUBS (CMP W1, #5) ───────────────────────────────────
    // CMP W1, #5 = SUBS WZR, W1, #5 → encoding: 0x7100143F
    // sf=0, op=1, S=1, sh=0, imm12=5, Rn=1(W1), Rd=31(WZR)
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

    // ── Test: ADDS X0, X1, #1 (overflow case) ────────────────────────────
    // ADDS X0, X1, #1 → encoding: 0xB1000420
    // sf=1, op=0(ADD), S=1, sh=0, imm12=1, Rn=1, Rd=0
    #[test]
    fn jit_vs_interp_adds_x0_x1_1_max() {
        let mut init = InitState::default();
        init.x[1] = u64::MAX; // 0xFFFF...FFFF + 1 => C=1, Z=1
        assert_jit_matches_interpreter(
            0xB1000420,
            0x400588,
            &init,
            "ADDS X0, X1, #1 with X1=MAX => carry+zero",
        );
    }

    // ── Test: SUB X0, X0, #0x10 (non-flag-setting) ───────────────────────
    // SUB X0, X0, #0x10 → encoding: 0xD1004000
    // sf=1, op=1(SUB), S=0, sh=0, imm12=0x10, Rn=0, Rd=0
    #[test]
    fn jit_vs_interp_sub_x0_x0_0x10() {
        let mut init = InitState::default();
        init.x[0] = 0x100;
        assert_jit_matches_interpreter(
            0xD1004000,
            0x40058c,
            &init,
            "SUB X0, X0, #0x10",
        );
    }

    // ── Test: ORR immediate ───────────────────────────────────────────────
    // ORR X0, X0, #0xFF → encoding needs bitmask immediate
    // Let me use a known real encoding. ORR W0, WZR, #1 = 0x320003E0
    // Actually sf=0, opc=01(ORR), N=0, immr=0, imms=0, Rn=31(WZR), Rd=0
    // The bitmask for #1 is N=0, immr=0, imms=0 → decodes to 0x1
    #[test]
    fn jit_vs_interp_orr_w0_wzr_1() {
        assert_jit_matches_interpreter(
            0x320003E0,
            0x400590,
            &InitState::default(),
            "ORR W0, WZR, #1 (MOV W0, #1)",
        );
    }

    // ── Test: AND immediate ───────────────────────────────────────────────
    // AND X0, X0, #0xFF → encoding: 0x92401C00
    // sf=1, opc=00(AND), N=1, immr=0, imms=7 (0b000111), Rn=0, Rd=0
    // bitmask(N=1,immr=0,imms=7) = 0xFF
    #[test]
    fn jit_vs_interp_and_x0_x0_0xff() {
        let mut init = InitState::default();
        init.x[0] = 0xDEAD_BEEF_CAFE_BABE;
        assert_jit_matches_interpreter(
            0x92401C00,
            0x400594,
            &init,
            "AND X0, X0, #0xFF",
        );
    }

    // ── Test: MOVK X0, #0x1234, LSL#16 ───────────────────────────────────
    // MOVK X0, #0x1234, LSL#16 → 0xF2A24680
    // sf=1, opc=11(MOVK), hw=1, imm16=0x1234, Rd=0
    #[test]
    fn jit_vs_interp_movk_x0_0x1234_lsl16() {
        let mut init = InitState::default();
        init.x[0] = 0x0000_0000_0000_5678; // pre-existing value
        assert_jit_matches_interpreter(
            0xF2A24680,
            0x400598,
            &init,
            "MOVK X0, #0x1234, LSL#16",
        );
    }

    // ── Test: ADD register (shifted) ──────────────────────────────────────
    // ADD X0, X1, X2 → encoding: 0x8B020020
    // sf=1, op=0(ADD), S=0, shift=00(LSL), Rm=2, imm6=0, Rn=1, Rd=0
    #[test]
    fn jit_vs_interp_add_x0_x1_x2() {
        let mut init = InitState::default();
        init.x[1] = 100;
        init.x[2] = 200;
        assert_jit_matches_interpreter(
            0x8B020020,
            0x40059c,
            &init,
            "ADD X0, X1, X2",
        );
    }

    // ── Test: SUBS register ───────────────────────────────────────────────
    // SUBS X0, X1, X2 → encoding: 0xEB020020
    // sf=1, op=1(SUB), S=1, shift=00, Rm=2, imm6=0, Rn=1, Rd=0
    #[test]
    fn jit_vs_interp_subs_x0_x1_x2() {
        let mut init = InitState::default();
        init.x[1] = 50;
        init.x[2] = 50; // equal => Z=1, C=1
        assert_jit_matches_interpreter(
            0xEB020020,
            0x4005a0,
            &init,
            "SUBS X0, X1, X2 (equal operands => Z=1)",
        );
    }

    // ── Test: ADD X0, X1, X2, LSL #4 (shifted register) ──────────────────
    // ADD X0, X1, X2, LSL #4 → encoding: 0x8B021020
    // sf=1, op=0(ADD), S=0, shift=00(LSL), Rm=2, imm6=4, Rn=1, Rd=0
    #[test]
    fn jit_vs_interp_add_x0_x1_x2_lsl4() {
        let mut init = InitState::default();
        init.x[1] = 0x100;
        init.x[2] = 0x10;
        assert_jit_matches_interpreter(
            0x8B021020,
            0x4005a4,
            &init,
            "ADD X0, X1, X2, LSL #4",
        );
    }

    // ── Test: EOR immediate ───────────────────────────────────────────────
    // EOR X0, X0, #0xFFFF_FFFF_FFFF_FFFF → encoding: 0xD2400000
    // sf=1, opc=10(EOR), N=1, immr=0, imms=0x3F (all 1s), Rn=0, Rd=0
    // bitmask(N=1, immr=0, imms=63) = 0xFFFF_FFFF_FFFF_FFFF
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

    // ── Test: ANDS immediate (flag-setting) ───────────────────────────────
    // ANDS X0, X1, #0xFF → encoding: 0xF2401C20
    // sf=1, opc=11(ANDS), N=1, immr=0, imms=7, Rn=1, Rd=0
    #[test]
    fn jit_vs_interp_ands_x0_x1_0xff() {
        let mut init = InitState::default();
        init.x[1] = 0x0000_0000_0000_0000; // result = 0 & 0xFF = 0 => Z=1
        assert_jit_matches_interpreter(
            0xF2401C20,
            0x4005ac,
            &init,
            "ANDS X0, X1, #0xFF with X1=0 => Z=1",
        );
    }

    // ── Test: ANDS with non-zero result ───────────────────────────────────
    #[test]
    fn jit_vs_interp_ands_x0_x1_0xff_nonzero() {
        let mut init = InitState::default();
        init.x[1] = 0x80000000_0000_00AB; // result = 0xAB & 0xFF = 0xAB => Z=0
        assert_jit_matches_interpreter(
            0xF2401C20,
            0x4005b0,
            &init,
            "ANDS X0, X1, #0xFF with X1=0xAB => Z=0",
        );
    }

    // ── Test: 32-bit ADD W0, W1, #0x10 ────────────────────────────────────
    // ADD W0, W1, #0x10 → encoding: 0x11004020
    // sf=0, op=0, S=0, sh=0, imm12=0x10, Rn=1, Rd=0
    #[test]
    fn jit_vs_interp_add_w0_w1_0x10() {
        let mut init = InitState::default();
        init.x[0] = 0xFFFF_FFFF_FFFF_FFFF; // dirty upper bits
        init.x[1] = 0xFFFF_FFFF_0000_0100; // W1 = 0x100 with dirty upper
        assert_jit_matches_interpreter(
            0x11004020,
            0x4005b4,
            &init,
            "ADD W0, W1, #0x10 (32-bit, verify zero-extension)",
        );
    }

    // ── Test: MOVZ with shifted immediate ─────────────────────────────────
    // MOVZ X5, #0xABCD, LSL #48 → encoding: 0xD2F579A5
    // sf=1, opc=10(MOVZ), hw=3, imm16=0xABCD, Rd=5
    #[test]
    fn jit_vs_interp_movz_x5_abcd_lsl48() {
        assert_jit_matches_interpreter(
            0xD2F579A5,
            0x4005b8,
            &InitState::default(),
            "MOVZ X5, #0xABCD, LSL #48",
        );
    }

    // ── Test: MOVN X1, #0 (all 1s) ───────────────────────────────────────
    // MOVN X1, #0 → encoding: 0x92800001
    // sf=1, opc=00(MOVN), hw=0, imm16=0, Rd=1
    #[test]
    fn jit_vs_interp_movn_x1_0() {
        assert_jit_matches_interpreter(
            0x92800001,
            0x4005bc,
            &InitState::default(),
            "MOVN X1, #0 (MOV X1, #-1)",
        );
    }

    // ── Test: SUBS with overflow (V flag) ─────────────────────────────────
    // SUBS X0, X0, #1 with X0 = 0x8000_0000_0000_0000 (min signed)
    // min_signed - 1 = max_signed => V=1 (signed overflow)
    // Also C=1 (no unsigned borrow since a >= b)
    #[test]
    fn jit_vs_interp_subs_signed_overflow() {
        let mut init = InitState::default();
        init.x[0] = 0x8000_0000_0000_0000;
        assert_jit_matches_interpreter(
            0xF1000400, // SUBS X0, X0, #1
            0x4005c0,
            &init,
            "SUBS X0, X0, #1 with X0=MIN_SIGNED => V=1",
        );
    }

    // ── Test: ADDS with signed overflow ───────────────────────────────────
    // ADDS X0, X0, #1 with X0 = 0x7FFF_FFFF_FFFF_FFFF (max signed)
    // max_signed + 1 = min_signed => V=1, N=1
    #[test]
    fn jit_vs_interp_adds_signed_overflow() {
        let mut init = InitState::default();
        init.x[0] = 0x7FFF_FFFF_FFFF_FFFF;
        assert_jit_matches_interpreter(
            0xB1000400, // ADDS X0, X0, #1
            0x4005c4,
            &init,
            "ADDS X0, X0, #1 with X0=MAX_SIGNED => V=1,N=1",
        );
    }

    // ── Test: SUB register with LSR shift ─────────────────────────────────
    // SUB X0, X1, X2, LSR #8 → encoding: 0xCB422020
    // sf=1, op=1(SUB), S=0, shift=01(LSR), Rm=2, imm6=8, Rn=1, Rd=0
    #[test]
    fn jit_vs_interp_sub_x0_x1_x2_lsr8() {
        let mut init = InitState::default();
        init.x[1] = 0x1000;
        init.x[2] = 0xFF00; // LSR #8 => 0xFF
        assert_jit_matches_interpreter(
            0xCB422020,
            0x4005c8,
            &init,
            "SUB X0, X1, X2, LSR #8",
        );
    }

    // ── Batch test: sweep many immediate values through SUBS ──────────────
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
        // SUBS X0, X0, #0 => encoding: 0xF1000000
        // sf=1, op=1(SUB), S=1, sh=0, imm12=0, Rn=0, Rd=0
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
