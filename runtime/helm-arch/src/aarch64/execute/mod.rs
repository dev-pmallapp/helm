//! AArch64 instruction execution.
//!
//! Entry point: [`execute`] — takes a decoded [`Instruction`] and mutable references
//! to [`Aarch64ArchState`] and a [`MemInterface`].
//! Returns `Ok(bool)` where `bool` indicates whether PC was written.
//! If `false`, the caller should advance PC by 4.

pub mod branch;
pub mod dp;
pub mod fp;
pub mod helpers;
pub mod ldst;
pub mod mul_div;
pub mod simd;
pub mod sysreg;

use crate::aarch64::arch_state::Aarch64ArchState;
use crate::aarch64::insn::{Instruction, Opcode};
use helm_core::{HartException, MemInterface};

/// Execute one decoded AArch64 instruction.
pub fn execute(
    insn: &Instruction,
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
) -> Result<bool, HartException> {
    use Opcode::*;
    match insn.opcode {
        Adr | Adrp | AddImm | SubImm | AddsImm | SubsImm | AndImm | OrrImm | EorImm | AndsImm
        | Movz | Movn | Movk | Sbfm | Ubfm | Bfm | Extr | AddReg | SubReg | AddsReg | SubsReg
        | AndReg | BicReg | OrrReg | OrnReg | EorReg | EonReg | AndsReg | BicsReg | Lsl | Lsr
        | Asr | Ror | Clz | Cls | Rev | Rev16 | Rev32 | Rbit | Adc | Adcs | Sbc | Sbcs | Csel
        | Csinc | Csinv | Csneg | Ccmp | Ccmn | AddExt | SubExt | AddsExt | SubsExt => {
            dp::exec_dp(insn, a, mem)
        }
        Mul | Madd | Msub | Mneg | Smulh | Umulh | Udiv | Sdiv | Smaddl | Smsubl | Umaddl
        | Umsubl => mul_div::exec_mul_div(insn, a, mem),
        Ldr | Ldrb | Ldrh | Ldrsb | Ldrsh | Ldrsw | Ldur | Ldurb | Ldurh | Ldursb | Ldursh
        | Ldursw | Str | Strb | Strh | Stur | Sturb | Sturh | Ldp | Stp | Ldxr | Ldaxr | Stxr
        | Stlxr | Ldxp | Ldaxp | Stxp | Stlxp | Clrex | LdrLit | LdrswLit | LdrSimd | StrSimd
        | LdurSimd | SturSimd | LdpSimd | StpSimd | Ldar | Stlr | Ldadd | Ldclr | Ldeor | Ldset
        | LdSmax | LdSmin | LdUmax | LdUmin | Swp | Cas | Casp | Prfm | DcZva | Ldapr | Ldaprh
        | Ldaprb | LdapurB | LdapurH | Ldapur | StlurB | StlurH | Stlur => {
            ldst::exec_ldst(insn, a, mem)
        }
        B | Bl | Br | Blr | Ret | BCond | Cbz | Cbnz | Tbz | Tbnz | Svc | Brk | Nop | Wfe | Sev
        | Sevl | Wfi | Dmb | Dsb | Isb | Esb | Sb | Eret | Hvc | Smc | Yield | MsrImm | Bti
        | PacHint | PacReg | PacRegZ | AutReg | AutRegZ | Xpac
        | RetAut | BrAut | BlrAut | BrAutZ | BlrAutZ | EretAut => {
            branch::exec_branch(insn, a, mem)
        }
        Fadd | Fsub | Fmul | Fdiv | Fsqrt | Fabs | Fneg | Fmax | Fmin | Fmaxnm | Fminnm | Fmadd
        | Fmsub | Fnmadd | Fnmsub | Fcmp | Fcmpe | Fcvt | FcvtzsGpr | FcvtzuGpr | ScvtfGpr
        | UcvtfGpr | FcvtnsGpr | FcvtnuGpr | FcvtmsGpr | FcvtmuGpr | FcvtpsGpr | FcvtpuGpr
        | FcvtasGpr | FcvtauGpr | FcvtzsVec | FcvtzuVec | Fsel | FmovImm | FmovReg | FmovGpr
        | Crc32 | Crc32c | Fccmp | Fccmpe | Fnmul | Fjcvtzs => fp::exec_fp(insn, a, mem),
        SimdDup | SimdUmov | SimdSmov | SimdIns | SimdMovi | SimdAdd | SimdSub | SimdMul
        | SimdCmgt0 | SimdCmeq0 | SimdCmlt0 | SimdCmge0 | SimdCmle0 | SimdCmeq | SimdUmaxv
        | SimdUminv | SimdCmgt | SimdCmge | SimdCmhi | SimdCmhs | SimdAnd | SimdOrr | SimdEor
        | SimdBic | SimdNot | SimdAbs | SimdNeg | Sdot | Udot | Fcadd | Fcmla | Sha3 | Sha512
        | Sm3 | Sm4 | ScalarAddp | SimdFmov | SimdFadd | SimdFsub | SimdFmul | SimdFdiv
        | SimdFabs | SimdFneg | SimdFsqrt | SimdFcmeq | SimdFcmgt | SimdFcmge | SimdFcvtzs
        | SimdFcvtzu | SimdScvtf | SimdUcvtf | SimdFrintm | SimdFrintn | SimdFrintp
        | SimdFrintz | SimdLd1 | SimdSt1 | SimdLd2 | SimdSt2 | SimdLd3 | SimdSt3 | SimdLd4
        | SimdSt4 | SimdLd1r | SimdOther | SimdMvni | SimdOrrImm | SimdBif | SimdBit | SimdBsl
        | SimdCmtst | SimdSshl | SimdUshl | SimdSshr | SimdUshr | SimdShl | SimdTbl | SimdTbx
        | SimdZip1 | SimdZip2 | SimdUzp1 | SimdUzp2 | SimdTrn1 | SimdTrn2 | SimdExt | SimdRev64
        | SimdRev32 | SimdRev16 | SimdSxtl | SimdUxtl | SimdCnt | SimdClz | SimdSmin | SimdUmin
        | SimdSmax | SimdUmax | SimdAddp | SimdAddv => simd::exec_simd(insn, a, mem),
        // FlagM
        Setf8 | Setf16 | Cfinv | Rmif | Xaflag | Axflag => dp::exec_dp(insn, a, mem),
        Mrs | Msr | Sys => sysreg::exec_sysreg(insn, a, mem),
        _ => Err(HartException::IllegalInstruction {
            pc: a.pc,
            raw: insn.raw,
        }),
    }
}
