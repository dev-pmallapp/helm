//! Dynasm-rs JIT backend for RISC-V64 -> x86-64 translation.
//!
//! # Calling convention
//!
//! The compiled block is called as:
//! ```text
//! extern "C" fn(regs: *mut u64, mem: *mut u8) -> u64
//! ```
//! - `rdi` = pointer to flat register array (`[u64; 40]`)
//!   - Slots 0-31: integer registers x0-x31 (x0 always zero)
//!   - Slot 32: PC
//!   - Slots 33-37: reserved
//!   - Slots 38-39: reserved for helper fn ptrs (stencil backend)
//! - `rsi` = pointer to `FlatMem` or `JitFsContext` (passed to memory helpers)
//! - Returns exit code in `rax` (`EXIT_*` constants from `block.rs`)
//!
//! # x0 invariant
//!
//! RISC-V x0 is hardwired to zero. All emitters skip writes when `rd == 0`.

#![allow(missing_docs)]
#![allow(unsafe_code)]

use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi};
use helm_arch::riscv::insn::Instruction;

use crate::block::{CompiledBlock, JitBlockFn, EXIT_END_OF_BLOCK};
use crate::regs::REG_PC_RV64;

pub mod emit;

/// Maximum number of guest instructions per compiled block.
const MAX_BLOCK_INSNS: usize = 64;

/// Dynasm-rs JIT backend for RISC-V64.
pub struct DynasmBackendRv64;

impl DynasmBackendRv64 {
    /// Create a new RV64 dynasm backend instance.
    pub fn new() -> Self {
        Self
    }

    /// Compile a block of decoded RV64 instructions into x86-64 machine code.
    pub fn compile_block(&mut self, pc: u64, insns: &[Instruction]) -> Option<CompiledBlock> {
        compile_block_rv64(pc, insns)
    }

    /// Human-readable name of this backend.
    pub fn name(&self) -> &str {
        "dynasm-rv64"
    }
}

impl Default for DynasmBackendRv64 {
    fn default() -> Self {
        Self::new()
    }
}

