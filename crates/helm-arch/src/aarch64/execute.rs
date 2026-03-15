//! AArch64 instruction execution.
//!
//! Entry point: [`execute`] — takes a decoded [`Instruction`] and mutable references
//! to [`Aarch64ArchState`] and a [`MemInterface`].
//! Returns `Ok(bool)` where `bool` indicates whether PC was written by the instruction.
//! If `false`, the caller should advance PC by 4.

use helm_core::{AccessType, HartException, MemFault, MemInterface};

use super::arch_state::Aarch64ArchState;
use super::insn::{Instruction, Opcode};

/// Execute one decoded AArch64 instruction.
///
/// Returns `Ok(pc_written)`:
/// - `true`  → instruction updated PC (branch taken, SVC, etc.)
/// - `false` → caller should advance PC by 4
///
/// Returns `Err(HartException)` on traps (SVC, BRK, undefined instruction, faults).
pub fn execute(
    insn: &Instruction,
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
) -> Result<bool, HartException> {
    use Opcode::*;

    let mut pc_written = false;

    match insn.opcode {
        // ── ADR / ADRP ──────────────────────────────────────────────────────
        Adr => {
            let val = a.pc.wrapping_add(insn.imm as u64);
            a.write_x(insn.rd, val);
        }
        Adrp => {
            let base = a.pc & !0xFFF;
            let val  = base.wrapping_add((insn.imm as u64) << 12);
            a.write_x(insn.rd, val);
        }

        // ── ADD / SUB immediate ─────────────────────────────────────────────
        AddImm => {
            let src = if insn.sf { a.read_xsp(insn.rn) } else { a.read_xsp(insn.rn) & 0xFFFF_FFFF };
            let res = src.wrapping_add(insn.imm as u64);
            if insn.sf { a.write_xsp(insn.rd, res); } else { a.write_xsp(insn.rd, res & 0xFFFF_FFFF); }
        }
        SubImm => {
            let src = if insn.sf { a.read_xsp(insn.rn) } else { a.read_xsp(insn.rn) & 0xFFFF_FFFF };
            let res = src.wrapping_sub(insn.imm as u64);
            if insn.sf { a.write_xsp(insn.rd, res); } else { a.write_xsp(insn.rd, res & 0xFFFF_FFFF); }
        }
        AddsImm => {
            let src = a.read_x(insn.rn);
            let imm = insn.imm as u64;
            let (res, c) = src.overflowing_add(imm);
            let v = add_overflow64(src, imm, res);
            if insn.sf {
                a.set_nzcv64(res, c, v);
                a.write_x(insn.rd, res);
            } else {
                let r32 = res as u32;
                a.set_nzcv(r32 >> 31 != 0, r32 == 0,
                    (src as u32).overflowing_add(imm as u32).1,
                    add_overflow32(src as u32, imm as u32, r32));
                a.write_x(insn.rd, r32 as u64);
            }
        }
        SubsImm => {
            let src = a.read_x(insn.rn);
            let imm = insn.imm as u64;
            let (res, b) = src.overflowing_sub(imm);
            let v = sub_overflow64(src, imm, res);
            if insn.sf {
                a.set_nzcv64(res, !b, v); // ARM carry = NOT borrow
                a.write_x(insn.rd, res);
            } else {
                let r32 = res as u32;
                let (_, b32) = (src as u32).overflowing_sub(imm as u32);
                a.set_nzcv(r32 >> 31 != 0, r32 == 0, !b32,
                    sub_overflow32(src as u32, imm as u32, r32));
                a.write_x(insn.rd, r32 as u64);
            }
        }

        // ── Logical immediate ───────────────────────────────────────────────
        AndImm => { binop_imm(a, insn, |x, y| x & y); }
        OrrImm => { binop_imm(a, insn, |x, y| x | y); }
        EorImm => { binop_imm(a, insn, |x, y| x ^ y); }
        AndsImm => {
            let res = binop_imm_ret(a, insn, |x, y| x & y);
            a.set_nzcv64(res, false, false);
        }

        // ── MOV wide ────────────────────────────────────────────────────────
        Movz => {
            if insn.sf { a.write_x(insn.rd, insn.imm as u64); }
            else        { a.write_w(insn.rd, insn.imm as u32); }
        }
        Movn => {
            if insn.sf { a.write_x(insn.rd, insn.imm as u64); }
            else        { a.write_w(insn.rd, insn.imm as u32); }
        }
        Movk => {
            let shift = insn.imm2 * 16;
            let mask  = !(0xFFFFu64 << shift);
            let old   = a.read_x(insn.rd);
            let val   = (old & mask) | ((insn.imm as u64 & 0xFFFF) << shift);
            if insn.sf { a.write_x(insn.rd, val); } else { a.write_w(insn.rd, val as u32); }
        }

        // ── Bitfield ────────────────────────────────────────────────────────
        Sbfm => { exec_sbfm(a, insn); }
        Ubfm => { exec_ubfm(a, insn); }
        Bfm  => { exec_bfm(a, insn); }
        Extr => {
            let immr = insn.imm as u32;
            let rs1 = a.read_x(insn.rn);
            let rs2 = a.read_x(insn.rm);
            let val = if insn.sf {
                if immr == 0 { rs1 } else { (rs1 << (64 - immr)) | (rs2 >> immr) }
            } else {
                let r = ((rs1 as u32) << (32 - immr)) | ((rs2 as u32) >> immr);
                r as u64
            };
            a.write_x(insn.rd, val);
        }

        // ── ADD/SUB register ────────────────────────────────────────────────
        AddReg | SubReg | AddsReg | SubsReg => {
            let src  = a.read_xsp(insn.rn);
            let rm   = apply_shift(a.read_x(insn.rm), insn.shift_type, insn.shift_amt, insn.sf);
            exec_addsub_reg(a, insn, src, rm)?;
        }

        // ── Logical register ────────────────────────────────────────────────
        AndReg  => { let v = log_reg(a, insn, |x,y| x & y, false); a.write_x(insn.rd, v); }
        BicReg  => { let v = log_reg(a, insn, |x,y| x &!y, false); a.write_x(insn.rd, v); }
        OrrReg  => { let v = log_reg(a, insn, |x,y| x | y, false); a.write_x(insn.rd, v); }
        OrnReg  => { let v = log_reg(a, insn, |x,y| x |!y, false); a.write_x(insn.rd, v); }
        EorReg  => { let v = log_reg(a, insn, |x,y| x ^ y, false); a.write_x(insn.rd, v); }
        EonReg  => { let v = log_reg(a, insn, |x,y| x ^!y, false); a.write_x(insn.rd, v); }
        AndsReg => { let v = log_reg(a, insn, |x,y| x & y, true);  a.write_x(insn.rd, v); }
        BicsReg => { let v = log_reg(a, insn, |x,y| x &!y, true);  a.write_x(insn.rd, v); }

        // ── Shift ───────────────────────────────────────────────────────────
        Lsl | Lsr | Asr | Ror => {
            let src = a.read_x(insn.rn);
            let sh  = (a.read_x(insn.rm) % if insn.sf { 64 } else { 32 }) as u32;
            let res = match insn.opcode {
                Lsl => if insn.sf { src << sh } else { ((src as u32) << sh) as u64 },
                Lsr => if insn.sf { src >> sh } else { ((src as u32) >> sh) as u64 },
                Asr => if insn.sf { ((src as i64) >> sh) as u64 } else { ((src as i32) >> sh) as u64 },
                Ror => if insn.sf { src.rotate_right(sh) } else { (src as u32).rotate_right(sh) as u64 },
                _ => unreachable!(),
            };
            a.write_x(insn.rd, res);
        }

        // ── Multiply ────────────────────────────────────────────────────────
        Mul | Madd => {
            let rn = a.read_x(insn.rn);
            let rm = a.read_x(insn.rm);
            let ra = if insn.opcode == Madd { a.read_x(insn.ra) } else { 0 };
            let res = if insn.sf {
                rn.wrapping_mul(rm).wrapping_add(ra)
            } else {
                ((rn as u32).wrapping_mul(rm as u32) as u64).wrapping_add(ra) & 0xFFFF_FFFF
            };
            a.write_x(insn.rd, res);
        }
        Msub | Mneg => {
            let rn = a.read_x(insn.rn);
            let rm = a.read_x(insn.rm);
            let ra = if insn.opcode == Msub { a.read_x(insn.ra) } else { 0 };
            let res = ra.wrapping_sub(rn.wrapping_mul(rm));
            a.write_x(insn.rd, res);
        }
        Smulh => {
            let rn = a.read_x(insn.rn) as i64 as i128;
            let rm = a.read_x(insn.rm) as i64 as i128;
            a.write_x(insn.rd, ((rn * rm) >> 64) as u64);
        }
        Umulh => {
            let rn = a.read_x(insn.rn) as u128;
            let rm = a.read_x(insn.rm) as u128;
            a.write_x(insn.rd, ((rn * rm) >> 64) as u64);
        }

        // ── Divide ──────────────────────────────────────────────────────────
        Udiv => {
            let rn = a.read_x(insn.rn);
            let rm = a.read_x(insn.rm);
            a.write_x(insn.rd, if rm == 0 { 0 } else { rn / rm });
        }
        Sdiv => {
            let rn = a.read_x(insn.rn) as i64;
            let rm = a.read_x(insn.rm) as i64;
            a.write_x(insn.rd, if rm == 0 { 0 } else { rn.wrapping_div(rm) as u64 });
        }

        // ── 1-source ────────────────────────────────────────────────────────
        Clz => {
            let src = a.read_x(insn.rn);
            let v = if insn.sf { src.leading_zeros() as u64 } else { (src as u32).leading_zeros() as u64 };
            a.write_x(insn.rd, v);
        }
        Cls => {
            let src = a.read_x(insn.rn);
            let v = if insn.sf {
                (src ^ (src << 1)).leading_zeros() as u64
            } else {
                ((src as u32) ^ ((src as u32) << 1)).leading_zeros() as u64 - 1
            };
            a.write_x(insn.rd, v);
        }
        Rev => {
            let src = a.read_x(insn.rn);
            let v = if insn.sf { src.swap_bytes() } else { (src as u32).swap_bytes() as u64 };
            a.write_x(insn.rd, v);
        }
        Rev16 => {
            let src = a.read_x(insn.rn);
            let v = ((src & 0xFF00_FF00_FF00_FF00) >> 8) | ((src & 0x00FF_00FF_00FF_00FF) << 8);
            a.write_x(insn.rd, v);
        }
        Rev32 => {
            let src = a.read_x(insn.rn);
            let hi  = (src >> 32) as u32;
            let lo  = src as u32;
            a.write_x(insn.rd, ((lo.swap_bytes() as u64) << 32) | hi.swap_bytes() as u64);
        }
        Rbit => {
            let src = a.read_x(insn.rn);
            let v = if insn.sf { src.reverse_bits() } else { (src as u32).reverse_bits() as u64 };
            a.write_x(insn.rd, v);
        }

        // ── ADC / SBC ────────────────────────────────────────────────────────
        Adc | Adcs => {
            let rn = a.read_x(insn.rn);
            let rm = a.read_x(insn.rm);
            let c  = a.flag_c() as u64;
            let (r1, c1) = rn.overflowing_add(rm);
            let (res, c2) = r1.overflowing_add(c);
            a.write_x(insn.rd, res);
            if insn.opcode == Adcs { a.set_nzcv64(res, c1 || c2, add_overflow64(rn, rm, res)); }
        }
        Sbc | Sbcs => {
            let rn = a.read_x(insn.rn);
            let rm = a.read_x(insn.rm);
            let c  = a.flag_c() as u64;
            let (r1, b1) = rn.overflowing_sub(rm);
            let (res, b2) = r1.overflowing_sub(1 - c);
            a.write_x(insn.rd, res);
            if insn.opcode == Sbcs { a.set_nzcv64(res, !(b1 || b2), sub_overflow64(rn, rm, res)); }
        }

        // ── Conditional select ───────────────────────────────────────────────
        Csel  => {
            let val = if a.eval_cond(insn.cond) { a.read_x(insn.rn) } else { a.read_x(insn.rm) };
            a.write_x(insn.rd, val);
        }
        Csinc => {
            let val = if a.eval_cond(insn.cond) { a.read_x(insn.rn) } else { a.read_x(insn.rm).wrapping_add(1) };
            a.write_x(insn.rd, val);
        }
        Csinv => {
            let val = if a.eval_cond(insn.cond) { a.read_x(insn.rn) } else { !a.read_x(insn.rm) };
            a.write_x(insn.rd, val);
        }
        Csneg => {
            let val = if a.eval_cond(insn.cond) { a.read_x(insn.rn) } else { a.read_x(insn.rm).wrapping_neg() };
            a.write_x(insn.rd, val);
        }

        // ── Conditional compare ──────────────────────────────────────────────
        Ccmp | Ccmn => {
            if a.eval_cond(insn.cond) {
                let rn  = a.read_x(insn.rn);
                let rm  = if insn.rm == 0 && insn.imm != 0 { insn.imm as u64 } else { a.read_x(insn.rm) };
                let (res, b) = if insn.opcode == Ccmp {
                    rn.overflowing_sub(rm)
                } else {
                    rn.overflowing_add(rm)
                };
                let v = if insn.opcode == Ccmp { sub_overflow64(rn, rm, res) } else { add_overflow64(rn, rm, res) };
                let c = if insn.opcode == Ccmp { !b } else { b };
                a.set_nzcv64(res, c, v);
            } else {
                // Use nzcv_imm directly
                a.nzcv = insn.nzcv_imm << 28;
            }
        }

        // ── Load/Store ───────────────────────────────────────────────────────
        Ldr | Ldrb | Ldrh | Ldrsb | Ldrsh | Ldrsw
        | Ldur | Ldurb | Ldurh | Ldursb | Ldursh | Ldursw => {
            let base = a.read_xsp(insn.rn);
            let ea   = compute_ea(a, base, insn);
            writeback_pre(a, insn, base, ea);
            let (sz, signed) = ldst_size(insn.opcode);
            let raw = mem.read(ea, sz, AccessType::Load)
                .map_err(|e| mem_fault_load(e, ea))?;
            let val = if signed { sign_extend(raw, sz) } else { raw };
            a.write_x(insn.rd, val);
            writeback_post(a, insn, ea);
        }
        Str | Strb | Strh | Stur | Sturb | Sturh => {
            let base = a.read_xsp(insn.rn);
            let ea   = compute_ea(a, base, insn);
            writeback_pre(a, insn, base, ea);
            let (sz, _) = ldst_size(insn.opcode);
            let val = a.read_x(insn.rd);
            mem.write(ea, sz, val, AccessType::Store)
                .map_err(|e| mem_fault_store(e, ea))?;
            writeback_post(a, insn, ea);
        }
        Ldp => {
            let base = a.read_xsp(insn.rn);
            let ea   = compute_ea(a, base, insn);
            writeback_pre(a, insn, base, ea);
            let sz   = if insn.sf { 8 } else { 4 };
            let v1   = mem.read(ea,      sz, AccessType::Load).map_err(|e| mem_fault_load(e, ea))?;
            let v2   = mem.read(ea + sz as u64, sz, AccessType::Load).map_err(|e| mem_fault_load(e, ea))?;
            let (v1, v2) = if insn.signed_load {
                (sign_extend(v1, sz), sign_extend(v2, sz))
            } else { (v1, v2) };
            a.write_x(insn.rd, v1);
            a.write_x(insn.pair_second, v2);
            writeback_post(a, insn, ea);
        }
        Stp => {
            let base = a.read_xsp(insn.rn);
            let ea   = compute_ea(a, base, insn);
            writeback_pre(a, insn, base, ea);
            let sz   = if insn.sf { 8 } else { 4 };
            let v1   = a.read_x(insn.rd);
            let v2   = a.read_x(insn.pair_second);
            mem.write(ea,      sz, v1, AccessType::Store).map_err(|e| mem_fault_store(e, ea))?;
            mem.write(ea + sz as u64, sz, v2, AccessType::Store).map_err(|e| mem_fault_store(e, ea))?;
            writeback_post(a, insn, ea);
        }

        // ── Exclusive (stub — SE mode doesn't need true exclusives) ─────────
        Ldxr | Ldaxr => {
            let base = a.read_xsp(insn.rn);
            let sz   = if insn.sf { 8 } else { 4 };
            let val  = mem.read(base, sz, AccessType::Atomic).map_err(|e| mem_fault_load(e, base))?;
            a.write_x(insn.rd, val);
        }
        Stxr | Stlxr => {
            let base = a.read_xsp(insn.rn);
            let sz   = if insn.sf { 8 } else { 4 };
            let val  = a.read_x(insn.rd);
            mem.write(base, sz, val, AccessType::Atomic).map_err(|e| mem_fault_store(e, base))?;
            a.write_x(insn.rm, 0); // success
        }
        Clrex => { /* no-op in SE mode */ }

        // ── Branches ────────────────────────────────────────────────────────
        B => {
            a.pc = a.pc.wrapping_add(insn.imm as u64);
            pc_written = true;
        }
        Bl => {
            a.write_x(30, a.pc.wrapping_add(4)); // LR = PC+4
            a.pc = a.pc.wrapping_add(insn.imm as u64);
            pc_written = true;
        }
        Br => {
            a.pc = a.read_x(insn.rn);
            pc_written = true;
        }
        Blr => {
            a.write_x(30, a.pc.wrapping_add(4));
            a.pc = a.read_x(insn.rn);
            pc_written = true;
        }
        Ret => {
            a.pc = a.read_x(insn.rn); // default rn=30 (LR)
            pc_written = true;
        }
        BCond => {
            if a.eval_cond(insn.cond) {
                a.pc = a.pc.wrapping_add(insn.imm as u64);
                pc_written = true;
            }
        }
        Cbz => {
            if a.read_x(insn.rd) == 0 {
                a.pc = a.pc.wrapping_add(insn.imm as u64);
                pc_written = true;
            }
        }
        Cbnz => {
            if a.read_x(insn.rd) != 0 {
                a.pc = a.pc.wrapping_add(insn.imm as u64);
                pc_written = true;
            }
        }
        Tbz => {
            if a.read_x(insn.rn) & (1 << insn.imm2) == 0 {
                a.pc = a.pc.wrapping_add(insn.imm as u64);
                pc_written = true;
            }
        }
        Tbnz => {
            if a.read_x(insn.rn) & (1 << insn.imm2) != 0 {
                a.pc = a.pc.wrapping_add(insn.imm as u64);
                pc_written = true;
            }
        }

        // ── System / SVC ─────────────────────────────────────────────────────
        Svc => {
            // Raise EnvironmentCall with imm16 in imm field.
            // The engine dispatch on HartException will call the syscall handler.
            return Err(HartException::EnvironmentCall {
                pc: a.pc,
                nr: a.x[8], // AArch64 Linux: syscall nr in X8
            });
        }
        Brk => {
            return Err(HartException::Breakpoint { pc: a.pc });
        }
        Nop | Wfi | Wfe | Sev | Sevl => { /* no-op in SE mode */ }
        Dmb | Dsb | Isb => { /* memory barriers — no-op in SE single-threaded mode */ }
        Eret => {
            // In SE mode this shouldn't happen but handle gracefully
            a.pc = a.elr_el1;
            pc_written = true;
        }
        Hvc | Smc => {
            return Err(HartException::Unsupported);
        }

        // ── MRS / MSR ────────────────────────────────────────────────────────
        Mrs => {
            let val = read_sysreg(a, insn.imm as u32);
            a.write_x(insn.rd, val);
        }
        Msr => {
            // Immediate MSR (PSTATE fields): check if Rn encodes a field
            let val = a.read_x(insn.rd); // Rt is actually in rd field for MSR
            write_sysreg(a, insn.imm as u32, val);
        }
        Sys => { /* TLBI, DC, IC: no-op in SE mode */ }

        // ── FP ───────────────────────────────────────────────────────────────
        FmovReg => {
            // FMOV between FP registers
            let src = a.v[insn.rn as usize];
            a.v[insn.rd as usize] = src;
        }
        FmovGpr => {
            // FMOV between GPR and FP register
            // Direction encoded in raw bits; decode simplistically here
            if insn.sf {
                // FMOV Xd, Fn or FMOV Fd, Xn
                let to_fp = (insn.raw >> 16) & 1 == 0;
                if to_fp {
                    a.v[insn.rd as usize] = a.read_x(insn.rn) as u128;
                } else {
                    a.write_x(insn.rd, a.v[insn.rn as usize] as u64);
                }
            } else {
                let to_fp = (insn.raw >> 16) & 1 == 0;
                if to_fp {
                    a.v[insn.rd as usize] = a.read_w(insn.rn) as u128;
                } else {
                    a.write_w(insn.rd, a.v[insn.rn as usize] as u32);
                }
            }
        }
        FmovImm => {
            // 8-bit immediate to FP register
            let imm8 = insn.imm as u32;
            let f32_val = fp_imm8_to_f32(imm8);
            if insn.ftype == 1 {
                a.v[insn.rd as usize] = (f64::from(f32_val)).to_bits() as u128;
            } else {
                a.v[insn.rd as usize] = f32_val.to_bits() as u128;
            }
        }
        Fadd | Fsub | Fmul | Fdiv | Fmax | Fmin | Fmaxnm | Fminnm => {
            exec_fp_binary(a, insn);
        }
        Fsqrt | Fabs | Fneg => {
            exec_fp_unary(a, insn);
        }
        Fcmp | Fcmpe => {
            exec_fcmp(a, insn);
        }
        Fcvt => {
            exec_fcvt(a, insn);
        }
        FcvtzsGpr | FcvtzuGpr | ScvtfGpr | UcvtfGpr => {
            exec_fp_gpr_convert(a, insn);
        }
        Fmadd | Fmsub | Fnmadd | Fnmsub => {
            exec_fp_fused(a, insn);
        }
        Fsel => {
            let val = if a.eval_cond(insn.cond) {
                a.v[insn.rn as usize]
            } else {
                a.v[insn.rm as usize]
            };
            a.v[insn.rd as usize] = val;
        }

        SimdOther | SimdAdd | SimdSub | SimdMul | SimdAnd | SimdOrr | SimdEor | SimdBic
        | SimdLd1 | SimdSt1 | FcvtzsVec | FcvtzuVec => {
            // Unimplemented SIMD — silently skip for now.
            // TODO(phase-2): implement SIMD integer and vector operations
        }

        Undefined => {
            return Err(HartException::IllegalInstruction { pc: a.pc, raw: insn.raw });
        }
    }

    Ok(pc_written)
}

