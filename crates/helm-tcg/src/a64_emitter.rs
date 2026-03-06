//! AArch64 → TCG emitter.
//!
//! Implements the generated `Decode*Handler` traits to emit [`TcgOp`]
//! sequences into a [`TcgContext`].  This reuses the same `.decode`
//! files as `helm-isa`, ensuring decode logic stays in sync.

use crate::context::TcgContext;
use crate::ir::TcgOp;
use crate::interp::{REG_PC, REG_NZCV};
use helm_core::HelmError;

// ---------------------------------------------------------------------------
// Generated handler traits (trait definitions only — extracted from codegen)
// ---------------------------------------------------------------------------

// The generated files contain both trait defs AND dispatch fns.
// We need the traits at module level but the dispatch fns inside impl blocks.
// Since they share one file, we define the traits manually here to match
// what the codegen produces, and include the dispatch fns inside impls.

/// Handler trait for branch instructions. Auto-generated from aarch64-branch.decode.
pub trait DecodeAarch64BranchHandler {
    fn handle_b(&mut self, insn: u32, imm26: u32) -> Result<(), HelmError>;
    fn handle_bl(&mut self, insn: u32, imm26: u32) -> Result<(), HelmError>;
    fn handle_b_cond(&mut self, insn: u32, imm19: u32, cond: u32) -> Result<(), HelmError>;
    fn handle_cbz(&mut self, insn: u32, sf: u32, imm19: u32, rt: u32) -> Result<(), HelmError>;
    fn handle_cbnz(&mut self, insn: u32, sf: u32, imm19: u32, rt: u32) -> Result<(), HelmError>;
    fn handle_tbz(&mut self, insn: u32, b5: u32, b40: u32, imm14: u32, rt: u32) -> Result<(), HelmError>;
    fn handle_tbnz(&mut self, insn: u32, b5: u32, b40: u32, imm14: u32, rt: u32) -> Result<(), HelmError>;
    fn handle_br(&mut self, insn: u32, rn: u32) -> Result<(), HelmError>;
    fn handle_blr(&mut self, insn: u32, rn: u32) -> Result<(), HelmError>;
    fn handle_ret(&mut self, insn: u32, rn: u32) -> Result<(), HelmError>;
    fn handle_svc(&mut self, insn: u32, imm16: u32) -> Result<(), HelmError>;
    fn handle_hvc(&mut self, insn: u32, imm16: u32) -> Result<(), HelmError>;
    fn handle_brk(&mut self, insn: u32, imm16: u32) -> Result<(), HelmError>;
    fn handle_nop(&mut self, insn: u32) -> Result<(), HelmError>;
}

/// Emitter that translates A64 instructions into TCG op sequences.
pub struct A64TcgEmitter<'a> {
    pub ctx: &'a mut TcgContext,
    pub pc: u64,
    /// Set to true when the current instruction ends the basic block.
    pub end_block: bool,
}

/// Result of translating a single instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslateAction {
    /// Keep translating the next instruction in this block.
    Continue,
    /// This instruction ends the block (branch, syscall).
    EndBlock,
    /// Instruction not handled — fall back to interpretive step().
    Unhandled,
}

// Dispatch function for branch decode (included inside impl block).
// This is the generated code from decode_aarch64_branch_handler.rs,
// but only the dispatch function, not the trait.
impl<'a> A64TcgEmitter<'a> {
    pub fn new(ctx: &'a mut TcgContext, pc: u64) -> Self {
        Self { ctx, pc, end_block: false }
    }
}

