//! RISC-V 64-bit executor implementing Executor trait.
//!
//! Handles RV64I base integer instructions. Enough for simple programs.

use helm_core::cpu::CpuState;
use helm_core::exec::Executor;
use helm_core::insn::{DecodedInsn, ExceptionInfo, ExecOutcome, InsnFlags, MemAccessInfo};
use helm_core::mem::MemoryAccess;

/// RV64 executor handling base integer instructions.
pub struct Rv64Executor;

impl Rv64Executor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Rv64Executor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor for Rv64Executor {
    fn execute(
        &mut self,
        insn: &DecodedInsn,
        cpu: &mut dyn CpuState,
        mem: &mut dyn MemoryAccess,
    ) -> ExecOutcome {
        let pc = cpu.pc();

        // Check for compressed instruction
        if insn.len == 2 {
            // Compressed instructions not fully implemented yet
            return ExecOutcome {
                next_pc: pc + 2,
                ..ExecOutcome::default()
            };
        }

        let raw = u32::from_le_bytes([
            insn.encoding_bytes[0],
            insn.encoding_bytes[1],
            insn.encoding_bytes[2],
            insn.encoding_bytes[3],
        ]);

        let opcode = raw & 0x7F;
        let rd = ((raw >> 7) & 0x1F) as u16;
        let rs1 = ((raw >> 15) & 0x1F) as u16;
        let rs2 = ((raw >> 20) & 0x1F) as u16;
        let funct3 = (raw >> 12) & 0x7;
        let funct7 = (raw >> 25) & 0x7F;

        let mut outcome = ExecOutcome {
            next_pc: pc + 4,
            ..ExecOutcome::default()
        };

        match opcode {
            // R-type ALU
            0b0110011 => {
                let a = cpu.gpr(rs1);
                let b = cpu.gpr(rs2);
                let result = match (funct3, funct7) {
                    (0, 0) => a.wrapping_add(b),                    // ADD
                    (0, 0x20) => a.wrapping_sub(b),                 // SUB
                    (1, 0) => a << (b & 63),                        // SLL
                    (2, 0) => ((a as i64) < (b as i64)) as u64,     // SLT
                    (3, 0) => (a < b) as u64,                       // SLTU
                    (4, 0) => a ^ b,                                // XOR
                    (5, 0) => a >> (b & 63),                        // SRL
                    (5, 0x20) => ((a as i64) >> (b & 63)) as u64,   // SRA
                    (6, 0) => a | b,                                // OR
                    (7, 0) => a & b,                                // AND
                    // M extension
                    (0, 1) => a.wrapping_mul(b),                    // MUL
                    (4, 1) => if b == 0 { u64::MAX } else { a / b },// DIV (unsigned)
                    (5, 1) => {                                      // DIVU
                        if b == 0 { u64::MAX }
                        else { ((a as i64).wrapping_div(b as i64)) as u64 }
                    }
                    (6, 1) => if b == 0 { a } else { a % b },      // REM
                    _ => 0,
                };
                cpu.set_gpr(rd, result);
            }
            // I-type ALU
            0b0010011 => {
                let a = cpu.gpr(rs1);
                let imm = ((raw as i32) >> 20) as i64 as u64;
                let shamt = (raw >> 20) & 0x3F;
                let result = match funct3 {
                    0 => a.wrapping_add(imm),                       // ADDI
                    1 => a << shamt,                                // SLLI
                    2 => ((a as i64) < (imm as i64)) as u64,        // SLTI
                    3 => (a < imm) as u64,                          // SLTIU
                    4 => a ^ imm,                                   // XORI
                    5 => {
                        if funct7 & 0x20 != 0 {
                            ((a as i64) >> shamt) as u64             // SRAI
                        } else {
                            a >> shamt                               // SRLI
                        }
                    }
                    6 => a | imm,                                   // ORI
                    7 => a & imm,                                   // ANDI
                    _ => 0,
                };
                cpu.set_gpr(rd, result);
            }
            // Load
            0b0000011 => {
                let base = cpu.gpr(rs1);
                let imm = ((raw as i32) >> 20) as i64;
                let addr = base.wrapping_add(imm as u64);
                let val = match funct3 {
                    0 => mem.read(addr, 1).unwrap_or(0) as i8 as i64 as u64,  // LB
                    1 => mem.read(addr, 2).unwrap_or(0) as i16 as i64 as u64, // LH
                    2 => mem.read(addr, 4).unwrap_or(0) as i32 as i64 as u64, // LW
                    3 => mem.read(addr, 8).unwrap_or(0),                       // LD
                    4 => mem.read(addr, 1).unwrap_or(0) & 0xFF,               // LBU
                    5 => mem.read(addr, 2).unwrap_or(0) & 0xFFFF,             // LHU
                    6 => mem.read(addr, 4).unwrap_or(0) & 0xFFFF_FFFF,        // LWU
                    _ => 0,
                };
                cpu.set_gpr(rd, val);
                outcome.mem_accesses[0] = MemAccessInfo {
                    addr,
                    size: [1, 2, 4, 8, 1, 2, 4, 0][funct3 as usize],
                    is_write: false,
                };
                outcome.mem_access_count = 1;
            }
            // Store
            0b0100011 => {
                let base = cpu.gpr(rs1);
                let imm = (((raw >> 25) & 0x7F) << 5 | ((raw >> 7) & 0x1F)) as i32;
                let imm = ((imm << 20) >> 20) as i64; // sign-extend from 12 bits
                let addr = base.wrapping_add(imm as u64);
                let val = cpu.gpr(rs2);
                let size = match funct3 {
                    0 => 1, // SB
                    1 => 2, // SH
                    2 => 4, // SW
                    3 => 8, // SD
                    _ => 0,
                };
                if size > 0 {
                    let _ = mem.write(addr, size, val);
                }
                outcome.mem_accesses[0] = MemAccessInfo {
                    addr,
                    size: size as u8,
                    is_write: true,
                };
                outcome.mem_access_count = 1;
            }
            // Branch
            0b1100011 => {
                let a = cpu.gpr(rs1);
                let b = cpu.gpr(rs2);
                let taken = match funct3 {
                    0 => a == b,                                    // BEQ
                    1 => a != b,                                    // BNE
                    4 => (a as i64) < (b as i64),                   // BLT
                    5 => (a as i64) >= (b as i64),                  // BGE
                    6 => a < b,                                     // BLTU
                    7 => a >= b,                                    // BGEU
                    _ => false,
                };
                if taken {
                    let imm = decode_b_imm(raw);
                    outcome.next_pc = pc.wrapping_add(imm as u64);
                }
                outcome.branch_taken = taken;
            }
            // JAL
            0b1101111 => {
                cpu.set_gpr(rd, pc + 4); // link
                let imm = decode_j_imm(raw);
                outcome.next_pc = pc.wrapping_add(imm as u64);
                outcome.branch_taken = true;
            }
            // JALR
            0b1100111 => {
                let base = cpu.gpr(rs1);
                let imm = ((raw as i32) >> 20) as i64;
                cpu.set_gpr(rd, pc + 4);
                outcome.next_pc = base.wrapping_add(imm as u64) & !1;
                outcome.branch_taken = true;
            }
            // LUI
            0b0110111 => {
                let imm = (raw & 0xFFFFF000) as i32 as i64 as u64;
                cpu.set_gpr(rd, imm);
            }
            // AUIPC
            0b0010111 => {
                let imm = (raw & 0xFFFFF000) as i32 as i64 as u64;
                cpu.set_gpr(rd, pc.wrapping_add(imm));
            }
            // SYSTEM
            0b1110011 => {
                if funct3 == 0 {
                    let imm = (raw >> 20) & 0xFFF;
                    if imm == 0 {
                        // ECALL
                        outcome.exception = Some(ExceptionInfo {
                            class: 8 + cpu.privilege_level() as u32,
                            iss: 0,
                            vaddr: 0,
                            target_el: 3, // Machine mode
                        });
                    }
                    // EBREAK, MRET, SRET — not implemented in stub
                }
                // CSR instructions not implemented in stub
            }
            // FENCE
            0b0001111 => {} // NOP
            _ => {} // Unknown — NOP
        }

        outcome
    }
}

/// Decode B-type immediate (branch offset).
fn decode_b_imm(raw: u32) -> i64 {
    let imm12 = (raw >> 31) & 1;
    let imm10_5 = (raw >> 25) & 0x3F;
    let imm4_1 = (raw >> 8) & 0xF;
    let imm11 = (raw >> 7) & 1;
    let imm = (imm12 << 12) | (imm11 << 11) | (imm10_5 << 5) | (imm4_1 << 1);
    // Sign-extend from bit 12
    ((imm as i32) << 19 >> 19) as i64
}

/// Decode J-type immediate (JAL offset).
fn decode_j_imm(raw: u32) -> i64 {
    let imm20 = (raw >> 31) & 1;
    let imm10_1 = (raw >> 21) & 0x3FF;
    let imm11 = (raw >> 20) & 1;
    let imm19_12 = (raw >> 12) & 0xFF;
    let imm = (imm20 << 20) | (imm19_12 << 12) | (imm11 << 11) | (imm10_1 << 1);
    ((imm as i32) << 11 >> 11) as i64
}
