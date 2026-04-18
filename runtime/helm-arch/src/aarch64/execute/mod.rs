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
use helm_probe::CpuProbes;

/// Execute one decoded AArch64 instruction.
///
/// The optional `probes` parameter allows the caller to supply a [`CpuProbes`]
/// bundle so that probe events (e.g. [`BranchEvent`]) are fired inline during
/// execution. When `None`, execution behaves identically but no probe events
/// are emitted — the engine may still fire them after the call returns.
pub fn execute(
    insn: &Instruction,
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
    probes: Option<&CpuProbes>,
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
        | PacHint | PacReg | PacRegZ | AutReg | AutRegZ | Xpac | RetAut | BrAut | BlrAut
        | BrAutZ | BlrAutZ | EretAut => branch::exec_branch(insn, a, mem, probes),
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
        SimdUmaxp | SimdSmaxp | SimdXtn => simd::exec_simd(insn, a, mem),
        // FlagM
        Setf8 | Setf16 | Cfinv | Rmif | Xaflag | Axflag => dp::exec_dp(insn, a, mem),
        Mrs | Msr | Sys => sysreg::exec_sysreg(insn, a, mem),
        _ => Err(HartException::IllegalInstruction {
            pc: a.pc,
            raw: insn.raw,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_core::{AccessType, MemFault};
    use helm_probe::CpuProbes;

    struct DummyMem;

    impl MemInterface for DummyMem {
        fn read(
            &mut self,
            _addr: u64,
            _size: usize,
            _ty: AccessType,
        ) -> Result<u64, MemFault> {
            Ok(0)
        }

        fn write(
            &mut self,
            _addr: u64,
            _size: usize,
            _val: u64,
            _ty: AccessType,
        ) -> Result<(), MemFault> {
            Ok(())
        }
    }

    fn wrong_group_insn(opcode: Opcode) -> Instruction {
        let mut insn = Instruction::zeroed();
        insn.opcode = opcode;
        insn.raw = 0xDEAD_BEEF;
        insn.pc = 0x1000;
        insn
    }

    macro_rules! assert_wrong_dispatch_faults {
        ($exec:path, $opcode:expr) => {{
            let mut state = Aarch64ArchState::new();
            let mut mem = DummyMem;
            let err = $exec(&wrong_group_insn($opcode), &mut state, &mut mem).unwrap_err();
            assert_eq!(
                err,
                HartException::IllegalInstruction {
                    pc: 0x1000,
                    raw: 0xDEAD_BEEF,
                }
            );
        }};
    }

    #[test]
    fn group_dispatch_mismatches_return_illegal_instruction() {
        assert_wrong_dispatch_faults!(dp::exec_dp, Opcode::Brk);
        assert_wrong_dispatch_faults!(mul_div::exec_mul_div, Opcode::Brk);
        assert_wrong_dispatch_faults!(ldst::exec_ldst, Opcode::Brk);
        // branch::exec_branch has an extra `probes` parameter
        {
            let mut state = Aarch64ArchState::new();
            let mut mem = DummyMem;
            let err = branch::exec_branch(
                &wrong_group_insn(Opcode::AddImm),
                &mut state,
                &mut mem,
                None,
            )
            .unwrap_err();
            assert_eq!(
                err,
                HartException::IllegalInstruction {
                    pc: 0x1000,
                    raw: 0xDEAD_BEEF,
                }
            );
        }
        assert_wrong_dispatch_faults!(fp::exec_fp, Opcode::Brk);
        assert_wrong_dispatch_faults!(simd::exec_simd, Opcode::Brk);
        assert_wrong_dispatch_faults!(sysreg::exec_sysreg, Opcode::Brk);
    }

    #[test]
    fn casp_returns_illegal_instruction_instead_of_succeeding() {
        let mut state = Aarch64ArchState::new();
        let mut mem = DummyMem;
        let err = ldst::exec_ldst(&wrong_group_insn(Opcode::Casp), &mut state, &mut mem)
            .unwrap_err();
        assert_eq!(
            err,
            HartException::IllegalInstruction {
                pc: 0x1000,
                raw: 0xDEAD_BEEF,
            }
        );
    }

    #[test]
    fn execute_with_none_probes_works() {
        // NOP: 0xD503_201F
        let mut state = Aarch64ArchState::new();
        state.pc = 0x1000;
        let mut mem = DummyMem;
        let insn = crate::aarch64::decode::decode(0xD503_201F, state.pc).expect("decode NOP");
        let pc_written = execute(&insn, &mut state, &mut mem, None).expect("NOP should succeed");
        assert!(!pc_written, "NOP does not write PC");
    }

    #[test]
    fn execute_with_default_probes_works() {
        // B #4 (unconditional branch forward by 4 bytes): 0x14000001
        let mut state = Aarch64ArchState::new();
        state.pc = 0x1000;
        let mut mem = DummyMem;
        let probes = CpuProbes::default();
        let insn = crate::aarch64::decode::decode(0x1400_0001, state.pc).expect("decode B");
        let pc_written =
            execute(&insn, &mut state, &mut mem, Some(&probes)).expect("B should succeed");
        assert!(pc_written, "B writes PC");
        assert_eq!(state.pc, 0x1004, "B #4 targets PC+4");
    }

    #[test]
    fn execute_branch_not_taken_with_probes() {
        // CBZ X0, #8 — when X0 != 0, branch is NOT taken.
        // Encoding: sf=1, 0b011010_0, imm19=2 (offset=8), Rt=0
        // 0xB400_0040 = CBZ X0, #+8
        let mut state = Aarch64ArchState::new();
        state.pc = 0x2000;
        state.write_x(0, 42); // X0 != 0 => not taken
        let mut mem = DummyMem;
        let probes = CpuProbes::default();
        let insn = crate::aarch64::decode::decode(0xB400_0040, state.pc).expect("decode CBZ");
        let pc_written =
            execute(&insn, &mut state, &mut mem, Some(&probes)).expect("CBZ should succeed");
        assert!(!pc_written, "CBZ not taken when X0 != 0");
    }

    /// Verify that branch probe fires inline during execute when
    /// the `instrumentation` feature is active.
    #[test]
    #[cfg(feature = "instrumentation")]
    fn branch_probe_fires_on_taken_branch() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;

        let mut probes = CpuProbes::default();
        let target_seen = Arc::new(AtomicU64::new(0));
        let target_clone = target_seen.clone();
        probes.branch.subscribe(move |ev| {
            assert!(ev.taken, "branch should be taken");
            target_clone.store(ev.target, Ordering::Relaxed);
        });

        // B #8 (unconditional branch, offset = +8 bytes)
        // Encoding: 0b000101 | imm26=2 => 0x14000002
        let mut state = Aarch64ArchState::new();
        state.pc = 0x1000;
        let mut mem = DummyMem;
        let insn = crate::aarch64::decode::decode(0x1400_0002, state.pc).expect("decode B");
        let pc_written =
            execute(&insn, &mut state, &mut mem, Some(&probes)).expect("B should succeed");
        assert!(pc_written);
        assert_eq!(state.pc, 0x1008);
        assert_eq!(
            target_seen.load(Ordering::Relaxed),
            0x1008,
            "branch probe should have captured target=0x1008"
        );
    }

    /// Verify that branch probe fires with taken=false for a not-taken
    /// conditional branch.
    #[test]
    #[cfg(feature = "instrumentation")]
    fn branch_probe_fires_on_not_taken_conditional() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let mut probes = CpuProbes::default();
        let probe_fired = Arc::new(AtomicBool::new(false));
        let taken_seen = Arc::new(AtomicBool::new(true)); // default true; expect false
        let fired_clone = probe_fired.clone();
        let taken_clone = taken_seen.clone();
        probes.branch.subscribe(move |ev| {
            fired_clone.store(true, Ordering::Relaxed);
            taken_clone.store(ev.taken, Ordering::Relaxed);
        });

        // CBZ X0, #8 with X0=1 => not taken
        let mut state = Aarch64ArchState::new();
        state.pc = 0x2000;
        state.write_x(0, 1);
        let mut mem = DummyMem;
        let insn = crate::aarch64::decode::decode(0xB400_0040, state.pc).expect("decode CBZ");
        let pc_written =
            execute(&insn, &mut state, &mut mem, Some(&probes)).expect("CBZ should succeed");
        assert!(!pc_written);
        assert!(
            probe_fired.load(Ordering::Relaxed),
            "branch probe should have fired"
        );
        assert!(
            !taken_seen.load(Ordering::Relaxed),
            "branch should report taken=false"
        );
    }
}
