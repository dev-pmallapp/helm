//! Direct-threaded interpreter dispatch table for AArch64.
//!
//! Replaces the top-level `match insn.opcode` in the AArch64 hot loop with an
//! O(1) function-pointer table lookup. Each entry dispatches directly to the
//! appropriate execute sub-function, skipping the multi-arm match.
//!
//! # Table Layout
//!
//! `EXEC_TABLE` is indexed by `insn.opcode as u16`. The Opcode enum is
//! `#[repr(u16)]` (304 variants, discriminants 0..=303).
//! Table size is 320 (next power-of-two past 304) for cache alignment.
//!
//! # Performance
//!
//! Replaces a branch-predictor-hostile 16-arm match with a single indirect
//! call. Expected improvement: ~5→6 MIPS on the interpreter fallback path.

use helm_arch::aarch64::{arch_state::Aarch64ArchState, insn::Instruction};
use helm_core::{HartException, MemInterface};

/// Interpreter execution function signature.
///
/// Returns `Ok(pc_written)`: true if PC was written by the instruction
/// (branch/exception), false if caller should advance PC by 4.
pub type ExecFn = fn(
    insn: &Instruction,
    state: &mut Aarch64ArchState,
    mem: &mut dyn MemInterface,
) -> Result<bool, HartException>;

/// Dispatch table: `EXEC_TABLE[opcode as u16]` → direct function.
///
/// All 304 implemented opcodes map to a group-dispatcher function.
/// Unknown/unimplemented opcodes map to `exec_unimpl`.
pub static EXEC_TABLE: [ExecFn; 320] = build_table();

/// Dispatch one instruction via the table.
///
/// This is a drop-in replacement for calling `aarch64_execute(&insn, state, mem)`.
#[inline(always)]
pub fn dispatch(
    insn: &Instruction,
    state: &mut Aarch64ArchState,
    mem: &mut dyn MemInterface,
) -> Result<bool, HartException> {
    let idx = insn.opcode as u16 as usize;
    // SAFETY: table has 320 entries; u16 max is 65535 but we only have 304
    // valid opcodes. The enum repr(u16) guarantees discriminants < 320.
    let f = EXEC_TABLE[idx.min(319)];
    f(insn, state, mem)
}

// ── Table builder ────────────────────────────────────────────────────────────

