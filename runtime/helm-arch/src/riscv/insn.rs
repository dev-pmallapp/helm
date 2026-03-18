//! RISC-V RV64GC instruction set — all variant definitions.
//!
//! Fields use the RISC-V ISA naming convention:
//!   `rd`, `rs1`, `rs2`, `rs3` — register indices (5 bits → u8)
//!   `imm`                      — sign-extended immediate (various widths, stored as i64)
//!   `csr`                      — 12-bit CSR address (stored as u16)
//!   `shamt`                    — shift amount (6 bits for RV64)
//!   `aq`, `rl`                 — memory ordering bits for atomics
//!
//! C (compressed) instructions are expanded to their 32-bit equivalents before
//! reaching this enum; they do not appear as distinct variants here.

#![allow(non_camel_case_types, clippy::upper_case_acronyms)]

/// A decoded RISC-V instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instruction {
    // ── RV64I — Base Integer ─────────────────────────────────────────────────
    LUI   { rd: u8, imm: i64 },
    AUIPC { rd: u8, imm: i64 },

    JAL  { rd: u8, imm: i64 },
    JALR { rd: u8, rs1: u8, imm: i64 },

    BEQ  { rs1: u8, rs2: u8, imm: i64 },
    BNE  { rs1: u8, rs2: u8, imm: i64 },
    BLT  { rs1: u8, rs2: u8, imm: i64 },
    BGE  { rs1: u8, rs2: u8, imm: i64 },
    BLTU { rs1: u8, rs2: u8, imm: i64 },
    BGEU { rs1: u8, rs2: u8, imm: i64 },

    LB  { rd: u8, rs1: u8, imm: i64 },
    LH  { rd: u8, rs1: u8, imm: i64 },
    LW  { rd: u8, rs1: u8, imm: i64 },
    LD  { rd: u8, rs1: u8, imm: i64 },
    LBU { rd: u8, rs1: u8, imm: i64 },
    LHU { rd: u8, rs1: u8, imm: i64 },
    LWU { rd: u8, rs1: u8, imm: i64 },

    SB { rs1: u8, rs2: u8, imm: i64 },
    SH { rs1: u8, rs2: u8, imm: i64 },
    SW { rs1: u8, rs2: u8, imm: i64 },
    SD { rs1: u8, rs2: u8, imm: i64 },

    ADDI  { rd: u8, rs1: u8, imm: i64 },
    SLTI  { rd: u8, rs1: u8, imm: i64 },
    SLTIU { rd: u8, rs1: u8, imm: i64 },
    XORI  { rd: u8, rs1: u8, imm: i64 },
    ORI   { rd: u8, rs1: u8, imm: i64 },
    ANDI  { rd: u8, rs1: u8, imm: i64 },
    SLLI  { rd: u8, rs1: u8, shamt: u8 },
    SRLI  { rd: u8, rs1: u8, shamt: u8 },
    SRAI  { rd: u8, rs1: u8, shamt: u8 },

    ADD  { rd: u8, rs1: u8, rs2: u8 },
    SUB  { rd: u8, rs1: u8, rs2: u8 },
    SLL  { rd: u8, rs1: u8, rs2: u8 },
    SLT  { rd: u8, rs1: u8, rs2: u8 },
    SLTU { rd: u8, rs1: u8, rs2: u8 },
    XOR  { rd: u8, rs1: u8, rs2: u8 },
    SRL  { rd: u8, rs1: u8, rs2: u8 },
    SRA  { rd: u8, rs1: u8, rs2: u8 },
    OR   { rd: u8, rs1: u8, rs2: u8 },
    AND  { rd: u8, rs1: u8, rs2: u8 },

    // 32-bit word ops (RV64I only)
    ADDIW { rd: u8, rs1: u8, imm: i64 },
    SLLIW { rd: u8, rs1: u8, shamt: u8 },
    SRLIW { rd: u8, rs1: u8, shamt: u8 },
    SRAIW { rd: u8, rs1: u8, shamt: u8 },
    ADDW  { rd: u8, rs1: u8, rs2: u8 },
    SUBW  { rd: u8, rs1: u8, rs2: u8 },
    SLLW  { rd: u8, rs1: u8, rs2: u8 },
    SRLW  { rd: u8, rs1: u8, rs2: u8 },
    SRAW  { rd: u8, rs1: u8, rs2: u8 },

    FENCE   { pred: u8, succ: u8 },
    FENCE_I,
    ECALL,
    EBREAK,

    // ── Zicsr ────────────────────────────────────────────────────────────────
    CSRRW  { rd: u8, rs1: u8, csr: u16 },
    CSRRS  { rd: u8, rs1: u8, csr: u16 },
    CSRRC  { rd: u8, rs1: u8, csr: u16 },
    CSRRWI { rd: u8, uimm: u8, csr: u16 },
    CSRRSI { rd: u8, uimm: u8, csr: u16 },
    CSRRCI { rd: u8, uimm: u8, csr: u16 },

    // ── RV64M — Integer Multiply/Divide ──────────────────────────────────────
    MUL    { rd: u8, rs1: u8, rs2: u8 },
    MULH   { rd: u8, rs1: u8, rs2: u8 },
    MULHSU { rd: u8, rs1: u8, rs2: u8 },
    MULHU  { rd: u8, rs1: u8, rs2: u8 },
    DIV    { rd: u8, rs1: u8, rs2: u8 },
    DIVU   { rd: u8, rs1: u8, rs2: u8 },
    REM    { rd: u8, rs1: u8, rs2: u8 },
    REMU   { rd: u8, rs1: u8, rs2: u8 },
    MULW   { rd: u8, rs1: u8, rs2: u8 },
    DIVW   { rd: u8, rs1: u8, rs2: u8 },
    DIVUW  { rd: u8, rs1: u8, rs2: u8 },
    REMW   { rd: u8, rs1: u8, rs2: u8 },
    REMUW  { rd: u8, rs1: u8, rs2: u8 },

    // ── RV64A — Atomic ───────────────────────────────────────────────────────
    LR_W      { rd: u8, rs1: u8, aq: bool, rl: bool },
    SC_W      { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOSWAP_W { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOADD_W  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOXOR_W  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOAND_W  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOOR_W   { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOMIN_W  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOMAX_W  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOMINU_W { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOMAXU_W { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    LR_D      { rd: u8, rs1: u8, aq: bool, rl: bool },
    SC_D      { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOSWAP_D { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOADD_D  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOXOR_D  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOAND_D  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOOR_D   { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOMIN_D  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOMAX_D  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOMINU_D { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AMOMAXU_D { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },

    // ── RV64F — Single-Precision Float ───────────────────────────────────────
    FLW  { rd: u8, rs1: u8, imm: i64 },
    FSW  { rs1: u8, rs2: u8, imm: i64 },
    FMADD_S  { rd: u8, rs1: u8, rs2: u8, rs3: u8, rm: u8 },
    FMSUB_S  { rd: u8, rs1: u8, rs2: u8, rs3: u8, rm: u8 },
    FNMSUB_S { rd: u8, rs1: u8, rs2: u8, rs3: u8, rm: u8 },
    FNMADD_S { rd: u8, rs1: u8, rs2: u8, rs3: u8, rm: u8 },
    FADD_S   { rd: u8, rs1: u8, rs2: u8, rm: u8 },
    FSUB_S   { rd: u8, rs1: u8, rs2: u8, rm: u8 },
    FMUL_S   { rd: u8, rs1: u8, rs2: u8, rm: u8 },
    FDIV_S   { rd: u8, rs1: u8, rs2: u8, rm: u8 },
    FSQRT_S  { rd: u8, rs1: u8, rm: u8 },
    FSGNJ_S  { rd: u8, rs1: u8, rs2: u8 },
    FSGNJN_S { rd: u8, rs1: u8, rs2: u8 },
    FSGNJX_S { rd: u8, rs1: u8, rs2: u8 },
    FMIN_S   { rd: u8, rs1: u8, rs2: u8 },
    FMAX_S   { rd: u8, rs1: u8, rs2: u8 },
    FCVT_W_S  { rd: u8, rs1: u8, rm: u8 },
    FCVT_WU_S { rd: u8, rs1: u8, rm: u8 },
    FCVT_L_S  { rd: u8, rs1: u8, rm: u8 },
    FCVT_LU_S { rd: u8, rs1: u8, rm: u8 },
    FMV_X_W  { rd: u8, rs1: u8 },
    FEQ_S    { rd: u8, rs1: u8, rs2: u8 },
    FLT_S    { rd: u8, rs1: u8, rs2: u8 },
    FLE_S    { rd: u8, rs1: u8, rs2: u8 },
    FCLASS_S { rd: u8, rs1: u8 },
    FCVT_S_W  { rd: u8, rs1: u8, rm: u8 },
    FCVT_S_WU { rd: u8, rs1: u8, rm: u8 },
    FCVT_S_L  { rd: u8, rs1: u8, rm: u8 },
    FCVT_S_LU { rd: u8, rs1: u8, rm: u8 },
    FMV_W_X  { rd: u8, rs1: u8 },

    // ── RV64D — Double-Precision Float ───────────────────────────────────────
    FLD  { rd: u8, rs1: u8, imm: i64 },
    FSD  { rs1: u8, rs2: u8, imm: i64 },
    FMADD_D  { rd: u8, rs1: u8, rs2: u8, rs3: u8, rm: u8 },
    FMSUB_D  { rd: u8, rs1: u8, rs2: u8, rs3: u8, rm: u8 },
    FNMSUB_D { rd: u8, rs1: u8, rs2: u8, rs3: u8, rm: u8 },
    FNMADD_D { rd: u8, rs1: u8, rs2: u8, rs3: u8, rm: u8 },
    FADD_D   { rd: u8, rs1: u8, rs2: u8, rm: u8 },
    FSUB_D   { rd: u8, rs1: u8, rs2: u8, rm: u8 },
    FMUL_D   { rd: u8, rs1: u8, rs2: u8, rm: u8 },
    FDIV_D   { rd: u8, rs1: u8, rs2: u8, rm: u8 },
    FSQRT_D  { rd: u8, rs1: u8, rm: u8 },
    FSGNJ_D  { rd: u8, rs1: u8, rs2: u8 },
    FSGNJN_D { rd: u8, rs1: u8, rs2: u8 },
    FSGNJX_D { rd: u8, rs1: u8, rs2: u8 },
    FMIN_D   { rd: u8, rs1: u8, rs2: u8 },
    FMAX_D   { rd: u8, rs1: u8, rs2: u8 },
    FCVT_S_D  { rd: u8, rs1: u8, rm: u8 },
    FCVT_D_S  { rd: u8, rs1: u8, rm: u8 },
    FEQ_D    { rd: u8, rs1: u8, rs2: u8 },
    FLT_D    { rd: u8, rs1: u8, rs2: u8 },
    FLE_D    { rd: u8, rs1: u8, rs2: u8 },
    FCLASS_D  { rd: u8, rs1: u8 },
    FCVT_W_D  { rd: u8, rs1: u8, rm: u8 },
    FCVT_WU_D { rd: u8, rs1: u8, rm: u8 },
    FCVT_L_D  { rd: u8, rs1: u8, rm: u8 },
    FCVT_LU_D { rd: u8, rs1: u8, rm: u8 },
    FMV_X_D   { rd: u8, rs1: u8 },
    FCVT_D_W  { rd: u8, rs1: u8, rm: u8 },
    FCVT_D_WU { rd: u8, rs1: u8, rm: u8 },
    FCVT_D_L  { rd: u8, rs1: u8, rm: u8 },
    FCVT_D_LU { rd: u8, rs1: u8, rm: u8 },
    FMV_D_X   { rd: u8, rs1: u8 },

    // ── Privileged ───────────────────────────────────────────────────────────
    MRET,
    SRET,
    WFI,
    SFENCE_VMA { rs1: u8, rs2: u8 },
}

impl Instruction {
    /// Returns `true` if this is a branch or jump (changes PC non-linearly).
    pub fn is_control_flow(&self) -> bool {
        matches!(
            self,
            Self::JAL { .. }
                | Self::JALR { .. }
                | Self::BEQ { .. }
                | Self::BNE { .. }
                | Self::BLT { .. }
                | Self::BGE { .. }
                | Self::BLTU { .. }
                | Self::BGEU { .. }
                | Self::ECALL
                | Self::EBREAK
                | Self::MRET
                | Self::SRET
        )
    }

    /// Returns `true` if this instruction accesses memory (load or store).
    pub fn is_mem_access(&self) -> bool {
        matches!(
            self,
            Self::LB { .. }
                | Self::LH { .. }
                | Self::LW { .. }
                | Self::LD { .. }
                | Self::LBU { .. }
                | Self::LHU { .. }
                | Self::LWU { .. }
                | Self::SB { .. }
                | Self::SH { .. }
                | Self::SW { .. }
                | Self::SD { .. }
                | Self::FLW { .. }
                | Self::FSW { .. }
                | Self::FLD { .. }
                | Self::FSD { .. }
                | Self::LR_W { .. }
                | Self::SC_W { .. }
                | Self::LR_D { .. }
                | Self::SC_D { .. }
        )
    }
}