// ── Helpers: arithmetic ───────────────────────────────────────────────────────

#[inline]
fn add_overflow64(a: u64, b: u64, res: u64) -> bool {
    ((!(a ^ b)) & (a ^ res)) >> 63 != 0
}
#[inline]
fn sub_overflow64(a: u64, b: u64, res: u64) -> bool {
    (((a ^ b)) & (a ^ res)) >> 63 != 0
}
#[inline]
fn add_overflow32(a: u32, b: u32, res: u32) -> bool {
    ((!(a ^ b)) & (a ^ res)) >> 31 != 0
}
#[inline]
fn sub_overflow32(a: u32, b: u32, res: u32) -> bool {
    (((a ^ b)) & (a ^ res)) >> 31 != 0
}

#[inline]
fn sign_extend(v: u64, size: usize) -> u64 {
    let shift = 64 - size * 8;
    ((v as i64) << shift >> shift) as u64
}

fn apply_shift(val: u64, stype: u32, amt: u32, sf: bool) -> u64 {
    let amt = amt & if sf { 63 } else { 31 };
    match stype {
        0 => val << amt,
        1 => val >> amt,
        2 => ((val as i64) >> amt) as u64,
        3 => val.rotate_right(amt),
        _ => val,
    }
}

// ── Helpers: binary ops ───────────────────────────────────────────────────────