const fn build_table() -> [ExecFn; 320] {
    // Default all entries to exec_unimpl.
    let mut table = [exec_unimpl as ExecFn; 320];

    // Fill known opcodes using the existing grouped sub-dispatchers.
    // The indices must match Opcode discriminants exactly.
    use helm_arch::aarch64::insn::Opcode::*;

    // ── Data processing — immediate (indices 0–16) ───────────────────────────
    table[Adr as usize] = exec_dp;
    table[Adrp as usize] = exec_dp;
    table[AddImm as usize] = exec_dp;
    table[SubImm as usize] = exec_dp;
    table[AddsImm as usize] = exec_dp;
    table[SubsImm as usize] = exec_dp;
    table[AndImm as usize] = exec_dp;
    table[OrrImm as usize] = exec_dp;
    table[EorImm as usize] = exec_dp;
    table[AndsImm as usize] = exec_dp;
    table[Movn as usize] = exec_dp;
    table[Movz as usize] = exec_dp;
    table[Movk as usize] = exec_dp;
    table[Sbfm as usize] = exec_dp;
    table[Bfm as usize] = exec_dp;
    table[Ubfm as usize] = exec_dp;
    table[Extr as usize] = exec_dp;

    // ── Data processing — register ───────────────────────────────────────────
    table[AddReg as usize] = exec_dp;
    table[SubReg as usize] = exec_dp;
    table[AddsReg as usize] = exec_dp;
    table[SubsReg as usize] = exec_dp;
    table[AddExt as usize] = exec_dp;
    table[SubExt as usize] = exec_dp;
    table[AddsExt as usize] = exec_dp;
    table[SubsExt as usize] = exec_dp;
    table[AndReg as usize] = exec_dp;
    table[OrrReg as usize] = exec_dp;
    table[EorReg as usize] = exec_dp;
    table[AndsReg as usize] = exec_dp;
    table[BicReg as usize] = exec_dp;
    table[OrnReg as usize] = exec_dp;
    table[EonReg as usize] = exec_dp;
    table[BicsReg as usize] = exec_dp;
    table[Adc as usize] = exec_dp;
    table[Adcs as usize] = exec_dp;
    table[Sbc as usize] = exec_dp;
    table[Sbcs as usize] = exec_dp;
    table[Mul as usize] = exec_mul_div;
    table[Madd as usize] = exec_mul_div;
    table[Msub as usize] = exec_mul_div;
    table[Mneg as usize] = exec_mul_div;
    table[Smulh as usize] = exec_mul_div;
    table[Umulh as usize] = exec_mul_div;
    table[Udiv as usize] = exec_mul_div;
    table[Sdiv as usize] = exec_mul_div;
    table[Smaddl as usize] = exec_mul_div;
    table[Smsubl as usize] = exec_mul_div;
    table[Umaddl as usize] = exec_mul_div;
    table[Umsubl as usize] = exec_mul_div;
    table[Lsl as usize] = exec_dp;
    table[Lsr as usize] = exec_dp;
    table[Asr as usize] = exec_dp;
    table[Ror as usize] = exec_dp;
    table[Clz as usize] = exec_dp;
    table[Cls as usize] = exec_dp;
    table[Rev as usize] = exec_dp;
    table[Rev16 as usize] = exec_dp;
    table[Rev32 as usize] = exec_dp;
    table[Rbit as usize] = exec_dp;
    table[Csel as usize] = exec_dp;
    table[Csinc as usize] = exec_dp;
    table[Csinv as usize] = exec_dp;
    table[Csneg as usize] = exec_dp;
    table[Ccmp as usize] = exec_dp;
    table[Ccmn as usize] = exec_dp;

    // ── Load/store ───────────────────────────────────────────────────────────
    table[Ldr as usize] = exec_ldst;
    table[Ldrb as usize] = exec_ldst;
    table[Ldrh as usize] = exec_ldst;
    table[Ldrsb as usize] = exec_ldst;
    table[Ldrsh as usize] = exec_ldst;
    table[Ldrsw as usize] = exec_ldst;
    table[Ldur as usize] = exec_ldst;
    table[Ldurb as usize] = exec_ldst;
    table[Ldurh as usize] = exec_ldst;
    table[Ldursb as usize] = exec_ldst;
    table[Ldursh as usize] = exec_ldst;
    table[Ldursw as usize] = exec_ldst;
    table[Str as usize] = exec_ldst;
    table[Strb as usize] = exec_ldst;
    table[Strh as usize] = exec_ldst;
    table[Stur as usize] = exec_ldst;
    table[Sturb as usize] = exec_ldst;
    table[Sturh as usize] = exec_ldst;
    table[Ldp as usize] = exec_ldst;
    table[Stp as usize] = exec_ldst;
    table[Ldxr as usize] = exec_ldst;
    table[Ldaxr as usize] = exec_ldst;
    table[Stxr as usize] = exec_ldst;
    table[Stlxr as usize] = exec_ldst;
    table[Ldxp as usize] = exec_ldst;
    table[Ldaxp as usize] = exec_ldst;
    table[Stxp as usize] = exec_ldst;
    table[Stlxp as usize] = exec_ldst;
    table[Clrex as usize] = exec_ldst;
    table[LdrLit as usize] = exec_ldst;
    table[LdrswLit as usize] = exec_ldst;
    table[LdrSimd as usize] = exec_ldst;
    table[StrSimd as usize] = exec_ldst;
    table[LdurSimd as usize] = exec_ldst;
    table[SturSimd as usize] = exec_ldst;
    table[LdpSimd as usize] = exec_ldst;
    table[StpSimd as usize] = exec_ldst;
    table[Ldar as usize] = exec_ldst;
    table[Stlr as usize] = exec_ldst;
    table[Ldadd as usize] = exec_ldst;
    table[Ldclr as usize] = exec_ldst;
    table[Ldeor as usize] = exec_ldst;
    table[Ldset as usize] = exec_ldst;
    table[LdSmax as usize] = exec_ldst;
    table[LdSmin as usize] = exec_ldst;
    table[LdUmax as usize] = exec_ldst;
    table[LdUmin as usize] = exec_ldst;
    table[Swp as usize] = exec_ldst;
    table[Cas as usize] = exec_ldst;
    table[Casp as usize] = exec_ldst;
    table[Prfm as usize] = exec_ldst;
    table[DcZva as usize] = exec_ldst;
    table[Ldapr as usize] = exec_ldst;
    table[Ldaprh as usize] = exec_ldst;
    table[Ldaprb as usize] = exec_ldst;
    table[LdapurB as usize] = exec_ldst;
    table[LdapurH as usize] = exec_ldst;
    table[Ldapur as usize] = exec_ldst;
    table[StlurB as usize] = exec_ldst;
    table[StlurH as usize] = exec_ldst;
    table[Stlur as usize] = exec_ldst;

    // ── Branches ─────────────────────────────────────────────────────────────
    table[B as usize] = exec_branch;
    table[Bl as usize] = exec_branch;
    table[Br as usize] = exec_branch;
    table[Blr as usize] = exec_branch;
    table[Ret as usize] = exec_branch;
    table[BCond as usize] = exec_branch;
    table[Cbz as usize] = exec_branch;
    table[Cbnz as usize] = exec_branch;
    table[Tbz as usize] = exec_branch;
    table[Tbnz as usize] = exec_branch;
    table[Svc as usize] = exec_branch;
    table[Brk as usize] = exec_branch;
    table[Nop as usize] = exec_branch;
    table[Wfe as usize] = exec_branch;
    table[Sev as usize] = exec_branch;
    table[Sevl as usize] = exec_branch;
    table[Wfi as usize] = exec_branch;
    table[Dmb as usize] = exec_branch;
    table[Dsb as usize] = exec_branch;
    table[Isb as usize] = exec_branch;
    table[Esb as usize] = exec_branch;
    table[Sb as usize] = exec_branch;
    table[Eret as usize] = exec_branch;
    table[Hvc as usize] = exec_branch;
    table[Smc as usize] = exec_branch;
    table[Yield as usize] = exec_branch;
    table[MsrImm as usize] = exec_branch;
    table[Bti as usize] = exec_branch;

    // ── FP / SIMD ────────────────────────────────────────────────────────────
    table[Fadd as usize] = exec_fp;
    table[Fsub as usize] = exec_fp;
    table[Fmul as usize] = exec_fp;
    table[Fdiv as usize] = exec_fp;
    table[Fsqrt as usize] = exec_fp;
    table[Fabs as usize] = exec_fp;
    table[Fneg as usize] = exec_fp;
    table[Fmax as usize] = exec_fp;
    table[Fmin as usize] = exec_fp;
    table[Fmaxnm as usize] = exec_fp;
    table[Fminnm as usize] = exec_fp;
    table[Fmadd as usize] = exec_fp;
    table[Fmsub as usize] = exec_fp;
    table[Fnmadd as usize] = exec_fp;
    table[Fnmsub as usize] = exec_fp;
    table[Fcmp as usize] = exec_fp;
    table[Fcmpe as usize] = exec_fp;
    table[Fcvt as usize] = exec_fp;
    table[FcvtzsGpr as usize] = exec_fp;
    table[FcvtzuGpr as usize] = exec_fp;
    table[ScvtfGpr as usize] = exec_fp;
    table[UcvtfGpr as usize] = exec_fp;
    table[FcvtnsGpr as usize] = exec_fp;
    table[FcvtnuGpr as usize] = exec_fp;
    table[FcvtmsGpr as usize] = exec_fp;
    table[FcvtmuGpr as usize] = exec_fp;
    table[FcvtpsGpr as usize] = exec_fp;
    table[FcvtpuGpr as usize] = exec_fp;
    table[FcvtasGpr as usize] = exec_fp;
    table[FcvtauGpr as usize] = exec_fp;
    table[FcvtzsVec as usize] = exec_fp;
    table[FcvtzuVec as usize] = exec_fp;
    table[Fsel as usize] = exec_fp;
    table[FmovImm as usize] = exec_fp;
    table[FmovReg as usize] = exec_fp;
    table[FmovGpr as usize] = exec_fp;
    table[Crc32 as usize] = exec_fp;
    table[Crc32c as usize] = exec_fp;
    table[Fccmp as usize] = exec_fp;
    table[Fccmpe as usize] = exec_fp;
    table[Fnmul as usize] = exec_fp;
    table[Fjcvtzs as usize] = exec_fp;

    // ── System registers ─────────────────────────────────────────────────────
    table[Mrs as usize] = exec_sysreg;
    table[Msr as usize] = exec_sysreg;
    table[Sys as usize] = exec_sysreg;

    // ── SIMD ─────────────────────────────────────────────────────────────────
    table[SimdDup as usize] = exec_simd;
    table[SimdUmov as usize] = exec_simd;
    table[SimdSmov as usize] = exec_simd;
    table[SimdIns as usize] = exec_simd;
    table[SimdMovi as usize] = exec_simd;
    table[SimdAdd as usize] = exec_simd;
    table[SimdSub as usize] = exec_simd;
    table[SimdMul as usize] = exec_simd;
    table[SimdCmgt0 as usize] = exec_simd;
    table[SimdCmeq0 as usize] = exec_simd;
    table[SimdCmlt0 as usize] = exec_simd;
    table[SimdCmge0 as usize] = exec_simd;
    table[SimdCmle0 as usize] = exec_simd;
    table[SimdCmeq as usize] = exec_simd;
    table[SimdUmaxv as usize] = exec_simd;
    table[SimdUminv as usize] = exec_simd;
    table[SimdCmgt as usize] = exec_simd;
    table[SimdCmge as usize] = exec_simd;
    table[SimdCmhi as usize] = exec_simd;
    table[SimdCmhs as usize] = exec_simd;
    table[SimdAnd as usize] = exec_simd;
    table[SimdOrr as usize] = exec_simd;
    table[SimdEor as usize] = exec_simd;
    table[SimdBic as usize] = exec_simd;
    table[SimdNot as usize] = exec_simd;
    table[SimdAbs as usize] = exec_simd;
    table[SimdNeg as usize] = exec_simd;
    table[Sdot as usize] = exec_simd;
    table[Udot as usize] = exec_simd;
    table[Fcadd as usize] = exec_simd;
    table[Fcmla as usize] = exec_simd;
    table[Sha3 as usize] = exec_simd;
    table[Sha512 as usize] = exec_simd;
    table[Sm3 as usize] = exec_simd;
    table[Sm4 as usize] = exec_simd;
    table[ScalarAddp as usize] = exec_simd;
    table[SimdFmov as usize] = exec_simd;
    table[SimdFadd as usize] = exec_simd;
    table[SimdFsub as usize] = exec_simd;
    table[SimdFmul as usize] = exec_simd;
    table[SimdFdiv as usize] = exec_simd;
    table[SimdFabs as usize] = exec_simd;
    table[SimdFneg as usize] = exec_simd;
    table[SimdFsqrt as usize] = exec_simd;
    table[SimdFcmeq as usize] = exec_simd;
    table[SimdFcmgt as usize] = exec_simd;
    table[SimdFcmge as usize] = exec_simd;

    // FlagM / v8.4 extras
    table[Setf8 as usize] = exec_dp;
    table[Setf16 as usize] = exec_dp;
    table[Cfinv as usize] = exec_dp;
    table[Rmif as usize] = exec_dp;
    table[Xaflag as usize] = exec_dp;
    table[Axflag as usize] = exec_dp;

    // BTI, SHA3/SHA512/SM3/SM4 → already covered above (simd/branch)
    // Undefined → exec_unimpl (already default)

    table
}

