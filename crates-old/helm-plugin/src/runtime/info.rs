//! Introspection types passed to plugin callbacks.

use helm_core::types::Addr;

/// Read-only view of an instruction during translation.
#[derive(Debug, Clone)]
pub struct InsnInfo {
    pub vaddr: Addr,
    pub bytes: Vec<u8>,
    pub size: usize,
    pub mnemonic: String,
    pub symbol: Option<String>,
}

/// Read-only view of a translated block.
#[derive(Debug, Clone)]
pub struct TbInfo {
    pub pc: Addr,
    pub insn_count: usize,
    pub size: usize,
}

/// Memory access details provided to mem callbacks.
#[derive(Debug, Clone)]
pub struct MemInfo {
    pub vaddr: Addr,
    pub size: usize,
    pub is_store: bool,
    pub paddr: Option<Addr>,
}

/// Syscall entry details.
#[derive(Debug, Clone)]
pub struct SyscallInfo {
    pub number: u64,
    pub args: [u64; 6],
    pub vcpu_idx: usize,
}

/// Syscall return details.
#[derive(Debug, Clone)]
pub struct SyscallRetInfo {
    pub number: u64,
    pub ret_value: u64,
    pub vcpu_idx: usize,
}

// ═══════════════════════════════════════════════════════════════════
// Fault diagnostics
// ═══════════════════════════════════════════════════════════════════

/// Classification of an execution fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    /// PC reached address 0 (NULL function pointer call / return).
    NullJump,
    /// PC landed in an unmapped or non-executable region.
    WildJump,
    /// Undefined / unimplemented instruction.
    Undef,
    /// Memory access to an invalid address.
    MemFault,
    /// Stack pointer outside expected bounds or misaligned.
    StackCorruption,
    /// Two threads share the same TLS pointer.
    TlsAliasing,
    /// A critical syscall returned -ENOSYS.
    UnsupportedSyscall,
}

impl std::fmt::Display for FaultKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NullJump => write!(f, "NullJump"),
            Self::WildJump => write!(f, "WildJump"),
            Self::Undef => write!(f, "Undef"),
            Self::MemFault => write!(f, "MemFault"),
            Self::StackCorruption => write!(f, "StackCorruption"),
            Self::TlsAliasing => write!(f, "TlsAliasing"),
            Self::UnsupportedSyscall => write!(f, "UnsupportedSyscall"),
        }
    }
}

/// Arch-specific register context at fault time.
#[derive(Debug, Clone)]
pub enum ArchContext {
    /// AArch64 register snapshot.
    Aarch64 {
        x: [u64; 31],
        sp: u64,
        pc: u64,
        nzcv: u32,
        tpidr_el0: u64,
        current_el: u8,
    },
    /// RISC-V register snapshot (future).
    Riscv { x: [u64; 32], pc: u64 },
    /// x86-64 register snapshot (future).
    X86_64 {
        rax: u64,
        rbx: u64,
        rcx: u64,
        rdx: u64,
        rsi: u64,
        rdi: u64,
        rsp: u64,
        rbp: u64,
        r8: u64,
        r9: u64,
        r10: u64,
        r11: u64,
        r12: u64,
        r13: u64,
        r14: u64,
        r15: u64,
        rip: u64,
        rflags: u64,
    },
    /// Unknown / no context available.
    None,
}

/// Diagnostic information emitted when the engine detects a fault.
#[derive(Debug, Clone)]
pub struct FaultInfo {
    /// Which vCPU faulted.
    pub vcpu_idx: usize,
    /// Guest PC at the faulting instruction.
    pub pc: u64,
    /// Raw instruction word (0 if unavailable).
    pub insn_word: u32,
    /// What kind of fault.
    pub fault_kind: FaultKind,
    /// Human-readable description from the engine.
    pub message: String,
    /// Total instructions executed before the fault.
    pub insn_count: u64,
    /// Arch-specific register snapshot.
    pub arch_context: ArchContext,
}
