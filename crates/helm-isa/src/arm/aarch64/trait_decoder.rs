//! Decoder trait implementation for AArch64.
//!
//! Produces `DecodedInsn` from raw A64 instruction bytes. Uses the
//! existing decode tree for pattern matching, then classifies into
//! `InsnClass` and `InsnFlags`.

use helm_core::decode::Decoder;
use helm_core::insn::{DecodedInsn, InsnClass, InsnFlags};
use helm_core::types::Addr;
use helm_core::HelmError;

/// AArch64 decoder implementing the `Decoder` trait.
///
/// Produces `DecodedInsn` with instruction classification for timing
/// and pipeline modelling. All A64 instructions are 4 bytes.
pub struct Aarch64TraitDecoder;

impl Decoder for Aarch64TraitDecoder {
    fn decode(&self, pc: Addr, bytes: &[u8]) -> Result<DecodedInsn, HelmError> {
        if bytes.len() < 4 {
            return Err(HelmError::Decode {
                addr: pc,
                reason: "need 4 bytes for A64".into(),
            });
        }

        let insn = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let mut di = DecodedInsn {
            pc,
            len: 4,
            ..DecodedInsn::default()
        };
        di.encoding_bytes[..4].copy_from_slice(&bytes[..4]);

        // Classify using top-level op0 field (bits [28:25])
        let op0 = (insn >> 25) & 0xF;
        match op0 {
            // Data processing — immediate
            0b1000 | 0b1001 => {
                di.class = classify_dp_imm(insn);
                let rd = (insn & 0x1F) as u16;
                let rn = ((insn >> 5) & 0x1F) as u16;
                di.dst_regs[0] = rd;
                di.dst_count = 1;
                di.src_regs[0] = rn;
                di.src_count = 1;
                if di.class == InsnClass::IntAlu {
                    di.flags |= InsnFlags::SETS_FLAGS;
                }
            }
            // Branch, exception, system
            0b1010 | 0b1011 => {
                classify_branch_sys(insn, &mut di);
            }
            // Loads and stores
            0b0100 | 0b0110 | 0b1100 | 0b1110 => {
                classify_ldst(insn, &mut di);
            }
            // Data processing — register
            0b0101 | 0b1101 => {
                di.class = classify_dp_reg(insn);
                let rd = (insn & 0x1F) as u16;
                let rn = ((insn >> 5) & 0x1F) as u16;
                let rm = ((insn >> 16) & 0x1F) as u16;
                di.dst_regs[0] = rd;
                di.dst_count = 1;
                di.src_regs[0] = rn;
                di.src_regs[1] = rm;
                di.src_count = 2;
            }
            // SIMD/FP
            0b0111 | 0b1111 => {
                di.class = InsnClass::SimdAlu;
                di.flags |= InsnFlags::SIMD;
            }
            _ => {
                di.class = InsnClass::Nop;
            }
        }

        Ok(di)
    }

    fn min_insn_size(&self) -> usize {
        4
    }
}

fn classify_dp_imm(insn: u32) -> InsnClass {
    let op0 = (insn >> 23) & 0x7;
    match op0 {
        0b010 | 0b011 => InsnClass::IntAlu, // ADD/SUB imm
        0b100 => InsnClass::IntAlu,         // Logical imm
        0b101 => InsnClass::IntAlu,         // MOVZ/MOVN/MOVK
        0b110 => InsnClass::IntAlu,         // Bitfield
        0b111 => InsnClass::IntAlu,         // Extract
        _ => InsnClass::IntAlu,
    }
}

fn classify_branch_sys(insn: u32, di: &mut DecodedInsn) {
    let op0 = (insn >> 29) & 0x7;
    match op0 {
        0b000 => {
            di.class = InsnClass::Branch;
            di.flags |= InsnFlags::BRANCH;
        }
        0b001 => {
            di.class = InsnClass::CondBranch;
            di.flags |= InsnFlags::BRANCH | InsnFlags::COND;
        }
        0b010 => {
            di.class = InsnClass::Branch;
            di.flags |= InsnFlags::BRANCH;
        }
        0b100 => {
            // BL
            di.class = InsnClass::Call;
            di.flags |= InsnFlags::BRANCH | InsnFlags::CALL;
        }
        0b101 => {
            // CBZ/CBNZ
            di.class = InsnClass::CondBranch;
            di.flags |= InsnFlags::BRANCH | InsnFlags::COND;
        }
        0b110 | 0b111 => {
            // Branches, exception, system, unconditional branch (register)
            // Distinguish by bits [25:24]
            let op1_25 = (insn >> 25) & 1;
            let op1_24_22 = (insn >> 22) & 0x7;

            if op0 == 0b110 && op1_25 == 1 {
                // Unconditional branch (register): BR, BLR, RET
                // bits [31:25] = 1101011
                let opc = (insn >> 21) & 0xF;
                match opc {
                    0b0000 => {
                        di.class = InsnClass::IndBranch;
                        di.flags |= InsnFlags::BRANCH;
                    }
                    0b0001 => {
                        di.class = InsnClass::Call;
                        di.flags |= InsnFlags::BRANCH | InsnFlags::CALL;
                    }
                    0b0010 => {
                        di.class = InsnClass::Return;
                        di.flags |= InsnFlags::BRANCH | InsnFlags::RETURN;
                    }
                    _ => {
                        di.class = InsnClass::IndBranch;
                        di.flags |= InsnFlags::BRANCH;
                    }
                }
            } else if op0 == 0b110 && op1_25 == 0 {
                // Exception generation (SVC/HVC/SMC/BRK) or system (MSR/MRS)
                let l_op0 = (insn >> 22) & 0x7;
                if l_op0 == 0b000 {
                    // Exception: SVC, HVC, SMC, BRK
                    let opc = (insn >> 21) & 0x7;
                    match opc {
                        0b000 => {
                            di.class = InsnClass::Syscall;
                            di.flags |= InsnFlags::SYSCALL;
                        }
                        0b001 => {
                            di.class = InsnClass::Syscall;
                            di.flags |= InsnFlags::HV_CALL;
                        }
                        0b010 => {
                            di.class = InsnClass::Syscall;
                            di.flags |= InsnFlags::TRAP; // SMC
                        }
                        0b110 => {
                            di.class = InsnClass::Nop;
                            di.flags |= InsnFlags::TRAP; // BRK
                        }
                        _ => {
                            di.class = InsnClass::Nop;
                        }
                    }
                } else {
                    // System: MSR/MRS, barriers, hints
                    di.class = InsnClass::SysRegAccess;
                    di.flags |= InsnFlags::SYSREG;
                }
            } else {
                // TBZ/TBNZ (op0=0b111)
                di.class = InsnClass::CondBranch;
                di.flags |= InsnFlags::BRANCH | InsnFlags::COND;
            }
        }
        _ => {
            di.class = InsnClass::Nop;
        }
    }
}

