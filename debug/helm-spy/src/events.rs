// helm-spy: event types for the analysis layer
//
// These are the enriched event types that carry classification, context, and
// enrichment. They are richer than raw probe events: they carry InsnClass,
// BranchKind, ArchContext, and fault classification.

/// Instruction class -- index into IndexedCounter for instruction mix analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum InsnClass {
    IntAlu  = 0,
    IntMul  = 1,
    Branch  = 2,
    Load    = 3,
    Store   = 4,
    FpAlu   = 5,
    SimdAlu = 6,
    System  = 7,
    Nop     = 8,
    Atomic  = 9,
    Unknown = 10,
}

impl InsnClass {
    pub const COUNT: usize = 11;
    pub const LABELS: [&'static str; Self::COUNT] = [
        "IntAlu", "IntMul", "Branch", "Load", "Store",
        "FpAlu", "SimdAlu", "System", "Nop", "Atomic", "Unknown",
    ];
}

/// Branch type -- for branch mix analysis and predictor simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BranchKind {
    DirectCond   = 0,
    DirectUncond = 1,
    Call         = 2,
    Return       = 3,
    IndirectJump = 4,
    IndirectCall = 5,
}

impl BranchKind {
    pub const COUNT: usize = 6;
    pub const LABELS: [&'static str; Self::COUNT] = [
        "DirectCond", "DirectUncond", "Call", "Return", "IndirectJump", "IndirectCall",
    ];
}

/// Optional full register dump. Default is None (zero cost to construct).
#[derive(Debug, Clone, Default)]
pub enum ArchContext {
    #[default]
    None,
    Aarch64 {
        x: [u64; 31],
        sp: u64,
        pc: u64,
        nzcv: u32,
        fpsr: u32,
    },
    Riscv64 {
        x: [u64; 32],
        pc: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    InsnAbort,
    DataAbort,
    StoreAbort,
    Svc,
    Undefined,
    Other,
}

/// Emitted once per retired instruction (post-step).
#[derive(Debug, Clone)]
pub struct InsnInfo {
    pub vcpu_idx: usize,
    pub pc: u64,
    pub raw: u32,
    pub size: u8,
    pub class: InsnClass,
    pub opcode_name: &'static str,
    pub is_stub: bool,
    pub context: ArchContext,
    pub insn_count: u64,
}

#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub pc: u64,
    pub target: u64,
    pub taken: bool,
    pub kind: BranchKind,
    pub insn_count: u64,
}

#[derive(Debug, Clone)]
pub struct MemInfo {
    pub vaddr: u64,
    pub size: u8,
    pub is_store: bool,
    pub is_atomic: bool,
    pub pc: u64,
}

#[derive(Debug, Clone)]
pub struct SyscallInfo {
    pub vcpu_idx: usize,
    pub nr: u64,
    pub args: [u64; 6],
    pub pc: u64,
}

#[derive(Debug, Clone)]
pub struct SyscallRetInfo {
    pub vcpu_idx: usize,
    pub nr: u64,
    pub retval: i64,
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