fn binop_imm(a: &mut Aarch64ArchState, i: &Instruction, f: impl Fn(u64, u64) -> u64) {
    let src = a.read_xsp(i.rn);
    let res = f(src, i.imm as u64);
    if i.sf { a.write_xsp(i.rd, res); } else { a.write_xsp(i.rd, (res as u32) as u64); }
}

fn binop_imm_ret(a: &mut Aarch64ArchState, i: &Instruction, f: impl Fn(u64, u64) -> u64) -> u64 {
    let src = a.read_xsp(i.rn);
    let res = f(src, i.imm as u64);
    if i.sf { a.write_x(i.rd, res); } else { a.write_x(i.rd, (res as u32) as u64); }
    res
}

fn log_reg(a: &mut Aarch64ArchState, i: &Instruction, f: impl Fn(u64, u64) -> u64, setf: bool) -> u64 {
    let rn  = a.read_x(i.rn);
    let rm  = apply_shift(a.read_x(i.rm), i.shift_type, i.shift_amt, i.sf);
    let res = f(rn, rm);
    let res = if i.sf { res } else { (res as u32) as u64 };
    if setf { a.set_nzcv64(res, false, false); }
    res
}

fn exec_addsub_reg(
    a: &mut Aarch64ArchState,
    i: &Instruction,
    src: u64,
    rm: u64,
) -> Result<(), HartException> {
    let (res, c, v) = match i.opcode {
        Opcode::AddReg | Opcode::AddsReg => {
            let (r, c) = src.overflowing_add(rm);
            (r, c, add_overflow64(src, rm, r))
        }
        _ => {
            let (r, b) = src.overflowing_sub(rm);
            (r, !b, sub_overflow64(src, rm, r))
        }
    };
    let res = if i.sf { res } else { (res as u32) as u64 };
    a.write_xsp(i.rd, res);
    if matches!(i.opcode, Opcode::AddsReg | Opcode::SubsReg) {
        a.set_nzcv64(res, c, v);
    }
    Ok(())
}

