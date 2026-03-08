//! AArch64 → TCG emitter.
//!
//! Implements the generated `Decode*Handler` traits to emit [`TcgOp`]
//! sequences into a [`TcgContext`].  Decode dispatch is auto-generated
//! from the same `.decode` files used by `helm-isa`, ensuring the two
//! backends stay in lock-step.

#![allow(clippy::unusual_byte_groupings, clippy::identity_op)]

use crate::context::TcgContext;
use crate::ir::{TcgOp, TcgTemp};
use crate::interp::{REG_NZCV, REG_PC, REG_SP};
use helm_core::HelmError;

// ── Generated handler traits (module-level) ─────────────────────────────────
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_branch_trait.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_dp_imm_trait.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_dp_reg_trait.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_ldst_trait.rs"));

/// Emitter that translates A64 instructions into TCG op sequences.
pub struct A64TcgEmitter<'a> {
    /// TCG context accumulating the op stream.
    pub ctx: &'a mut TcgContext,
    /// Guest PC of the instruction being translated.
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

// ═══════════════════════════════════════════════════════════════════
// Construction + top-level dispatch + generated dispatch functions
// ═══════════════════════════════════════════════════════════════════

impl<'a> A64TcgEmitter<'a> {
    /// Create a new emitter for one instruction at `pc`.
    pub fn new(ctx: &'a mut TcgContext, pc: u64) -> Self {
        Self { ctx, pc, end_block: false }
    }
}

// ── Generated dispatch functions (each wrapped in its own impl block) ────────
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_branch_dispatch.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_dp_imm_dispatch.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_dp_reg_dispatch.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_ldst_dispatch.rs"));

impl A64TcgEmitter<'_> {
    /// Translate a single A64 instruction.
    pub fn translate_insn(&mut self, insn: u32) -> TranslateAction {
        self.end_block = false;

        let op0 = (insn >> 25) & 0xF;
        let result = match op0 {
            0b1000 | 0b1001 => self.decode_aarch64_dp_imm_dispatch(insn),
            0b1010 | 0b1011 => self.decode_aarch64_branch_dispatch(insn),
            0b0100 | 0b0110 | 0b1100 | 0b1110 => self.decode_aarch64_ldst_dispatch(insn),
            0b0101 | 0b1101 => self.decode_aarch64_dp_reg_dispatch(insn),
            _ => return TranslateAction::Unhandled,
        };

        match result {
            Ok(()) => {
                if self.end_block { TranslateAction::EndBlock } else { TranslateAction::Continue }
            }
            Err(_) => TranslateAction::Unhandled,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════

/// Sign-extend a `bits`-wide value to i64.
fn sext(val: u32, bits: u32) -> i64 {
    let shift = 32 - bits;
    ((val << shift) as i32 >> shift) as i64
}

/// Decode a bitmask immediate (shared by logical-immediate instructions).
fn decode_bitmask(n: u32, imms: u32, immr: u32, is64: bool) -> u64 {
    let len = if n == 1 { 6 } else { (imms ^ 0x3F).leading_zeros() - 26 };
    let esize = 1u32 << len;
    let levels = esize - 1;
    let s = imms & levels;
    let r = immr & levels;
    let welem: u64 = if s + 1 >= 64 { u64::MAX } else { (1u64 << (s + 1)) - 1 };
    let elem = welem.rotate_right(r);
    let mask = if esize >= 64 { u64::MAX } else { (1u64 << esize) - 1 };
    let elem = elem & mask;
    let mut result = 0u64;
    let mut i = 0;
    while i < 64 { result |= elem << i; i += esize; }
    if !is64 { result &= 0xFFFF_FFFF; }
    result
}

/// Mask to 32-bit when sf=0.
fn mask_sf(val: u64, sf: u32) -> u64 {
    if sf == 0 { val & 0xFFFF_FFFF } else { val }
}

// ── Private emitter helpers ─────────────────────────────────────────────────

impl A64TcgEmitter<'_> {
    /// Read guest register `n` (0-30); reg 31 reads as zero.
    fn xn(&mut self, n: u32) -> TcgTemp {
        if n >= 31 { self.ctx.movi(0) } else { self.ctx.read_reg(n as u16) }
    }

    /// Read guest register `n`; reg 31 = SP.
    fn xn_sp(&mut self, n: u32) -> TcgTemp {
        if n == 31 { self.ctx.read_reg(REG_SP) } else { self.ctx.read_reg(n as u16) }
    }

    /// Write guest register `n` (0-30); writes to reg 31 are discarded.
    fn set_xn(&mut self, n: u32, val: TcgTemp) {
        if n < 31 { self.ctx.write_reg(n as u16, val); }
    }

    /// Write guest register `n`; reg 31 = SP.
    fn set_xn_sp(&mut self, n: u32, val: TcgTemp) {
        if n == 31 { self.ctx.write_reg(REG_SP, val); }
        else { self.ctx.write_reg(n as u16, val); }
    }

    /// Truncate to 32 bits if sf=0.
    fn maybe_trunc32(&mut self, src: TcgTemp, sf: u32) -> TcgTemp {
        if sf == 0 {
            let t = self.ctx.temp();
            self.ctx.emit(TcgOp::Zext { dst: t, src, from_bits: 32 });
            t
        } else { src }
    }

    /// Emit add/sub and optionally set NZCV.
    fn emit_awc(&mut self, a: TcgTemp, b: TcgTemp, is_sub: bool, set_flags: bool, sf: u32) -> TcgTemp {
        let result = if is_sub {
            let d = self.ctx.temp();
            self.ctx.emit(TcgOp::Sub { dst: d, a, b });
            d
        } else {
            self.ctx.add(a, b)
        };
        let result = self.maybe_trunc32(result, sf);
        if set_flags { self.emit_nzcv(result, sf); }
        result
    }

    /// Emit NZCV flag computation (N and Z only — C/V require carry logic).
    fn emit_nzcv(&mut self, result: TcgTemp, sf: u32) {
        let zero = self.ctx.movi(0);
        let sign_bit = if sf == 1 { 63u32 } else { 31 };
        let sign_mask = self.ctx.movi(1u64 << sign_bit);

        let n_bit = self.ctx.temp();
        self.ctx.emit(TcgOp::And { dst: n_bit, a: result, b: sign_mask });
        let is_neg = self.ctx.temp();
        self.ctx.emit(TcgOp::SetNe { dst: is_neg, a: n_bit, b: zero });
        let thirty_one = self.ctx.movi(31);
        let n_shifted = self.ctx.temp();
        self.ctx.emit(TcgOp::Shl { dst: n_shifted, a: is_neg, b: thirty_one });

        let is_z = self.ctx.temp();
        self.ctx.emit(TcgOp::SetEq { dst: is_z, a: result, b: zero });
        let thirty = self.ctx.movi(30);
        let z_shifted = self.ctx.temp();
        self.ctx.emit(TcgOp::Shl { dst: z_shifted, a: is_z, b: thirty });

        let nz = self.ctx.temp();
        self.ctx.emit(TcgOp::Or { dst: nz, a: n_shifted, b: z_shifted });
        self.ctx.emit(TcgOp::WriteReg { reg_id: REG_NZCV, src: nz });
    }

    /// Read NZCV and evaluate an A64 condition code.
    fn emit_cond_check(&mut self, cond: u32) -> TcgTemp {
        let nzcv = self.ctx.read_reg(REG_NZCV);
        let base_cond = cond >> 1;
        let invert = cond & 1;

        let flag_val = match base_cond {
            0 => self.extract_flag(nzcv, 30),       // EQ: Z==1
            1 => self.extract_flag(nzcv, 29),       // CS: C==1
            2 => self.extract_flag(nzcv, 31),       // MI: N==1
            3 => self.extract_flag(nzcv, 28),       // VS: V==1
            4 => { // HI: C==1 && Z==0
                let c = self.extract_flag(nzcv, 29);
                let z = self.extract_flag(nzcv, 30);
                let one = self.ctx.movi(1);
                let not_z = self.ctx.temp();
                self.ctx.emit(TcgOp::Xor { dst: not_z, a: z, b: one });
                let r = self.ctx.temp();
                self.ctx.emit(TcgOp::And { dst: r, a: c, b: not_z });
                r
            }
            5 => { // GE: N==V
                let n = self.extract_flag(nzcv, 31);
                let v = self.extract_flag(nzcv, 28);
                let eq = self.ctx.temp();
                self.ctx.emit(TcgOp::SetEq { dst: eq, a: n, b: v });
                eq
            }
            6 => { // GT: N==V && Z==0
                let n = self.extract_flag(nzcv, 31);
                let v = self.extract_flag(nzcv, 28);
                let z = self.extract_flag(nzcv, 30);
                let nv_eq = self.ctx.temp();
                self.ctx.emit(TcgOp::SetEq { dst: nv_eq, a: n, b: v });
                let one = self.ctx.movi(1);
                let not_z = self.ctx.temp();
                self.ctx.emit(TcgOp::Xor { dst: not_z, a: z, b: one });
                let r = self.ctx.temp();
                self.ctx.emit(TcgOp::And { dst: r, a: nv_eq, b: not_z });
                r
            }
            7 => self.ctx.movi(1),                  // AL
            _ => unreachable!(),
        };

        if invert == 1 && base_cond != 7 {
            let one = self.ctx.movi(1);
            let inv = self.ctx.temp();
            self.ctx.emit(TcgOp::Xor { dst: inv, a: flag_val, b: one });
            inv
        } else { flag_val }
    }

    /// Extract a single flag bit at the given position from NZCV.
    fn extract_flag(&mut self, nzcv: TcgTemp, bit_pos: u32) -> TcgTemp {
        let shift_amt = self.ctx.movi(bit_pos as u64);
        let shifted = self.ctx.temp();
        self.ctx.emit(TcgOp::Shr { dst: shifted, a: nzcv, b: shift_amt });
        let one = self.ctx.movi(1);
        let flag = self.ctx.temp();
        self.ctx.emit(TcgOp::And { dst: flag, a: shifted, b: one });
        flag
    }

    /// Apply LSL/LSR/ASR shift to a register temp.
    fn apply_shift(&mut self, src: TcgTemp, stype: u32, amount: u32) -> TcgTemp {
        if amount == 0 { return src; }
        let shift_t = self.ctx.movi(amount as u64);
        let d = self.ctx.temp();
        match stype {
            0 => self.ctx.emit(TcgOp::Shl { dst: d, a: src, b: shift_t }),
            1 => self.ctx.emit(TcgOp::Shr { dst: d, a: src, b: shift_t }),
            2 => self.ctx.emit(TcgOp::Sar { dst: d, a: src, b: shift_t }),
            _ => self.ctx.emit(TcgOp::Shr { dst: d, a: src, b: shift_t }),
        }
        d
    }

    /// Extend register for ADD/SUB (extended register) instructions.
    fn extend_reg(&mut self, rm: u32, option: u32, imm3: u32) -> TcgTemp {
        let raw = self.xn(rm);
        let extended = match option {
            0 => { let t = self.ctx.temp(); self.ctx.emit(TcgOp::Zext { dst: t, src: raw, from_bits: 8 }); t }
            1 => { let t = self.ctx.temp(); self.ctx.emit(TcgOp::Zext { dst: t, src: raw, from_bits: 16 }); t }
            2 => { let t = self.ctx.temp(); self.ctx.emit(TcgOp::Zext { dst: t, src: raw, from_bits: 32 }); t }
            3 => raw,
            4 => { let t = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: t, src: raw, from_bits: 8 }); t }
            5 => { let t = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: t, src: raw, from_bits: 16 }); t }
            6 => { let t = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: t, src: raw, from_bits: 32 }); t }
            _ => raw,
        };
        if imm3 != 0 {
            let shift_t = self.ctx.movi(imm3 as u64);
            let shifted = self.ctx.temp();
            self.ctx.emit(TcgOp::Shl { dst: shifted, a: extended, b: shift_t });
            shifted
        } else { extended }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Branch handler implementation