impl A64TcgEmitter<'_> {
    #[allow(unused_variables, clippy::unusual_byte_groupings)]
    fn decode_branch_dispatch(&mut self, insn: u32) -> Result<(), HelmError> {
        if insn & 0xfc000000 == 0x14000000 {
            let imm26 = (insn >> 0) & 0x3ffffff;
            return self.handle_b(insn, imm26);
        }
        if insn & 0xfc000000 == 0x94000000 {
            let imm26 = (insn >> 0) & 0x3ffffff;
            return self.handle_bl(insn, imm26);
        }
        if insn & 0xff000010 == 0x54000000 {
            let imm19 = (insn >> 5) & 0x7ffff;
            let cond = (insn >> 0) & 0xf;
            return self.handle_b_cond(insn, imm19, cond);
        }
        if insn & 0x7f000000 == 0x34000000 {
            let sf = (insn >> 31) & 0x1;
            let imm19 = (insn >> 5) & 0x7ffff;
            let rt = (insn >> 0) & 0x1f;
            return self.handle_cbz(insn, sf, imm19, rt);
        }
        if insn & 0x7f000000 == 0x35000000 {
            let sf = (insn >> 31) & 0x1;
            let imm19 = (insn >> 5) & 0x7ffff;
            let rt = (insn >> 0) & 0x1f;
            return self.handle_cbnz(insn, sf, imm19, rt);
        }
        if insn & 0x7f000000 == 0x36000000 {
            let b5 = (insn >> 31) & 0x1;
            let b40 = (insn >> 19) & 0x1f;
            let imm14 = (insn >> 5) & 0x3fff;
            let rt = (insn >> 0) & 0x1f;
            return self.handle_tbz(insn, b5, b40, imm14, rt);
        }
        if insn & 0x7f000000 == 0x37000000 {
            let b5 = (insn >> 31) & 0x1;
            let b40 = (insn >> 19) & 0x1f;
            let imm14 = (insn >> 5) & 0x3fff;
            let rt = (insn >> 0) & 0x1f;
            return self.handle_tbnz(insn, b5, b40, imm14, rt);
        }
        if insn & 0xfffffc1f == 0xd61f0000 {
            let rn = (insn >> 5) & 0x1f;
            return self.handle_br(insn, rn);
        }
        if insn & 0xfffffc1f == 0xd63f0000 {
            let rn = (insn >> 5) & 0x1f;
            return self.handle_blr(insn, rn);
        }
        if insn & 0xfffffc1f == 0xd65f0000 {
            let rn = (insn >> 5) & 0x1f;
            return self.handle_ret(insn, rn);
        }
        if insn & 0xffe0001f == 0xd4000001 {
            let imm16 = (insn >> 5) & 0xffff;
            return self.handle_svc(insn, imm16);
        }
        if insn & 0xffe0001f == 0xd4000002 {
            let imm16 = (insn >> 5) & 0xffff;
            return self.handle_hvc(insn, imm16);
        }
        if insn & 0xffe0001f == 0xd4200000 {
            let imm16 = (insn >> 5) & 0xffff;
            return self.handle_brk(insn, imm16);
        }
        if insn & 0xffffffff == 0xd503201f {
            return self.handle_nop(insn);
        }
        Err(HelmError::Decode { addr: self.pc, reason: format!("unhandled branch/sys {insn:#010x}") })
    }

    /// Translate a single A64 instruction.
    pub fn translate_insn(&mut self, insn: u32) -> TranslateAction {
        self.end_block = false;

        let op0 = (insn >> 25) & 0xF;
        let result = match op0 {
            // DP-immediate: 100x
            0b1000 | 0b1001 => self.translate_dp_imm(insn),
            // Branches: 101x
            0b1010 | 0b1011 => self.decode_branch_dispatch(insn),
            // Load/store: x1x0
            0b0100 | 0b0110 | 0b1100 | 0b1110 => self.translate_ldst(insn),
            // DP-register: x101
            0b0101 | 0b1101 => self.translate_dp_reg(insn),
            // SIMD/FP: not yet
            _ => return TranslateAction::Unhandled,
        };

        match result {
            Ok(()) => {
                if self.end_block {
                    TranslateAction::EndBlock
                } else {
                    TranslateAction::Continue
                }
            }
            Err(_) => TranslateAction::Unhandled,
        }
    }

    // ── DP-immediate translation ────────────────────────────────────

    fn translate_dp_imm(&mut self, insn: u32) -> Result<(), HelmError> {
        let sf = (insn >> 31) & 1;
        let rd = (insn & 0x1F) as u16;
        let rn = ((insn >> 5) & 0x1F) as u16;
        let op_hi = (insn >> 23) & 0x7;

        match op_hi {
            // ADR/ADRP (pc-relative)
            0b000 | 0b001 => {
                let immlo = (insn >> 29) & 3;
                let immhi = sext((insn >> 5) & 0x7FFFF, 19) as u64;
                let imm = ((immhi << 2) | immlo as u64) as i64;
                let is_page = (insn >> 31) & 1 == 1;
                let base = if is_page { self.pc & !0xFFF } else { self.pc };
                let val = if is_page {
                    base.wrapping_add((imm << 12) as u64)
                } else {
                    base.wrapping_add(imm as u64)
                };
                let v = self.ctx.movi(val);
                self.ctx.write_reg(rd, v);
            }
            // ADD/ADDS/SUB/SUBS (immediate)
            0b010 | 0b011 => {
                let op = (insn >> 29) & 3; // 00=ADD, 01=ADDS, 10=SUB, 11=SUBS
                let sh = (insn >> 22) & 3;
                let mut imm12 = ((insn >> 10) & 0xFFF) as u64;
                if sh == 1 { imm12 <<= 12; }

                let src = self.ctx.read_reg(rn);
                let imm_t = self.ctx.movi(imm12);
                let result = if op & 2 == 0 {
                    self.ctx.add(src, imm_t)
                } else {
                    let dst = self.ctx.temp();
                    self.ctx.emit(TcgOp::Sub { dst, a: src, b: imm_t });
                    dst
                };
                // Truncate to 32-bit if sf=0
                let final_val = if sf == 0 {
                    let t = self.ctx.temp();
                    self.ctx.emit(TcgOp::Zext { dst: t, src: result, from_bits: 32 });
                    t
                } else {
                    result
                };
                if rd < 31 { // don't write to XZR
                    self.ctx.write_reg(rd, final_val);
                }
                // TODO: ADDS/SUBS set NZCV flags
            }
            // MOV wide: MOVN(00)/MOVZ(10)/MOVK(11)
            0b100 | 0b101 => {
                let opc = (insn >> 29) & 3;
                let hw = (insn >> 21) & 3;
                let imm16 = ((insn >> 5) & 0xFFFF) as u64;
                let shifted = imm16 << (hw * 16);

                match opc {
                    0b10 => { // MOVZ
                        let v = self.ctx.movi(shifted);
                        self.ctx.write_reg(rd, v);
                    }
                    0b00 => { // MOVN
                        let v = self.ctx.movi(!shifted);
                        self.ctx.write_reg(rd, v);
                    }
                    0b11 => { // MOVK — merge into existing value
                        let old = self.ctx.read_reg(rd);
                        let mask = self.ctx.movi(!(0xFFFF_u64 << (hw * 16)));
                        let masked = self.ctx.temp();
                        self.ctx.emit(TcgOp::And { dst: masked, a: old, b: mask });
                        let bits = self.ctx.movi(shifted);
                        let result = self.ctx.temp();
                        self.ctx.emit(TcgOp::Or { dst: result, a: masked, b: bits });
                        self.ctx.write_reg(rd, result);
                    }
                    _ => return Err(HelmError::Decode { addr: self.pc, reason: "invalid MOV opc".into() }),
                }
            }
            // Logical immediate, bitfield, extract — fall back for now
            _ => return Err(HelmError::Decode { addr: self.pc, reason: "unhandled dp-imm".into() }),
        }
        Ok(())
    }

    // ── Load/store translation ──────────────────────────────────────

    fn translate_ldst(&mut self, insn: u32) -> Result<(), HelmError> {
        let size = (insn >> 30) & 3;
        let rn = ((insn >> 5) & 0x1F) as u16;
        let rt = (insn & 0x1F) as u16;

        // Unsigned offset: size 111001 opc imm12 Rn Rt
        if (insn >> 24) & 0x3F == 0b111001 {
            let opc = (insn >> 22) & 3;
            let imm12 = ((insn >> 10) & 0xFFF) as u64;
            let offset = imm12 << size;

            let base = self.ctx.read_reg(rn);
            let addr = self.ctx.addi(base, offset as i64);
            let sz = (1u8 << size) as u8;

            return match opc {
                0 => { // STR
                    let val = self.ctx.read_reg(rt);
                    self.ctx.store(addr, val, sz);
                    Ok(())
                }
                1 => { // LDR (zero-extend)
                    let val = self.ctx.load(addr, sz);
                    self.ctx.write_reg(rt, val);
                    Ok(())
                }
                2 => { // LDR (sign-extend to 64)
                    let raw = self.ctx.load(addr, sz);
                    let ext = self.ctx.temp();
                    self.ctx.emit(TcgOp::Sext { dst: ext, src: raw, from_bits: sz * 8 });
                    self.ctx.write_reg(rt, ext);
                    Ok(())
                }
                _ => Err(HelmError::Decode { addr: self.pc, reason: "unhandled ldst opc".into() }),
            };
        }

        // Other load/store variants — fall back
        Err(HelmError::Decode { addr: self.pc, reason: "unhandled ldst".into() })
    }

    // ── DP-register translation ─────────────────────────────────────

    fn translate_dp_reg(&mut self, insn: u32) -> Result<(), HelmError> {
        let sf = (insn >> 31) & 1;
        let rd = (insn & 0x1F) as u16;
        let rn = ((insn >> 5) & 0x1F) as u16;
        let rm = ((insn >> 16) & 0x1F) as u16;

        // Add/sub shifted register: sf op 0 01011 shift 0 Rm imm6 Rn Rd
        let top8 = (insn >> 24) & 0xFF;
        if top8 & 0x1F == 0b01011 && (insn >> 21) & 1 == 0 {
            let op = (insn >> 29) & 3;
            let src_n = self.ctx.read_reg(rn);
            let src_m = self.ctx.read_reg(rm);
            // TODO: apply shift to Rm

            let result = if op & 2 == 0 {
                self.ctx.add(src_n, src_m) // ADD
            } else {
                let dst = self.ctx.temp();
                self.ctx.emit(TcgOp::Sub { dst, a: src_n, b: src_m });
                dst // SUB
            };

            let final_val = if sf == 0 {
                let t = self.ctx.temp();
                self.ctx.emit(TcgOp::Zext { dst: t, src: result, from_bits: 32 });
                t
            } else {
                result
            };
            if rd < 31 {
                self.ctx.write_reg(rd, final_val);
            }
            return Ok(());
        }

        // Logical shifted register: sf opc 01010 shift N Rm imm6 Rn Rd
        if top8 & 0x1F == 0b01010 {
            let opc = (insn >> 29) & 3;
            let n_bit = (insn >> 21) & 1;
            let src_n = self.ctx.read_reg(rn);
            let mut src_m = self.ctx.read_reg(rm);

            if n_bit == 1 {
                let t = self.ctx.temp();
                self.ctx.emit(TcgOp::Not { dst: t, src: src_m });
                src_m = t;
            }

            let result = match opc {
                0b00 => { let d = self.ctx.temp(); self.ctx.emit(TcgOp::And { dst: d, a: src_n, b: src_m }); d }
                0b01 => { let d = self.ctx.temp(); self.ctx.emit(TcgOp::Or { dst: d, a: src_n, b: src_m }); d }
                0b10 => { let d = self.ctx.temp(); self.ctx.emit(TcgOp::Xor { dst: d, a: src_n, b: src_m }); d }
                0b11 => { let d = self.ctx.temp(); self.ctx.emit(TcgOp::And { dst: d, a: src_n, b: src_m }); d } // ANDS
                _ => unreachable!(),
            };

            if sf == 0 {
                let t = self.ctx.temp();
                self.ctx.emit(TcgOp::Zext { dst: t, src: result, from_bits: 32 });
                if rd < 31 { self.ctx.write_reg(rd, t); }
            } else {
                if rd < 31 { self.ctx.write_reg(rd, result); }
            }
            return Ok(());
        }

        // Other dp-reg — fall back
        Err(HelmError::Decode { addr: self.pc, reason: "unhandled dp-reg".into() })
    }
}

