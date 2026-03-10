//! RISC-V 64-bit decoder implementing Decoder trait.
//!
//! Handles RV64I base integer instructions + C extension (16-bit).

use helm_core::decode::Decoder;
use helm_core::insn::{DecodedInsn, InsnClass, InsnFlags};
use helm_core::types::Addr;
use helm_core::HelmError;

/// RV64 decoder producing DecodedInsn.
pub struct Rv64Decoder;

impl Decoder for Rv64Decoder {
    fn decode(&self, pc: Addr, bytes: &[u8]) -> Result<DecodedInsn, HelmError> {
        if bytes.len() < 2 {
            return Err(HelmError::Decode {
                addr: pc,
                reason: "need at least 2 bytes".into(),
            });
        }

        // Check for compressed instruction (C extension): bits [1:0] != 0b11
        let is_compressed = bytes[0] & 0x3 != 0x3;

        if is_compressed {
            decode_compressed(pc, bytes)
        } else {
            if bytes.len() < 4 {
                return Err(HelmError::Decode {
                    addr: pc,
                    reason: "need 4 bytes for standard RV instruction".into(),
                });
            }
            decode_standard(pc, bytes)
        }
    }

    fn min_insn_size(&self) -> usize {
        2 // C extension
    }
}

fn decode_standard(pc: Addr, bytes: &[u8]) -> Result<DecodedInsn, HelmError> {
    let insn = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let opcode = insn & 0x7F;
    let rd = ((insn >> 7) & 0x1F) as u16;
    let rs1 = ((insn >> 15) & 0x1F) as u16;
    let rs2 = ((insn >> 20) & 0x1F) as u16;
    let funct3 = (insn >> 12) & 0x7;

    let mut di = DecodedInsn {
        pc,
        len: 4,
        ..DecodedInsn::default()
    };
    di.encoding_bytes[..4].copy_from_slice(&bytes[..4]);

    match opcode {
        // R-type: ADD, SUB, SLL, SLT, SLTU, XOR, SRL, SRA, OR, AND
        0b0110011 => {
            let funct7 = (insn >> 25) & 0x7F;
            di.class = if funct7 == 1 {
                // M extension: MUL, MULH, DIV, REM
                if funct3 >= 4 { InsnClass::IntDiv } else { InsnClass::IntMul }
            } else {
                InsnClass::IntAlu
            };
            di.dst_regs[0] = rd;
            di.dst_count = 1;
            di.src_regs[0] = rs1;
            di.src_regs[1] = rs2;
            di.src_count = 2;
        }
        // R-type 64-bit: ADDW, SUBW, etc.
        0b0111011 => {
            di.class = InsnClass::IntAlu;
            di.dst_regs[0] = rd;
            di.dst_count = 1;
            di.src_regs[0] = rs1;
            di.src_regs[1] = rs2;
            di.src_count = 2;
        }
        // I-type ALU: ADDI, SLTI, XORI, ORI, ANDI, SLLI, SRLI, SRAI
        0b0010011 => {
            di.class = InsnClass::IntAlu;
            di.dst_regs[0] = rd;
            di.dst_count = 1;
            di.src_regs[0] = rs1;
            di.src_count = 1;
            di.imm = ((insn as i32) >> 20) as i64;
        }
        // I-type ALU 32-bit: ADDIW, SLLIW, etc.
        0b0011011 => {
            di.class = InsnClass::IntAlu;
            di.dst_regs[0] = rd;
            di.dst_count = 1;
            di.src_regs[0] = rs1;
            di.src_count = 1;
        }
        // Load: LB, LH, LW, LD, LBU, LHU, LWU
        0b0000011 => {
            di.class = InsnClass::Load;
            di.flags |= InsnFlags::LOAD;
            di.dst_regs[0] = rd;
            di.dst_count = 1;
            di.src_regs[0] = rs1;
            di.src_count = 1;
            di.mem_count = 1;
        }
        // Store: SB, SH, SW, SD
        0b0100011 => {
            di.class = InsnClass::Store;
            di.flags |= InsnFlags::STORE;
            di.src_regs[0] = rs1;
            di.src_regs[1] = rs2;
            di.src_count = 2;
            di.mem_count = 1;
        }
        // Branch: BEQ, BNE, BLT, BGE, BLTU, BGEU
        0b1100011 => {
            di.class = InsnClass::CondBranch;
            di.flags |= InsnFlags::BRANCH | InsnFlags::COND;
            di.src_regs[0] = rs1;
            di.src_regs[1] = rs2;
            di.src_count = 2;
        }
        // JAL
        0b1101111 => {
            di.class = InsnClass::Call;
            di.flags |= InsnFlags::BRANCH | InsnFlags::CALL;
            di.dst_regs[0] = rd;
            di.dst_count = 1;
        }
        // JALR
        0b1100111 => {
            di.class = if rd == 0 {
                InsnClass::Return // JALR x0, rs1, 0 = RET
            } else {
                InsnClass::IndBranch
            };
            di.flags |= InsnFlags::BRANCH;
            if rd == 0 { di.flags |= InsnFlags::RETURN; }
            di.dst_regs[0] = rd;
            di.dst_count = 1;
            di.src_regs[0] = rs1;
            di.src_count = 1;
        }
        // LUI
        0b0110111 => {
            di.class = InsnClass::IntAlu;
            di.dst_regs[0] = rd;
            di.dst_count = 1;
            di.imm = ((insn & 0xFFFFF000) as i32) as i64;
        }
        // AUIPC
        0b0010111 => {
            di.class = InsnClass::IntAlu;
            di.dst_regs[0] = rd;
            di.dst_count = 1;
        }
        // SYSTEM: ECALL, EBREAK, CSR*
        0b1110011 => {
            if funct3 == 0 {
                let imm = (insn >> 20) & 0xFFF;
                match imm {
                    0 => {
                        di.class = InsnClass::Syscall;
                        di.flags |= InsnFlags::SYSCALL;
                    }
                    1 => {
                        di.class = InsnClass::Nop;
                        di.flags |= InsnFlags::TRAP; // EBREAK
                    }
                    _ => {
                        di.class = InsnClass::SysRegAccess;
                        di.flags |= InsnFlags::PRIVILEGED;
                    }
                }
            } else {
                // CSR instructions
                di.class = InsnClass::SysRegAccess;
                di.flags |= InsnFlags::SYSREG;
                di.dst_regs[0] = rd;
                di.dst_count = 1;
                di.src_regs[0] = rs1;
                di.src_count = 1;
            }
        }
        // AMO (A extension)
        0b0101111 => {
            di.class = InsnClass::Atomic;
            di.flags |= InsnFlags::ATOMIC | InsnFlags::LOAD | InsnFlags::STORE;
            di.dst_regs[0] = rd;
            di.dst_count = 1;
            di.src_regs[0] = rs1;
            di.src_regs[1] = rs2;
            di.src_count = 2;
            di.mem_count = 1;
        }
        // FENCE
        0b0001111 => {
            di.class = InsnClass::Fence;
            di.flags |= InsnFlags::FENCE;
        }
        _ => {
            di.class = InsnClass::Nop;
        }
    }

    Ok(di)
}

