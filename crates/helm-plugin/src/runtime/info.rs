/// Per-instruction info passed to callbacks.
#[derive(Debug, Clone)]
pub struct InsnInfo {
    pub pc: u64,
    pub raw: u32,
    pub size: u8,
    pub class: InsnClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsnClass {
    IntAlu, IntMul, Branch, Load, Store,
    FpAlu, SimdAlu, System, Nop, Atomic, Unknown,
}

#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub pc: u64,
    pub target: u64,
    pub taken: bool,
    pub kind: BranchKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchKind {
    DirectCond, DirectUncond, Call, Return, IndirectJump, IndirectCall,
}

#[derive(Debug, Clone)]
pub struct MemInfo {
    pub vaddr: u64,
    pub size: u8,
    pub is_store: bool,
    pub is_atomic: bool,
}

#[derive(Debug, Clone)]
pub struct SyscallInfo {
    pub vcpu_idx: usize,
    pub number: u64,
    pub args: [u64; 6],
}

#[derive(Debug, Clone)]
pub struct SyscallRetInfo {
    pub vcpu_idx: usize,
    pub number: u64,
    pub ret_value: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    IllegalInstruction, MemoryFault, StackCorruption,
    NullDereference, WildJump, UnsupportedSyscall, Breakpoint,
}

impl std::fmt::Display for FaultKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone)]
pub enum ArchContext {
    Aarch64 { x: [u64; 31], sp: u64, pc: u64, nzcv: u32 },
    RiscV { x: [u64; 32], pc: u64 },
    None,
}

#[derive(Debug, Clone)]
pub struct FaultInfo {
    pub vcpu_idx: usize,
    pub pc: u64,
    pub raw: u32,
    pub kind: FaultKind,
    pub message: String,
    pub insn_count: u64,
    pub context: ArchContext,
}
