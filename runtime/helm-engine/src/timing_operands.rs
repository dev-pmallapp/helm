use helm_arch::{aarch64::insn::Instruction as Aarch64Insn, riscv::Instruction as RiscvInsn};
use helm_timing::{
    TIMING_AARCH64_SP_REG, TIMING_FP_REG_BASE, TIMING_MAX_DST_REGS, TIMING_MAX_SRC_REGS,
    TIMING_VEC_REG_BASE,
};

fn push_timing_reg(dst: &mut [u8; TIMING_MAX_SRC_REGS], len: &mut u8, reg: Option<u8>) {
    if let Some(reg) = reg {
        if usize::from(*len) < TIMING_MAX_SRC_REGS {
            dst[*len as usize] = reg;
            *len += 1;
        }
    }
}

fn push_timing_dst_reg(dst: &mut [u8; TIMING_MAX_DST_REGS], len: &mut u8, reg: Option<u8>) {
    if let Some(reg) = reg {
        if usize::from(*len) < TIMING_MAX_DST_REGS {
            dst[*len as usize] = reg;
            *len += 1;
        }
    }
}

fn riscv_int_timing_reg(reg: u8) -> Option<u8> {
    (reg != 0).then_some(reg)
}

fn riscv_fp_timing_reg(reg: u8) -> u8 {
    TIMING_FP_REG_BASE + reg
}

fn aarch64_sp_timing_reg(reg: u32) -> Option<u8> {
    (reg == 31).then_some(TIMING_AARCH64_SP_REG)
}

fn aarch64_int_timing_reg(reg: u32) -> Option<u8> {
    (reg < 31).then_some(reg as u8)
}

fn aarch64_vec_timing_reg(reg: u32) -> Option<u8> {
    (reg < 32).then_some(TIMING_VEC_REG_BASE + reg as u8)
}

fn push_aarch64_src_gp(
    dst: &mut [u8; TIMING_MAX_SRC_REGS],
    len: &mut u8,
    reg: u32,
    allow_sp: bool,
) {
    if allow_sp && reg == 31 {
        push_timing_reg(dst, len, aarch64_sp_timing_reg(reg));
    } else {
        push_timing_reg(dst, len, aarch64_int_timing_reg(reg));
    }
}

fn aarch64_dst_gp(reg: u32, allow_sp: bool) -> Option<u8> {
    if allow_sp && reg == 31 {
        aarch64_sp_timing_reg(reg)
    } else {
        aarch64_int_timing_reg(reg)
    }
}

fn aarch64_mem_uses_reg_offset(insn: &Aarch64Insn) -> bool {
    insn.extend_type != 0 || (insn.rm != 0 && !insn.post_index)
}

