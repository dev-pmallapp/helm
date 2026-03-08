//! Per-ISA TCG frontends and register maps.
//!
//! Each target implements [`TcgFrontend`] to translate guest instructions
//! into [`TcgOp`](crate::ir::TcgOp) sequences, and provides a register
//! map that the interpreter uses to shuttle state between the guest CPU
//! and the flat register array.
//!
//! ```text
//! target/
//!   aarch64/   — ARMv8 AArch64 (A64 instruction set)
//!   riscv64/   — RISC-V 64-bit (RV64GC)
//!   x86_64/    — x86-64 (AMD64 / Intel 64)
//! ```

pub mod aarch64;
pub mod riscv64;
pub mod x86_64;

use crate::context::TcgContext;
use crate::ir::TcgOp;

/// Result of translating a single guest instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslateAction {
    /// Keep translating the next instruction in this block.
    Continue,
    /// This instruction ends the block (branch, syscall, etc.).
    EndBlock,
    /// Instruction not handled — fall back to interpretive step().
    Unhandled,
}

/// Frontend trait — translates guest instructions into TcgOps.
///
/// Each ISA provides an implementation that decodes one instruction
/// at a time and emits ops into a [`TcgContext`].
pub trait TcgFrontend {
    /// Translate one guest instruction at the current position.
    /// Returns whether to continue, end the block, or fall back.
    fn translate_insn(&mut self, insn_word: u32) -> TranslateAction;

    /// Guest instruction size in bytes (4 for A64/RV64, variable for x86).
    fn insn_size(&self) -> usize {
        4
    }

    /// Architecture name for diagnostics.
    fn arch_name(&self) -> &'static str;
}

/// ISA-specific target-op opcodes.
///
/// Instead of polluting the shared [`TcgOp`] enum with per-ISA variants,
/// ISA-specific operations use `TcgOp::TargetOp { opcode, .. }` and each
/// target defines its opcode constants here.
pub mod target_ops {
    // AArch64 target-op opcodes (0x0100..0x01FF)
    pub const A64_DAIF_SET: u32 = 0x0100;
    pub const A64_DAIF_CLR: u32 = 0x0101;
    pub const A64_SPSEL: u32 = 0x0102;
    pub const A64_SVC: u32 = 0x0103;
    pub const A64_ERET: u32 = 0x0104;
    pub const A64_WFI: u32 = 0x0105;

    // RISC-V target-op opcodes (0x0200..0x02FF) — reserved
    pub const RV64_ECALL: u32 = 0x0200;
    pub const RV64_EBREAK: u32 = 0x0201;
    pub const RV64_WFI: u32 = 0x0202;
    pub const RV64_MRET: u32 = 0x0203;
    pub const RV64_SRET: u32 = 0x0204;
    pub const RV64_FENCE: u32 = 0x0205;
    pub const RV64_CSR_RW: u32 = 0x0206;

    // x86-64 target-op opcodes (0x0300..0x03FF) — reserved
    pub const X86_SYSCALL: u32 = 0x0300;
    pub const X86_SYSRET: u32 = 0x0301;
    pub const X86_HLT: u32 = 0x0302;
    pub const X86_CPUID: u32 = 0x0303;
    pub const X86_RDMSR: u32 = 0x0304;
    pub const X86_WRMSR: u32 = 0x0305;
}
