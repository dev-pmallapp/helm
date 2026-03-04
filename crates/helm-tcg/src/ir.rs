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