pub(crate) fn aarch64_timing_src_regs(insn: &Aarch64Insn) -> ([u8; TIMING_MAX_SRC_REGS], u8) {
    use helm_arch::aarch64::insn::Opcode as O;

    let mut regs = [0; TIMING_MAX_SRC_REGS];
    let mut len = 0;

    match insn.opcode {
        O::Adr
        | O::Adrp
        | O::Movn
        | O::Movz
        | O::Mrs
        | O::Nop
        | O::Yield
        | O::Wfi
        | O::Wfe
        | O::Sev
        | O::Sevl
        | O::B
        | O::Bl
        | O::BCond
        | O::Svc
        | O::Hvc
        | O::Smc
        | O::Eret
        | O::Brk => {}

        O::AddImm | O::SubImm | O::AddsImm | O::SubsImm => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, true);
        }

        O::AndImm | O::OrrImm | O::EorImm | O::AndsImm | O::Sbfm | O::Bfm | O::Ubfm => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, false);
        }

        O::Movk => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rd, false);
        }

        O::Cbz | O::Cbnz | O::Tbz | O::Tbnz => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rd, false);
        }

        O::Crc32 | O::Crc32c => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, false);
            push_aarch64_src_gp(&mut regs, &mut len, insn.rm, false);
        }

        O::Msr | O::Sys | O::DcZva => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rd, false);
        }

        O::Setf8 | O::Setf16 | O::Rmif => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, false);
        }

        O::AddExt | O::SubExt | O::AddsExt | O::SubsExt => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, true);
            push_aarch64_src_gp(&mut regs, &mut len, insn.rm, false);
        }

        O::AddReg
        | O::SubReg
        | O::AddsReg
        | O::SubsReg
        | O::AndReg
        | O::OrrReg
        | O::EorReg
        | O::AndsReg
        | O::BicReg
        | O::OrnReg
        | O::EonReg
        | O::BicsReg
        | O::Adc
        | O::Adcs
        | O::Sbc
        | O::Sbcs
        | O::Mul
        | O::Mneg
        | O::Smulh
        | O::Umulh
        | O::Sdiv
        | O::Udiv
        | O::Lsl
        | O::Lsr
        | O::Asr
        | O::Ror
        | O::Extr
        | O::Csel
        | O::Csinc
        | O::Csinv
        | O::Csneg
        | O::Ccmn
        | O::Ccmp
        | O::Br
        | O::Blr
        | O::Ret => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, false);
            push_aarch64_src_gp(&mut regs, &mut len, insn.rm, false);
        }

        O::Madd | O::Msub | O::Smaddl | O::Smsubl | O::Umaddl | O::Umsubl => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, false);
            push_aarch64_src_gp(&mut regs, &mut len, insn.rm, false);
            push_aarch64_src_gp(&mut regs, &mut len, insn.ra, false);
        }

        O::Ldr
        | O::Ldrb
        | O::Ldrh
        | O::Ldrsb
        | O::Ldrsh
        | O::Ldrsw
        | O::Ldur
        | O::Ldurb
        | O::Ldurh
        | O::Ldursb
        | O::Ldursh
        | O::Ldursw
        | O::Ldxr
        | O::Ldaxr
        | O::Ldar
        | O::Ldapr
        | O::Ldaprh
        | O::Ldaprb
        | O::LdapurB
        | O::LdapurH
        | O::Ldapur
        | O::Prfm
        | O::Ldp
        | O::Ldxp
        | O::Ldaxp
        | O::LdrSimd
        | O::LdurSimd
        | O::LdpSimd => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, true);
            if aarch64_mem_uses_reg_offset(insn) {
                push_aarch64_src_gp(&mut regs, &mut len, insn.rm, false);
            }
        }

        O::Str
        | O::Strb
        | O::Strh
        | O::Stur
        | O::Sturb
        | O::Sturh
        | O::Stlr
        | O::StlurB
        | O::StlurH
        | O::Stlur
        | O::Stxr
        | O::Stlxr
        | O::Swp
        | O::Cas => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, true);
            push_aarch64_src_gp(&mut regs, &mut len, insn.rd, false);
            if aarch64_mem_uses_reg_offset(insn) {
                push_aarch64_src_gp(&mut regs, &mut len, insn.rm, false);
            }
        }

        O::Stp | O::Stxp | O::Stlxp => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, true);
            push_aarch64_src_gp(&mut regs, &mut len, insn.rd, false);
            push_aarch64_src_gp(&mut regs, &mut len, insn.pair_second, false);
            if aarch64_mem_uses_reg_offset(insn) {
                push_aarch64_src_gp(&mut regs, &mut len, insn.rm, false);
            }
        }

        O::StrSimd | O::SturSimd => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, true);
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rd));
            if aarch64_mem_uses_reg_offset(insn) {
                push_aarch64_src_gp(&mut regs, &mut len, insn.rm, false);
            }
        }

        O::StpSimd => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, true);
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rd));
            push_timing_reg(
                &mut regs,
                &mut len,
                aarch64_vec_timing_reg(insn.pair_second),
            );
            if aarch64_mem_uses_reg_offset(insn) {
                push_aarch64_src_gp(&mut regs, &mut len, insn.rm, false);
            }
        }

        O::FmovReg
        | O::Fadd
        | O::Fsub
        | O::Fmul
        | O::Fdiv
        | O::Fmax
        | O::Fmin
        | O::Fmaxnm
        | O::Fminnm
        | O::Fcmp
        | O::Fcmpe
        | O::Fsel
        | O::Fccmp
        | O::Fccmpe
        | O::Fnmul
        | O::Fcadd
        | O::Fcmla => {
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rn));
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rm));
        }

        O::Fmadd | O::Fmsub | O::Fnmadd | O::Fnmsub => {
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rn));
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rm));
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.ra));
        }

        O::Fabs
        | O::Fneg
        | O::Fsqrt
        | O::Fcvt
        | O::FcvtzsGpr
        | O::FcvtzuGpr
        | O::FcvtnsGpr
        | O::FcvtnuGpr
        | O::FcvtmsGpr
        | O::FcvtmuGpr
        | O::FcvtpsGpr
        | O::FcvtpuGpr
        | O::FcvtasGpr
        | O::FcvtauGpr => {
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rn));
        }

        O::ScvtfGpr | O::UcvtfGpr | O::FmovGpr => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, false);
        }

        O::Fjcvtzs => {
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rn));
        }

        O::SimdDup => {
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, false);
        }

        O::SimdUmov | O::SimdSmov => {
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rn));
        }

        O::SimdIns => {
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rd));
            push_aarch64_src_gp(&mut regs, &mut len, insn.rn, false);
        }

        O::SimdAdd
        | O::SimdSub
        | O::SimdMul
        | O::SimdAnd
        | O::SimdOrr
        | O::SimdEor
        | O::SimdBic
        | O::SimdCmeq
        | O::SimdCmgt
        | O::SimdCmge
        | O::SimdCmhi
        | O::SimdCmhs
        | O::SimdSmin
        | O::SimdUmin
        | O::SimdSmax
        | O::SimdUmax
        | O::SimdFadd
        | O::SimdFsub
        | O::SimdFmul
        | O::SimdFdiv
        | O::Sdot
        | O::Udot => {
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rn));
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rm));
        }

        O::SimdCmgt0
        | O::SimdCmeq0
        | O::SimdCmlt0
        | O::SimdCmge0
        | O::SimdCmle0
        | O::SimdNot
        | O::SimdAbs
        | O::SimdNeg
        | O::SimdClz
        | O::SimdCnt
        | O::SimdUmaxv
        | O::SimdUminv
        | O::SimdAddv
        | O::SimdUshr
        | O::ScalarAddp => {
            push_timing_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rn));
        }

        _ => {}
    }

    (regs, len)
}