// ── Helpers: bitfield ────────────────────────────────────────────────────────

fn exec_sbfm(a: &mut Aarch64ArchState, i: &Instruction) {
    let immr = i.imm as u32;
    let imms = i.imm2 as u32;
    let src = a.read_x(i.rn);
    let len = if i.sf { 64u32 } else { 32 };
    let val = if imms >= immr {
        // Copy bits [imms:immr] and sign-extend
        let width = imms - immr + 1;
        let extracted = (src >> immr) & ((1u64 << width) - 1);
        sign_extend(extracted, width as usize)
    } else {
        // Rotate + sign extend
        let width = imms + 1;
        let shifted = src.rotate_right(immr) & ((1u64 << width) - 1);
        sign_extend(shifted, width as usize)
    };
    a.write_x(i.rd, val);
}

fn exec_ubfm(a: &mut Aarch64ArchState, i: &Instruction) {
    let immr = i.imm as u32;
    let imms = i.imm2 as u32;
    let src = a.read_x(i.rn);
    let val = if imms >= immr {
        let width = imms - immr + 1;
        (src >> immr) & ((1u64 << width) - 1)
    } else {
        let width = imms + 1;
        (src.rotate_right(immr)) & ((1u64 << width) - 1)
    };
    a.write_x(i.rd, val);
}