fn decode_compressed(pc: Addr, bytes: &[u8]) -> Result<DecodedInsn, HelmError> {
    let mut di = DecodedInsn {
        pc,
        len: 2,
        class: InsnClass::IntAlu,
        ..DecodedInsn::default()
    };
    di.encoding_bytes[..2].copy_from_slice(&bytes[..2]);

    // Compressed instructions are complex to decode fully.
    // For the stub, classify based on quadrant (bits [1:0]).
    let quadrant = bytes[0] & 0x3;
    let funct3 = (bytes[1] >> 5) & 0x7;

    match quadrant {
        0b00 => {
            // C.ADDI4SPN, C.LW, C.SW, etc.
            match funct3 {
                0b010 | 0b011 | 0b110 => {
                    di.class = InsnClass::Load;
                    di.flags |= InsnFlags::LOAD;
                    di.mem_count = 1;
                }
                0b101 | 0b111 => {
                    di.class = InsnClass::Store;
                    di.flags |= InsnFlags::STORE;
                    di.mem_count = 1;
                }
                _ => di.class = InsnClass::IntAlu,
            }
        }
        0b01 => {
            // C.ADDI, C.JAL, C.LI, C.LUI, C.BEQZ, C.BNEZ, etc.
            match funct3 {
                0b001 | 0b101 => {
                    di.class = InsnClass::Branch;
                    di.flags |= InsnFlags::BRANCH;
                }
                0b110 | 0b111 => {
                    di.class = InsnClass::CondBranch;
                    di.flags |= InsnFlags::BRANCH | InsnFlags::COND;
                }
                _ => di.class = InsnClass::IntAlu,
            }
        }
        0b10 => {
            // C.SLLI, C.LWSP, C.SWSP, C.JR, C.MV, C.ADD, C.JALR
            match funct3 {
                0b010 | 0b011 => {
                    di.class = InsnClass::Load;
                    di.flags |= InsnFlags::LOAD;
                    di.mem_count = 1;
                }
                0b110 | 0b111 => {
                    di.class = InsnClass::Store;
                    di.flags |= InsnFlags::STORE;
                    di.mem_count = 1;
                }
                0b100 => {
                    // C.JR / C.JALR / C.MV / C.ADD
                    di.class = InsnClass::IndBranch;
                    di.flags |= InsnFlags::BRANCH;
                }
                _ => di.class = InsnClass::IntAlu,
            }
        }
        _ => {} // quadrant 0b11 is standard (shouldn't reach here)
    }

    Ok(di)
}
