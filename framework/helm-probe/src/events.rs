/// Broad instruction class -- matches helm-spy's `InsnClass` exactly.
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
/// `raw` is 0 on `pre_step` (instruction not yet fetched).
/// `insn_class` is `Unknown` on `pre_step`; only `post_step` carries meaningful classification.
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

/// Emitted for each data memory access (SE mode via `InstrumentedMem`).
#[derive(Debug, Clone)]
pub struct MemAccessEvent {
    pub addr: u64,
    pub size: u8,
    pub is_store: bool,
    pub pc: u64,
}

/// Emitted on every branch instruction. Replaces `sim_branch!()`.
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

/// MMU translation access type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmuAccessKind {
    Read,
    Write,
    Execute,
}

/// Emitted for each AArch64 MMU translation that touches the software TLB.
#[derive(Debug, Clone)]
pub struct MmuTranslateEvent {
    /// Virtual address being translated.
    pub va: u64,
    /// Translation access type.
    pub access: MmuAccessKind,
    /// Whether the translation hit in the TLB.
    pub tlb_hit: bool,
    /// Whether the translation missed in the TLB.
    pub tlb_miss: bool,
    /// Whether a stage-1 walk was performed.
    pub stage1_walk: bool,
    /// Whether a stage-2 walk was performed.
    pub stage2_walk: bool,
}

// ── JIT events ──────────────────────────────────────────────────────────────

/// Which JIT backend compiled a block or trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JitBackendId {
    /// Stencil copy-and-patch baseline.
    Stencil,
    /// Dynasm optimised tier.
    Dynasm,
    /// Any other / external backend.
    Other,
}

/// Emitted when a JIT backend compiles a new block.
#[derive(Debug, Clone)]
pub struct JitBlockCompileEvent {
    /// Guest PC at the start of the compiled block.
    pub pc: u64,
    /// Number of guest instructions in the block.
    pub insn_count: u32,
    /// Backend that produced the block.
    pub backend: JitBackendId,
}

/// Snapshot of guest register state at a JIT block boundary.
#[derive(Debug, Clone)]
pub struct JitBlockContext {
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub nzcv: u32,
    pub current_el: u8,
}

/// Emitted each time a compiled block is dispatched.
#[derive(Debug, Clone)]
pub struct JitBlockExecuteEvent {
    /// Guest PC at block entry.
    pub pc: u64,
    /// Guest PC after block exit.
    pub next_pc: u64,
    /// Number of guest instructions retired by this execution.
    pub insns_retired: u32,
    /// Exit code returned by the compiled block.
    pub exit_code: u64,
    /// Register context at block exit (populated when instrumentation is active).
    pub context: Option<JitBlockContext>,
}

/// Emitted when a trace is compiled from recorded hot-path blocks.
#[derive(Debug, Clone)]
pub struct JitTraceCompileEvent {
    /// Guest PC of the trace header (loop entry).
    pub start_pc: u64,
    /// Number of guest instructions in the trace body.
    pub insn_count: u32,
    /// Number of guard exit points in the trace.
    pub guard_count: u32,
}

/// Emitted each time a compiled trace is dispatched.
#[derive(Debug, Clone)]
pub struct JitTraceExecuteEvent {
    /// Guest PC of the trace header.
    pub start_pc: u64,
    /// Exit code (END_OF_BLOCK or EXIT_GUARD_BASE + id).
    pub exit_code: u64,
    /// Guest PC after trace exit.
    pub resume_pc: u64,
    /// Guest instructions retired before exiting.
    pub insns_retired: u32,
}

/// Block cache events (hit, miss, evict, promote).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitCacheOp {
    Hit,
    Miss,
    Evict,
    Promote,
}

/// Emitted on block-cache lookups and mutations.
#[derive(Debug, Clone)]
pub struct JitCacheEvent {
    /// Guest PC involved.
    pub pc: u64,
    /// What happened.
    pub op: JitCacheOp,
    /// Execution count at the time of the event (hits/promotes only).
    pub exec_count: u32,
}

/// Emitted when a trace guard fires (side exit from a compiled trace).
#[derive(Debug, Clone)]
pub struct JitGuardExitEvent {
    /// Guest PC of the trace header.
    pub trace_pc: u64,
    /// Guard index within the trace.
    pub guard_id: u32,
    /// Guest PC at which execution resumes after the guard.
    pub resume_pc: u64,
    /// Cumulative miss count for this guard.
    pub miss_count: u32,
    /// Whether the trace will be retired after this exit.
    pub retiring: bool,
}

/// Emitted when the JIT falls back to the interpreter.
#[derive(Debug, Clone)]
pub struct JitFallbackEvent {
    /// Guest PC where fallback begins.
    pub pc: u64,
    /// Number of instructions the interpreter batch retired.
    pub insns: u64,
    /// Opcode name that caused the fallback (if unsupported-start).
    pub reason: Option<&'static str>,
}

/// Single register mismatch between JIT and interpreter.
#[derive(Debug, Clone)]
pub struct JitVerifyMismatch {
    /// Register name: "x0"-"x30", "sp", "pc", "nzcv"
    pub name: &'static str,
    /// Value produced by JIT compiled block.
    pub jit_val: u64,
    /// Value produced by interpreter.
    pub interp_val: u64,
}

/// Emitted when JIT block verification detects a register mismatch.
#[derive(Debug, Clone)]
pub struct JitVerifyEvent {
    /// Guest PC at block entry.
    pub pc: u64,
    /// Number of guest instructions in the block.
    pub insn_count: u32,
    /// Which backend compiled the block.
    pub backend: JitBackendId,
    /// All register mismatches found.
    pub mismatches: Vec<JitVerifyMismatch>,
}
