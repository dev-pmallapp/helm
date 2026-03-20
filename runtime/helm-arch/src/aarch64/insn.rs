//! AArch64 decoded instruction representation.
//!
//! Rather than a massive enum (AArch64 has ~1000+ instruction variants),
//! we use a compact struct capturing the decoded fields.  The opcode enum
//! covers the logical instruction kinds; operands are extracted separately.

/// Top-level instruction kind after decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    // ── Data processing — immediate ──────────────────────────────────────────
    Adr,
    Adrp,
    AddImm,
    SubImm,
    AddsImm,
    SubsImm,
    AndImm,
    OrrImm,
    EorImm,
    AndsImm,
    Movn,
    Movz,
    Movk,
    Sbfm,
    Bfm,
    Ubfm,
    Extr,

    // ── Data processing — register ───────────────────────────────────────────
    AddReg,
    SubReg,
    AddsReg,
    SubsReg,
    AddExt,
    SubExt,
    AddsExt,
    SubsExt, // extended register
    AndReg,
    OrrReg,
    EorReg,
    AndsReg,
    BicReg,
    OrnReg,
    EonReg,
    BicsReg,
    Adc,
    Adcs,
    Sbc,
    Sbcs,
    Mul,
    Madd,
    Msub,
    Mneg,
    Smulh,
    Umulh,
    Smaddl,
    Smsubl,
    Umaddl,
    Umsubl, // widening multiply-add
    Sdiv,
    Udiv,
    Lsl,
    Lsr,
    Asr,
    Ror,
    Cls,
    Clz,
    Rev,
    Rev16,
    Rev32,
    Rbit,
    Crc32,
    Crc32c,
    // Conditional select
    Csel,
    Csinc,
    Csinv,
    Csneg,
    // Conditional compare
    Ccmn,
    Ccmp,

    // ── Load/Store ───────────────────────────────────────────────────────────
    Ldr,
    Ldrb,
    Ldrh,
    Ldrsb,
    Ldrsh,
    Ldrsw,
    LdrLit,   // LDR Xt, =label (PC-relative literal)
    LdrswLit, // LDRSW Xt, label
    Str,
    Strb,
    Strh,
    Ldp,
    Stp,
    Ldur,
    Ldurb,
    Ldurh,
    Ldursb,
    Ldursh,
    Ldursw,
    Stur,
    Sturb,
    Sturh,
    Prfm, // prefetch → NOP
    // Exclusive / ordered
    Ldxr,
    Stxr,
    Ldaxr,
    Stlxr,
    Ldxp,
    Stxp,
    Ldaxp,
    Stlxp,
    Clrex,
    Ldar,
    Stlr, // load-acquire / store-release
    // LSE atomics — arithmetic (o3=0)
    Ldadd,
    Ldclr,
    Ldeor,
    Ldset,
    LdSmax, // atomic signed max
    LdSmin, // atomic signed min
    LdUmax, // atomic unsigned max
    LdUmin, // atomic unsigned min
    // LSE atomics — swap (o3=1)
    Swp,
    Cas,
    Casp,
    // SIMD load/store
    LdrSimd,
    StrSimd, // scalar FP/SIMD LDR/STR (B/H/S/D/Q)
    LdpSimd,
    StpSimd, // SIMD pair LDP/STP
    LdurSimd,
    SturSimd, // SIMD unscaled offset

    // ── Branches / system ────────────────────────────────────────────────────
    B,
    Bl,
    Br,
    Blr,
    Ret,
    BCond, // B.cond
    Cbz,
    Cbnz,
    Tbz,
    Tbnz,
    Svc,
    Hvc,
    Smc,
    Eret,
    Nop,
    Wfi,
    Wfe,
    Sev,
    Sevl,
    Yield,
    Dmb,
    Dsb,
    Isb,
    Esb,
    Sb,
    Brk, // software breakpoint
    Mrs,
    Msr,    // system register access
    MsrImm, // MSR to PSTATE (DAIFSet, DAIFClr, SPSel)
    Sys,    // general SYS instruction (TLBI, DC, etc.)
    DcZva,  // DC ZVA — data cache zero by VA

    // ── FP / SIMD ────────────────────────────────────────────────────────────
    FmovImm,
    FmovReg,
    FmovGpr,
    Fadd,
    Fsub,
    Fmul,
    Fdiv,
    Fsqrt,
    Fabs,
    Fneg,
    Fmax,
    Fmin,
    Fmaxnm,
    Fminnm,
    Fmadd,
    Fmsub,
    Fnmadd,
    Fnmsub,
    Fcmp,
    Fcmpe,
    Fcvt,
    FcvtzsGpr,
    FcvtzuGpr,
    ScvtfGpr,
    UcvtfGpr,
    FcvtnsGpr,
    FcvtnuGpr, // round to nearest
    FcvtmsGpr,
    FcvtmuGpr, // round toward -inf (floor)
    FcvtpsGpr,
    FcvtpuGpr, // round toward +inf (ceil)
    FcvtasGpr,
    FcvtauGpr, // round ties-away
    FcvtzsVec,
    FcvtzuVec,
    Fsel,
    Fccmp,
    Fccmpe,
    Fnmul,
    // SIMD data processing
    SimdDup,
    SimdIns,
    SimdUmov,
    SimdSmov,
    SimdMovi,
    SimdMvni,
    SimdFmov,
    SimdAdd,
    SimdSub,
    SimdMul,
    SimdAnd,
    SimdOrr,
    SimdEor,
    SimdBic,
    SimdBif,
    SimdBit,
    SimdBsl,
    SimdOrrImm,
    SimdNot,
    SimdNeg,
    SimdAbs,
    SimdCmgt0,
    SimdCmeq0,
    SimdCmlt0,
    SimdCmge0,
    SimdCmle0,
    SimdCmeq,
    SimdCmgt,
    SimdCmge,
    SimdCmhi,
    SimdCmhs,
    SimdCmtst,
    SimdAddp,
    SimdAddv,
    SimdUmaxv,
    SimdUminv,
    SimdSshl,
    SimdUshl,
    SimdSshr,
    SimdUshr,
    SimdShl,
    SimdTbl,
    SimdTbx,
    SimdZip1,
    SimdZip2,
    SimdUzp1,
    SimdUzp2,
    SimdTrn1,
    SimdTrn2,
    SimdExt,
    SimdRev64,
    SimdRev32,
    SimdRev16,
    SimdCnt,
    SimdClz,
    SimdSxtl,
    SimdUxtl,
    SimdSmin,
    SimdUmin,
    SimdSmax,
    SimdUmax,
    SimdFadd,
    SimdFsub,
    SimdFmul,
    SimdFdiv,
    SimdFabs,
    SimdFneg,
    SimdFsqrt,
    SimdFcmeq,
    SimdFcmgt,
    SimdFcmge,
    SimdFcvtzs,
    SimdFcvtzu,
    SimdScvtf,
    SimdUcvtf,
    SimdFrintm,
    SimdFrintn,
    SimdFrintp,
    SimdFrintz,
    // SIMD load/store
    SimdLd1,
    SimdSt1,
    SimdLd2,
    SimdSt2,
    SimdLd3,
    SimdSt3,
    SimdLd4,
    SimdSt4,
    SimdLd1r, // LD1R (replicate)
    // Catch-all for unimplemented SIMD
    SimdOther,

    // ── Scalar SIMD ──────────────────────────────────────────────────────────
    ScalarAddp, // ADDP Dd, Vn.2D — scalar pair-wise add (sums two D lanes)

    // ── v8.3 — LRCPC: load-acquire RCpc ─────────────────────────────────────
    Ldapr,   // LDAPR  (64/32-bit)
    Ldaprh,  // LDAPRH (16-bit)
    Ldaprb,  // LDAPRB (8-bit)

    // ── v8.4 — RCPC2: load/store unscaled with release/acquire semantics ─────
    LdapurB,  // LDAPUR byte
    LdapurH,  // LDAPUR halfword
    Ldapur,   // LDAPUR word/doubleword
    StlurB,   // STLUR  byte
    StlurH,   // STLUR  halfword
    Stlur,    // STLUR  word/doubleword

    // ── v8.3 — JSCVT ─────────────────────────────────────────────────────────
    Fjcvtzs,  // FJCVTZS: float-to-int per JS ToInt32 spec

    // ── v8.3 — FCMA: complex FP ──────────────────────────────────────────────
    Fcadd,    // FCADD: complex FP add with 90/270 degree rotation
    Fcmla,    // FCMLA: complex FP multiply-accumulate

    // ── v8.4 — DotProduct ────────────────────────────────────────────────────
    Sdot,     // SDOT: signed 8-bit dot product accumulate
    Udot,     // UDOT: unsigned 8-bit dot product accumulate

    // ── v8.4 — FlagM ─────────────────────────────────────────────────────────
    Setf8,    // SETF8:  set NZCV from byte value
    Setf16,   // SETF16: set NZCV from halfword value
    Cfinv,    // CFINV:  invert carry flag
    Rmif,     // RMIF:   rotate, mask into flags
    Xaflag,   // XAFLAG: FP/int flag transfer hint
    Axflag,   // AXFLAG: FP/int flag transfer hint

    // ── v8.5 — BTI (needed for Linux kernels compiled with BTI) ──────────────
    Bti,      // BTI: branch target identification (NOP in functional mode)

    // ── Crypto stubs (decode only — raise IllegalInstruction on execute) ──────
    Sha3,     // SHA3 family (EOR3, RAX1, XAR, BCAX)
    Sha512,   // SHA512H, SHA512H2, SHA512SU0, SHA512SU1
    Sm3,      // SM3 family
    Sm4,      // SM4 family

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
    pub raw: u32, // original encoding (for error messages)
    pub pc: u64,  // PC at decode time

    // Register fields (31 = XZR / WZR, or SP in address contexts)
    pub rd: u32,
    pub rn: u32,
    pub rm: u32,
    pub ra: u32, // for MADD/FMADD etc.

    // Immediates / offsets (sign-extended where applicable)
    pub imm: i64,
    pub imm2: u64, // second immediate (e.g. MOVK shift, bit positions)

    // Size / qualifier bits
    pub sf: bool,        // 64-bit operation?
    pub cond: u32,       // 4-bit condition code
    pub opc: u32,        // sub-opcode within group
    pub shift_type: u32, // 0=LSL,1=LSR,2=ASR,3=ROR
    pub shift_amt: u32,
    pub extend_type: u32, // UXTB=0..SXTX=7
    pub extend_amt: u32,

    // Load/store specifics
    pub size: u32, // 0=byte,1=half,2=word,3=dword
    pub pre_index: bool,
    pub post_index: bool,
    pub signed_load: bool,
    pub pair_second: u32, // Rt2 for LDP/STP
    pub acquire: bool,
    pub release: bool,

    // FP specifics
    pub ftype: u32, // 0=SP, 1=DP, 3=HP
    pub fp_rounding: u32,
    pub nzcv_imm: u32, // for CCMP/CCMN immediate
}

