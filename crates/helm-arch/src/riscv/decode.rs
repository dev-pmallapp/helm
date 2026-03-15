//! RISC-V RV64GC instruction decoder.
//!
//! Entry point: [`decode`] — takes a raw 32-bit word and returns [`Instruction`].
//! For 16-bit (C) instructions, call [`expand_c`] first to get the 32-bit equivalent.
//!
//! # Immediate extraction helpers
//! All helpers sign-extend to i64 per the RISC-V spec.

use super::insn::Instruction;
use crate::DecodeError;

// ── Bit-extraction helpers ────────────────────────────────────────────────────

#[inline(always)]
fn bits(raw: u32, hi: u32, lo: u32) -> u32 {
    (raw >> lo) & ((1 << (hi - lo + 1)) - 1)
}

#[inline(always)]
fn bit(raw: u32, pos: u32) -> u32 { (raw >> pos) & 1 }

#[inline(always)]
fn rd(raw: u32) -> u8  { bits(raw, 11, 7) as u8 }
#[inline(always)]
fn rs1(raw: u32) -> u8 { bits(raw, 19, 15) as u8 }
#[inline(always)]
fn rs2(raw: u32) -> u8 { bits(raw, 24, 20) as u8 }
#[inline(always)]
fn rs3(raw: u32) -> u8 { bits(raw, 31, 27) as u8 }
#[inline(always)]
fn funct3(raw: u32) -> u32 { bits(raw, 14, 12) }
#[inline(always)]
fn funct7(raw: u32) -> u32 { bits(raw, 31, 25) }
#[inline(always)]
fn rm(raw: u32) -> u8 { funct3(raw) as u8 }

/// I-type immediate: bits [31:20], sign-extended.
#[inline(always)]
fn imm_i(raw: u32) -> i64 {
    ((raw as i32) >> 20) as i64
}

/// S-type immediate: bits [31:25|11:7], sign-extended.
#[inline(always)]
fn imm_s(raw: u32) -> i64 {
    let hi = (raw >> 25) & 0x7F;
    let lo = (raw >> 7) & 0x1F;
    (((hi << 5 | lo) as i32) << 20 >> 20) as i64
}

/// B-type immediate: bits [31|7|30:25|11:8]<<1, sign-extended.
#[inline(always)]
fn imm_b(raw: u32) -> i64 {
    let imm = (bit(raw, 31) << 12)
        | (bit(raw, 7) << 11)
        | (bits(raw, 30, 25) << 5)
        | (bits(raw, 11, 8) << 1);
    ((imm as i32) << 19 >> 19) as i64
}

/// U-type immediate: bits [31:12] << 12.
#[inline(always)]
fn imm_u(raw: u32) -> i64 {
    ((raw & 0xFFFFF000) as i32) as i64
}

/// J-type immediate: bits [31|19:12|20|30:21]<<1, sign-extended.
#[inline(always)]
fn imm_j(raw: u32) -> i64 {
    let imm = (bit(raw, 31) << 20)
        | (bits(raw, 19, 12) << 12)
        | (bit(raw, 20) << 11)
        | (bits(raw, 30, 21) << 1);
    ((imm as i32) << 11 >> 11) as i64
}

/// Shift amount for 64-bit shifts (6 bits).
#[inline(always)]
fn shamt64(raw: u32) -> u8 { bits(raw, 25, 20) as u8 }
/// Shift amount for 32-bit word shifts (5 bits).
#[inline(always)]
fn shamt32(raw: u32) -> u8 { bits(raw, 24, 20) as u8 }

// ── Decoder ───────────────────────────────────────────────────────────────────

