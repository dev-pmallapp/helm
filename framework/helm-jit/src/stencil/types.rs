//! Core types for the stencil (copy-and-patch) JIT backend.
//!
//! Stencils are pre-compiled x86-64 code templates with "holes" — relocations
//! that get patched at JIT time with instruction-specific values (register
//! offsets, immediates, branch targets, helper function pointers).

#![allow(missing_docs)]

use crate::helpers;

/// Which register field a hole refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegField {
    Rd,
    Rn,
    Rm,
    Ra,
    Rt,
    Rt2,
}

/// Runtime helper functions that stencils can call via function-pointer holes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperFn {
    MemRead,
    MemWrite,
}

impl HelperFn {
    /// Get the function pointer address for this helper.
    pub fn address(self) -> u64 {
        match self {
            Self::MemRead => helpers::jit_mem_read as *const () as u64,
            Self::MemWrite => helpers::jit_mem_write as *const () as u64,
        }
    }
}

/// What value to patch into a relocation hole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoleKind {
    /// Byte offset of a register in the flat array: `field_value * 8`.
    RegOffset(RegField),
    /// Sign-extend immediate from `from_bits` to 64 bits.
    ImmSext { from_bits: u8 },
    /// Zero-extend immediate to 64 bits.
    ImmZext,
    /// Address of a runtime helper function.
    Helper(HelperFn),
    /// Absolute guest PC of a branch target.
    BranchTarget,
    /// Fallthrough PC (current PC + instruction size).
    NextPc,
    /// Signed pre/post-index immediate.
    Simm,
    /// Shift amount (zero-extended).
    Shamt,
}

/// How a relocation should be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocKind {
    /// R_X86_64_32S — absolute 32-bit signed (displacement/immediate).
    Abs32,
    /// R_X86_64_PLT32 — PC-relative 32-bit (for `call` instructions).
    /// Value = S + A - P, where A is typically -4.
    PcRel32,
}

/// A single relocation record within a stencil.
#[derive(Debug, Clone, Copy)]
pub struct StencilReloc {
    /// Byte offset within the stencil's code where the 4-byte value is patched.
    pub byte_offset: u32,
    /// What value to write at this offset.
    pub hole: HoleKind,
    /// How to apply the relocation (absolute vs PC-relative).
    pub kind: RelocKind,
}

/// A pre-compiled x86-64 code template for a single guest instruction.
///
/// Stencil data is `&'static` — generated at build time, zero allocation at
/// runtime. The `bytes` array contains x86-64 machine code with sentinel
/// values at relocation sites. At JIT time, bytes are memcpy'd into an
/// executable buffer and holes are patched with resolved values.
pub struct Stencil {
    /// Raw x86-64 machine code bytes.
    pub bytes: &'static [u8],
    /// Number of bytes to copy for non-terminator stencils (epilogue stripped).
    /// For terminators, this equals `bytes.len()`.
    pub body_len: usize,
    /// Relocation holes to patch after copying.
    pub relocs: &'static [StencilReloc],
    /// Whether this stencil ends a basic block (branch, syscall, etc.).
    pub is_terminator: bool,
}

/// All fields extracted from a decoded guest instruction, ready for hole
/// patching. Both AArch64 and RISC-V instructions are normalized into this
/// common representation.
#[derive(Debug, Clone, Copy, Default)]
pub struct DecodedFields {
    // Register indices (0-30 = X0-X30 / x0-x31, 31 = XZR/SP for AArch64)
    pub rd: u8,
    pub rn: u8,
    pub rm: u8,
    pub ra: u8,
    pub rt: u8,
    pub rt2: u8,

    // Immediates
    pub imm: i64,
    pub simm: i64,
    pub shamt: u8,

    // Control fields
    pub sf: u8,    // 0 = 32-bit, 1 = 64-bit
    pub shift: u8, // shift type (0=LSL, 1=LSR, 2=ASR, 3=ROR)
    pub cond: u8,  // 4-bit condition code

    // Computed addresses
    pub branch_target: u64,
    pub next_pc: u64,
}