fn exec_bfm(a: &mut Aarch64ArchState, i: &Instruction) {
    let immr = i.imm as u32;
    let imms = i.imm2 as u32;
    let src  = a.read_x(i.rn);
    let dst  = a.read_x(i.rd);
    let width = if imms >= immr { imms - immr + 1 } else { imms + 1 };
    let mask = (1u64 << width) - 1;
    let extracted = if imms >= immr { (src >> immr) & mask } else { src & mask };
    let shift = if imms >= immr { 0 } else { (64 - immr) & 63 };
    let val = (dst & !(mask << shift)) | ((extracted & mask) << shift);
    a.write_x(i.rd, val);
}

// ── Helpers: load/store address ───────────────────────────────────────────────

fn compute_ea(a: &Aarch64ArchState, base: u64, i: &Instruction) -> u64 {
    if i.extend_type != 0 || i.rm != 0 && !i.post_index {
        // Register offset
        let rm = a.read_x(i.rm);
        let ext = apply_extend(rm, i.extend_type, i.extend_amt);
        base.wrapping_add(ext)
    } else if i.post_index {
        base // effective address is base; writeback applies offset after
    } else {
        base.wrapping_add(i.imm as u64)
    }
}

fn apply_extend(val: u64, etype: u32, amt: u32) -> u64 {
    let extended = match etype {
        0 => val & 0xFF,            // UXTB
        1 => val & 0xFFFF,          // UXTH
        2 => val & 0xFFFF_FFFF,     // UXTW / LSL
        3 => val,                   // UXTX / LSL64
        4 => (val as i8) as u64,    // SXTB
        5 => (val as i16) as u64,   // SXTH
        6 => (val as i32) as u64,   // SXTW
        7 => val,                   // SXTX
        _ => val,
    };
    extended << amt
}

