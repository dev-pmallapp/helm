/// Per-instruction info passed to callbacks.
#[derive(Debug, Clone)]
pub struct PluginInsnInfo {
    pub pc: u64,
    pub raw: u32,
    pub size: u8,
    pub class: InsnClass,
    /// Opcode name (e.g. "SimdOther", "AddImm"). Empty string if unknown.
    pub opcode_name: &'static str,
    /// True if this instruction was silently skipped (unimplemented stub).
    pub is_stub: bool,
    /// Architectural context at time of execution.
    pub context: ArchContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsnClass {
    IntAlu,
    IntMul,
    Branch,
    Load,
    Store,
    FpAlu,
    SimdAlu,
    System,
    Nop,
    Atomic,
    Unknown,
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
    DirectCond,
    DirectUncond,
    Call,
    Return,
    IndirectJump,
    IndirectCall,
}

#[derive(Debug, Clone)]
pub struct MemInfo {
    pub pc: u64,
    pub raw: u32,
    pub opcode_name: &'static str,
    pub class: InsnClass,
    pub vaddr: u64,
    pub paddr: u64,
    pub size: u8,
    pub is_store: bool,
    pub is_atomic: bool,
    pub value_before: Option<u64>,
    pub value_after: Option<u64>,
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
    IllegalInstruction,
    MemoryFault,
    StackCorruption,
    NullDereference,
    WildJump,
    UnsupportedSyscall,
    Breakpoint,
}

impl std::fmt::Display for FaultKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone)]
pub enum ArchContext {
    Aarch64 {
        x: [u64; 31],
        sp: u64,
        pc: u64,
        nzcv: u32,
        current_el: u8,
        tpidrro_el0: u64,
    },
    RiscV {
        x: [u64; 32],
        pc: u64,
    },
    None,
}