/// Decode a 32-bit RISC-V instruction word.
///
/// C-extension (16-bit) words must be expanded to 32-bit via [`expand_c`] first.
pub fn decode(raw: u32, pc: u64) -> Result<Instruction, DecodeError> {
    use Instruction::*;

    let opcode = raw & 0x7F;

    match opcode {
        // LUI
        0b0110111 => Ok(LUI { rd: rd(raw), imm: imm_u(raw) }),
        // AUIPC
        0b0010111 => Ok(AUIPC { rd: rd(raw), imm: imm_u(raw) }),
        // JAL
        0b1101111 => Ok(JAL { rd: rd(raw), imm: imm_j(raw) }),
        // JALR
        0b1100111 => {
            if funct3(raw) == 0 { Ok(JALR { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) })
            } else { Err(DecodeError::Unknown { raw, pc }) }
        }
        // Branches
        0b1100011 => match funct3(raw) {
            0b000 => Ok(BEQ  { rs1: rs1(raw), rs2: rs2(raw), imm: imm_b(raw) }),
            0b001 => Ok(BNE  { rs1: rs1(raw), rs2: rs2(raw), imm: imm_b(raw) }),
            0b100 => Ok(BLT  { rs1: rs1(raw), rs2: rs2(raw), imm: imm_b(raw) }),
            0b101 => Ok(BGE  { rs1: rs1(raw), rs2: rs2(raw), imm: imm_b(raw) }),
            0b110 => Ok(BLTU { rs1: rs1(raw), rs2: rs2(raw), imm: imm_b(raw) }),
            0b111 => Ok(BGEU { rs1: rs1(raw), rs2: rs2(raw), imm: imm_b(raw) }),
            _     => Err(DecodeError::Unknown { raw, pc }),
        },
        // Loads
        0b0000011 => match funct3(raw) {
            0b000 => Ok(LB  { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b001 => Ok(LH  { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b010 => Ok(LW  { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b011 => Ok(LD  { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b100 => Ok(LBU { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b101 => Ok(LHU { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b110 => Ok(LWU { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            _     => Err(DecodeError::Unknown { raw, pc }),
        },
        // Stores
        0b0100011 => match funct3(raw) {
            0b000 => Ok(SB { rs1: rs1(raw), rs2: rs2(raw), imm: imm_s(raw) }),
            0b001 => Ok(SH { rs1: rs1(raw), rs2: rs2(raw), imm: imm_s(raw) }),
            0b010 => Ok(SW { rs1: rs1(raw), rs2: rs2(raw), imm: imm_s(raw) }),
            0b011 => Ok(SD { rs1: rs1(raw), rs2: rs2(raw), imm: imm_s(raw) }),
            _     => Err(DecodeError::Unknown { raw, pc }),
        },
        // OP-IMM (64-bit immediate ALU)
        0b0010011 => match funct3(raw) {
            0b000 => Ok(ADDI  { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b010 => Ok(SLTI  { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b011 => Ok(SLTIU { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b100 => Ok(XORI  { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b110 => Ok(ORI   { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b111 => Ok(ANDI  { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b001 => Ok(SLLI  { rd: rd(raw), rs1: rs1(raw), shamt: shamt64(raw) }),
            0b101 => {
                if funct7(raw) >> 1 == 0 { Ok(SRLI { rd: rd(raw), rs1: rs1(raw), shamt: shamt64(raw) })
                } else                   { Ok(SRAI { rd: rd(raw), rs1: rs1(raw), shamt: shamt64(raw) }) }
            }
            _ => Err(DecodeError::Unknown { raw, pc }),
        },
        // OP (64-bit register ALU + M extension)
        0b0110011 => match (funct7(raw), funct3(raw)) {
            (0b0000000, 0b000) => Ok(ADD  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0100000, 0b000) => Ok(SUB  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000000, 0b001) => Ok(SLL  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000000, 0b010) => Ok(SLT  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000000, 0b011) => Ok(SLTU { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000000, 0b100) => Ok(XOR  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000000, 0b101) => Ok(SRL  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0100000, 0b101) => Ok(SRA  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000000, 0b110) => Ok(OR   { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000000, 0b111) => Ok(AND  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            // M extension
            (0b0000001, 0b000) => Ok(MUL    { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000001, 0b001) => Ok(MULH   { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000001, 0b010) => Ok(MULHSU { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000001, 0b011) => Ok(MULHU  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000001, 0b100) => Ok(DIV    { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000001, 0b101) => Ok(DIVU   { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000001, 0b110) => Ok(REM    { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000001, 0b111) => Ok(REMU   { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            _ => Err(DecodeError::Unknown { raw, pc }),
        },
        // OP-IMM-32 (word-size immediate ALU)
        0b0011011 => match funct3(raw) {
            0b000 => Ok(ADDIW { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b001 => Ok(SLLIW { rd: rd(raw), rs1: rs1(raw), shamt: shamt32(raw) }),
            0b101 => {
                if funct7(raw) == 0 { Ok(SRLIW { rd: rd(raw), rs1: rs1(raw), shamt: shamt32(raw) })
                } else               { Ok(SRAIW { rd: rd(raw), rs1: rs1(raw), shamt: shamt32(raw) }) }
            }
            _ => Err(DecodeError::Unknown { raw, pc }),
        },
        // OP-32 (word-size register ALU + M extension)
        0b0111011 => match (funct7(raw), funct3(raw)) {
            (0b0000000, 0b000) => Ok(ADDW  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0100000, 0b000) => Ok(SUBW  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000000, 0b001) => Ok(SLLW  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000000, 0b101) => Ok(SRLW  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0100000, 0b101) => Ok(SRAW  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000001, 0b000) => Ok(MULW  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000001, 0b100) => Ok(DIVW  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000001, 0b101) => Ok(DIVUW { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000001, 0b110) => Ok(REMW  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            (0b0000001, 0b111) => Ok(REMUW { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw) }),
            _ => Err(DecodeError::Unknown { raw, pc }),
        },
        // MISC-MEM (FENCE / FENCE.I)
        0b0001111 => match funct3(raw) {
            0b000 => Ok(FENCE   { pred: bits(raw, 27, 24) as u8, succ: bits(raw, 23, 20) as u8 }),
            0b001 => Ok(FENCE_I),
            _     => Err(DecodeError::Unknown { raw, pc }),
        },
        // SYSTEM (ECALL, EBREAK, CSR, WFI, MRET, SRET, SFENCE.VMA)
        0b1110011 => decode_system(raw, pc),
        // AMO (A extension)
        0b0101111 => decode_amo(raw, pc),
        // LOAD-FP (F/D)
        0b0000111 => match funct3(raw) {
            0b010 => Ok(FLW { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            0b011 => Ok(FLD { rd: rd(raw), rs1: rs1(raw), imm: imm_i(raw) }),
            _     => Err(DecodeError::Unknown { raw, pc }),
        },
        // STORE-FP (F/D)
        0b0100111 => match funct3(raw) {
            0b010 => Ok(FSW { rs1: rs1(raw), rs2: rs2(raw), imm: imm_s(raw) }),
            0b011 => Ok(FSD { rs1: rs1(raw), rs2: rs2(raw), imm: imm_s(raw) }),
            _     => Err(DecodeError::Unknown { raw, pc }),
        },
        // MADD / MSUB / NMSUB / NMADD (F/D fused)
        0b1000011 => Ok(FMADD_D  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw), rs3: rs3(raw), rm: rm(raw) }),
        0b1000111 => Ok(FMSUB_D  { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw), rs3: rs3(raw), rm: rm(raw) }),
        0b1001011 => Ok(FNMSUB_D { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw), rs3: rs3(raw), rm: rm(raw) }),
        0b1001111 => Ok(FNMADD_D { rd: rd(raw), rs1: rs1(raw), rs2: rs2(raw), rs3: rs3(raw), rm: rm(raw) }),
        // OP-FP (F/D arithmetic)
        0b1010011 => decode_fp(raw, pc),
        _ => Err(DecodeError::Unknown { raw, pc }),
    }
}

fn decode_system(raw: u32, pc: u64) -> Result<Instruction, DecodeError> {
    use Instruction::*;
    let f3 = funct3(raw);
    let f12 = bits(raw, 31, 20) as u16;

    if f3 == 0 {
        match f12 {
            0x000 => return Ok(ECALL),
            0x001 => return Ok(EBREAK),
            0x302 => return Ok(MRET),
            0x102 => return Ok(SRET),
            0x105 => return Ok(WFI),
            _ => {}
        }
        if bits(raw, 31, 25) == 0b0001001 {
            return Ok(SFENCE_VMA { rs1: rs1(raw), rs2: rs2(raw) });
        }
        return Err(DecodeError::Unknown { raw, pc });
    }

    // CSR instructions
    let csr = f12;
    match f3 {
        0b001 => Ok(CSRRW  { rd: rd(raw), rs1: rs1(raw), csr }),
        0b010 => Ok(CSRRS  { rd: rd(raw), rs1: rs1(raw), csr }),
        0b011 => Ok(CSRRC  { rd: rd(raw), rs1: rs1(raw), csr }),
        0b101 => Ok(CSRRWI { rd: rd(raw), uimm: rs1(raw), csr }),
        0b110 => Ok(CSRRSI { rd: rd(raw), uimm: rs1(raw), csr }),
        0b111 => Ok(CSRRCI { rd: rd(raw), uimm: rs1(raw), csr }),
        _     => Err(DecodeError::Unknown { raw, pc }),
    }
}

fn decode_amo(raw: u32, pc: u64) -> Result<Instruction, DecodeError> {
    use Instruction::*;
    let f3 = funct3(raw);
    let f5 = bits(raw, 31, 27);
    let aq = bit(raw, 26) != 0;
    let rl = bit(raw, 25) != 0;
    let (r1, r2, rd_) = (rs1(raw), rs2(raw), rd(raw));

    match (f5, f3) {
        (0b00010, 0b010) => Ok(LR_W      { rd: rd_, rs1: r1, aq, rl }),
        (0b00011, 0b010) => Ok(SC_W      { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b00001, 0b010) => Ok(AMOSWAP_W { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b00000, 0b010) => Ok(AMOADD_W  { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b00100, 0b010) => Ok(AMOXOR_W  { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b01100, 0b010) => Ok(AMOAND_W  { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b01000, 0b010) => Ok(AMOOR_W   { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b10000, 0b010) => Ok(AMOMIN_W  { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b10100, 0b010) => Ok(AMOMAX_W  { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b11000, 0b010) => Ok(AMOMINU_W { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b11100, 0b010) => Ok(AMOMAXU_W { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b00010, 0b011) => Ok(LR_D      { rd: rd_, rs1: r1, aq, rl }),
        (0b00011, 0b011) => Ok(SC_D      { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b00001, 0b011) => Ok(AMOSWAP_D { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b00000, 0b011) => Ok(AMOADD_D  { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b00100, 0b011) => Ok(AMOXOR_D  { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b01100, 0b011) => Ok(AMOAND_D  { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b01000, 0b011) => Ok(AMOOR_D   { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b10000, 0b011) => Ok(AMOMIN_D  { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b10100, 0b011) => Ok(AMOMAX_D  { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b11000, 0b011) => Ok(AMOMINU_D { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        (0b11100, 0b011) => Ok(AMOMAXU_D { rd: rd_, rs1: r1, rs2: r2, aq, rl }),
        _ => Err(DecodeError::Unknown { raw, pc }),
    }
}

fn decode_fp(raw: u32, pc: u64) -> Result<Instruction, DecodeError> {
    use Instruction::*;
    let f7 = funct3(raw);    // rm field
    let f25 = bits(raw, 31, 25);  // funct7 equivalent
    let (r1, r2, rd_) = (rs1(raw), rs2(raw), rd(raw));
    let rm_ = rm(raw);

    // Discriminate by top 5 bits (fmt + opcode) then rs2 for unary ops.
    match f25 {
        0b0000000 => Ok(FADD_S  { rd: rd_, rs1: r1, rs2: r2, rm: rm_ }),
        0b0000100 => Ok(FSUB_S  { rd: rd_, rs1: r1, rs2: r2, rm: rm_ }),
        0b0001000 => Ok(FMUL_S  { rd: rd_, rs1: r1, rs2: r2, rm: rm_ }),
        0b0001100 => Ok(FDIV_S  { rd: rd_, rs1: r1, rs2: r2, rm: rm_ }),
        0b0101100 if r2 == 0 => Ok(FSQRT_S { rd: rd_, rs1: r1, rm: rm_ }),
        0b0010000 => match f7 {
            0b000 => Ok(FSGNJ_S  { rd: rd_, rs1: r1, rs2: r2 }),
            0b001 => Ok(FSGNJN_S { rd: rd_, rs1: r1, rs2: r2 }),
            0b010 => Ok(FSGNJX_S { rd: rd_, rs1: r1, rs2: r2 }),
            _     => Err(DecodeError::Unknown { raw, pc }),
        },
        0b0010100 => match f7 {
            0b000 => Ok(FMIN_S { rd: rd_, rs1: r1, rs2: r2 }),
            0b001 => Ok(FMAX_S { rd: rd_, rs1: r1, rs2: r2 }),
            _     => Err(DecodeError::Unknown { raw, pc }),
        },
        0b1100000 => match r2 {
            0 => Ok(FCVT_W_S  { rd: rd_, rs1: r1, rm: rm_ }),
            1 => Ok(FCVT_WU_S { rd: rd_, rs1: r1, rm: rm_ }),
            2 => Ok(FCVT_L_S  { rd: rd_, rs1: r1, rm: rm_ }),
            3 => Ok(FCVT_LU_S { rd: rd_, rs1: r1, rm: rm_ }),
            _ => Err(DecodeError::Unknown { raw, pc }),
        },
        0b1110000 if r2 == 0 && f7 == 0 => Ok(FMV_X_W  { rd: rd_, rs1: r1 }),
        0b1010000 => match f7 {
            0b010 => Ok(FEQ_S    { rd: rd_, rs1: r1, rs2: r2 }),
            0b001 => Ok(FLT_S    { rd: rd_, rs1: r1, rs2: r2 }),
            0b000 => Ok(FLE_S    { rd: rd_, rs1: r1, rs2: r2 }),
            _     => Err(DecodeError::Unknown { raw, pc }),
        },
        0b1110000 if r2 == 0 && f7 == 1 => Ok(FCLASS_S { rd: rd_, rs1: r1 }),
        0b1101000 => match r2 {
            0 => Ok(FCVT_S_W  { rd: rd_, rs1: r1, rm: rm_ }),
            1 => Ok(FCVT_S_WU { rd: rd_, rs1: r1, rm: rm_ }),
            2 => Ok(FCVT_S_L  { rd: rd_, rs1: r1, rm: rm_ }),
            3 => Ok(FCVT_S_LU { rd: rd_, rs1: r1, rm: rm_ }),
            _ => Err(DecodeError::Unknown { raw, pc }),
        },
        0b1111000 if r2 == 0 && f7 == 0 => Ok(FMV_W_X  { rd: rd_, rs1: r1 }),
        // Double-precision
        0b0000001 => Ok(FADD_D  { rd: rd_, rs1: r1, rs2: r2, rm: rm_ }),
        0b0000101 => Ok(FSUB_D  { rd: rd_, rs1: r1, rs2: r2, rm: rm_ }),
        0b0001001 => Ok(FMUL_D  { rd: rd_, rs1: r1, rs2: r2, rm: rm_ }),
        0b0001101 => Ok(FDIV_D  { rd: rd_, rs1: r1, rs2: r2, rm: rm_ }),
        0b0101101 if r2 == 0 => Ok(FSQRT_D  { rd: rd_, rs1: r1, rm: rm_ }),
        0b0010001 => match f7 {
            0b000 => Ok(FSGNJ_D  { rd: rd_, rs1: r1, rs2: r2 }),
            0b001 => Ok(FSGNJN_D { rd: rd_, rs1: r1, rs2: r2 }),
            0b010 => Ok(FSGNJX_D { rd: rd_, rs1: r1, rs2: r2 }),
            _     => Err(DecodeError::Unknown { raw, pc }),
        },
        0b0010101 => match f7 {
            0b000 => Ok(FMIN_D { rd: rd_, rs1: r1, rs2: r2 }),
            0b001 => Ok(FMAX_D { rd: rd_, rs1: r1, rs2: r2 }),
            _     => Err(DecodeError::Unknown { raw, pc }),
        },
        0b0100000 if r2 == 1 => Ok(FCVT_S_D  { rd: rd_, rs1: r1, rm: rm_ }),
        0b0100001 if r2 == 0 => Ok(FCVT_D_S  { rd: rd_, rs1: r1, rm: rm_ }),
        0b1010001 => match f7 {
            0b010 => Ok(FEQ_D    { rd: rd_, rs1: r1, rs2: r2 }),
            0b001 => Ok(FLT_D    { rd: rd_, rs1: r1, rs2: r2 }),
            0b000 => Ok(FLE_D    { rd: rd_, rs1: r1, rs2: r2 }),
            _     => Err(DecodeError::Unknown { raw, pc }),
        },
        0b1110001 if r2 == 0 => match f7 {
            0b001 => Ok(FCLASS_D  { rd: rd_, rs1: r1 }),
            0b000 => Ok(FMV_X_D   { rd: rd_, rs1: r1 }),
            _     => Err(DecodeError::Unknown { raw, pc }),
        },
        0b1100001 => match r2 {
            0 => Ok(FCVT_W_D  { rd: rd_, rs1: r1, rm: rm_ }),
            1 => Ok(FCVT_WU_D { rd: rd_, rs1: r1, rm: rm_ }),
            2 => Ok(FCVT_L_D  { rd: rd_, rs1: r1, rm: rm_ }),
            3 => Ok(FCVT_LU_D { rd: rd_, rs1: r1, rm: rm_ }),
            _ => Err(DecodeError::Unknown { raw, pc }),
        },
        0b1101001 => match r2 {
            0 => Ok(FCVT_D_W  { rd: rd_, rs1: r1, rm: rm_ }),
            1 => Ok(FCVT_D_WU { rd: rd_, rs1: r1, rm: rm_ }),
            2 => Ok(FCVT_D_L  { rd: rd_, rs1: r1, rm: rm_ }),
            3 => Ok(FCVT_D_LU { rd: rd_, rs1: r1, rm: rm_ }),
            _ => Err(DecodeError::Unknown { raw, pc }),
        },
        0b1111001 if r2 == 0 && f7 == 0 => Ok(FMV_D_X   { rd: rd_, rs1: r1 }),
        _ => Err(DecodeError::Unknown { raw, pc }),
    }
}

/// Expand a 16-bit RV-C instruction to a 32-bit equivalent, then call [`decode`].
///
/// Returns `Err(DecodeError::Unknown)` for unrecognised C encodings.
pub fn expand_c(c: u16, pc: u64) -> Result<Instruction, DecodeError> {
    // TODO(phase-0): implement full C-extension expansion (80 variants).
    // See RISC-V spec Volume I, Chapter 26.
    let _ = (c, pc);
    Err(DecodeError::Unimplemented)
}