pub(crate) fn aarch64_timing_dst_regs(insn: &Aarch64Insn) -> ([u8; TIMING_MAX_DST_REGS], u8) {
    use helm_arch::aarch64::insn::Opcode as O;

    let mut regs = [0; TIMING_MAX_DST_REGS];
    let mut len = 0;

    match insn.opcode {
        O::AddImm
        | O::SubImm
        | O::AddsImm
        | O::SubsImm
        | O::AddExt
        | O::SubExt
        | O::AddsExt
        | O::SubsExt => {
            push_timing_dst_reg(&mut regs, &mut len, aarch64_dst_gp(insn.rd, true));
        }

        O::Adr
        | O::Adrp
        | O::Crc32
        | O::Crc32c
        | O::AndImm
        | O::OrrImm
        | O::EorImm
        | O::AndsImm
        | O::Movn
        | O::Movz
        | O::Movk
        | O::Sbfm
        | O::Bfm
        | O::Ubfm
        | O::Extr
        | O::AddReg
        | O::SubReg
        | O::AddsReg
        | O::SubsReg
        | O::AndReg
        | O::OrrReg
        | O::EorReg
        | O::AndsReg
        | O::BicReg
        | O::OrnReg
        | O::EonReg
        | O::BicsReg
        | O::Adc
        | O::Adcs
        | O::Sbc
        | O::Sbcs
        | O::Mul
        | O::Madd
        | O::Msub
        | O::Mneg
        | O::Smulh
        | O::Umulh
        | O::Smaddl
        | O::Smsubl
        | O::Umaddl
        | O::Umsubl
        | O::Sdiv
        | O::Udiv
        | O::Lsl
        | O::Lsr
        | O::Asr
        | O::Ror
        | O::Cls
        | O::Clz
        | O::Rev
        | O::Rev16
        | O::Rev32
        | O::Rbit
        | O::Csel
        | O::Csinc
        | O::Csinv
        | O::Csneg
        | O::Ldr
        | O::Ldrb
        | O::Ldrh
        | O::Ldrsb
        | O::Ldrsh
        | O::Ldrsw
        | O::LdrLit
        | O::LdrswLit
        | O::Ldur
        | O::Ldurb
        | O::Ldurh
        | O::Ldursb
        | O::Ldursh
        | O::Ldursw
        | O::Ldxr
        | O::Ldaxr
        | O::Ldar
        | O::Ldapr
        | O::Ldaprh
        | O::Ldaprb
        | O::LdapurB
        | O::LdapurH
        | O::Ldapur
        | O::Mrs
        | O::FcvtzsGpr
        | O::FcvtzuGpr
        | O::FcvtnsGpr
        | O::FcvtnuGpr
        | O::FcvtmsGpr
        | O::FcvtmuGpr
        | O::FcvtpsGpr
        | O::FcvtpuGpr
        | O::FcvtasGpr
        | O::FcvtauGpr
        | O::FmovGpr
        | O::SimdUmov
        | O::SimdSmov
        | O::Fjcvtzs => {
            push_timing_dst_reg(&mut regs, &mut len, aarch64_dst_gp(insn.rd, false));
        }

        O::Ldp | O::Ldxp | O::Ldaxp => {
            push_timing_dst_reg(&mut regs, &mut len, aarch64_dst_gp(insn.rd, false));
            push_timing_dst_reg(&mut regs, &mut len, aarch64_dst_gp(insn.pair_second, false));
        }

        O::Stxr | O::Stlxr | O::Stxp | O::Stlxp => {
            push_timing_dst_reg(&mut regs, &mut len, aarch64_dst_gp(insn.rm, false));
        }

        O::LdrSimd
        | O::LdurSimd
        | O::FmovImm
        | O::FmovReg
        | O::Fadd
        | O::Fsub
        | O::Fmul
        | O::Fdiv
        | O::Fsqrt
        | O::Fabs
        | O::Fneg
        | O::Fmax
        | O::Fmin
        | O::Fmaxnm
        | O::Fminnm
        | O::Fmadd
        | O::Fmsub
        | O::Fnmadd
        | O::Fnmsub
        | O::Fcvt
        | O::ScvtfGpr
        | O::UcvtfGpr
        | O::Fsel
        | O::Fnmul
        | O::SimdDup
        | O::SimdIns
        | O::SimdMovi
        | O::SimdMvni
        | O::SimdFmov
        | O::SimdAdd
        | O::SimdSub
        | O::SimdMul
        | O::SimdCmgt0
        | O::SimdCmeq0
        | O::SimdCmlt0
        | O::SimdCmge0
        | O::SimdCmle0
        | O::SimdCmeq
        | O::SimdCmgt
        | O::SimdCmge
        | O::SimdCmhi
        | O::SimdCmhs
        | O::SimdAnd
        | O::SimdOrr
        | O::SimdEor
        | O::SimdBic
        | O::SimdNot
        | O::SimdNeg
        | O::SimdAbs
        | O::SimdUmaxv
        | O::SimdUminv
        | O::SimdAddv
        | O::SimdSshl
        | O::SimdUshl
        | O::SimdSshr
        | O::SimdUshr
        | O::SimdShl
        | O::SimdExt
        | O::SimdRev64
        | O::SimdRev32
        | O::SimdRev16
        | O::SimdCnt
        | O::SimdClz
        | O::SimdSxtl
        | O::SimdUxtl
        | O::SimdSmin
        | O::SimdUmin
        | O::SimdSmax
        | O::SimdUmax
        | O::SimdFadd
        | O::SimdFsub
        | O::SimdFmul
        | O::SimdFdiv
        | O::SimdFabs
        | O::SimdFneg
        | O::SimdFsqrt
        | O::SimdFcvtzs
        | O::SimdFcvtzu
        | O::SimdScvtf
        | O::SimdUcvtf
        | O::SimdFrintm
        | O::SimdFrintn
        | O::SimdFrintp
        | O::SimdFrintz
        | O::ScalarAddp
        | O::Fcadd
        | O::Fcmla
        | O::Sdot
        | O::Udot => {
            push_timing_dst_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rd));
        }

        O::LdpSimd => {
            push_timing_dst_reg(&mut regs, &mut len, aarch64_vec_timing_reg(insn.rd));
            push_timing_dst_reg(
                &mut regs,
                &mut len,
                aarch64_vec_timing_reg(insn.pair_second),
            );
        }

        _ => {}
    }

    (regs, len)
}