// ── Group dispatcher stubs ───────────────────────────────────────────────────
//
// The sub-dispatcher functions in helm-arch use `&mut impl MemInterface`
// (generic). The dispatch table uses `&mut dyn MemInterface` for a uniform
// signature. A thin newtype wrapper bridges the two without overhead.

use helm_core::MemFault;

/// Thin newtype that lets `&mut dyn MemInterface` satisfy `impl MemInterface`.
struct DynMemBridge<'a>(&'a mut dyn MemInterface);

impl MemInterface for DynMemBridge<'_> {
    #[inline(always)]
    fn read(&mut self, addr: u64, size: usize, ty: helm_core::AccessType) -> Result<u64, MemFault> {
        self.0.read(addr, size, ty)
    }
    #[inline(always)]
    fn write(
        &mut self,
        addr: u64,
        size: usize,
        val: u64,
        ty: helm_core::AccessType,
    ) -> Result<(), MemFault> {
        self.0.write(addr, size, val, ty)
    }
}

use helm_arch::aarch64::execute;

fn exec_dp(
    insn: &Instruction,
    state: &mut Aarch64ArchState,
    mem: &mut dyn MemInterface,
) -> Result<bool, HartException> {
    execute::dp::exec_dp(insn, state, &mut DynMemBridge(mem))
}