fn writeback_pre(a: &mut Aarch64ArchState, i: &Instruction, base: u64, ea: u64) {
    if i.pre_index { a.write_xsp(i.rn, ea); }
}

fn writeback_post(a: &mut Aarch64ArchState, i: &Instruction, ea: u64) {
    if i.post_index {
        let new_base = ea.wrapping_add(i.imm as u64);
        a.write_xsp(i.rn, new_base);
    }
}

fn ldst_size(op: Opcode) -> (usize, bool) {
    match op {
        Opcode::Ldrb  | Opcode::Strb  | Opcode::Ldurb  | Opcode::Sturb  => (1, false),
        Opcode::Ldrsb | Opcode::Ldursb                                    => (1, true),
        Opcode::Ldrh  | Opcode::Strh  | Opcode::Ldurh  | Opcode::Sturh  => (2, false),
        Opcode::Ldrsh | Opcode::Ldursh                                    => (2, true),
        Opcode::Ldrsw | Opcode::Ldursw                                    => (4, true),
        _                                                                  => (8, false),
    }
}

// ── Helpers: system registers ─────────────────────────────────────────────────

fn read_sysreg(a: &Aarch64ArchState, encoded: u32) -> u64 {
    // Decode: [15:14]=op0, [13:11]=op1, [10:7]=CRn, [6:3]=CRm, [2:0]=op2
    // Common system registers in SE mode:
    match encoded {
        // TPIDR_EL0
        0b11_011_1101_0000_010 => a.tpidr_el0,
        // NZCV
        0b11_011_0100_0010_000 => a.nzcv as u64,
        // FPCR
        0b11_011_0100_0100_000 => a.fpcr as u64,
        // FPSR
        0b11_011_0100_0100_001 => a.fpsr as u64,
        // CTR_EL0 (cache type register)
        0b11_011_0000_0000_001 => 0x8444_C004,
        // DCZID_EL0
        0b11_011_0000_0000_111 => 0x0000_0004, // DC ZVA block size = 2^(4+1) = 64 bytes
        // CNTVCT_EL0
        0b11_011_1110_0000_010 => a.cntvct_el0,
        // CNTFRQ_EL0
        0b11_011_1110_0000_000 => a.cntfrq_el0,
        // MIDR_EL1
        0b11_000_0000_0000_000 => a.midr_el1,
        // MPIDR_EL1
        0b11_000_0000_0000_101 => a.mpidr_el1,
        // ID_AA64PFR0_EL1
        0b11_000_0000_0100_000 => a.id_aa64pfr0_el1,
        // ID_AA64ISAR0_EL1
        0b11_000_0000_0110_000 => a.id_aa64isar0_el1,
        // ID_AA64MMFR0_EL1
        0b11_000_0000_0111_000 => a.id_aa64mmfr0_el1,
        // SCTLR_EL1
        0b11_000_0001_0000_000 => a.sctlr_el1,
        // Unknown — return 0
        _ => 0,
    }
}