pub(crate) fn riscv_timing_src_regs(insn: &RiscvInsn) -> ([u8; TIMING_MAX_SRC_REGS], u8) {
    let mut regs = [0; TIMING_MAX_SRC_REGS];
    let mut len = 0;

    use helm_arch::riscv::Instruction as I;

    match *insn {
        I::JALR { rs1, .. }
        | I::LB { rs1, .. }
        | I::LH { rs1, .. }
        | I::LW { rs1, .. }
        | I::LD { rs1, .. }
        | I::LBU { rs1, .. }
        | I::LHU { rs1, .. }
        | I::LWU { rs1, .. }
        | I::ADDI { rs1, .. }
        | I::SLTI { rs1, .. }
        | I::SLTIU { rs1, .. }
        | I::XORI { rs1, .. }
        | I::ORI { rs1, .. }
        | I::ANDI { rs1, .. }
        | I::SLLI { rs1, .. }
        | I::SRLI { rs1, .. }
        | I::SRAI { rs1, .. }
        | I::ADDIW { rs1, .. }
        | I::SLLIW { rs1, .. }
        | I::SRLIW { rs1, .. }
        | I::SRAIW { rs1, .. }
        | I::CSRRW { rs1, .. }
        | I::CSRRS { rs1, .. }
        | I::CSRRC { rs1, .. }
        | I::LR_W { rs1, .. }
        | I::LR_D { rs1, .. } => {
            push_timing_reg(&mut regs, &mut len, riscv_int_timing_reg(rs1));
        }
        I::BEQ { rs1, rs2, .. }
        | I::BNE { rs1, rs2, .. }
        | I::BLT { rs1, rs2, .. }
        | I::BGE { rs1, rs2, .. }
        | I::BLTU { rs1, rs2, .. }
        | I::BGEU { rs1, rs2, .. }
        | I::SB { rs1, rs2, .. }
        | I::SH { rs1, rs2, .. }
        | I::SW { rs1, rs2, .. }
        | I::SD { rs1, rs2, .. }
        | I::ADD { rs1, rs2, .. }
        | I::SUB { rs1, rs2, .. }
        | I::SLL { rs1, rs2, .. }
        | I::SLT { rs1, rs2, .. }
        | I::SLTU { rs1, rs2, .. }
        | I::XOR { rs1, rs2, .. }
        | I::SRL { rs1, rs2, .. }
        | I::SRA { rs1, rs2, .. }
        | I::OR { rs1, rs2, .. }
        | I::AND { rs1, rs2, .. }
        | I::ADDW { rs1, rs2, .. }
        | I::SUBW { rs1, rs2, .. }
        | I::SLLW { rs1, rs2, .. }
        | I::SRLW { rs1, rs2, .. }
        | I::SRAW { rs1, rs2, .. }
        | I::MUL { rs1, rs2, .. }
        | I::MULH { rs1, rs2, .. }
        | I::MULHSU { rs1, rs2, .. }
        | I::MULHU { rs1, rs2, .. }
        | I::DIV { rs1, rs2, .. }
        | I::DIVU { rs1, rs2, .. }
        | I::REM { rs1, rs2, .. }
        | I::REMU { rs1, rs2, .. }
        | I::MULW { rs1, rs2, .. }
        | I::DIVW { rs1, rs2, .. }
        | I::DIVUW { rs1, rs2, .. }
        | I::REMW { rs1, rs2, .. }
        | I::REMUW { rs1, rs2, .. }
        | I::SC_W { rs1, rs2, .. }
        | I::AMOSWAP_W { rs1, rs2, .. }
        | I::AMOADD_W { rs1, rs2, .. }
        | I::AMOXOR_W { rs1, rs2, .. }
        | I::AMOAND_W { rs1, rs2, .. }
        | I::AMOOR_W { rs1, rs2, .. }
        | I::AMOMIN_W { rs1, rs2, .. }
        | I::AMOMAX_W { rs1, rs2, .. }
        | I::AMOMINU_W { rs1, rs2, .. }
        | I::AMOMAXU_W { rs1, rs2, .. }
        | I::SC_D { rs1, rs2, .. }
        | I::AMOSWAP_D { rs1, rs2, .. }
        | I::AMOADD_D { rs1, rs2, .. }
        | I::AMOXOR_D { rs1, rs2, .. }
        | I::AMOAND_D { rs1, rs2, .. }
        | I::AMOOR_D { rs1, rs2, .. }
        | I::AMOMIN_D { rs1, rs2, .. }
        | I::AMOMAX_D { rs1, rs2, .. }
        | I::AMOMINU_D { rs1, rs2, .. }
        | I::AMOMAXU_D { rs1, rs2, .. } => {
            push_timing_reg(&mut regs, &mut len, riscv_int_timing_reg(rs1));
            push_timing_reg(&mut regs, &mut len, riscv_int_timing_reg(rs2));
        }
        I::FLW { rs1, .. } | I::FLD { rs1, .. } => {
            push_timing_reg(&mut regs, &mut len, riscv_int_timing_reg(rs1));
        }
        I::FSW { rs1, rs2, .. } | I::FSD { rs1, rs2, .. } => {
            push_timing_reg(&mut regs, &mut len, riscv_int_timing_reg(rs1));
            push_timing_reg(&mut regs, &mut len, Some(riscv_fp_timing_reg(rs2)));
        }
        I::FMADD_S { rs1, rs2, rs3, .. }
        | I::FMSUB_S { rs1, rs2, rs3, .. }
        | I::FNMSUB_S { rs1, rs2, rs3, .. }
        | I::FNMADD_S { rs1, rs2, rs3, .. }
        | I::FMADD_D { rs1, rs2, rs3, .. }
        | I::FMSUB_D { rs1, rs2, rs3, .. }
        | I::FNMSUB_D { rs1, rs2, rs3, .. }
        | I::FNMADD_D { rs1, rs2, rs3, .. } => {
            push_timing_reg(&mut regs, &mut len, Some(riscv_fp_timing_reg(rs1)));
            push_timing_reg(&mut regs, &mut len, Some(riscv_fp_timing_reg(rs2)));
            push_timing_reg(&mut regs, &mut len, Some(riscv_fp_timing_reg(rs3)));
        }
        I::FADD_S { rs1, rs2, .. }
        | I::FSUB_S { rs1, rs2, .. }
        | I::FMUL_S { rs1, rs2, .. }
        | I::FDIV_S { rs1, rs2, .. }
        | I::FSGNJ_S { rs1, rs2, .. }
        | I::FSGNJN_S { rs1, rs2, .. }
        | I::FSGNJX_S { rs1, rs2, .. }
        | I::FMIN_S { rs1, rs2, .. }
        | I::FMAX_S { rs1, rs2, .. }
        | I::FEQ_S { rs1, rs2, .. }
        | I::FLT_S { rs1, rs2, .. }
        | I::FLE_S { rs1, rs2, .. }
        | I::FADD_D { rs1, rs2, .. }
        | I::FSUB_D { rs1, rs2, .. }
        | I::FMUL_D { rs1, rs2, .. }
        | I::FDIV_D { rs1, rs2, .. }
        | I::FSGNJ_D { rs1, rs2, .. }
        | I::FSGNJN_D { rs1, rs2, .. }
        | I::FSGNJX_D { rs1, rs2, .. }
        | I::FMIN_D { rs1, rs2, .. }
        | I::FMAX_D { rs1, rs2, .. }
        | I::FEQ_D { rs1, rs2, .. }
        | I::FLT_D { rs1, rs2, .. }
        | I::FLE_D { rs1, rs2, .. } => {
            push_timing_reg(&mut regs, &mut len, Some(riscv_fp_timing_reg(rs1)));
            push_timing_reg(&mut regs, &mut len, Some(riscv_fp_timing_reg(rs2)));
        }
        I::FSQRT_S { rs1, .. }
        | I::FCVT_W_S { rs1, .. }
        | I::FCVT_WU_S { rs1, .. }
        | I::FCVT_L_S { rs1, .. }
        | I::FCVT_LU_S { rs1, .. }
        | I::FMV_X_W { rs1, .. }
        | I::FCLASS_S { rs1, .. }
        | I::FSQRT_D { rs1, .. }
        | I::FCVT_S_D { rs1, .. }
        | I::FCVT_W_D { rs1, .. }
        | I::FCVT_WU_D { rs1, .. }
        | I::FCVT_L_D { rs1, .. }
        | I::FCVT_LU_D { rs1, .. }
        | I::FMV_X_D { rs1, .. }
        | I::FCVT_D_S { rs1, .. }
        | I::FCLASS_D { rs1, .. } => {
            push_timing_reg(&mut regs, &mut len, Some(riscv_fp_timing_reg(rs1)));
        }
        I::FCVT_S_W { rs1, .. }
        | I::FCVT_S_WU { rs1, .. }
        | I::FCVT_S_L { rs1, .. }
        | I::FCVT_S_LU { rs1, .. }
        | I::FMV_W_X { rs1, .. }
        | I::FCVT_D_W { rs1, .. }
        | I::FCVT_D_WU { rs1, .. }
        | I::FCVT_D_L { rs1, .. }
        | I::FCVT_D_LU { rs1, .. }
        | I::FMV_D_X { rs1, .. } => {
            push_timing_reg(&mut regs, &mut len, riscv_int_timing_reg(rs1));
        }
        _ => {}
    }

    (regs, len)
}