impl Instruction {
    pub fn undefined(raw: u32, pc: u64) -> Self {
        Self {
            opcode: Opcode::Undefined,
            raw,
            pc,
            ..Self::zeroed()
        }
    }

    pub fn zeroed() -> Self {
        Self {
            opcode: Opcode::Nop,
            raw: 0,
            pc: 0,
            rd: 0,
            rn: 0,
            rm: 0,
            ra: 0,
            imm: 0,
            imm2: 0,
            sf: true,
            cond: 0,
            opc: 0,
            shift_type: 0,
            shift_amt: 0,
            extend_type: 0,
            extend_amt: 0,
            size: 3,
            pre_index: false,
            post_index: false,
            signed_load: false,
            pair_second: 0,
            acquire: false,
            release: false,
            ftype: 0,
            fp_rounding: 0,
            nzcv_imm: 0,
        }
    }

    pub fn is_branch(&self) -> bool {
        matches!(
            self.opcode,
            Opcode::B
                | Opcode::Bl
                | Opcode::Br
                | Opcode::Blr
                | Opcode::Ret
                | Opcode::BCond
                | Opcode::Cbz
                | Opcode::Cbnz
                | Opcode::Tbz
                | Opcode::Tbnz
                | Opcode::Svc
                | Opcode::Hvc
                | Opcode::Smc
                | Opcode::Eret
        )
    }