fn write_sysreg(a: &mut Aarch64ArchState, encoded: u32, val: u64) {
    match encoded {
        0b11_011_1101_0000_010 => a.tpidr_el0 = val,
        0b11_011_0100_0010_000 => a.nzcv = val as u32,
        0b11_011_0100_0100_000 => a.fpcr = val as u32,
        0b11_011_0100_0100_001 => a.fpsr = val as u32,
        0b11_000_0001_0000_000 => a.sctlr_el1 = val,
        0b11_000_0010_0000_000 => a.tcr_el1 = val,
        0b11_000_0010_0000_001 => a.ttbr0_el1 = val,
        0b11_000_0010_0000_011 => a.ttbr1_el1 = val,
        0b11_000_1100_0000_000 => a.vbar_el1 = val,
        0b11_000_1010_0010_000 => a.mair_el1 = val,
        _ => { /* ignore writes to unknown registers */ }
    }
}

// ── Helpers: FP ──────────────────────────────────────────────────────────────

fn fp_imm8_to_f32(imm8: u32) -> f32 {
    // ARM VFP 8-bit FP immediate: sign(1) exp(4) mantissa(3)
    let sign  = (imm8 >> 7) & 1;
    let exp4  = (imm8 >> 4) & 0xF;
    let mant3 = imm8 & 0x7;
    let exp = if exp4 & 0x8 != 0 { (exp4 | 0xFFFF_FFF8) as i32 } else { exp4 as i32 };
    let exp_biased = (exp + 127) as u32;
    let bits = (sign << 31) | ((exp_biased & 0xFF) << 23) | (mant3 << 20);
    f32::from_bits(bits)
}

fn exec_fp_binary(a: &mut Aarch64ArchState, i: &Instruction) {
    if i.ftype == 1 {
        // Double precision
        let rn = f64::from_bits(a.v[i.rn as usize] as u64);
        let rm = f64::from_bits(a.v[i.rm as usize] as u64);
        let res: f64 = match i.opcode {
            Opcode::Fadd   => rn + rm,
            Opcode::Fsub   => rn - rm,
            Opcode::Fmul   => rn * rm,
            Opcode::Fdiv   => rn / rm,
            Opcode::Fmax   => if rn >= rm { rn } else { rm },
            Opcode::Fmin   => if rn <= rm { rn } else { rm },
            Opcode::Fmaxnm => rn.max(rm),
            Opcode::Fminnm => rn.min(rm),
            _ => 0.0,
        };
        a.v[i.rd as usize] = res.to_bits() as u128;
    } else {
        // Single precision
        let rn = f32::from_bits(a.v[i.rn as usize] as u32);
        let rm = f32::from_bits(a.v[i.rm as usize] as u32);
        let res: f32 = match i.opcode {
            Opcode::Fadd   => rn + rm,
            Opcode::Fsub   => rn - rm,
            Opcode::Fmul   => rn * rm,
            Opcode::Fdiv   => rn / rm,
            Opcode::Fmax   => if rn >= rm { rn } else { rm },
            Opcode::Fmin   => if rn <= rm { rn } else { rm },
            Opcode::Fmaxnm => rn.max(rm),
            Opcode::Fminnm => rn.min(rm),
            _ => 0.0,
        };
        a.v[i.rd as usize] = res.to_bits() as u128;
    }
}