impl ArchContext {
    pub fn arch_name(&self) -> &'static str {
        match self {
            Self::Aarch64 { .. } => "aarch64",
            Self::RiscV { .. } => "riscv64",
            Self::None => "none",
        }
    }

    pub fn default_register_names(&self) -> Vec<String> {
        match self {
            Self::Aarch64 { .. } => [
                "pc",
                "sp",
                "lr",
                "fp",
                "x0",
                "x1",
                "x2",
                "x3",
                "current_el",
                "nzcv",
                "tpidrro_el0",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            Self::RiscV { .. } => ["pc", "sp", "ra", "fp", "a0", "a1", "a2", "a3"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            Self::None => Vec::new(),
        }
    }

    pub fn all_register_names(&self) -> Vec<String> {
        match self {
            Self::Aarch64 { .. } => {
                let mut names = vec![
                    "pc".to_string(),
                    "sp".to_string(),
                    "lr".to_string(),
                    "fp".to_string(),
                    "current_el".to_string(),
                    "nzcv".to_string(),
                    "tpidrro_el0".to_string(),
                ];
                names.extend((0..=30).map(|idx| format!("x{idx}")));
                names
            }
            Self::RiscV { .. } => {
                let mut names = vec![
                    "pc".to_string(),
                    "ra".to_string(),
                    "sp".to_string(),
                    "gp".to_string(),
                    "tp".to_string(),
                    "fp".to_string(),
                ];
                names.extend((0..=31).map(|idx| format!("x{idx}")));
                names
            }
            Self::None => Vec::new(),
        }
    }

    pub fn lookup_register(&self, name: &str) -> Option<(String, u64)> {
        let name = name.trim().to_ascii_lowercase();
        match self {
            Self::Aarch64 {
                x,
                sp,
                pc,
                nzcv,
                current_el,
                tpidrro_el0,
            } => match name.as_str() {
                "pc" => Some(("pc".to_string(), *pc)),
                "sp" => Some(("sp".to_string(), *sp)),
                "nzcv" => Some(("nzcv".to_string(), *nzcv as u64)),
                "current_el" | "el" => Some(("current_el".to_string(), *current_el as u64)),
                "tpidrro_el0" | "tpidrro" => Some(("tpidrro_el0".to_string(), *tpidrro_el0)),
                "lr" | "x30" => Some(("lr".to_string(), x[30])),
                "fp" | "x29" => Some(("fp".to_string(), x[29])),
                _ if name.starts_with('x') => name[1..]
                    .parse::<usize>()
                    .ok()
                    .filter(|idx| *idx <= 30)
                    .map(|idx| (format!("x{idx}"), x[idx])),
                _ => None,
            },
            Self::RiscV { x, pc } => match riscv_reg_index(&name) {
                Some(None) => Some(("pc".to_string(), *pc)),
                Some(Some((idx, label))) => Some((label.to_string(), x[idx])),
                None => None,
            },
            Self::None => None,
        }
    }
}

fn riscv_reg_index(name: &str) -> Option<Option<(usize, &'static str)>> {
    match name {
        "pc" => Some(None),
        "zero" | "x0" => Some(Some((0, "x0"))),
        "ra" | "x1" => Some(Some((1, "ra"))),
        "sp" | "x2" => Some(Some((2, "sp"))),
        "gp" | "x3" => Some(Some((3, "gp"))),
        "tp" | "x4" => Some(Some((4, "tp"))),
        "t0" | "x5" => Some(Some((5, "t0"))),
        "t1" | "x6" => Some(Some((6, "t1"))),
        "t2" | "x7" => Some(Some((7, "t2"))),
        "s0" | "fp" | "x8" => Some(Some((8, "fp"))),
        "s1" | "x9" => Some(Some((9, "s1"))),
        "a0" | "x10" => Some(Some((10, "a0"))),
        "a1" | "x11" => Some(Some((11, "a1"))),
        "a2" | "x12" => Some(Some((12, "a2"))),
        "a3" | "x13" => Some(Some((13, "a3"))),
        "a4" | "x14" => Some(Some((14, "a4"))),
        "a5" | "x15" => Some(Some((15, "a5"))),
        "a6" | "x16" => Some(Some((16, "a6"))),
        "a7" | "x17" => Some(Some((17, "a7"))),
        "s2" | "x18" => Some(Some((18, "s2"))),
        "s3" | "x19" => Some(Some((19, "s3"))),
        "s4" | "x20" => Some(Some((20, "s4"))),
        "s5" | "x21" => Some(Some((21, "s5"))),
        "s6" | "x22" => Some(Some((22, "s6"))),
        "s7" | "x23" => Some(Some((23, "s7"))),
        "s8" | "x24" => Some(Some((24, "s8"))),
        "s9" | "x25" => Some(Some((25, "s9"))),
        "s10" | "x26" => Some(Some((26, "s10"))),
        "s11" | "x27" => Some(Some((27, "s11"))),
        "t3" | "x28" => Some(Some((28, "t3"))),
        "t4" | "x29" => Some(Some((29, "t4"))),
        "t5" | "x30" => Some(Some((30, "t5"))),
        "t6" | "x31" => Some(Some((31, "t6"))),
        _ => None,
    }
}

/// JIT block dispatch info passed to jit-block callbacks.
#[derive(Debug, Clone)]
pub struct JitBlockInfo {
    pub pc: u64,
    pub next_pc: u64,
    pub insns_retired: u32,
    pub exit_code: u64,
    pub context: ArchContext,
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

/// Cause of an EL transition observed via [`ExceptionInfo`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionCause {
    /// Synchronous trap (HVC/SVC/SMC, sysreg trap, abort, BRK, etc.).
    Sync,
    /// Physical IRQ delivery via the IRQ vector slot.
    Irq,
}

impl std::fmt::Display for ExceptionCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Per-EL-transition information passed to `on_exception` callbacks.
///
/// One event is delivered each time the simulated CPU enters an exception
/// vector — synchronous (HVC/SVC/SMC, sysreg trap, abort, BRK, …) or IRQ.
/// `context` snapshots architectural state captured *immediately after* the
/// vector is dispatched (so `pc` already points at `vector_pc`).
#[derive(Debug, Clone)]
pub struct ExceptionInfo {
    pub vcpu_idx: usize,
    pub cause: ExceptionCause,
    pub from_el: u8,
    pub target_el: u8,
    pub vector_pc: u64,
    pub elr: u64,
    pub spsr: u32,
    pub esr: u32,
    pub far: u64,
    pub insn_count: u64,
    pub context: ArchContext,
}

impl ExceptionInfo {
    /// Exception class field of `esr` (`ESR_ELx[31:26]`); only meaningful for
    /// synchronous exceptions.
    #[inline]
    pub fn ec(&self) -> u8 {
        ((self.esr >> 26) & 0x3F) as u8
    }

    /// ISS field of `esr` (`ESR_ELx[24:0]`); only meaningful for synchronous
    /// exceptions where the EC encodes a syndrome with ISS bits.
    #[inline]
    pub fn iss(&self) -> u32 {
        self.esr & 0x01FF_FFFF
    }

    /// HVC/SVC/SMC immediate field (low 16 bits of ISS) when this is a
    /// hypercall/syscall trap.
    #[inline]
    pub fn imm16(&self) -> u16 {
        (self.esr & 0xFFFF) as u16
    }
}