fn exec_mul_div(
    insn: &Instruction,
    state: &mut Aarch64ArchState,
    mem: &mut dyn MemInterface,
) -> Result<bool, HartException> {
    execute::mul_div::exec_mul_div(insn, state, &mut DynMemBridge(mem))
}

fn exec_ldst(
    insn: &Instruction,
    state: &mut Aarch64ArchState,
    mem: &mut dyn MemInterface,
) -> Result<bool, HartException> {
    execute::ldst::exec_ldst(insn, state, &mut DynMemBridge(mem))
}

fn exec_branch(
    insn: &Instruction,
    state: &mut Aarch64ArchState,
    mem: &mut dyn MemInterface,
) -> Result<bool, HartException> {
    execute::branch::exec_branch(insn, state, &mut DynMemBridge(mem))
}

fn exec_fp(
    insn: &Instruction,
    state: &mut Aarch64ArchState,
    mem: &mut dyn MemInterface,
) -> Result<bool, HartException> {
    execute::fp::exec_fp(insn, state, &mut DynMemBridge(mem))
}

fn exec_sysreg(
    insn: &Instruction,
    state: &mut Aarch64ArchState,
    mem: &mut dyn MemInterface,
) -> Result<bool, HartException> {
    execute::sysreg::exec_sysreg(insn, state, &mut DynMemBridge(mem))
}

fn exec_simd(
    insn: &Instruction,
    state: &mut Aarch64ArchState,
    mem: &mut dyn MemInterface,
) -> Result<bool, HartException> {
    execute::simd::exec_simd(insn, state, &mut DynMemBridge(mem))
}

fn exec_unimpl(
    insn: &Instruction,
    _state: &mut Aarch64ArchState,
    _mem: &mut dyn MemInterface,
) -> Result<bool, HartException> {
    Err(HartException::IllegalInstruction {
        pc: insn.pc,
        raw: 0,
    })
}