fn exec_fp_unary(a: &mut Aarch64ArchState, i: &Instruction) {
    if i.ftype == 1 {
        let rn = f64::from_bits(a.v[i.rn as usize] as u64);
        let res: f64 = match i.opcode {
            Opcode::Fsqrt => rn.sqrt(),
            Opcode::Fabs  => rn.abs(),
            Opcode::Fneg  => -rn,
            _ => rn,
        };
        a.v[i.rd as usize] = res.to_bits() as u128;
    } else {
        let rn = f32::from_bits(a.v[i.rn as usize] as u32);
        let res: f32 = match i.opcode {
            Opcode::Fsqrt => rn.sqrt(),
            Opcode::Fabs  => rn.abs(),
            Opcode::Fneg  => -rn,
            _ => rn,
        };
        a.v[i.rd as usize] = res.to_bits() as u128;
    }
}

fn exec_fcmp(a: &mut Aarch64ArchState, i: &Instruction) {
    let (rn_is_zero, z, n, c, v) = if i.ftype == 1 {
        let rn = f64::from_bits(a.v[i.rn as usize] as u64);
        let rm = f64::from_bits(a.v[i.rm as usize] as u64);
        let unordered = rn.is_nan() || rm.is_nan();
        (false, rn == rm, rn < rm, !(rn < rm) || unordered, unordered)
    } else {
        let rn = f32::from_bits(a.v[i.rn as usize] as u32);
        let rm = f32::from_bits(a.v[i.rm as usize] as u32);
        let unordered = rn.is_nan() || rm.is_nan();
        (false, rn == rm, rn < rm, !(rn < rm) || unordered, unordered)
    };
    a.set_nzcv(n, z, c, v);
}

fn exec_fcvt(a: &mut Aarch64ArchState, i: &Instruction) {
    // FCVT between FP sizes — simplified
    if i.ftype == 0 && (i.raw >> 15) & 3 == 1 {
        // SP → DP
        let rn = f32::from_bits(a.v[i.rn as usize] as u32);
        a.v[i.rd as usize] = f64::from(rn).to_bits() as u128;
    } else if i.ftype == 1 && (i.raw >> 15) & 3 == 0 {
        // DP → SP
        let rn = f64::from_bits(a.v[i.rn as usize] as u64);
        a.v[i.rd as usize] = (rn as f32).to_bits() as u128;
    }
}

fn exec_fp_gpr_convert(a: &mut Aarch64ArchState, i: &Instruction) {
    match i.opcode {
        Opcode::FcvtzsGpr => {
            let rn = f64::from_bits(a.v[i.rn as usize] as u64);
            a.write_x(i.rd, rn as i64 as u64);
        }
        Opcode::FcvtzuGpr => {
            let rn = f64::from_bits(a.v[i.rn as usize] as u64);
            a.write_x(i.rd, rn as u64);
        }
        Opcode::ScvtfGpr => {
            let rn = a.read_x(i.rn) as i64 as f64;
            a.v[i.rd as usize] = rn.to_bits() as u128;
        }
        Opcode::UcvtfGpr => {
            let rn = a.read_x(i.rn) as f64;
            a.v[i.rd as usize] = rn.to_bits() as u128;
        }
        _ => {}
    }
}

fn exec_fp_fused(a: &mut Aarch64ArchState, i: &Instruction) {
    if i.ftype == 1 {
        let rn = f64::from_bits(a.v[i.rn as usize] as u64);
        let rm = f64::from_bits(a.v[i.rm as usize] as u64);
        let ra = f64::from_bits(a.v[i.ra as usize] as u64);
        let res = match i.opcode {
            Opcode::Fmadd  =>  rn * rm + ra,
            Opcode::Fmsub  => -rn * rm + ra,
            Opcode::Fnmadd => -rn * rm - ra,
            Opcode::Fnmsub =>  rn * rm - ra,
            _ => 0.0,
        };
        a.v[i.rd as usize] = res.to_bits() as u128;
    }
}

// ── Memory fault conversion ───────────────────────────────────────────────────

fn mem_fault_load(e: MemFault, addr: u64) -> HartException {
    let _ = e;
    HartException::LoadAccessFault { addr }
}
fn mem_fault_store(e: MemFault, addr: u64) -> HartException {
    let _ = e;
    HartException::StoreAccessFault { addr }
}