pub(crate) fn riscv_timing_dst_regs(insn: &RiscvInsn) -> ([u8; TIMING_MAX_DST_REGS], u8) {
    use helm_arch::riscv::Instruction as I;

    let mut regs = [0; TIMING_MAX_DST_REGS];
    let mut len = 0;

    match *insn {
        I::LUI { rd, .. }
        | I::AUIPC { rd, .. }
        | I::JAL { rd, .. }
        | I::JALR { rd, .. }
        | I::LB { rd, .. }
        | I::LH { rd, .. }
        | I::LW { rd, .. }
        | I::LD { rd, .. }
        | I::LBU { rd, .. }
        | I::LHU { rd, .. }
        | I::LWU { rd, .. }
        | I::ADDI { rd, .. }
        | I::SLTI { rd, .. }
        | I::SLTIU { rd, .. }
        | I::XORI { rd, .. }
        | I::ORI { rd, .. }
        | I::ANDI { rd, .. }
        | I::SLLI { rd, .. }
        | I::SRLI { rd, .. }
        | I::SRAI { rd, .. }
        | I::ADD { rd, .. }
        | I::SUB { rd, .. }
        | I::SLL { rd, .. }
        | I::SLT { rd, .. }
        | I::SLTU { rd, .. }
        | I::XOR { rd, .. }
        | I::SRL { rd, .. }
        | I::SRA { rd, .. }
        | I::OR { rd, .. }
        | I::AND { rd, .. }
        | I::ADDIW { rd, .. }
        | I::SLLIW { rd, .. }
        | I::SRLIW { rd, .. }
        | I::SRAIW { rd, .. }
        | I::ADDW { rd, .. }
        | I::SUBW { rd, .. }
        | I::SLLW { rd, .. }
        | I::SRLW { rd, .. }
        | I::SRAW { rd, .. }
        | I::CSRRW { rd, .. }
        | I::CSRRS { rd, .. }
        | I::CSRRC { rd, .. }
        | I::CSRRWI { rd, .. }
        | I::CSRRSI { rd, .. }
        | I::CSRRCI { rd, .. }
        | I::MUL { rd, .. }
        | I::MULH { rd, .. }
        | I::MULHSU { rd, .. }
        | I::MULHU { rd, .. }
        | I::DIV { rd, .. }
        | I::DIVU { rd, .. }
        | I::REM { rd, .. }
        | I::REMU { rd, .. }
        | I::MULW { rd, .. }
        | I::DIVW { rd, .. }
        | I::DIVUW { rd, .. }
        | I::REMW { rd, .. }
        | I::REMUW { rd, .. }
        | I::LR_W { rd, .. }
        | I::SC_W { rd, .. }
        | I::AMOSWAP_W { rd, .. }
        | I::AMOADD_W { rd, .. }
        | I::AMOXOR_W { rd, .. }
        | I::AMOAND_W { rd, .. }
        | I::AMOOR_W { rd, .. }
        | I::AMOMIN_W { rd, .. }
        | I::AMOMAX_W { rd, .. }
        | I::AMOMINU_W { rd, .. }
        | I::AMOMAXU_W { rd, .. }
        | I::LR_D { rd, .. }
        | I::SC_D { rd, .. }
        | I::AMOSWAP_D { rd, .. }
        | I::AMOADD_D { rd, .. }
        | I::AMOXOR_D { rd, .. }
        | I::AMOAND_D { rd, .. }
        | I::AMOOR_D { rd, .. }
        | I::AMOMIN_D { rd, .. }
        | I::AMOMAX_D { rd, .. }
        | I::AMOMINU_D { rd, .. }
        | I::AMOMAXU_D { rd, .. }
        | I::FCVT_W_S { rd, .. }
        | I::FCVT_WU_S { rd, .. }
        | I::FCVT_L_S { rd, .. }
        | I::FCVT_LU_S { rd, .. }
        | I::FMV_X_W { rd, .. }
        | I::FEQ_S { rd, .. }
        | I::FLT_S { rd, .. }
        | I::FLE_S { rd, .. }
        | I::FCLASS_S { rd, .. }
        | I::FCVT_W_D { rd, .. }
        | I::FCVT_WU_D { rd, .. }
        | I::FCVT_L_D { rd, .. }
        | I::FCVT_LU_D { rd, .. }
        | I::FMV_X_D { rd, .. }
        | I::FEQ_D { rd, .. }
        | I::FLT_D { rd, .. }
        | I::FLE_D { rd, .. }
        | I::FCLASS_D { rd, .. } => {
            push_timing_dst_reg(&mut regs, &mut len, riscv_int_timing_reg(rd));
        }
        I::FLW { rd, .. }
        | I::FMADD_S { rd, .. }
        | I::FMSUB_S { rd, .. }
        | I::FNMSUB_S { rd, .. }
        | I::FNMADD_S { rd, .. }
        | I::FADD_S { rd, .. }
        | I::FSUB_S { rd, .. }
        | I::FMUL_S { rd, .. }
        | I::FDIV_S { rd, .. }
        | I::FSQRT_S { rd, .. }
        | I::FSGNJ_S { rd, .. }
        | I::FSGNJN_S { rd, .. }
        | I::FSGNJX_S { rd, .. }
        | I::FMIN_S { rd, .. }
        | I::FMAX_S { rd, .. }
        | I::FCVT_S_W { rd, .. }
        | I::FCVT_S_WU { rd, .. }
        | I::FCVT_S_L { rd, .. }
        | I::FCVT_S_LU { rd, .. }
        | I::FMV_W_X { rd, .. }
        | I::FLD { rd, .. }
        | I::FMADD_D { rd, .. }
        | I::FMSUB_D { rd, .. }
        | I::FNMSUB_D { rd, .. }
        | I::FNMADD_D { rd, .. }
        | I::FADD_D { rd, .. }
        | I::FSUB_D { rd, .. }
        | I::FMUL_D { rd, .. }
        | I::FDIV_D { rd, .. }
        | I::FSQRT_D { rd, .. }
        | I::FSGNJ_D { rd, .. }
        | I::FSGNJN_D { rd, .. }
        | I::FSGNJX_D { rd, .. }
        | I::FMIN_D { rd, .. }
        | I::FMAX_D { rd, .. }
        | I::FCVT_S_D { rd, .. }
        | I::FCVT_D_S { rd, .. }
        | I::FCVT_D_W { rd, .. }
        | I::FCVT_D_WU { rd, .. }
        | I::FCVT_D_L { rd, .. }
        | I::FCVT_D_LU { rd, .. }
        | I::FMV_D_X { rd, .. } => {
            push_timing_dst_reg(&mut regs, &mut len, Some(riscv_fp_timing_reg(rd)));
        }
        _ => {}
    }

    (regs, len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_arch::{aarch64::decode::decode as decode_a64, riscv::decode::decode as decode_rv};

    fn a64(raw: u32) -> Aarch64Insn {
        decode_a64(raw, 0x1000).expect("aarch64 decode")
    }

    fn rv(raw: u32) -> RiscvInsn {
        decode_rv(raw, 0x1000).expect("riscv decode")
    }

    fn a64_dp2(sf: u32, opcode: u32, rm: u32, rn: u32, rd: u32) -> u32 {
        (sf << 31) | (0b0011010110 << 21) | (rm << 16) | (opcode << 10) | (rn << 5) | rd
    }

    #[test]
    fn aarch64_add_sp_imm_uses_sp_as_src_and_dst() {
        let insn = a64(0x9100_43FF); // add sp, sp, #16
        let (srcs, src_count) = aarch64_timing_src_regs(&insn);
        let (dsts, dst_count) = aarch64_timing_dst_regs(&insn);

        assert_eq!(&srcs[..src_count as usize], &[TIMING_AARCH64_SP_REG]);
        assert_eq!(&dsts[..dst_count as usize], &[TIMING_AARCH64_SP_REG]);
    }

    #[test]
    fn aarch64_pair_load_exposes_both_destinations() {
        let insn = a64(0xA940_07E0); // ldp x0, x1, [sp]
        let (srcs, src_count) = aarch64_timing_src_regs(&insn);
        let (dsts, dst_count) = aarch64_timing_dst_regs(&insn);

        assert_eq!(&srcs[..src_count as usize], &[TIMING_AARCH64_SP_REG]);
        assert_eq!(&dsts[..dst_count as usize], &[0, 1]);
    }

    #[test]
    fn aarch64_reg_offset_load_uses_offset_register() {
        let insn = a64(0xF862_7A63); // ldr x3, [x19, x2, lsl #3]
        let (srcs, src_count) = aarch64_timing_src_regs(&insn);
        let (dsts, dst_count) = aarch64_timing_dst_regs(&insn);

        assert_eq!(&srcs[..src_count as usize], &[19, 2]);
        assert_eq!(&dsts[..dst_count as usize], &[3]);
    }

    #[test]
    fn aarch64_simd_add_uses_vector_sources() {
        let insn = a64(0x4EE7_8463); // add v3.2d, v3.2d, v7.2d
        let (srcs, src_count) = aarch64_timing_src_regs(&insn);
        let (dsts, dst_count) = aarch64_timing_dst_regs(&insn);

        assert_eq!(
            &srcs[..src_count as usize],
            &[TIMING_VEC_REG_BASE + 3, TIMING_VEC_REG_BASE + 7]
        );
        assert_eq!(&dsts[..dst_count as usize], &[TIMING_VEC_REG_BASE + 3]);
    }

    #[test]
    fn aarch64_setf8_reads_rn() {
        let mut insn = Aarch64Insn::zeroed();
        insn.opcode = helm_arch::aarch64::insn::Opcode::Setf8;
        insn.rn = 1;
        let (srcs, src_count) = aarch64_timing_src_regs(&insn);
        let (dsts, dst_count) = aarch64_timing_dst_regs(&insn);

        assert_eq!(&srcs[..src_count as usize], &[1]);
        assert!(
            dst_count == 0,
            "SETF8 should not produce a GPR timing destination"
        );
        assert_eq!(&dsts[..dst_count as usize], &[] as &[u8]);
    }

    #[test]
    fn aarch64_crc32_uses_rn_rm_and_rd() {
        let insn = a64(a64_dp2(0, 0b010000, 2, 1, 0)); // crc32b w0, w1, w2
        let (srcs, src_count) = aarch64_timing_src_regs(&insn);
        let (dsts, dst_count) = aarch64_timing_dst_regs(&insn);

        assert_eq!(&srcs[..src_count as usize], &[1, 2]);
        assert_eq!(&dsts[..dst_count as usize], &[0]);
    }

    #[test]
    fn aarch64_fjcvtzs_uses_vector_source_and_gpr_dest() {
        let mut insn = Aarch64Insn::zeroed();
        insn.opcode = helm_arch::aarch64::insn::Opcode::Fjcvtzs;
        insn.rn = 2;
        insn.rd = 0;
        let (srcs, src_count) = aarch64_timing_src_regs(&insn);
        let (dsts, dst_count) = aarch64_timing_dst_regs(&insn);

        assert_eq!(&srcs[..src_count as usize], &[TIMING_VEC_REG_BASE + 2]);
        assert_eq!(&dsts[..dst_count as usize], &[0]);
    }

    #[test]
    fn aarch64_simd_umov_uses_vector_source_and_gpr_dest() {
        let mut insn = Aarch64Insn::zeroed();
        insn.opcode = helm_arch::aarch64::insn::Opcode::SimdUmov;
        insn.rn = 2;
        insn.rd = 1;
        let (srcs, src_count) = aarch64_timing_src_regs(&insn);
        let (dsts, dst_count) = aarch64_timing_dst_regs(&insn);

        assert_eq!(&srcs[..src_count as usize], &[TIMING_VEC_REG_BASE + 2]);
        assert_eq!(&dsts[..dst_count as usize], &[1]);
    }

    #[test]
    fn aarch64_simd_cmeq0_uses_vector_source_and_dest() {
        let mut insn = Aarch64Insn::zeroed();
        insn.opcode = helm_arch::aarch64::insn::Opcode::SimdCmeq0;
        insn.rn = 1;
        insn.rd = 3;
        let (srcs, src_count) = aarch64_timing_src_regs(&insn);
        let (dsts, dst_count) = aarch64_timing_dst_regs(&insn);

        assert_eq!(&srcs[..src_count as usize], &[TIMING_VEC_REG_BASE + 1]);
        assert_eq!(&dsts[..dst_count as usize], &[TIMING_VEC_REG_BASE + 3]);
    }

    #[test]
    fn riscv_addi_tracks_src_and_dst() {
        let insn = rv(0x0010_8113); // addi x2, x1, 1
        let (srcs, src_count) = riscv_timing_src_regs(&insn);
        let (dsts, dst_count) = riscv_timing_dst_regs(&insn);

        assert_eq!(&srcs[..src_count as usize], &[1]);
        assert_eq!(&dsts[..dst_count as usize], &[2]);
    }
}