/// Compile a block of decoded RISC-V64 instructions into x86-64 machine code.
///
/// # Arguments
/// - `pc`: guest PC at the start of the block
/// - `insns`: slice of pre-decoded instructions starting at `pc`
///   (must be at least 1 instruction; the slice may be longer than needed)
///
/// # Returns
/// - `Some(CompiledBlock)` on success (at least one instruction compiled)
/// - `None` if the first instruction is unsupported or the slice is empty
pub fn compile_block_rv64(pc: u64, insns: &[Instruction]) -> Option<CompiledBlock> {
    if insns.is_empty() {
        return None;
    }

    let mut ops = Assembler::new().ok()?;
    let mut insn_count: u32 = 0;
    let mut current_pc = pc;

    // ── Instruction emission ────────────────────────────────────────────────
    for (i, insn) in insns.iter().enumerate() {
        if i >= MAX_BLOCK_INSNS {
            break;
        }

        match emit::emit_rv64_insn(&mut ops, insn, current_pc) {
            Some(true) => {
                // Block-terminating instruction (branch/ecall/ebreak).
                // The emitter already wrote the PC update, exit code, and `ret`.
                insn_count += 1;
                break;
            }
            Some(false) => {
                // Non-terminating instruction.
                insn_count += 1;
                current_pc += 4; // All expanded instructions are 4 bytes
            }
            None => {
                // Unsupported opcode -- stop compilation here.
                break;
            }
        }
    }

    if insn_count == 0 {
        return None;
    }

    // ── Epilogue (fall-through case) ────────────────────────────────────────
    // If the last instruction was a block terminator, it already emitted its
    // own ret. This epilogue handles the non-terminator fall-through: update
    // PC to the next sequential instruction and return EXIT_END_OF_BLOCK.
    let next_pc = pc + u64::from(insn_count) * 4;
    let pc_off = (REG_PC_RV64 * 8) as i32;

    dynasm!(ops
        ; mov rax, QWORD next_pc as i64
        ; mov QWORD [rdi + pc_off], rax
        ; mov rax, QWORD EXIT_END_OF_BLOCK as i64
        ; ret
    );

    let buf = ops.finalize().ok()?;
    let entry: JitBlockFn = unsafe { std::mem::transmute(buf.ptr(dynasmrt::AssemblyOffset(0))) };
    Some(unsafe { CompiledBlock::new(buf, entry, pc, insn_count) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regs::{arch_to_flat_rv64, flat_to_arch_rv64, REG_COUNT_RV64};
    use helm_arch::riscv::insn::Instruction;

    /// Run a JIT block on the given initial register state and return the exit code.
    fn run_block(block: &CompiledBlock, regs: &mut [u64; REG_COUNT_RV64]) -> u64 {
        unsafe { (block.entry)(regs.as_mut_ptr(), std::ptr::null_mut()) }
    }

    #[test]
    fn compile_single_addi() {
        let insns = [Instruction::ADDI {
            rd: 1,
            rs1: 0,
            imm: 42,
        }];
        let block = compile_block_rv64(0x1000, &insns);
        assert!(block.is_some());
        let block = block.unwrap();
        assert_eq!(block.guest_pc, 0x1000);
        assert_eq!(block.insn_count, 1);
    }

    #[test]
    fn empty_insns_returns_none() {
        assert!(compile_block_rv64(0x2000, &[]).is_none());
    }

    #[test]
    fn unsupported_first_insn_returns_none() {
        // CSR instructions are unsupported
        let insns = [Instruction::CSRRW {
            rd: 1,
            rs1: 2,
            csr: 0x300,
        }];
        assert!(compile_block_rv64(0x3000, &insns).is_none());
    }

    #[test]
    fn execute_addi_x1_x0_42() {
        let insns = [Instruction::ADDI {
            rd: 1,
            rs1: 0,
            imm: 42,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        let exit = run_block(&block, &mut regs);
        assert_eq!(exit, EXIT_END_OF_BLOCK);
        assert_eq!(regs[0], 0, "x0 must remain zero");
        assert_eq!(regs[1], 42);
        assert_eq!(regs[REG_PC_RV64], 0x1004);
    }

    #[test]
    fn execute_addi_to_x0_is_nop() {
        let insns = [Instruction::ADDI {
            rd: 0,
            rs1: 1,
            imm: 100,
        }];
        let block = compile_block_rv64(0x2000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[1] = 999;
        let exit = run_block(&block, &mut regs);
        assert_eq!(exit, EXIT_END_OF_BLOCK);
        assert_eq!(regs[0], 0, "writing to x0 must be a no-op");
        assert_eq!(regs[1], 999, "rs1 unchanged");
    }

    #[test]
    fn execute_add_sequence() {
        let insns = [
            Instruction::ADDI {
                rd: 1,
                rs1: 0,
                imm: 10,
            },
            Instruction::ADDI {
                rd: 2,
                rs1: 0,
                imm: 20,
            },
            Instruction::ADD {
                rd: 3,
                rs1: 1,
                rs2: 2,
            },
        ];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        assert_eq!(block.insn_count, 3);
        let mut regs = [0u64; REG_COUNT_RV64];
        let exit = run_block(&block, &mut regs);
        assert_eq!(exit, EXIT_END_OF_BLOCK);
        assert_eq!(regs[1], 10);
        assert_eq!(regs[2], 20);
        assert_eq!(regs[3], 30);
        assert_eq!(regs[REG_PC_RV64], 0x100C);
    }

    #[test]
    fn execute_lui() {
        let insns = [Instruction::LUI {
            rd: 5,
            imm: 0x12345_000,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        run_block(&block, &mut regs);
        assert_eq!(regs[5], 0x12345_000);
    }

    #[test]
    fn execute_auipc() {
        let insns = [Instruction::AUIPC { rd: 5, imm: 0x1000 }];
        let block = compile_block_rv64(0x8000_0000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        run_block(&block, &mut regs);
        assert_eq!(regs[5], 0x8000_1000);
    }

    #[test]
    fn execute_sub() {
        let insns = [
            Instruction::ADDI {
                rd: 1,
                rs1: 0,
                imm: 100,
            },
            Instruction::ADDI {
                rd: 2,
                rs1: 0,
                imm: 30,
            },
            Instruction::SUB {
                rd: 3,
                rs1: 1,
                rs2: 2,
            },
        ];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        run_block(&block, &mut regs);
        assert_eq!(regs[3], 70);
    }

    #[test]
    fn execute_slt_signed() {
        // -1 < 1 (signed) => true
        let insns = [Instruction::SLT {
            rd: 3,
            rs1: 1,
            rs2: 2,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[1] = u64::MAX; // -1 as i64
        regs[2] = 1;
        run_block(&block, &mut regs);
        assert_eq!(regs[3], 1, "-1 < 1 signed");
    }

    #[test]
    fn execute_sltu_unsigned() {
        // 1 < 0xFFFF... (unsigned) => true
        let insns = [Instruction::SLTU {
            rd: 3,
            rs1: 1,
            rs2: 2,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[1] = 1;
        regs[2] = u64::MAX;
        run_block(&block, &mut regs);
        assert_eq!(regs[3], 1, "1 < MAX unsigned");
    }

    #[test]
    fn execute_shift_imm() {
        let insns = [
            Instruction::ADDI {
                rd: 1,
                rs1: 0,
                imm: 1,
            },
            Instruction::SLLI {
                rd: 2,
                rs1: 1,
                shamt: 10,
            },
        ];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        run_block(&block, &mut regs);
        assert_eq!(regs[2], 1024);
    }

    #[test]
    fn execute_addiw_sign_extends() {
        // ADDIW with result that has bit 31 set => sign-extended to 64
        let insns = [Instruction::ADDIW {
            rd: 1,
            rs1: 2,
            imm: 1,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[2] = 0x7FFF_FFFF; // Max positive i32
        run_block(&block, &mut regs);
        // 0x7FFF_FFFF + 1 = 0x8000_0000 => sign-extended to 0xFFFF_FFFF_8000_0000
        assert_eq!(regs[1], 0xFFFF_FFFF_8000_0000u64);
    }

    #[test]
    fn execute_jal() {
        let insns = [Instruction::JAL { rd: 1, imm: 0x100 }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        let exit = run_block(&block, &mut regs);
        assert_eq!(exit, EXIT_END_OF_BLOCK);
        assert_eq!(regs[1], 0x1004, "rd = pc + 4 (return address)");
        assert_eq!(regs[REG_PC_RV64], 0x1100, "PC = pc + imm");
    }

    #[test]
    fn execute_jalr() {
        let insns = [Instruction::JALR {
            rd: 1,
            rs1: 2,
            imm: 0x10,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[2] = 0x2001; // odd address
        let exit = run_block(&block, &mut regs);
        assert_eq!(exit, EXIT_END_OF_BLOCK);
        assert_eq!(regs[1], 0x1004, "rd = pc + 4");
        assert_eq!(
            regs[REG_PC_RV64], 0x2010,
            "PC = (rs1 + imm) & ~1 = (0x2001 + 0x10) & ~1 = 0x2010"
        );
    }

    #[test]
    fn execute_beq_taken() {
        let insns = [Instruction::BEQ {
            rs1: 1,
            rs2: 2,
            imm: 0x20,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[1] = 42;
        regs[2] = 42;
        run_block(&block, &mut regs);
        assert_eq!(regs[REG_PC_RV64], 0x1020, "branch taken");
    }

    #[test]
    fn execute_beq_not_taken() {
        let insns = [Instruction::BEQ {
            rs1: 1,
            rs2: 2,
            imm: 0x20,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[1] = 42;
        regs[2] = 99;
        run_block(&block, &mut regs);
        assert_eq!(regs[REG_PC_RV64], 0x1004, "branch not taken");
    }

    #[test]
    fn execute_ecall() {
        let insns = [Instruction::ECALL];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        let exit = run_block(&block, &mut regs);
        assert_eq!(exit, crate::block::EXIT_SYSCALL);
        assert_eq!(regs[REG_PC_RV64], 0x1000, "PC points at ECALL");
    }

    #[test]
    fn execute_fence_is_nop() {
        let insns = [
            Instruction::ADDI {
                rd: 1,
                rs1: 0,
                imm: 5,
            },
            Instruction::FENCE { pred: 0, succ: 0 },
            Instruction::ADDI {
                rd: 2,
                rs1: 0,
                imm: 10,
            },
        ];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        assert_eq!(block.insn_count, 3);
        let mut regs = [0u64; REG_COUNT_RV64];
        run_block(&block, &mut regs);
        assert_eq!(regs[1], 5);
        assert_eq!(regs[2], 10);
    }

    #[test]
    fn execute_mul() {
        let insns = [Instruction::MUL {
            rd: 3,
            rs1: 1,
            rs2: 2,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[1] = 7;
        regs[2] = 6;
        run_block(&block, &mut regs);
        assert_eq!(regs[3], 42);
    }

    #[test]
    fn execute_div_by_zero() {
        let insns = [Instruction::DIV {
            rd: 3,
            rs1: 1,
            rs2: 2,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[1] = 100;
        regs[2] = 0;
        run_block(&block, &mut regs);
        assert_eq!(regs[3], u64::MAX, "div by zero = -1 (all ones)");
    }

    #[test]
    fn execute_rem_by_zero() {
        let insns = [Instruction::REM {
            rd: 3,
            rs1: 1,
            rs2: 2,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[1] = 42;
        regs[2] = 0;
        run_block(&block, &mut regs);
        assert_eq!(regs[3], 42, "rem by zero = dividend");
    }

    #[test]
    fn execute_div_signed_overflow() {
        // MIN_I64 / -1 = MIN_I64 (overflow)
        let insns = [Instruction::DIV {
            rd: 3,
            rs1: 1,
            rs2: 2,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[1] = i64::MIN as u64;
        regs[2] = u64::MAX; // -1
        run_block(&block, &mut regs);
        assert_eq!(regs[3], i64::MIN as u64, "signed overflow: MIN / -1 = MIN");
    }

    #[test]
    fn execute_rem_signed_overflow() {
        // MIN_I64 % -1 = 0 (overflow)
        let insns = [Instruction::REM {
            rd: 3,
            rs1: 1,
            rs2: 2,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[1] = i64::MIN as u64;
        regs[2] = u64::MAX; // -1
        run_block(&block, &mut regs);
        assert_eq!(regs[3], 0, "signed overflow: MIN % -1 = 0");
    }

    #[test]
    fn flat_round_trip() {
        let mut iregs = [0u64; 32];
        iregs[1] = 0xDEAD_BEEF;
        iregs[31] = 0x1234;
        let pc = 0x8000_0000u64;

        let mut flat = arch_to_flat_rv64(&iregs, pc);
        assert_eq!(flat[0], 0, "x0 always zero");
        assert_eq!(flat[1], 0xDEAD_BEEF);
        assert_eq!(flat[31], 0x1234);
        assert_eq!(flat[REG_PC_RV64], 0x8000_0000);

        // Simulate JIT modifying registers
        flat[1] = 42;
        flat[REG_PC_RV64] = 0x8000_0008;

        let mut iregs_out = [0u64; 32];
        let mut pc_out = 0u64;
        flat_to_arch_rv64(&mut flat, &mut iregs_out, &mut pc_out);
        assert_eq!(iregs_out[0], 0, "x0 stays zero");
        assert_eq!(iregs_out[1], 42);
        assert_eq!(pc_out, 0x8000_0008);
    }

    #[test]
    fn execute_blt_signed() {
        // -1 < 1 (signed) => taken
        let insns = [Instruction::BLT {
            rs1: 1,
            rs2: 2,
            imm: 0x100,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[1] = u64::MAX; // -1
        regs[2] = 1;
        run_block(&block, &mut regs);
        assert_eq!(regs[REG_PC_RV64], 0x1100, "BLT taken: -1 < 1");
    }

    #[test]
    fn execute_bltu_unsigned() {
        // 1 < MAX (unsigned) => taken
        let insns = [Instruction::BLTU {
            rs1: 1,
            rs2: 2,
            imm: 0x100,
        }];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        regs[1] = 1;
        regs[2] = u64::MAX;
        run_block(&block, &mut regs);
        assert_eq!(regs[REG_PC_RV64], 0x1100, "BLTU taken: 1 < MAX unsigned");
    }

    #[test]
    fn execute_xor_or_and() {
        let insns = [
            Instruction::XORI {
                rd: 1,
                rs1: 0,
                imm: 0xFF,
            },
            Instruction::ORI {
                rd: 2,
                rs1: 0,
                imm: 0x0F,
            },
            Instruction::AND {
                rd: 3,
                rs1: 1,
                rs2: 2,
            },
        ];
        let block = compile_block_rv64(0x1000, &insns).unwrap();
        let mut regs = [0u64; REG_COUNT_RV64];
        run_block(&block, &mut regs);
        assert_eq!(regs[1], 0xFF);
        assert_eq!(regs[2], 0x0F);
        assert_eq!(regs[3], 0x0F, "0xFF & 0x0F = 0x0F");
    }
}
