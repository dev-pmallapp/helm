/// Broad instruction class -- matches helm-spy's InsnClass exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum InsnClass {
    IntAlu = 0,
    IntMul = 1,
    Branch = 2,
    Load = 3,
    Store = 4,
    FpAlu = 5,
    SimdAlu = 6,
    System = 7,
    Nop = 8,
    Atomic = 9,
    Unknown = 10,
}

impl InsnClass {
    pub const COUNT: usize = 11;
    pub const LABELS: [&'static str; Self::COUNT] = [
        "IntAlu", "IntMul", "Branch", "Load", "Store", "FpAlu", "SimdAlu", "System", "Nop",
        "Atomic", "Unknown",
    ];
}

/// Emitted before or after each instruction step.
/// `raw` is 0 on pre_step (instruction not yet fetched).
/// `insn_class` is `Unknown` on pre_step; only `post_step` carries meaningful classification.
#[derive(Debug, Clone)]
pub struct CpuStepEvent {
    pub pc: u64,
    pub raw: u32,
    pub insn_class: InsnClass,
    pub is_stub: bool,
    #[cfg(feature = "probe-full")]
    pub insn_count: u64,
}

/// Emitted when a handled guest exception is delivered.
/// kind: "insn-abort" | "data-abort" | "store-abort" | "svc"
#[derive(Debug, Clone)]
pub struct CpuFaultEvent {
    pub pc: u64,
    pub raw: u32,
    pub kind: &'static str,
}

/// Emitted for each data memory access (SE mode via InstrumentedMem).
#[derive(Debug, Clone)]
pub struct MemAccessEvent {
    pub addr: u64,
    pub size: u8,
    pub is_store: bool,
    pub pc: u64,
}

/// Emitted on every branch instruction. Replaces sim_branch!().
/// Zero cost in release (ZST probe).
#[derive(Debug, Clone)]
pub struct BranchEvent {
    pub pc: u64,
    pub target: u64,
    pub taken: bool,
    pub kind: BranchKind,
}

/// Branch type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchKind {
    DirectCond,
    DirectUncond,
    Call,
    Return,
    IndirectJump,
    IndirectCall,
}

/// Emitted when an interrupt line changes state in the GIC.
#[derive(Debug, Clone)]
pub struct IrqEvent {
    pub irq_id: u32,
    pub asserted: bool,
}

/// Emitted on MMIO device register read or write.
#[derive(Debug, Clone)]
pub struct MmioEvent {
    pub addr: u64,
    pub size: u8,
    pub val: u64,
    pub is_write: bool,
}