fn classify_ldst(insn: u32, di: &mut DecodedInsn) {
    let op0 = (insn >> 28) & 0x3;
    let opc = (insn >> 22) & 0x3;
    let rn = ((insn >> 5) & 0x1F) as u16;
    let rt = (insn & 0x1F) as u16;

    di.src_regs[0] = rn; // base register
    di.src_count = 1;

    if op0 & 1 == 0 {
        // Load/store pairs, exclusive, etc.
        let op2 = (insn >> 23) & 0x7;
        match op2 {
            0b010 | 0b011 | 0b110 | 0b111 => {
                // LDP/STP
                let rt2 = ((insn >> 10) & 0x1F) as u16;
                let is_load = opc & 1 != 0;
                if is_load {
                    di.class = InsnClass::LoadPair;
                    di.flags |= InsnFlags::LOAD | InsnFlags::PAIR | InsnFlags::MULTI_MEM;
                    di.dst_regs[0] = rt;
                    di.dst_regs[1] = rt2;
                    di.dst_count = 2;
                    di.mem_count = 2;
                } else {
                    di.class = InsnClass::StorePair;
                    di.flags |= InsnFlags::STORE | InsnFlags::PAIR | InsnFlags::MULTI_MEM;
                    di.src_regs[1] = rt;
                    di.src_regs[2] = rt2;
                    di.src_count = 3;
                    di.mem_count = 2;
                }
            }
            0b000 | 0b001 => {
                // Exclusive loads/stores
                di.class = InsnClass::Atomic;
                di.flags |= InsnFlags::ATOMIC | InsnFlags::LOAD | InsnFlags::STORE;
                di.dst_regs[0] = rt;
                di.dst_count = 1;
                di.mem_count = 1;
            }
            _ => {
                di.class = InsnClass::Load;
                di.flags |= InsnFlags::LOAD;
                di.dst_regs[0] = rt;
                di.dst_count = 1;
                di.mem_count = 1;
            }
        }
    } else {
        // Single-register loads/stores
        let is_load = opc & 1 != 0;
        if is_load {
            di.class = InsnClass::Load;
            di.flags |= InsnFlags::LOAD;
            di.dst_regs[0] = rt;
            di.dst_count = 1;
        } else {
            di.class = InsnClass::Store;
            di.flags |= InsnFlags::STORE;
            di.src_regs[1] = rt;
            di.src_count = 2;
        }
        di.mem_count = 1;
    }
}

fn classify_dp_reg(insn: u32) -> InsnClass {
    // Data processing (register): top bits [28:25] = x1x1
    // Sub-groups distinguished by bits [28:24] and [31:29]:
    //   11011 = Data-processing (3 source): MUL, MADD, SMADDL, etc.
    //   x1011 = Add/subtract (shifted/extended register): ADD, SUB, etc.
    //   01010 = Logical (shifted register): AND, ORR, EOR, etc.
    //   11010 = Data-processing (2 source): UDIV, SDIV, CRC, etc.
    //   11010 + [31:30]=11 = Data-processing (1 source): RBIT, CLZ, REV
    let op_28_24 = (insn >> 24) & 0x1F;
    match op_28_24 {
        0b11011 => {
            // 3-source: MUL/MADD/MSUB/SMADDL/UMADDL
            InsnClass::IntMul
        }
        0b11010 => {
            // 2-source or 1-source
            let opcode = (insn >> 10) & 0x3F;
            match opcode {
                0b000010 | 0b000011 => InsnClass::IntDiv, // UDIV/SDIV
                _ => InsnClass::IntAlu,
            }
        }
        _ => InsnClass::IntAlu, // ADD/SUB, logical, etc.
    }
}
