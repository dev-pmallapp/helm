//! Unified instruction types consumed by all backends.
//!
//! A single `DecodedInsn` type is produced by every ISA decoder and consumed
//! by the executor, JIT translator, and timing backend. This eliminates the
//! old split between `MicroOp` (pipeline) and `TcgOp` (JIT).

use crate::types::{Addr, RegId};
use bitflags::bitflags;

/// ISA-independent decoded instruction. Single type consumed by all backends.
///
/// Designed to cover RISC (AArch64, RISC-V) and CISC (x86_64) alike.
/// - `len` is 1–15 for x86_64, 2 or 4 for ARM/RV
/// - `encoding_bytes` holds full encoding (x86 needs up to 15 bytes)
/// - `uop_count` > 1 for complex CISC instructions
/// - `mem_count` > 1 for string ops, PUSH/POP, ENTER/LEAVE
#[derive(Debug, Clone)]
pub struct DecodedInsn {
    pub pc: Addr,
    pub len: u8,
    pub encoding_bytes: [u8; 15],
    pub class: InsnClass,
    pub src_regs: [RegId; 6],
    pub dst_regs: [RegId; 4],
    pub src_count: u8,
    pub dst_count: u8,
    pub imm: i64,
    pub flags: InsnFlags,
    pub uop_count: u8,
    pub mem_count: u8,
}

impl Default for DecodedInsn {
    fn default() -> Self {
        Self {
            pc: 0,
            len: 0,
            encoding_bytes: [0; 15],
            class: InsnClass::Nop,
            src_regs: [0; 6],
            dst_regs: [0; 4],
            src_count: 0,
            dst_count: 0,
            imm: 0,
            flags: InsnFlags::empty(),
            uop_count: 1,
            mem_count: 0,
        }
    }
}

bitflags! {
    /// Behavioural / classification flags for a decoded instruction.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct InsnFlags: u32 {
        // ── Category bits (shared across all ISAs) ──────────────
        const BRANCH       = 1 << 0;
        const COND         = 1 << 1;
        const CALL         = 1 << 2;
        const RETURN       = 1 << 3;
        const LOAD         = 1 << 4;
        const STORE        = 1 << 5;
        const ATOMIC       = 1 << 6;
        const FENCE        = 1 << 7;
        const SYSCALL      = 1 << 8;
        const FLOAT        = 1 << 9;
        const SIMD         = 1 << 10;
        const SERIALIZE    = 1 << 11;

        // ── Memory-shape bits ───────────────────────────────────
        const LOAD_STORE   = 1 << 12;
        const MULTI_MEM    = 1 << 13;
        const PAIR         = 1 << 14;

        // ── CISC-specific bits ──────────────────────────────────
        const REP          = 1 << 15;
        const SEGMENT_OVR  = 1 << 16;
        const LOCK         = 1 << 17;
        const MICROCODE    = 1 << 18;
        const STRING_OP    = 1 << 19;
        const IO_PORT      = 1 << 20;
        const CRYPTO       = 1 << 21;

        // ── Privileged / system ─────────────────────────────────
        const PRIVILEGED   = 1 << 22;
        const TRAP         = 1 << 23;
        const SYSREG       = 1 << 24;
        const COPROC       = 1 << 25;
        const HV_CALL      = 1 << 26;

        // ── Pipeline hints ──────────────────────────────────────
        const PREFETCH     = 1 << 27;
        const CACHE_MAINT  = 1 << 28;
        const NOP          = 1 << 29;
        const SETS_FLAGS   = 1 << 30;
        const READS_FLAGS  = 1u32 << 31;
    }
}

/// Timing classification. One per instruction.
///
/// For CISC instructions that decompose into multiple uops, the class
/// reflects the dominant operation. The `uop_count` field in `DecodedInsn`
/// tells the pipeline model how many scheduler slots it consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InsnClass {
    // Integer
    IntAlu,
    IntMul,
    IntDiv,
    // Floating-point
    FpAlu,
    FpMul,
    FpDiv,
    FpCvt,
    // SIMD / vector
    SimdAlu,
    SimdMul,
    SimdFpAlu,
    SimdFpMul,
    SimdShuffle,
    // Memory
    Load,
    Store,
    LoadPair,
    StorePair,
    Atomic,
    Prefetch,
    // Control flow
    Branch,
    CondBranch,
    IndBranch,
    Call,
    Return,
    // System / special
    Syscall,
    Fence,
    Nop,
    CacheMaint,
    SysRegAccess,
    Crypto,
    IoPort,
    Microcode,
    StringOp,
}

/// Result of functionally executing one instruction.
#[derive(Debug, Clone)]
pub struct ExecOutcome {
    pub next_pc: Addr,
    pub mem_accesses: [MemAccessInfo; 2],
    pub mem_access_count: u8,
    pub branch_taken: bool,
    pub exception: Option<ExceptionInfo>,
    pub rep_ongoing: bool,
}

impl Default for ExecOutcome {
    fn default() -> Self {
        Self {
            next_pc: 0,
            mem_accesses: [MemAccessInfo::default(); 2],
            mem_access_count: 0,
            branch_taken: false,
            exception: None,
            rep_ongoing: false,
        }
    }
}

/// A single memory access performed by an instruction.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemAccessInfo {
    pub addr: Addr,
    pub size: u8,
    pub is_write: bool,
}

/// Exception/fault information.
#[derive(Debug, Clone)]
pub struct ExceptionInfo {
    pub class: u32,
    pub iss: u32,
    pub vaddr: Addr,
    pub target_el: u8,
}