// Helper: sign-extend a value from `bits` width to i64.
fn sext(val: u32, bits: u32) -> i64 {
    let shift = 32 - bits;
    ((val << shift) as i32 >> shift) as i64
}

// ---------------------------------------------------------------------------
// Branch handler implementation
// ---------------------------------------------------------------------------

impl DecodeAarch64BranchHandler for A64TcgEmitter<'_> {
    fn handle_b(&mut self, _insn: u32, imm26: u32) -> Result<(), HelmError> {
        let offset = sext(imm26, 26) << 2;
        let target = self.pc.wrapping_add(offset as u64);
        self.ctx.emit(TcgOp::GotoTb { target_pc: target });
        self.end_block = true;
        Ok(())
    }

    fn handle_bl(&mut self, _insn: u32, imm26: u32) -> Result<(), HelmError> {
        let offset = sext(imm26, 26) << 2;
        let target = self.pc.wrapping_add(offset as u64);
        let ret_addr = self.ctx.movi(self.pc + 4);
        self.ctx.write_reg(30, ret_addr);
        self.ctx.emit(TcgOp::GotoTb { target_pc: target });
        self.end_block = true;
        Ok(())
    }

    fn handle_b_cond(&mut self, _insn: u32, imm19: u32, _cond: u32) -> Result<(), HelmError> {
        // Conditional branches need NZCV evaluation — fall back for now
        let _ = imm19;
        Err(HelmError::Decode { addr: self.pc, reason: "b.cond TCG not yet implemented".into() })
    }

    fn handle_cbz(&mut self, _insn: u32, sf: u32, imm19: u32, rt: u32) -> Result<(), HelmError> {
        let offset = sext(imm19, 19) << 2;
        let target = self.pc.wrapping_add(offset as u64);
        let fallthrough = self.pc + 4;

        let reg_val = self.ctx.read_reg(rt as u16);
        let zero = self.ctx.movi(0);
        let val = if sf == 0 {
            let t = self.ctx.temp();
            self.ctx.emit(TcgOp::Zext { dst: t, src: reg_val, from_bits: 32 });
            t
        } else {
            reg_val
        };
        let is_zero = self.ctx.temp();
        self.ctx.emit(TcgOp::SetEq { dst: is_zero, a: val, b: zero });

        let taken_label = self.ctx.label();
        self.ctx.emit(TcgOp::BrCond { cond: is_zero, label: taken_label });
        self.ctx.emit(TcgOp::GotoTb { target_pc: fallthrough });
        self.ctx.emit(TcgOp::Label { id: taken_label });
        self.ctx.emit(TcgOp::GotoTb { target_pc: target });
        self.end_block = true;
        Ok(())
    }

    fn handle_cbnz(&mut self, _insn: u32, sf: u32, imm19: u32, rt: u32) -> Result<(), HelmError> {
        let offset = sext(imm19, 19) << 2;
        let target = self.pc.wrapping_add(offset as u64);
        let fallthrough = self.pc + 4;

        let reg_val = self.ctx.read_reg(rt as u16);
        let zero = self.ctx.movi(0);
        let val = if sf == 0 {
            let t = self.ctx.temp();
            self.ctx.emit(TcgOp::Zext { dst: t, src: reg_val, from_bits: 32 });
            t
        } else {
            reg_val
        };
        let is_nonzero = self.ctx.temp();
        self.ctx.emit(TcgOp::SetNe { dst: is_nonzero, a: val, b: zero });

        let taken_label = self.ctx.label();
        self.ctx.emit(TcgOp::BrCond { cond: is_nonzero, label: taken_label });
        self.ctx.emit(TcgOp::GotoTb { target_pc: fallthrough });
        self.ctx.emit(TcgOp::Label { id: taken_label });
        self.ctx.emit(TcgOp::GotoTb { target_pc: target });
        self.end_block = true;
        Ok(())
    }

    fn handle_tbz(&mut self, _insn: u32, b5: u32, b40: u32, imm14: u32, rt: u32) -> Result<(), HelmError> {
        let bit = (b5 << 5) | b40;
        let offset = sext(imm14, 14) << 2;
        let target = self.pc.wrapping_add(offset as u64);
        let fallthrough = self.pc + 4;

        let reg_val = self.ctx.read_reg(rt as u16);
        let bit_mask = self.ctx.movi(1u64 << bit);
        let masked = self.ctx.temp();
        self.ctx.emit(TcgOp::And { dst: masked, a: reg_val, b: bit_mask });
        let zero = self.ctx.movi(0);
        let is_zero = self.ctx.temp();
        self.ctx.emit(TcgOp::SetEq { dst: is_zero, a: masked, b: zero });

        let taken_label = self.ctx.label();
        self.ctx.emit(TcgOp::BrCond { cond: is_zero, label: taken_label });
        self.ctx.emit(TcgOp::GotoTb { target_pc: fallthrough });
        self.ctx.emit(TcgOp::Label { id: taken_label });
        self.ctx.emit(TcgOp::GotoTb { target_pc: target });
        self.end_block = true;
        Ok(())
    }

    fn handle_tbnz(&mut self, _insn: u32, b5: u32, b40: u32, imm14: u32, rt: u32) -> Result<(), HelmError> {
        let bit = (b5 << 5) | b40;
        let offset = sext(imm14, 14) << 2;
        let target = self.pc.wrapping_add(offset as u64);
        let fallthrough = self.pc + 4;

        let reg_val = self.ctx.read_reg(rt as u16);
        let bit_mask = self.ctx.movi(1u64 << bit);
        let masked = self.ctx.temp();
        self.ctx.emit(TcgOp::And { dst: masked, a: reg_val, b: bit_mask });
        let zero = self.ctx.movi(0);
        let is_nonzero = self.ctx.temp();
        self.ctx.emit(TcgOp::SetNe { dst: is_nonzero, a: masked, b: zero });

        let taken_label = self.ctx.label();
        self.ctx.emit(TcgOp::BrCond { cond: is_nonzero, label: taken_label });
        self.ctx.emit(TcgOp::GotoTb { target_pc: fallthrough });
        self.ctx.emit(TcgOp::Label { id: taken_label });
        self.ctx.emit(TcgOp::GotoTb { target_pc: target });
        self.end_block = true;
        Ok(())
    }

    fn handle_br(&mut self, _insn: u32, rn: u32) -> Result<(), HelmError> {
        let target = self.ctx.read_reg(rn as u16);
        self.ctx.emit(TcgOp::WriteReg { reg_id: REG_PC, src: target });
        self.ctx.emit(TcgOp::ExitTb);
        self.end_block = true;
        Ok(())
    }

    fn handle_blr(&mut self, _insn: u32, rn: u32) -> Result<(), HelmError> {
        let target = self.ctx.read_reg(rn as u16);
        let ret_addr = self.ctx.movi(self.pc + 4);
        self.ctx.write_reg(30, ret_addr);
        self.ctx.emit(TcgOp::WriteReg { reg_id: REG_PC, src: target });
        self.ctx.emit(TcgOp::ExitTb);
        self.end_block = true;
        Ok(())
    }

    fn handle_ret(&mut self, _insn: u32, rn: u32) -> Result<(), HelmError> {
        let target = self.ctx.read_reg(rn as u16);
        self.ctx.emit(TcgOp::WriteReg { reg_id: REG_PC, src: target });
        self.ctx.emit(TcgOp::ExitTb);
        self.end_block = true;
        Ok(())
    }

    fn handle_svc(&mut self, _insn: u32, _imm16: u32) -> Result<(), HelmError> {
        let nr = self.ctx.read_reg(8);
        self.ctx.emit(TcgOp::Syscall { nr });
        self.end_block = true;
        Ok(())
    }

    fn handle_hvc(&mut self, _insn: u32, _imm16: u32) -> Result<(), HelmError> {
        Err(HelmError::Decode { addr: self.pc, reason: "HVC not supported in SE mode".into() })
    }

    fn handle_brk(&mut self, _insn: u32, _imm16: u32) -> Result<(), HelmError> {
        self.ctx.emit(TcgOp::ExitTb);
        self.end_block = true;
        Ok(())
    }

    fn handle_nop(&mut self, _insn: u32) -> Result<(), HelmError> {
        Ok(())
    }
}
