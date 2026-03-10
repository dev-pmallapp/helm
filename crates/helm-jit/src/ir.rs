//! TCG intermediate representation — simple register-transfer ops.

/// A TCG temporary (virtual register).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TcgTemp(pub u32);

/// TCG operations — a minimal RISC-like IR for fast interpretation.
///
/// Each op works on [`TcgTemp`] virtual registers.  The interpreter
/// (or future JIT) maps these to host registers/memory.
#[derive(Debug, Clone)]
pub enum TcgOp {
    // -- Moves and constants -----------------------------------------
    /// dst = imm64
    Movi {
        dst: TcgTemp,
        value: u64,
    },
    /// dst = src
    Mov {
        dst: TcgTemp,
        src: TcgTemp,
    },

    // -- Arithmetic --------------------------------------------------
    Add {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },
    Sub {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },
    Mul {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },
    Div {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },
    /// Signed division: dst = (a as i64) / (b as i64).
    SDiv {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },

    /// dst = a + imm
    Addi {
        dst: TcgTemp,
        a: TcgTemp,
        imm: i64,
    },

    // -- Bitwise -----------------------------------------------------
    And {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },
    Or {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },
    Xor {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },
    Not {
        dst: TcgTemp,
        src: TcgTemp,
    },

    Shl {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },
    Shr {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },
    Sar {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },

    // -- Memory (guest address space) --------------------------------
    /// dst = load(addr, size_bytes)
    Load {
        dst: TcgTemp,
        addr: TcgTemp,
        size: u8,
    },
    /// store(addr, val, size_bytes)
    Store {
        addr: TcgTemp,
        val: TcgTemp,
        size: u8,
    },

    // -- Branches and labels -----------------------------------------
    /// Unconditional branch to label.
    Br {
        label: u32,
    },
    /// Branch if cond != 0.
    BrCond {
        cond: TcgTemp,
        label: u32,
    },
    /// Label marker (not an executable op).
    Label {
        id: u32,
    },

    // -- Comparisons (set flags / produce bool) ----------------------
    /// dst = (a == b) ? 1 : 0
    SetEq {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },
    SetNe {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },
    SetLt {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },
    SetGe {
        dst: TcgTemp,
        a: TcgTemp,
        b: TcgTemp,
    },

    // -- Sign/zero extension -----------------------------------------
    /// dst = sign_extend(src, from_bits)
    Sext {
        dst: TcgTemp,
        src: TcgTemp,
        from_bits: u8,
    },
    /// dst = zero_extend(src, from_bits)
    Zext {
        dst: TcgTemp,
        src: TcgTemp,
        from_bits: u8,
    },

    // -- System ------------------------------------------------------
    /// Invoke syscall with nr in the given temp.
    Syscall {
        nr: TcgTemp,
    },
    /// Exit the translated block (return to dispatcher).
    ExitTb,
    /// Goto the given guest PC (chain to next block).
    GotoTb {
        target_pc: u64,
    },

    // -- System registers (GICv3 / FS-mode) ------------------------------
    /// Read a system register (MRS).  `sysreg_id` is the 16-bit encoding
    /// `(op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2`.
    ReadSysReg {
        dst: TcgTemp,
        sysreg_id: u32,
    },
    /// Write a system register (MSR).
    WriteSysReg {
        sysreg_id: u32,
        src: TcgTemp,
    },

    // -- PSTATE immediate writes -----------------------------------------
    /// Set bits in DAIF: `DAIF |= (imm4 << 6)`.
    DaifSet {
        imm: u32,
    },
    /// Clear bits in DAIF: `DAIF &= ~(imm4 << 6)`.
    DaifClr {
        imm: u32,
    },
    /// Set SPSel: `sp_sel = imm & 1`.
    SetSpSel {
        imm: u32,
    },

    // -- Exception generation / return -----------------------------------
    /// FS-mode SVC: take exception to EL1 with ESR syndrome.
    SvcExc {
        imm16: u32,
    },
    /// Exception return: restore PC from ELR, PSTATE from SPSR.
    Eret,

    // -- Hints (with side effects) ---------------------------------------
    /// Wait For Interrupt — halt until IRQ pending.
    Wfi,

    // -- Phase 5: cache/TLB/barriers ─────────────────────────────────────
    /// DC ZVA — zero a cache-line-sized block at the given VA.
    DcZva {
        addr: TcgTemp,
    },
    /// TLB Invalidate.  `op` encodes `(op1 << 8) | (crm << 4) | op2`.
    Tlbi {
        op: u32,
        addr: TcgTemp,
    },
    /// Address Translation — write PAR_EL1 with translation result.
    /// `op` encodes `(op1 << 4) | op2`.
    At {
        op: u32,
        addr: TcgTemp,
    },
    /// Memory barrier (DSB/DMB/ISB).  `kind`: 0=DSB, 1=DMB, 2=ISB.
    Barrier {
        kind: u8,
    },
    /// Clear exclusive monitor.
    Clrex,

    // -- Phase 6: exception generation ───────────────────────────────────
    /// HVC exception.
    HvcExc {
        imm16: u32,
    },
    /// SMC exception.
    SmcExc {
        imm16: u32,
    },
    /// BRK (software breakpoint).
    BrkExc {
        imm16: u32,
    },
    /// HLT (halt / debug).
    HltExc {
        imm16: u32,
    },

    // -- Phase 8: PSTATE flag manipulation ───────────────────────────────
    /// Invert the C flag in NZCV.
    Cfinv,

    // -- Sync with architectural state -------------------------------
    /// Read guest architectural register into a temp.
    ReadReg {
        dst: TcgTemp,
        reg_id: u16,
    },
    /// Write a temp back to a guest architectural register.
    WriteReg {
        reg_id: u16,
        src: TcgTemp,
    },
}