// ═══════════════════════════════════════════════════════════════════

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

    fn handle_b_cond(&mut self, _insn: u32, imm19: u32, cond: u32) -> Result<(), HelmError> {
        let offset = sext(imm19, 19) << 2;
        let target = self.pc.wrapping_add(offset as u64);
        let fallthrough = self.pc + 4;
        let cond_val = self.emit_cond_check(cond);
        let taken_label = self.ctx.label();
        self.ctx.emit(TcgOp::BrCond { cond: cond_val, label: taken_label });
        self.ctx.emit(TcgOp::GotoTb { target_pc: fallthrough });
        self.ctx.emit(TcgOp::Label { id: taken_label });
        self.ctx.emit(TcgOp::GotoTb { target_pc: target });
        self.end_block = true;
        Ok(())
    }

    fn handle_cbz(&mut self, _insn: u32, sf: u32, imm19: u32, rt: u32) -> Result<(), HelmError> {
        let offset = sext(imm19, 19) << 2;
        let target = self.pc.wrapping_add(offset as u64);
        let fallthrough = self.pc + 4;
        let reg_val = self.xn(rt);
        let val = self.maybe_trunc32(reg_val, sf);
        let zero = self.ctx.movi(0);
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
        let reg_val = self.xn(rt);
        let val = self.maybe_trunc32(reg_val, sf);
        let zero = self.ctx.movi(0);
        let is_nz = self.ctx.temp();
        self.ctx.emit(TcgOp::SetNe { dst: is_nz, a: val, b: zero });
        let taken_label = self.ctx.label();
        self.ctx.emit(TcgOp::BrCond { cond: is_nz, label: taken_label });
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
        let reg_val = self.xn(rt);
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
        let reg_val = self.xn(rt);
        let bit_mask = self.ctx.movi(1u64 << bit);
        let masked = self.ctx.temp();
        self.ctx.emit(TcgOp::And { dst: masked, a: reg_val, b: bit_mask });
        let zero = self.ctx.movi(0);
        let is_nz = self.ctx.temp();
        self.ctx.emit(TcgOp::SetNe { dst: is_nz, a: masked, b: zero });
        let taken_label = self.ctx.label();
        self.ctx.emit(TcgOp::BrCond { cond: is_nz, label: taken_label });
        self.ctx.emit(TcgOp::GotoTb { target_pc: fallthrough });
        self.ctx.emit(TcgOp::Label { id: taken_label });
        self.ctx.emit(TcgOp::GotoTb { target_pc: target });
        self.end_block = true;
        Ok(())
    }

    fn handle_br(&mut self, _insn: u32, rn: u32) -> Result<(), HelmError> {
        let t = self.xn(rn); self.ctx.emit(TcgOp::WriteReg { reg_id: REG_PC, src: t }); self.ctx.emit(TcgOp::ExitTb); self.end_block = true; Ok(())
    }
    fn handle_blr(&mut self, _insn: u32, rn: u32) -> Result<(), HelmError> {
        let t = self.xn(rn); let ra = self.ctx.movi(self.pc + 4); self.ctx.write_reg(30, ra);
        self.ctx.emit(TcgOp::WriteReg { reg_id: REG_PC, src: t }); self.ctx.emit(TcgOp::ExitTb); self.end_block = true; Ok(())
    }
    fn handle_ret(&mut self, _insn: u32, rn: u32) -> Result<(), HelmError> {
        let t = self.xn(rn); self.ctx.emit(TcgOp::WriteReg { reg_id: REG_PC, src: t }); self.ctx.emit(TcgOp::ExitTb); self.end_block = true; Ok(())
    }
    fn handle_braz(&mut self, _i: u32, _m: u32, rn: u32) -> Result<(), HelmError> { self.handle_br(_i, rn) }
    fn handle_blraz(&mut self, _i: u32, _m: u32, rn: u32) -> Result<(), HelmError> { self.handle_blr(_i, rn) }
    fn handle_reta(&mut self, _i: u32, _m: u32) -> Result<(), HelmError> { self.handle_ret(_i, 30) }
    fn handle_bra(&mut self, _i: u32, _m: u32, rn: u32, _rm: u32) -> Result<(), HelmError> { self.handle_br(_i, rn) }
    fn handle_blra(&mut self, _i: u32, _m: u32, rn: u32, _rm: u32) -> Result<(), HelmError> { self.handle_blr(_i, rn) }
    fn handle_eret(&mut self, _i: u32) -> Result<(), HelmError> { self.ctx.emit(TcgOp::ExitTb); self.end_block = true; Ok(()) }
    fn handle_ereta(&mut self, _i: u32, _m: u32) -> Result<(), HelmError> { self.ctx.emit(TcgOp::ExitTb); self.end_block = true; Ok(()) }

    fn handle_svc(&mut self, _insn: u32, _imm16: u32) -> Result<(), HelmError> {
        let nr = self.ctx.read_reg(8); self.ctx.emit(TcgOp::Syscall { nr }); self.end_block = true; Ok(())
    }
    fn handle_hvc(&mut self, _i: u32, _imm16: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "HVC not supported".into() }) }
    fn handle_smc(&mut self, _i: u32, _imm16: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "SMC not supported".into() }) }
    fn handle_brk(&mut self, _i: u32, _imm16: u32) -> Result<(), HelmError> { self.ctx.emit(TcgOp::ExitTb); self.end_block = true; Ok(()) }
    fn handle_hlt(&mut self, _i: u32, _imm16: u32) -> Result<(), HelmError> { self.ctx.emit(TcgOp::ExitTb); self.end_block = true; Ok(()) }

    fn handle_nop(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_yield(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_wfe(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_wfi(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_sev(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_sevl(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_dgl(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_xpaclri(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_pacia1716(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_pacib1716(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_autia1716(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_autib1716(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_paciaz(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_paciasp(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_pacibz(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_pacibsp(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_autiaz(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_autiasp(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_autibz(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_autibsp(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_esb(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_gcsb(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_chkfeat(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_wfet(&mut self, _i: u32, _rd: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_wfit(&mut self, _i: u32, _rd: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_clrex(&mut self, _i: u32, _crm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_dsb(&mut self, _i: u32, _crm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_dmb(&mut self, _i: u32, _crm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_isb(&mut self, _i: u32, _crm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_sb(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_dsb_nxs(&mut self, _i: u32, _nxs_hi: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_cfinv(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_xaflag(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_axflag(&mut self, _i: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_msr_i_uao(&mut self, _i: u32, _imm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_msr_i_pan(&mut self, _i: u32, _imm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_msr_i_spsel(&mut self, _i: u32, _imm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_msr_i_sbss(&mut self, _i: u32, _imm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_msr_i_dit(&mut self, _i: u32, _imm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_msr_i_tco(&mut self, _i: u32, _imm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_msr_i_daifset(&mut self, _i: u32, _imm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_msr_i_daifclear(&mut self, _i: u32, _imm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_msr_i_allint(&mut self, _i: u32, _imm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_msr_i_svcr(&mut self, _i: u32, _mask: u32, _imm: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_sys(&mut self, _i: u32, _op1: u32, _crn: u32, _crm: u32, _op2: u32, _rt: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_sysl(&mut self, _i: u32, _op1: u32, _crn: u32, _crm: u32, _op2: u32, _rt: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_mrs(&mut self, _i: u32, _o0: u32, _op1: u32, _crn: u32, _crm: u32, _op2: u32, _rt: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_msr(&mut self, _i: u32, _o0: u32, _op1: u32, _crn: u32, _crm: u32, _op2: u32, _rt: u32) -> Result<(), HelmError> { Ok(()) }
}

// ═══════════════════════════════════════════════════════════════════
// DP-immediate handler implementation
// ═══════════════════════════════════════════════════════════════════

impl DecodeAarch64DpImmHandler for A64TcgEmitter<'_> {
    fn handle_adr(&mut self, _i: u32, immlo: u32, immhi: u32, rd: u32) -> Result<(), HelmError> {
        let imm = ((sext(immhi, 19) as u64) << 2) | immlo as u64;
        let v = self.ctx.movi(self.pc.wrapping_add(imm)); self.set_xn(rd, v); Ok(())
    }
    fn handle_adrp(&mut self, _i: u32, immlo: u32, immhi: u32, rd: u32) -> Result<(), HelmError> {
        let imm = (((sext(immhi, 19) as u64) << 2) | immlo as u64) as i64;
        let v = self.ctx.movi((self.pc & !0xFFF).wrapping_add((imm << 12) as u64)); self.set_xn(rd, v); Ok(())
    }
    fn handle_add_imm(&mut self, _i: u32, sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let mut imm = imm12 as u64; if sh == 1 { imm <<= 12; }
        let src = self.xn_sp(rn); let imm_t = self.ctx.movi(imm);
        let r = self.ctx.add(src, imm_t); let r = self.maybe_trunc32(r, sf); self.set_xn_sp(rd, r); Ok(())
    }
    fn handle_adds_imm(&mut self, _i: u32, sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let mut imm = imm12 as u64; if sh == 1 { imm <<= 12; }
        let src = self.xn_sp(rn); let imm_t = self.ctx.movi(imm);
        let r = self.emit_awc(src, imm_t, false, true, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_sub_imm(&mut self, _i: u32, sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let mut imm = imm12 as u64; if sh == 1 { imm <<= 12; }
        let src = self.xn_sp(rn); let imm_t = self.ctx.movi(imm);
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::Sub { dst: d, a: src, b: imm_t });
        let r = self.maybe_trunc32(d, sf); self.set_xn_sp(rd, r); Ok(())
    }
    fn handle_subs_imm(&mut self, _i: u32, sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let mut imm = imm12 as u64; if sh == 1 { imm <<= 12; }
        let src = self.xn_sp(rn); let imm_t = self.ctx.movi(imm);
        let r = self.emit_awc(src, imm_t, true, true, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_and_imm(&mut self, _i: u32, sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let bm = decode_bitmask(n, imms, immr, sf == 1); let src = self.xn(rn); let mask_t = self.ctx.movi(bm);
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::And { dst: d, a: src, b: mask_t });
        let r = self.maybe_trunc32(d, sf); self.set_xn_sp(rd, r); Ok(())
    }
    fn handle_orr_imm(&mut self, _i: u32, sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let bm = decode_bitmask(n, imms, immr, sf == 1); let src = self.xn(rn); let mask_t = self.ctx.movi(bm);
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::Or { dst: d, a: src, b: mask_t });
        let r = self.maybe_trunc32(d, sf); self.set_xn_sp(rd, r); Ok(())
    }
    fn handle_eor_imm(&mut self, _i: u32, sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let bm = decode_bitmask(n, imms, immr, sf == 1); let src = self.xn(rn); let mask_t = self.ctx.movi(bm);
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::Xor { dst: d, a: src, b: mask_t });
        let r = self.maybe_trunc32(d, sf); self.set_xn_sp(rd, r); Ok(())
    }
    fn handle_ands_imm(&mut self, _i: u32, sf: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let bm = decode_bitmask(n, imms, immr, sf == 1); let src = self.xn(rn); let mask_t = self.ctx.movi(bm);
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::And { dst: d, a: src, b: mask_t });
        let r = self.maybe_trunc32(d, sf); self.emit_nzcv(r, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_movn(&mut self, _i: u32, sf: u32, hw: u32, imm16: u32, rd: u32) -> Result<(), HelmError> {
        let v = self.ctx.movi(mask_sf(!(( imm16 as u64) << (hw * 16)), sf)); self.set_xn(rd, v); Ok(())
    }
    fn handle_movz(&mut self, _i: u32, sf: u32, hw: u32, imm16: u32, rd: u32) -> Result<(), HelmError> {
        let v = self.ctx.movi(mask_sf((imm16 as u64) << (hw * 16), sf)); self.set_xn(rd, v); Ok(())
    }
    fn handle_movk(&mut self, _i: u32, _sf: u32, hw: u32, imm16: u32, rd: u32) -> Result<(), HelmError> {
        let old = self.xn(rd); let cm = self.ctx.movi(!(0xFFFF_u64 << (hw * 16)));
        let masked = self.ctx.temp(); self.ctx.emit(TcgOp::And { dst: masked, a: old, b: cm });
        let bits = self.ctx.movi((imm16 as u64) << (hw * 16)); let r = self.ctx.temp();
        self.ctx.emit(TcgOp::Or { dst: r, a: masked, b: bits }); self.set_xn(rd, r); Ok(())
    }
    fn handle_sbfm(&mut self, _i: u32, sf: u32, _n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let src = self.xn(rn); let esize = if sf == 1 { 64u32 } else { 32 };
        let val = if imms >= immr {
            let w = imms - immr + 1; let sr = self.ctx.movi(immr as u64);
            let shifted = self.ctx.temp(); self.ctx.emit(TcgOp::Shr { dst: shifted, a: src, b: sr });
            if w < 64 { let m = self.ctx.movi((1u64 << w) - 1); let t = self.ctx.temp();
                self.ctx.emit(TcgOp::And { dst: t, a: shifted, b: m });
                let e = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: e, src: t, from_bits: w as u8 }); e
            } else { shifted }
        } else {
            let w = imms + 1;
            if w < 64 { let m = self.ctx.movi((1u64 << w) - 1); let t = self.ctx.temp();
                self.ctx.emit(TcgOp::And { dst: t, a: src, b: m });
                let sl = self.ctx.movi((esize - immr) as u64); let s = self.ctx.temp();
                self.ctx.emit(TcgOp::Shl { dst: s, a: t, b: sl });
                let e = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: e, src: s, from_bits: esize as u8 }); e
            } else { src }
        };
        let r = self.maybe_trunc32(val, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_bfm(&mut self, _i: u32, sf: u32, _n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let src = self.xn(rn); let dst_old = self.xn(rd);
        let esize = if sf == 1 { 64u32 } else { 32 };
        let wmask = decode_bitmask(if sf == 1 { 1 } else { 0 }, imms, immr, sf == 1);
        let rotated = if immr == 0 { src } else {
            let r = self.ctx.movi(immr as u64); let lo = self.ctx.temp();
            self.ctx.emit(TcgOp::Shr { dst: lo, a: src, b: r });
            let bl = self.ctx.movi((esize - immr) as u64); let hi = self.ctx.temp();
            self.ctx.emit(TcgOp::Shl { dst: hi, a: src, b: bl });
            let c = self.ctx.temp(); self.ctx.emit(TcgOp::Or { dst: c, a: hi, b: lo }); c
        };
        let wt = self.ctx.movi(wmask); let nwt = self.ctx.movi(!wmask);
        let sb = self.ctx.temp(); self.ctx.emit(TcgOp::And { dst: sb, a: rotated, b: wt });
        let db = self.ctx.temp(); self.ctx.emit(TcgOp::And { dst: db, a: dst_old, b: nwt });
        let mg = self.ctx.temp(); self.ctx.emit(TcgOp::Or { dst: mg, a: db, b: sb });
        let r = self.maybe_trunc32(mg, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_ubfm(&mut self, _i: u32, sf: u32, _n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let src = self.xn(rn); let esize = if sf == 1 { 64u32 } else { 32 };
        let val = if imms >= immr {
            let w = imms - immr + 1; let sr = self.ctx.movi(immr as u64);
            let shifted = self.ctx.temp(); self.ctx.emit(TcgOp::Shr { dst: shifted, a: src, b: sr });
            if w < 64 { let m = self.ctx.movi((1u64 << w) - 1); let t = self.ctx.temp();
                self.ctx.emit(TcgOp::And { dst: t, a: shifted, b: m }); t
            } else { shifted }
        } else {
            let w = imms + 1;
            if w < 64 { let m = self.ctx.movi((1u64 << w) - 1); let t = self.ctx.temp();
                self.ctx.emit(TcgOp::And { dst: t, a: src, b: m });
                let sl = self.ctx.movi((esize - immr) as u64); let s = self.ctx.temp();
                self.ctx.emit(TcgOp::Shl { dst: s, a: t, b: sl }); s
            } else { src }
        };
        let r = self.maybe_trunc32(val, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_extr(&mut self, _i: u32, sf: u32, _n: u32, rm: u32, imms: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let hi = self.xn(rn); let lo = self.xn(rm); let esize = if sf == 1 { 64u32 } else { 32 };
        if imms == 0 { let r = self.maybe_trunc32(lo, sf); self.set_xn(rd, r); }
        else {
            let sr = self.ctx.movi(imms as u64); let lp = self.ctx.temp();
            self.ctx.emit(TcgOp::Shr { dst: lp, a: lo, b: sr });
            let sl = self.ctx.movi((esize - imms) as u64); let hp = self.ctx.temp();
            self.ctx.emit(TcgOp::Shl { dst: hp, a: hi, b: sl });
            let c = self.ctx.temp(); self.ctx.emit(TcgOp::Or { dst: c, a: hp, b: lp });
            let r = self.maybe_trunc32(c, sf); self.set_xn(rd, r);
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════
// DP-register handler implementation
// ═══════════════════════════════════════════════════════════════════

impl DecodeAarch64DpRegHandler for A64TcgEmitter<'_> {
    fn handle_add_reg(&mut self, _i: u32, sf: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let _tmp_rm = self.xn(rm); let b = self.apply_shift(_tmp_rm, shift, imm6);
        let r = self.ctx.add(a, b); let r = self.maybe_trunc32(r, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_adds_reg(&mut self, _i: u32, sf: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let _tmp_rm = self.xn(rm); let b = self.apply_shift(_tmp_rm, shift, imm6);
        let r = self.emit_awc(a, b, false, true, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_sub_reg(&mut self, _i: u32, sf: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let _tmp_rm = self.xn(rm); let b = self.apply_shift(_tmp_rm, shift, imm6);
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::Sub { dst: d, a, b });
        let r = self.maybe_trunc32(d, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_subs_reg(&mut self, _i: u32, sf: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let _tmp_rm = self.xn(rm); let b = self.apply_shift(_tmp_rm, shift, imm6);
        let r = self.emit_awc(a, b, true, true, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_and_reg(&mut self, _i: u32, sf: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let _tmp_rm = self.xn(rm); let b = self.apply_shift(_tmp_rm, shift, imm6);
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::And { dst: d, a, b });
        let r = self.maybe_trunc32(d, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_orr_reg(&mut self, _i: u32, sf: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let _tmp_rm = self.xn(rm); let b = self.apply_shift(_tmp_rm, shift, imm6);
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::Or { dst: d, a, b });
        let r = self.maybe_trunc32(d, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_eor_reg(&mut self, _i: u32, sf: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let _tmp_rm = self.xn(rm); let b = self.apply_shift(_tmp_rm, shift, imm6);
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::Xor { dst: d, a, b });
        let r = self.maybe_trunc32(d, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_ands_reg(&mut self, _i: u32, sf: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let _tmp_rm = self.xn(rm); let b = self.apply_shift(_tmp_rm, shift, imm6);
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::And { dst: d, a, b });
        let r = self.maybe_trunc32(d, sf); self.emit_nzcv(r, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_bic(&mut self, _i: u32, sf: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let _tmp_rm = self.xn(rm); let br = self.apply_shift(_tmp_rm, shift, imm6);
        let b = self.ctx.temp(); self.ctx.emit(TcgOp::Not { dst: b, src: br });
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::And { dst: d, a, b });
        let r = self.maybe_trunc32(d, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_orn(&mut self, _i: u32, sf: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let _tmp_rm = self.xn(rm); let br = self.apply_shift(_tmp_rm, shift, imm6);
        let b = self.ctx.temp(); self.ctx.emit(TcgOp::Not { dst: b, src: br });
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::Or { dst: d, a, b });
        let r = self.maybe_trunc32(d, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_eon(&mut self, _i: u32, sf: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let _tmp_rm = self.xn(rm); let br = self.apply_shift(_tmp_rm, shift, imm6);
        let b = self.ctx.temp(); self.ctx.emit(TcgOp::Not { dst: b, src: br });
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::Xor { dst: d, a, b });
        let r = self.maybe_trunc32(d, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_bics(&mut self, _i: u32, sf: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let _tmp_rm = self.xn(rm); let br = self.apply_shift(_tmp_rm, shift, imm6);
        let b = self.ctx.temp(); self.ctx.emit(TcgOp::Not { dst: b, src: br });
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::And { dst: d, a, b });
        let r = self.maybe_trunc32(d, sf); self.emit_nzcv(r, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_madd(&mut self, _i: u32, sf: u32, rm: u32, ra: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let n = self.xn(rn); let m = self.xn(rm); let a = self.xn(ra);
        let p = self.ctx.temp(); self.ctx.emit(TcgOp::Mul { dst: p, a: n, b: m });
        let r = self.ctx.add(a, p); let r = self.maybe_trunc32(r, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_msub(&mut self, _i: u32, sf: u32, rm: u32, ra: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let n = self.xn(rn); let m = self.xn(rm); let a = self.xn(ra);
        let p = self.ctx.temp(); self.ctx.emit(TcgOp::Mul { dst: p, a: n, b: m });
        let r = self.ctx.temp(); self.ctx.emit(TcgOp::Sub { dst: r, a, b: p });
        let r = self.maybe_trunc32(r, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_smaddl(&mut self, _i: u32, rm: u32, ra: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let n = self.xn(rn); let m = self.xn(rm); let a = self.xn(ra);
        let ne = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: ne, src: n, from_bits: 32 });
        let me = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: me, src: m, from_bits: 32 });
        let p = self.ctx.temp(); self.ctx.emit(TcgOp::Mul { dst: p, a: ne, b: me });
        let r = self.ctx.add(a, p); self.set_xn(rd, r); Ok(())
    }
    fn handle_umaddl(&mut self, _i: u32, rm: u32, ra: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let n = self.xn(rn); let m = self.xn(rm); let a = self.xn(ra);
        let ne = self.ctx.temp(); self.ctx.emit(TcgOp::Zext { dst: ne, src: n, from_bits: 32 });
        let me = self.ctx.temp(); self.ctx.emit(TcgOp::Zext { dst: me, src: m, from_bits: 32 });
        let p = self.ctx.temp(); self.ctx.emit(TcgOp::Mul { dst: p, a: ne, b: me });
        let r = self.ctx.add(a, p); self.set_xn(rd, r); Ok(())
    }
    fn handle_smsubl(&mut self, _i: u32, rm: u32, ra: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let n = self.xn(rn); let m = self.xn(rm); let a = self.xn(ra);
        let ne = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: ne, src: n, from_bits: 32 });
        let me = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: me, src: m, from_bits: 32 });
        let p = self.ctx.temp(); self.ctx.emit(TcgOp::Mul { dst: p, a: ne, b: me });
        let r = self.ctx.temp(); self.ctx.emit(TcgOp::Sub { dst: r, a, b: p }); self.set_xn(rd, r); Ok(())
    }
    fn handle_umsubl(&mut self, _i: u32, rm: u32, ra: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let n = self.xn(rn); let m = self.xn(rm); let a = self.xn(ra);
        let ne = self.ctx.temp(); self.ctx.emit(TcgOp::Zext { dst: ne, src: n, from_bits: 32 });
        let me = self.ctx.temp(); self.ctx.emit(TcgOp::Zext { dst: me, src: m, from_bits: 32 });
        let p = self.ctx.temp(); self.ctx.emit(TcgOp::Mul { dst: p, a: ne, b: me });
        let r = self.ctx.temp(); self.ctx.emit(TcgOp::Sub { dst: r, a, b: p }); self.set_xn(rd, r); Ok(())
    }
    fn handle_smulh(&mut self, _i: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "SMULH needs 128-bit".into() }) }
    fn handle_umulh(&mut self, _i: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "UMULH needs 128-bit".into() }) }
    fn handle_add_ext(&mut self, _i: u32, sf: u32, rm: u32, option: u32, imm3: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn_sp(rn); let b = self.extend_reg(rm, option, imm3);
        let r = self.ctx.add(a, b); let r = self.maybe_trunc32(r, sf); self.set_xn_sp(rd, r); Ok(())
    }
    fn handle_adds_ext(&mut self, _i: u32, sf: u32, rm: u32, option: u32, imm3: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn_sp(rn); let b = self.extend_reg(rm, option, imm3);
        let r = self.emit_awc(a, b, false, true, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_sub_ext(&mut self, _i: u32, sf: u32, rm: u32, option: u32, imm3: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn_sp(rn); let b = self.extend_reg(rm, option, imm3);
        let d = self.ctx.temp(); self.ctx.emit(TcgOp::Sub { dst: d, a, b });
        let r = self.maybe_trunc32(d, sf); self.set_xn_sp(rd, r); Ok(())
    }
    fn handle_subs_ext(&mut self, _i: u32, sf: u32, rm: u32, option: u32, imm3: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn_sp(rn); let b = self.extend_reg(rm, option, imm3);
        let r = self.emit_awc(a, b, true, true, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_adc(&mut self, _i: u32, _sf: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "ADC needs carry".into() }) }
    fn handle_adcs(&mut self, _i: u32, _sf: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "ADCS needs carry".into() }) }
    fn handle_sbc(&mut self, _i: u32, _sf: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "SBC needs carry".into() }) }
    fn handle_sbcs(&mut self, _i: u32, _sf: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "SBCS needs carry".into() }) }
    fn handle_udiv(&mut self, _i: u32, sf: u32, rm: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let b = self.xn(rm); let d = self.ctx.temp();
        self.ctx.emit(TcgOp::Div { dst: d, a, b }); let r = self.maybe_trunc32(d, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_sdiv(&mut self, _i: u32, sf: u32, rm: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let b = self.xn(rm); let d = self.ctx.temp();
        self.ctx.emit(TcgOp::Div { dst: d, a, b }); let r = self.maybe_trunc32(d, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_lslv(&mut self, _i: u32, sf: u32, rm: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let b = self.xn(rm); let d = self.ctx.temp();
        self.ctx.emit(TcgOp::Shl { dst: d, a, b }); let r = self.maybe_trunc32(d, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_lsrv(&mut self, _i: u32, sf: u32, rm: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let b = self.xn(rm); let d = self.ctx.temp();
        self.ctx.emit(TcgOp::Shr { dst: d, a, b }); let r = self.maybe_trunc32(d, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_asrv(&mut self, _i: u32, sf: u32, rm: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let a = self.xn(rn); let b = self.xn(rm); let d = self.ctx.temp();
        self.ctx.emit(TcgOp::Sar { dst: d, a, b }); let r = self.maybe_trunc32(d, sf); self.set_xn(rd, r); Ok(())
    }
    fn handle_rorv(&mut self, _i: u32, _sf: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "RORV not in TCG IR".into() }) }
    fn handle_csel(&mut self, _i: u32, sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let cv = self.emit_cond_check(cond); let tl = self.ctx.label(); let dl = self.ctx.label();
        self.ctx.emit(TcgOp::BrCond { cond: cv, label: tl });
        let vf = self.xn(rm); let vf = self.maybe_trunc32(vf, sf); self.set_xn(rd, vf);
        self.ctx.emit(TcgOp::Br { label: dl }); self.ctx.emit(TcgOp::Label { id: tl });
        let vt = self.xn(rn); let vt = self.maybe_trunc32(vt, sf); self.set_xn(rd, vt);
        self.ctx.emit(TcgOp::Label { id: dl }); Ok(())
    }
    fn handle_csinc(&mut self, _i: u32, sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let cv = self.emit_cond_check(cond); let tl = self.ctx.label(); let dl = self.ctx.label();
        self.ctx.emit(TcgOp::BrCond { cond: cv, label: tl });
        let vf = self.xn(rm); let one = self.ctx.movi(1); let inc = self.ctx.add(vf, one);
        let inc = self.maybe_trunc32(inc, sf); self.set_xn(rd, inc);
        self.ctx.emit(TcgOp::Br { label: dl }); self.ctx.emit(TcgOp::Label { id: tl });
        let vt = self.xn(rn); let vt = self.maybe_trunc32(vt, sf); self.set_xn(rd, vt);
        self.ctx.emit(TcgOp::Label { id: dl }); Ok(())
    }
    fn handle_csinv(&mut self, _i: u32, sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let cv = self.emit_cond_check(cond); let tl = self.ctx.label(); let dl = self.ctx.label();
        self.ctx.emit(TcgOp::BrCond { cond: cv, label: tl });
        let vf = self.xn(rm); let inv = self.ctx.temp(); self.ctx.emit(TcgOp::Not { dst: inv, src: vf });
        let inv = self.maybe_trunc32(inv, sf); self.set_xn(rd, inv);
        self.ctx.emit(TcgOp::Br { label: dl }); self.ctx.emit(TcgOp::Label { id: tl });
        let vt = self.xn(rn); let vt = self.maybe_trunc32(vt, sf); self.set_xn(rd, vt);
        self.ctx.emit(TcgOp::Label { id: dl }); Ok(())
    }
    fn handle_csneg(&mut self, _i: u32, sf: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> Result<(), HelmError> {
        let cv = self.emit_cond_check(cond); let tl = self.ctx.label(); let dl = self.ctx.label();
        self.ctx.emit(TcgOp::BrCond { cond: cv, label: tl });
        let vf = self.xn(rm); let inv = self.ctx.temp(); self.ctx.emit(TcgOp::Not { dst: inv, src: vf });
        let one = self.ctx.movi(1); let neg = self.ctx.add(inv, one);
        let neg = self.maybe_trunc32(neg, sf); self.set_xn(rd, neg);
        self.ctx.emit(TcgOp::Br { label: dl }); self.ctx.emit(TcgOp::Label { id: tl });
        let vt = self.xn(rn); let vt = self.maybe_trunc32(vt, sf); self.set_xn(rd, vt);
        self.ctx.emit(TcgOp::Label { id: dl }); Ok(())
    }
    fn handle_rbit(&mut self, _i: u32, _sf: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "RBIT unimpl".into() }) }
    fn handle_rev16(&mut self, _i: u32, _sf: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "REV16 unimpl".into() }) }
    fn handle_rev32(&mut self, _i: u32, _sf: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "REV32 unimpl".into() }) }
    fn handle_rev(&mut self, _i: u32, _sf: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "REV unimpl".into() }) }
    fn handle_clz(&mut self, _i: u32, _sf: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CLZ unimpl".into() }) }
    fn handle_cls(&mut self, _i: u32, _sf: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CLS unimpl".into() }) }
    fn handle_ctz(&mut self, _i: u32, _sf: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CTZ unimpl".into() }) }
    fn handle_cnt(&mut self, _i: u32, _sf: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CNT unimpl".into() }) }
    fn handle_crc32b(&mut self, _i: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CRC unimpl".into() }) }
    fn handle_crc32h(&mut self, _i: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CRC unimpl".into() }) }
    fn handle_crc32w(&mut self, _i: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CRC unimpl".into() }) }
    fn handle_crc32x(&mut self, _i: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CRC unimpl".into() }) }
    fn handle_crc32cb(&mut self, _i: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CRC unimpl".into() }) }
    fn handle_crc32ch(&mut self, _i: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CRC unimpl".into() }) }
    fn handle_crc32cw(&mut self, _i: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CRC unimpl".into() }) }
    fn handle_crc32cx(&mut self, _i: u32, _rm: u32, _rn: u32, _rd: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CRC unimpl".into() }) }
    fn handle_ccmp_reg(&mut self, _i: u32, _sf: u32, _rm: u32, _cond: u32, _rn: u32, _nzcv: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_ccmn_reg(&mut self, _i: u32, _sf: u32, _rm: u32, _cond: u32, _rn: u32, _nzcv: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_ccmp_imm(&mut self, _i: u32, _sf: u32, _imm5: u32, _cond: u32, _rn: u32, _nzcv: u32) -> Result<(), HelmError> { Ok(()) }
    fn handle_ccmn_imm(&mut self, _i: u32, _sf: u32, _imm5: u32, _cond: u32, _rn: u32, _nzcv: u32) -> Result<(), HelmError> { Ok(()) }
}

// ═══════════════════════════════════════════════════════════════════
// Load/store handler implementation
// ═══════════════════════════════════════════════════════════════════

/// Ldst helpers on the emitter.
impl A64TcgEmitter<'_> {
    fn emit_ldr_uimm(&mut self, rn: u32, rt: u32, imm12: u32, sz: u8, sign_ext: bool, trunc32: bool) {
        let base = self.xn_sp(rn); let off = (imm12 as u64) * (sz as u64);
        let addr = self.ctx.addi(base, off as i64); let val = self.ctx.load(addr, sz);
        let val = if sign_ext { let e = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: e, src: val, from_bits: sz * 8 });
            if trunc32 { let t = self.ctx.temp(); self.ctx.emit(TcgOp::Zext { dst: t, src: e, from_bits: 32 }); t } else { e }
        } else { val };
        self.set_xn(rt, val);
    }
    fn emit_str_uimm(&mut self, rn: u32, rt: u32, imm12: u32, sz: u8) {
        let base = self.xn_sp(rn); let off = (imm12 as u64) * (sz as u64);
        let addr = self.ctx.addi(base, off as i64); let val = self.xn(rt); self.ctx.store(addr, val, sz);
    }
    fn emit_ldst_imm9(&mut self, rn: u32, rt: u32, imm9: u32, sz: u8, is_load: bool, sign_ext: bool, is_pre: bool) {
        let off = sext(imm9, 9); let base = self.xn_sp(rn);
        let (addr, wb) = if is_pre { let a = self.ctx.addi(base, off); (a, Some(a)) }
        else { (base, Some(self.ctx.addi(base, off))) };
        if is_load { let val = self.ctx.load(addr, sz);
            let val = if sign_ext { let e = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: e, src: val, from_bits: sz * 8 }); e } else { val };
            self.set_xn(rt, val);
        }
        if let Some(w) = wb { self.set_xn_sp(rn, w); }
    }
    fn emit_str_imm9(&mut self, rn: u32, rt: u32, imm9: u32, sz: u8, is_pre: bool) {
        let off = sext(imm9, 9); let base = self.xn_sp(rn);
        let (addr, wb) = if is_pre { let a = self.ctx.addi(base, off); (a, Some(a)) }
        else { (base, Some(self.ctx.addi(base, off))) };
        let val = self.xn(rt); self.ctx.store(addr, val, sz);
        if let Some(w) = wb { self.set_xn_sp(rn, w); }
    }
    fn emit_ldur(&mut self, rn: u32, rt: u32, imm9: u32, sz: u8, sign_ext: bool) {
        let off = sext(imm9, 9); let base = self.xn_sp(rn); let addr = self.ctx.addi(base, off);
        let val = self.ctx.load(addr, sz);
        let val = if sign_ext { let e = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: e, src: val, from_bits: sz * 8 }); e } else { val };
        self.set_xn(rt, val);
    }
    fn emit_stur(&mut self, rn: u32, rt: u32, imm9: u32, sz: u8) {
        let off = sext(imm9, 9); let base = self.xn_sp(rn); let addr = self.ctx.addi(base, off);
        let val = self.xn(rt); self.ctx.store(addr, val, sz);
    }
    fn emit_ldr_reg(&mut self, rn: u32, rt: u32, rm: u32, option: u32, s: u32, sz: u8, sign_ext: bool) {
        let base = self.xn_sp(rn); let log_sz = match sz { 1=>0, 2=>1, 4=>2, 8=>3, _=>0 };
        let sa = if s == 1 { log_sz } else { 0 }; let off = self.extend_reg(rm, option, sa);
        let addr = self.ctx.add(base, off); let val = self.ctx.load(addr, sz);
        let val = if sign_ext { let e = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: e, src: val, from_bits: sz * 8 }); e } else { val };
        self.set_xn(rt, val);
    }
    fn emit_str_reg(&mut self, rn: u32, rt: u32, rm: u32, option: u32, s: u32, sz: u8) {
        let base = self.xn_sp(rn); let log_sz = match sz { 1=>0, 2=>1, 4=>2, 8=>3, _=>0 };
        let sa = if s == 1 { log_sz } else { 0 }; let off = self.extend_reg(rm, option, sa);
        let addr = self.ctx.add(base, off); let val = self.xn(rt); self.ctx.store(addr, val, sz);
    }
    fn emit_simple_load(&mut self, rn: u32, rt: u32, sz: u8) {
        let base = self.xn_sp(rn); let val = self.ctx.load(base, sz); self.set_xn(rt, val);
    }
    fn emit_simple_store(&mut self, rn: u32, rt: u32, sz: u8) {
        let base = self.xn_sp(rn); let val = self.xn(rt); self.ctx.store(base, val, sz);
    }
    fn emit_ldp(&mut self, rn: u32, rt: u32, rt2: u32, imm7: u32, sz: u8, is_pre: bool, is_post: bool, sign_ext: bool) {
        let off = sext(imm7, 7) * (sz as i64); let base = self.xn_sp(rn);
        let (addr, wb) = if is_pre { let a = self.ctx.addi(base, off); (a, Some(a)) }
        else if is_post { (base, Some(self.ctx.addi(base, off))) }
        else { (self.ctx.addi(base, off), None) };
        let v0 = self.ctx.load(addr, sz); let addr2 = self.ctx.addi(addr, sz as i64); let v1 = self.ctx.load(addr2, sz);
        if sign_ext {
            let e0 = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: e0, src: v0, from_bits: sz * 8 }); self.set_xn(rt, e0);
            let e1 = self.ctx.temp(); self.ctx.emit(TcgOp::Sext { dst: e1, src: v1, from_bits: sz * 8 }); self.set_xn(rt2, e1);
        } else { self.set_xn(rt, v0); self.set_xn(rt2, v1); }
        if let Some(w) = wb { self.set_xn_sp(rn, w); }
    }
    fn emit_stp(&mut self, rn: u32, rt: u32, rt2: u32, imm7: u32, sz: u8, is_pre: bool, is_post: bool) {
        let off = sext(imm7, 7) * (sz as i64); let base = self.xn_sp(rn);
        let (addr, wb) = if is_pre { let a = self.ctx.addi(base, off); (a, Some(a)) }
        else if is_post { (base, Some(self.ctx.addi(base, off))) }
        else { (self.ctx.addi(base, off), None) };
        let v0 = self.xn(rt); self.ctx.store(addr, v0, sz);
        let addr2 = self.ctx.addi(addr, sz as i64); let v1 = self.xn(rt2); self.ctx.store(addr2, v1, sz);
        if let Some(w) = wb { self.set_xn_sp(rn, w); }
    }
}

impl DecodeAarch64LdstHandler for A64TcgEmitter<'_> {
    fn handle_ldr_x_uimm(&mut self, _i: u32, imm12: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_uimm(rn, rt, imm12, 8, false, false); Ok(()) }
    fn handle_ldr_w_uimm(&mut self, _i: u32, imm12: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_uimm(rn, rt, imm12, 4, false, false); Ok(()) }
    fn handle_ldrb_uimm(&mut self, _i: u32, imm12: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_uimm(rn, rt, imm12, 1, false, false); Ok(()) }
    fn handle_ldrh_uimm(&mut self, _i: u32, imm12: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_uimm(rn, rt, imm12, 2, false, false); Ok(()) }
    fn handle_ldrsw_uimm(&mut self, _i: u32, imm12: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_uimm(rn, rt, imm12, 4, true, false); Ok(()) }
    fn handle_ldrsb_x_uimm(&mut self, _i: u32, imm12: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_uimm(rn, rt, imm12, 1, true, false); Ok(()) }
    fn handle_ldrsb_w_uimm(&mut self, _i: u32, imm12: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_uimm(rn, rt, imm12, 1, true, true); Ok(()) }
    fn handle_ldrsh_x_uimm(&mut self, _i: u32, imm12: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_uimm(rn, rt, imm12, 2, true, false); Ok(()) }
    fn handle_ldrsh_w_uimm(&mut self, _i: u32, imm12: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_uimm(rn, rt, imm12, 2, true, true); Ok(()) }
    fn handle_str_x_uimm(&mut self, _i: u32, imm12: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_uimm(rn, rt, imm12, 8); Ok(()) }
    fn handle_str_w_uimm(&mut self, _i: u32, imm12: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_uimm(rn, rt, imm12, 4); Ok(()) }
    fn handle_strb_uimm(&mut self, _i: u32, imm12: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_uimm(rn, rt, imm12, 1); Ok(()) }
    fn handle_strh_uimm(&mut self, _i: u32, imm12: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_uimm(rn, rt, imm12, 2); Ok(()) }
    fn handle_ldr_x_post(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldst_imm9(rn, rt, imm9, 8, true, false, false); Ok(()) }
    fn handle_ldr_x_pre(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldst_imm9(rn, rt, imm9, 8, true, false, true); Ok(()) }
    fn handle_ldr_w_post(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldst_imm9(rn, rt, imm9, 4, true, false, false); Ok(()) }
    fn handle_ldr_w_pre(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldst_imm9(rn, rt, imm9, 4, true, false, true); Ok(()) }
    fn handle_ldrb_post(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldst_imm9(rn, rt, imm9, 1, true, false, false); Ok(()) }
    fn handle_ldrb_pre(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldst_imm9(rn, rt, imm9, 1, true, false, true); Ok(()) }
    fn handle_ldrh_post(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldst_imm9(rn, rt, imm9, 2, true, false, false); Ok(()) }
    fn handle_ldrh_pre(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldst_imm9(rn, rt, imm9, 2, true, false, true); Ok(()) }
    fn handle_str_x_post(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_imm9(rn, rt, imm9, 8, false); Ok(()) }
    fn handle_str_x_pre(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_imm9(rn, rt, imm9, 8, true); Ok(()) }
    fn handle_str_w_post(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_imm9(rn, rt, imm9, 4, false); Ok(()) }
    fn handle_str_w_pre(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_imm9(rn, rt, imm9, 4, true); Ok(()) }
    fn handle_strb_post(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_imm9(rn, rt, imm9, 1, false); Ok(()) }
    fn handle_strb_pre(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_imm9(rn, rt, imm9, 1, true); Ok(()) }
    fn handle_strh_post(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_imm9(rn, rt, imm9, 2, false); Ok(()) }
    fn handle_strh_pre(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_imm9(rn, rt, imm9, 2, true); Ok(()) }
    fn handle_ldrsw_post(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldst_imm9(rn, rt, imm9, 4, true, true, false); Ok(()) }
    fn handle_ldrsw_pre(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldst_imm9(rn, rt, imm9, 4, true, true, true); Ok(()) }
    fn handle_ldur_x(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldur(rn, rt, imm9, 8, false); Ok(()) }
    fn handle_ldur_w(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldur(rn, rt, imm9, 4, false); Ok(()) }
    fn handle_stur_x(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_stur(rn, rt, imm9, 8); Ok(()) }
    fn handle_stur_w(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_stur(rn, rt, imm9, 4); Ok(()) }
    fn handle_ldurb(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldur(rn, rt, imm9, 1, false); Ok(()) }
    fn handle_sturb(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_stur(rn, rt, imm9, 1); Ok(()) }
    fn handle_ldurh(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldur(rn, rt, imm9, 2, false); Ok(()) }
    fn handle_sturh(&mut self, _i: u32, imm9: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_stur(rn, rt, imm9, 2); Ok(()) }
    fn handle_ldr_x_reg(&mut self, _i: u32, rm: u32, option: u32, s: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_reg(rn, rt, rm, option, s, 8, false); Ok(()) }
    fn handle_ldr_w_reg(&mut self, _i: u32, rm: u32, option: u32, s: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_reg(rn, rt, rm, option, s, 4, false); Ok(()) }
    fn handle_ldrb_reg(&mut self, _i: u32, rm: u32, option: u32, s: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_reg(rn, rt, rm, option, s, 1, false); Ok(()) }
    fn handle_ldrh_reg(&mut self, _i: u32, rm: u32, option: u32, s: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_reg(rn, rt, rm, option, s, 2, false); Ok(()) }
    fn handle_str_x_reg(&mut self, _i: u32, rm: u32, option: u32, s: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_reg(rn, rt, rm, option, s, 8); Ok(()) }
    fn handle_str_w_reg(&mut self, _i: u32, rm: u32, option: u32, s: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_reg(rn, rt, rm, option, s, 4); Ok(()) }
    fn handle_strb_reg(&mut self, _i: u32, rm: u32, option: u32, s: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_reg(rn, rt, rm, option, s, 1); Ok(()) }
    fn handle_strh_reg(&mut self, _i: u32, rm: u32, option: u32, s: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_str_reg(rn, rt, rm, option, s, 2); Ok(()) }
    fn handle_ldrsw_reg(&mut self, _i: u32, rm: u32, option: u32, s: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldr_reg(rn, rt, rm, option, s, 4, true); Ok(()) }
    fn handle_ldr_lit_x(&mut self, _i: u32, imm19: u32, rt: u32) -> Result<(), HelmError> {
        let off = sext(imm19, 19) << 2; let a = self.ctx.movi(self.pc.wrapping_add(off as u64));
        let val = self.ctx.load(a, 8); self.set_xn(rt, val); Ok(())
    }
    fn handle_ldr_lit_w(&mut self, _i: u32, imm19: u32, rt: u32) -> Result<(), HelmError> {
        let off = sext(imm19, 19) << 2; let a = self.ctx.movi(self.pc.wrapping_add(off as u64));
        let val = self.ctx.load(a, 4); self.set_xn(rt, val); Ok(())
    }
    fn handle_ldrsw_lit(&mut self, _i: u32, imm19: u32, rt: u32) -> Result<(), HelmError> {
        let off = sext(imm19, 19) << 2; let a = self.ctx.movi(self.pc.wrapping_add(off as u64));
        let val = self.ctx.load(a, 4); let e = self.ctx.temp();
        self.ctx.emit(TcgOp::Sext { dst: e, src: val, from_bits: 32 }); self.set_xn(rt, e); Ok(())
    }
    fn handle_ldp_x(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldp(rn, rt, rt2, imm7, 8, false, false, false); Ok(()) }
    fn handle_stp_x(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_stp(rn, rt, rt2, imm7, 8, false, false); Ok(()) }
    fn handle_ldp_w(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldp(rn, rt, rt2, imm7, 4, false, false, false); Ok(()) }
    fn handle_stp_w(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_stp(rn, rt, rt2, imm7, 4, false, false); Ok(()) }
    fn handle_ldp_x_pre(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldp(rn, rt, rt2, imm7, 8, true, false, false); Ok(()) }
    fn handle_stp_x_pre(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_stp(rn, rt, rt2, imm7, 8, true, false); Ok(()) }
    fn handle_ldp_x_post(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldp(rn, rt, rt2, imm7, 8, false, true, false); Ok(()) }
    fn handle_stp_x_post(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_stp(rn, rt, rt2, imm7, 8, false, true); Ok(()) }
    fn handle_ldp_w_pre(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldp(rn, rt, rt2, imm7, 4, true, false, false); Ok(()) }
    fn handle_stp_w_pre(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_stp(rn, rt, rt2, imm7, 4, true, false); Ok(()) }
    fn handle_ldp_w_post(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldp(rn, rt, rt2, imm7, 4, false, true, false); Ok(()) }
    fn handle_stp_w_post(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_stp(rn, rt, rt2, imm7, 4, false, true); Ok(()) }
    fn handle_ldpsw(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldp(rn, rt, rt2, imm7, 4, false, false, true); Ok(()) }
    fn handle_ldpsw_pre(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldp(rn, rt, rt2, imm7, 4, true, false, true); Ok(()) }
    fn handle_ldpsw_post(&mut self, _i: u32, imm7: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_ldp(rn, rt, rt2, imm7, 4, false, true, true); Ok(()) }
    fn handle_ldxr_x(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_load(rn, rt, 8); Ok(()) }
    fn handle_stxr_x(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_store(rn, rt, 8); let z = self.ctx.movi(0); self.set_xn(rs, z); Ok(()) }
    fn handle_ldaxr_x(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_load(rn, rt, 8); Ok(()) }
    fn handle_stlxr_x(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_store(rn, rt, 8); let z = self.ctx.movi(0); self.set_xn(rs, z); Ok(()) }
    fn handle_ldxr_w(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_load(rn, rt, 4); Ok(()) }
    fn handle_stxr_w(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_store(rn, rt, 4); let z = self.ctx.movi(0); self.set_xn(rs, z); Ok(()) }
    fn handle_ldaxr_w(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_load(rn, rt, 4); Ok(()) }
    fn handle_stlxr_w(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_store(rn, rt, 4); let z = self.ctx.movi(0); self.set_xn(rs, z); Ok(()) }
    fn handle_ldxrb(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_load(rn, rt, 1); Ok(()) }
    fn handle_stxrb(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_store(rn, rt, 1); let z = self.ctx.movi(0); self.set_xn(rs, z); Ok(()) }
    fn handle_ldxrh(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_load(rn, rt, 2); Ok(()) }
    fn handle_stxrh(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_store(rn, rt, 2); let z = self.ctx.movi(0); self.set_xn(rs, z); Ok(()) }
    fn handle_ldar_x(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_load(rn, rt, 8); Ok(()) }
    fn handle_ldar_w(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_load(rn, rt, 4); Ok(()) }
    fn handle_stlr_x(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_store(rn, rt, 8); Ok(()) }
    fn handle_stlr_w(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_store(rn, rt, 4); Ok(()) }
    fn handle_ldarb(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_load(rn, rt, 1); Ok(()) }
    fn handle_stlrb(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_store(rn, rt, 1); Ok(()) }
    fn handle_ldarh(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_load(rn, rt, 2); Ok(()) }
    fn handle_stlrh(&mut self, _i: u32, rn: u32, rt: u32) -> Result<(), HelmError> { self.emit_simple_store(rn, rt, 2); Ok(()) }
    fn handle_ldxp_x(&mut self, _i: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let v0 = self.ctx.load(base, 8); self.set_xn(rt, v0); let a2 = self.ctx.addi(base, 8); let v1 = self.ctx.load(a2, 8); self.set_xn(rt2, v1); Ok(()) }
    fn handle_stxp_x(&mut self, _i: u32, rs: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let v0 = self.xn(rt); self.ctx.store(base, v0, 8); let a2 = self.ctx.addi(base, 8); let v1 = self.xn(rt2); self.ctx.store(a2, v1, 8); let z = self.ctx.movi(0); self.set_xn(rs, z); Ok(()) }
    fn handle_ldxp_w(&mut self, _i: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let v0 = self.ctx.load(base, 4); self.set_xn(rt, v0); let a2 = self.ctx.addi(base, 4); let v1 = self.ctx.load(a2, 4); self.set_xn(rt2, v1); Ok(()) }
    fn handle_stxp_w(&mut self, _i: u32, rs: u32, rt2: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let v0 = self.xn(rt); self.ctx.store(base, v0, 4); let a2 = self.ctx.addi(base, 4); let v1 = self.xn(rt2); self.ctx.store(a2, v1, 4); let z = self.ctx.movi(0); self.set_xn(rs, z); Ok(()) }
    fn handle_swp_x(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let old = self.ctx.load(base, 8); self.set_xn(rt, old); let nv = self.xn(rs); self.ctx.store(base, nv, 8); Ok(()) }
    fn handle_ldadd_x(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let old = self.ctx.load(base, 8); self.set_xn(rt, old); let s = self.xn(rs); let nv = self.ctx.add(old, s); self.ctx.store(base, nv, 8); Ok(()) }
    fn handle_ldclr_x(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let old = self.ctx.load(base, 8); self.set_xn(rt, old); let s = self.xn(rs); let inv = self.ctx.temp(); self.ctx.emit(TcgOp::Not { dst: inv, src: s }); let nv = self.ctx.temp(); self.ctx.emit(TcgOp::And { dst: nv, a: old, b: inv }); self.ctx.store(base, nv, 8); Ok(()) }
    fn handle_ldeor_x(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let old = self.ctx.load(base, 8); self.set_xn(rt, old); let s = self.xn(rs); let nv = self.ctx.temp(); self.ctx.emit(TcgOp::Xor { dst: nv, a: old, b: s }); self.ctx.store(base, nv, 8); Ok(()) }
    fn handle_ldset_x(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let old = self.ctx.load(base, 8); self.set_xn(rt, old); let s = self.xn(rs); let nv = self.ctx.temp(); self.ctx.emit(TcgOp::Or { dst: nv, a: old, b: s }); self.ctx.store(base, nv, 8); Ok(()) }
    fn handle_ldsmax_x(&mut self, _i: u32, _rs: u32, _rn: u32, _rt: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "LDSMAX unimpl".into() }) }
    fn handle_ldsmin_x(&mut self, _i: u32, _rs: u32, _rn: u32, _rt: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "LDSMIN unimpl".into() }) }
    fn handle_ldumax_x(&mut self, _i: u32, _rs: u32, _rn: u32, _rt: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "LDUMAX unimpl".into() }) }
    fn handle_ldumin_x(&mut self, _i: u32, _rs: u32, _rn: u32, _rt: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "LDUMIN unimpl".into() }) }
    fn handle_swp_w(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let old = self.ctx.load(base, 4); self.set_xn(rt, old); let nv = self.xn(rs); self.ctx.store(base, nv, 4); Ok(()) }
    fn handle_ldadd_w(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let old = self.ctx.load(base, 4); self.set_xn(rt, old); let s = self.xn(rs); let nv = self.ctx.add(old, s); self.ctx.store(base, nv, 4); Ok(()) }
    fn handle_ldclr_w(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let old = self.ctx.load(base, 4); self.set_xn(rt, old); let s = self.xn(rs); let inv = self.ctx.temp(); self.ctx.emit(TcgOp::Not { dst: inv, src: s }); let nv = self.ctx.temp(); self.ctx.emit(TcgOp::And { dst: nv, a: old, b: inv }); self.ctx.store(base, nv, 4); Ok(()) }
    fn handle_ldeor_w(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let old = self.ctx.load(base, 4); self.set_xn(rt, old); let s = self.xn(rs); let nv = self.ctx.temp(); self.ctx.emit(TcgOp::Xor { dst: nv, a: old, b: s }); self.ctx.store(base, nv, 4); Ok(()) }
    fn handle_ldset_w(&mut self, _i: u32, rs: u32, rn: u32, rt: u32) -> Result<(), HelmError> { let base = self.xn_sp(rn); let old = self.ctx.load(base, 4); self.set_xn(rt, old); let s = self.xn(rs); let nv = self.ctx.temp(); self.ctx.emit(TcgOp::Or { dst: nv, a: old, b: s }); self.ctx.store(base, nv, 4); Ok(()) }
    fn handle_cas_x(&mut self, _i: u32, _rs: u32, _rn: u32, _rt: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CAS unimpl".into() }) }
    fn handle_cas_w(&mut self, _i: u32, _rs: u32, _rn: u32, _rt: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CAS unimpl".into() }) }
    fn handle_casa_x(&mut self, _i: u32, _rs: u32, _rn: u32, _rt: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CASA unimpl".into() }) }
    fn handle_casa_w(&mut self, _i: u32, _rs: u32, _rn: u32, _rt: u32) -> Result<(), HelmError> { Err(HelmError::Decode { addr: self.pc, reason: "CASA unimpl".into() }) }
}
