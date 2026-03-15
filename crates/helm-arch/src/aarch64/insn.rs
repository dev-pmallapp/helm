//! AArch64 decoded instruction representation.
//!
//! Rather than a massive enum (AArch64 has ~1000+ instruction variants),
//! we use a compact struct capturing the decoded fields.  The opcode enum
//! covers the logical instruction kinds; operands are extracted separately.

/// Top-level instruction kind after decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    // ── Data processing — immediate ──────────────────────────────────────────
    Adr, Adrp,
    AddImm, SubImm, AddsImm, SubsImm,
    AndImm, OrrImm, EorImm, AndsImm,
    Movn, Movz, Movk,
    Sbfm, Bfm, Ubfm,
    Extr,

    // ── Data processing — register ───────────────────────────────────────────
    AddReg, SubReg, AddsReg, SubsReg,
    AndReg, OrrReg, EorReg, AndsReg, BicReg, OrnReg, EonReg, BicsReg,
    Adc, Adcs, Sbc, Sbcs,
    Mul, Madd, Msub, Mneg,
    Smulh, Umulh,
    Sdiv, Udiv,
    Lsl, Lsr, Asr, Ror,
    Cls, Clz, Rev, Rev16, Rev32,
    Rbit,
    // Conditional select
    Csel, Csinc, Csinv, Csneg,
    // Conditional compare
    Ccmn, Ccmp,

    // ── Load/Store ───────────────────────────────────────────────────────────
    Ldr, Ldrb, Ldrh, Ldrsb, Ldrsh, Ldrsw,
    Str, Strb, Strh,
    Ldp, Stp,
    Ldur, Ldurb, Ldurh, Ldursb, Ldursh, Ldursw,
    Stur, Sturb, Sturh,
    // Atomics (Phase 1)
    Ldxr, Stxr, Ldaxr, Stlxr, Clrex,

    // ── Branches / system ────────────────────────────────────────────────────
    B, Bl, Br, Blr, Ret,
    BCond,       // B.cond
    Cbz, Cbnz,
    Tbz, Tbnz,
    Svc,
    Hvc, Smc,
    Eret,
    Nop, Wfi, Wfe, Sev, Sevl,
    Dmb, Dsb, Isb,
    Brk,         // software breakpoint
    Mrs, Msr,    // system register access
    Sys,         // general SYS instruction (TLBI, DC, etc.)

    // ── FP / SIMD ────────────────────────────────────────────────────────────
    FmovImm, FmovReg, FmovGpr,
    Fadd, Fsub, Fmul, Fdiv, Fsqrt, Fabs, Fneg,
    Fmax, Fmin, Fmaxnm, Fminnm,
    Fmadd, Fmsub, Fnmadd, Fnmsub,
    Fcmp, Fcmpe,
    Fcvt,
    FcvtzsGpr, FcvtzuGpr, ScvtfGpr, UcvtfGpr,
    FcvtzsVec, FcvtzuVec,
    Fsel,
    // SIMD integer
    SimdAdd, SimdSub, SimdMul,
    SimdAnd, SimdOrr, SimdEor, SimdBic,
    SimdLd1, SimdSt1,
    // Catch-all for unimplemented SIMD
    SimdOther,

    /// Instruction not recognised (will raise `IllegalInstruction`).
    Undefined,
}

/// A fully decoded AArch64 instruction.
///
/// Fields are named after the AArch64 ARM manual operand names.
/// The `sf` bit selects 64-bit (`sf=1`) vs 32-bit (`sf=0`) operation.
#[derive(Debug, Clone, Copy)]
pub struct Instruction {
    pub opcode: Opcode,
    pub raw:    u32,    // original encoding (for error messages)
    pub pc:     u64,    // PC at decode time

    // Register fields (31 = XZR / WZR, or SP in address contexts)
    pub rd:  u32,
    pub rn:  u32,
    pub rm:  u32,
    pub ra:  u32,   // for MADD/FMADD etc.

    // Immediates / offsets (sign-extended where applicable)
    pub imm:    i64,
    pub imm2:   u64,    // second immediate (e.g. MOVK shift, bit positions)

    // Size / qualifier bits
    pub sf:   bool,   // 64-bit operation?
    pub cond: u32,    // 4-bit condition code
    pub opc:  u32,    // sub-opcode within group
    pub shift_type: u32,   // 0=LSL,1=LSR,2=ASR,3=ROR
    pub shift_amt:  u32,
    pub extend_type: u32,  // UXTB=0..SXTX=7
    pub extend_amt:  u32,

    // Load/store specifics
    pub size: u32,          // 0=byte,1=half,2=word,3=dword
    pub pre_index: bool,
    pub post_index: bool,
    pub signed_load: bool,
    pub pair_second: u32,   // Rt2 for LDP/STP
    pub acquire: bool,
    pub release: bool,

    // FP specifics
    pub ftype: u32,         // 0=SP, 1=DP, 3=HP
    pub fp_rounding: u32,
    pub nzcv_imm: u32,      // for CCMP/CCMN immediate
}

impl Instruction {
    pub fn undefined(raw: u32, pc: u64) -> Self {
        Self { opcode: Opcode::Undefined, raw, pc, ..Self::zeroed() }
    }

    pub fn zeroed() -> Self {
        Self {
            opcode: Opcode::Nop, raw: 0, pc: 0,
            rd: 0, rn: 0, rm: 0, ra: 0,
            imm: 0, imm2: 0,
            sf: true, cond: 0, opc: 0,
            shift_type: 0, shift_amt: 0,
            extend_type: 0, extend_amt: 0,
            size: 3, pre_index: false, post_index: false,
            signed_load: false, pair_second: 0,
            acquire: false, release: false,
            ftype: 0, fp_rounding: 0, nzcv_imm: 0,
        }
    }

    pub fn is_branch(&self) -> bool {
        matches!(
            self.opcode,
            Opcode::B | Opcode::Bl | Opcode::Br | Opcode::Blr | Opcode::Ret
            | Opcode::BCond | Opcode::Cbz | Opcode::Cbnz | Opcode::Tbz | Opcode::Tbnz
            | Opcode::Svc | Opcode::Eret
        )
    }

    pub fn is_mem_access(&self) -> bool {
        matches!(
            self.opcode,
            Opcode::Ldr | Opcode::Ldrb | Opcode::Ldrh | Opcode::Ldrsb
            | Opcode::Ldrsh | Opcode::Ldrsw | Opcode::Ldp
            | Opcode::Ldur | Opcode::Ldurb | Opcode::Ldurh
            | Opcode::Ldursb | Opcode::Ldursh | Opcode::Ldursw
            | Opcode::Str | Opcode::Strb | Opcode::Strh | Opcode::Stp
            | Opcode::Stur | Opcode::Sturb | Opcode::Sturh
        )
    }
}