    pub fn is_mem_access(&self) -> bool {
        matches!(
            self.opcode,
            Opcode::Ldr
                | Opcode::Ldrb
                | Opcode::Ldrh
                | Opcode::Ldrsb
                | Opcode::Ldrsh
                | Opcode::Ldrsw
                | Opcode::LdrLit
                | Opcode::LdrswLit
                | Opcode::Ldp
                | Opcode::Ldur
                | Opcode::Ldurb
                | Opcode::Ldurh
                | Opcode::Ldursb
                | Opcode::Ldursh
                | Opcode::Ldursw
                | Opcode::Str
                | Opcode::Strb
                | Opcode::Strh
                | Opcode::Stp
                | Opcode::Stur
                | Opcode::Sturb
                | Opcode::Sturh
                | Opcode::LdrSimd
                | Opcode::StrSimd
                | Opcode::LdpSimd
                | Opcode::StpSimd
                | Opcode::LdurSimd
                | Opcode::SturSimd
                | Opcode::Ldxr
                | Opcode::Stxr
                | Opcode::Ldaxr
                | Opcode::Stlxr
                | Opcode::Ldxp
                | Opcode::Stxp
                | Opcode::Ldaxp
                | Opcode::Stlxp
                | Opcode::Ldar
                | Opcode::Stlr
                | Opcode::Ldadd
                | Opcode::Ldclr
                | Opcode::Ldeor
                | Opcode::Ldset
                | Opcode::LdSmax
                | Opcode::LdSmin
                | Opcode::LdUmax
                | Opcode::LdUmin
                | Opcode::Swp
                | Opcode::Cas
        )
    }
}
