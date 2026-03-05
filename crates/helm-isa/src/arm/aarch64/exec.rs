#![allow(clippy::unusual_byte_groupings)]
//! AArch64 instruction executor for SE/FE mode.
//!
//! Fetch-decode-execute loop operating directly on `Aarch64Regs`
//! and `AddressSpace`.  No pipeline modelling — this is the FE path.

#![allow(clippy::unnecessary_cast, clippy::identity_op)]

use crate::arm::regs::Aarch64Regs;
use helm_core::types::Addr;
use helm_core::HelmResult;
use helm_memory::address_space::AddressSpace;

/// Execution state for one AArch64 vCPU.
pub struct Aarch64Cpu {
    pub regs: Aarch64Regs,
    pub halted: bool,
    pub exit_code: u64,
    pc_written: bool,
}

impl Aarch64Cpu {
    pub fn new() -> Self {
        Self {
            regs: Aarch64Regs::default(),
            halted: false,
            exit_code: 0,
            pc_written: false,
        }
    }

    pub fn xn(&self, n: u16) -> u64 {
        if n >= 31 {
            0
        } else {
            self.regs.x[n as usize]
        }
    }
    /// Read Xn or SP. Reg 31 = SP (not XZR). For base address registers.
    pub fn xn_sp(&self, n: u16) -> u64 {
        if n == 31 {
            self.regs.sp
        } else {
            self.regs.x[n as usize]
        }
    }
    pub fn set_xn(&mut self, n: u16, val: u64) {
        if n < 31 {
            self.regs.x[n as usize] = val;
        }
    }
    /// Write Xn or SP.
    pub fn set_xn_sp(&mut self, n: u16, val: u64) {
        if n == 31 {
            self.regs.sp = val;
        } else if n < 31 {
            self.regs.x[n as usize] = val;
        }
    }
    pub fn wn(&self, n: u16) -> u32 {
        self.xn(n) as u32
    }
    pub fn set_wn(&mut self, n: u16, val: u32) {
        self.set_xn(n, val as u64);
    }

    pub fn step(&mut self, mem: &mut AddressSpace) -> HelmResult<()> {
        let pc = self.regs.pc;
        let mut buf = [0u8; 4];
        mem.read(pc, &mut buf)?;
        let insn = u32::from_le_bytes(buf);
        self.pc_written = false;
        self.exec(pc, insn, mem)?;
        if !self.pc_written {
            self.regs.pc += 4;
        }
        Ok(())
    }

    fn exec(&mut self, pc: Addr, insn: u32, mem: &mut AddressSpace) -> HelmResult<()> {
        let op0 = (insn >> 25) & 0xF;
        match op0 {
            0b1000 | 0b1001 => self.exec_dp_imm(pc, insn),
            0b1010 | 0b1011 => self.exec_branch_sys(pc, insn),
            0b0100 | 0b0110 | 0b1100 | 0b1110 => self.exec_ldst(insn, mem),
            0b0101 | 0b1101 => self.exec_dp_reg(insn),
            0b0111 | 0b1111 => self.exec_simd_dp(insn),
            _ => {
                log::trace!(
                    "unimpl encoding group op0={:#06b} at PC={:#x} insn={:#010x}",
                    op0,
                    pc,
                    insn
                );
                Ok(())
            }
        }
    }

    // === Data Processing — Immediate ===
    fn exec_dp_imm(&mut self, pc: Addr, insn: u32) -> HelmResult<()> {
        let sf = (insn >> 31) & 1;
        let op_hi = (insn >> 23) & 0x7;
        match op_hi {
            0b000 | 0b001 => {
                // ADR / ADRP
                let rd = (insn & 0x1F) as u16;
                let immlo = ((insn >> 29) & 0x3) as u64;
                let immhi = sext((insn >> 5) & 0x7FFFF, 19) as u64;
                let imm = (immhi << 2) | immlo;
                if (insn >> 31) & 1 == 1 {
                    self.set_xn(rd, (pc & !0xFFF).wrapping_add(imm << 12));
                } else {
                    self.set_xn(rd, pc.wrapping_add(imm));
                }
            }
            0b010 | 0b011 => {
                // ADD/SUB immediate
                let op = (insn >> 30) & 1;
                let s = (insn >> 29) & 1;
                let sh = (insn >> 22) & 0x3;
                let imm12 = ((insn >> 10) & 0xFFF) as u64;
                let rn = ((insn >> 5) & 0x1F) as u16;
                let rd = (insn & 0x1F) as u16;
                let imm = if sh == 1 { imm12 << 12 } else { imm12 };
                let a = self.xn_sp(rn);
                let (r, c, v) = if op == 0 {
                    awc(a, imm, false, sf == 1)
                } else {
                    awc(a, !imm, true, sf == 1)
                };
                let r = mask(r, sf);
                if s == 1 {
                    self.flags(r, c, v, sf == 1);
                }
                if s == 0 {
                    self.set_xn_sp(rd, r);
                } else {
                    self.set_xn(rd, r);
                }
            }
            0b100 => {
                // Logical immediate
                let opc = (insn >> 29) & 0x3;
                let n = (insn >> 22) & 1;
                let immr = ((insn >> 16) & 0x3F) as u32;
                let imms = ((insn >> 10) & 0x3F) as u32;
                let rn = ((insn >> 5) & 0x1F) as u16;
                let rd = (insn & 0x1F) as u16;
                let imm = decode_bitmask(n, imms, immr, sf == 1);
                let a = self.xn(rn);
                let r = match opc {
                    0 => a & imm,
                    1 => a | imm,
                    2 => a ^ imm,
                    3 => {
                        let r = mask(a & imm, sf);
                        self.flags(r, false, false, sf == 1);
                        r
                    }
                    _ => a,
                };
                let r = mask(r, sf);
                // AND/ORR/EOR: Rd=31 means SP. ANDS: Rd=31 means XZR.
                if opc == 3 {
                    self.set_xn(rd, r);
                } else {
                    self.set_xn_sp(rd, r);
                }
            }
            0b101 => {
                // MOVN / MOVZ / MOVK
                let opc = (insn >> 29) & 0x3;
                let hw = ((insn >> 21) & 0x3) as u64;
                let imm16 = ((insn >> 5) & 0xFFFF) as u64;
                let rd = (insn & 0x1F) as u16;
                let shift = hw * 16;
                match opc {
                    0 => self.set_xn(rd, !(imm16 << shift)),
                    2 => self.set_xn(rd, imm16 << shift),
                    3 => {
                        let old = self.xn(rd);
                        let m = !(0xFFFFu64 << shift);
                        self.set_xn(rd, (old & m) | (imm16 << shift));
                    }
                    _ => {}
                }
            }
            0b110 => {
                // SBFM / BFM / UBFM
                let opc = (insn >> 29) & 0x3;
                let immr = ((insn >> 16) & 0x3F) as u32;
                let imms = ((insn >> 10) & 0x3F) as u32;
                let rn = ((insn >> 5) & 0x1F) as u16;
                let rd = (insn & 0x1F) as u16;
                let src = self.xn(rn);
                let r = match opc {
                    0 => {
                        // SBFM
                        if imms >= immr {
                            // SBFX / ASR: extract and sign-extend
                            let w = imms - immr + 1;
                            sext64(
                                (src >> immr) & (if w >= 64 { u64::MAX } else { (1u64 << w) - 1 }),
                                w,
                            )
                        } else {
                            // SXTB/SXTH/SXTW / shift-insert:
                            // extract low (imms+1) bits, shift left, sign-extend
                            let esize = if sf == 1 { 64u32 } else { 32 };
                            let w = imms + 1;
                            let bits = src & (if w >= 64 { u64::MAX } else { (1u64 << w) - 1 });
                            let shifted = bits << (esize - immr);
                            sext64(shifted, esize)
                        }
                    }
                    2 => {
                        // UBFM
                        if imms >= immr {
                            // UBFX / LSR: extract bitfield
                            let w = imms - immr + 1;
                            (src >> immr) & (if w >= 64 { u64::MAX } else { (1u64 << w) - 1 })
                        } else {
                            // LSL / zero-insert: extract low (imms+1) bits,
                            // shift left by (esize - immr)
                            let esize = if sf == 1 { 64u32 } else { 32 };
                            let w = imms + 1;
                            let bits = src & (if w >= 64 { u64::MAX } else { (1u64 << w) - 1 });
                            bits << (esize - immr)
                        }
                    }
                    _ => src,
                };
                self.set_xn(rd, mask(r, sf));
            }
            0b111 => {
                // EXTR
                let rm = ((insn >> 16) & 0x1F) as u16;
                let imms = ((insn >> 10) & 0x3F) as u32;
                let rn = ((insn >> 5) & 0x1F) as u16;
                let rd = (insn & 0x1F) as u16;
                let hi = self.xn(rn);
                let lo = self.xn(rm);
                let r = if sf == 1 {
                    (((hi as u128) << 64 | lo as u128) >> imms as u128) as u64
                } else {
                    let c = ((hi & 0xFFFF_FFFF) << 32) | (lo & 0xFFFF_FFFF);
                    (c >> imms) & 0xFFFF_FFFF
                };
                self.set_xn(rd, r);
            }
            _ => {}
        }
        Ok(())
    }

    // === Branches, Exception, System ===
    fn exec_branch_sys(&mut self, pc: Addr, insn: u32) -> HelmResult<()> {
        // B / BL
        if (insn >> 26) & 0x1F == 0b00101 {
            let link = (insn >> 31) & 1;
            let imm26 = sext(insn & 0x3FF_FFFF, 26);
            if link == 1 {
                self.set_xn(30, pc + 4);
            }
            self.regs.pc = pc.wrapping_add((imm26 << 2) as u64);
            self.pc_written = true;
            return Ok(());
        }
        // B.cond
        if (insn >> 25) & 0x7F == 0b0101010 {
            let imm19 = sext((insn >> 5) & 0x7FFFF, 19);
            let cond = (insn & 0xF) as u8;
            if self.cond(cond) {
                self.regs.pc = pc.wrapping_add((imm19 << 2) as u64);
                self.pc_written = true;
            }
            return Ok(());
        }
        // CBZ / CBNZ
        if (insn >> 25) & 0x3F == 0b011010 {
            let sf = (insn >> 31) & 1;
            let op = (insn >> 24) & 1;
            let imm19 = sext((insn >> 5) & 0x7FFFF, 19);
            let rt = (insn & 0x1F) as u16;
            let val = if sf == 1 {
                self.xn(rt)
            } else {
                self.wn(rt) as u64
            };
            let taken = if op == 0 { val == 0 } else { val != 0 };
            if taken {
                self.regs.pc = pc.wrapping_add((imm19 << 2) as u64);
                self.pc_written = true;
            }
            return Ok(());
        }
        // TBZ / TBNZ
        if (insn >> 25) & 0x3F == 0b011011 {
            let b5 = (insn >> 31) & 1;
            let op = (insn >> 24) & 1;
            let b40 = (insn >> 19) & 0x1F;
            let bit = (b5 << 5) | b40;
            let imm14 = sext((insn >> 5) & 0x3FFF, 14);
            let rt = (insn & 0x1F) as u16;
            let bit_set = (self.xn(rt) >> bit) & 1 != 0;
            let taken = if op == 0 { !bit_set } else { bit_set };
            if taken {
                self.regs.pc = pc.wrapping_add((imm14 << 2) as u64);
                self.pc_written = true;
            }
            return Ok(());
        }
        // BR / BLR / RET
        if (insn >> 25) & 0x7F == 0b1101011 {
            let opc = (insn >> 21) & 0x3;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let target = self.xn(rn);
            if opc == 1 {
                self.set_xn(30, pc + 4);
            }
            self.regs.pc = target;
            self.pc_written = true;
            return Ok(());
        }
        // SVC
        if insn & 0xFFE0_001F == 0xD400_0001 {
            return Err(helm_core::HelmError::Syscall {
                number: self.xn(8),
                reason: "SVC".into(),
            });
        }
        // NOP / hints
        if (insn >> 12) == 0xD5032 {
            return Ok(());
        }
        // MRS
        if insn & 0xFFF0_0000 == 0xD530_0000 {
            let rt = (insn & 0x1F) as u16;
            let sysreg = (insn >> 5) & 0x7FFF;
            let val = match sysreg {
                0x5E82 => self.regs.tpidr_el0,
                0x5A20 => self.regs.fpcr as u64,
                0x5A21 => self.regs.fpsr as u64,
                0x5F00 => 1_000_000_000,
                0x5F07 => 4,
                0x5801 => 0x8444_C004,
                _ => 0,
            };
            self.set_xn(rt, val);
            return Ok(());
        }
        // MSR
        if insn & 0xFFF0_0000 == 0xD510_0000 {
            let rt = (insn & 0x1F) as u16;
            let sysreg = (insn >> 5) & 0x7FFF;
            let val = self.xn(rt);
            match sysreg {
                0x5E82 => self.regs.tpidr_el0 = val,
                0x5A20 => self.regs.fpcr = val as u32,
                0x5A21 => self.regs.fpsr = val as u32,
                _ => {}
            }
            return Ok(());
        }
        // Barriers — NOP in SE
        if (insn >> 12) == 0xD5033 {
            return Ok(());
        }
        Ok(())
    }

    // === Loads and Stores ===
    fn exec_ldst(&mut self, insn: u32, mem: &mut AddressSpace) -> HelmResult<()> {
        let size = (insn >> 30) & 0x3;
        let v = (insn >> 26) & 1;
        if v == 1 {
            return self.exec_ldst_simd(insn, mem);
        }

        // LDP/STP
        let top5 = (insn >> 27) & 0x1F;
        if top5 == 0b10101 || top5 == 0b00101 {
            return self.exec_pair(insn, mem);
        }
        // Exclusive
        if (insn >> 24) & 0xFF == 0b11001000 || (insn >> 24) & 0xFF == 0b00001000 {
            return self.exec_exclusive(insn, mem);
        }
        // LSE atomics
        // LSE atomics: size 111000 AR 1 Rs o3 000 Rn Rt
        // bits[11:10] must be 00 — distinguishes from register-offset
        // loads (bits[11:10]=10)
        if (insn >> 24) & 0x3F == 0b111000 && (insn >> 21) & 1 == 1 && (insn >> 10) & 3 == 0 {
            return self.exec_atomic(insn, mem);
        }
        // Unsigned offset: size 111001 opc imm12 Rn Rt
        if (insn >> 24) & 0x3F == 0b111001 {
            let opc = (insn >> 22) & 0x3;
            let imm12 = ((insn >> 10) & 0xFFF) as u64;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rt = (insn & 0x1F) as u16;
            let offset = imm12 << size;
            let base = if rn == 31 { self.regs.sp } else { self.xn(rn) };
            let addr = base.wrapping_add(offset);
            let sz = 1usize << size;
            match opc {
                0 => {
                    wr(mem, addr, self.xn(rt), sz)?;
                }
                1 => {
                    self.set_xn(rt, rd(mem, addr, sz)?);
                }
                2 => {
                    let v = rd(mem, addr, sz)?;
                    self.set_xn(rt, sext64(v, (sz * 8) as u32));
                }
                3 => {
                    let v = rd(mem, addr, sz)?;
                    self.set_xn(rt, sext64(v, (sz * 8) as u32) & 0xFFFF_FFFF);
                }
                _ => {}
            }
            return Ok(());
        }
        // Pre/post/unscaled/reg: size 111000 opc ...
        if (insn >> 24) & 0x3F == 0b111000 {
            let opc = (insn >> 22) & 0x3;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rt = (insn & 0x1F) as u16;
            let base = if rn == 31 { self.regs.sp } else { self.xn(rn) };
            let sz = 1usize << size;
            let idx = (insn >> 10) & 0x3;
            let (addr, wb) = if idx == 0b10 {
                // Register offset: LDR/STR Xt, [Xn, Xm, {extend {#amount}}]
                let rm = ((insn >> 16) & 0x1F) as u16;
                let option = (insn >> 13) & 0x7;
                let s_bit = (insn >> 12) & 1;
                let shift = if s_bit == 1 { size } else { 0 };
                let rm_val = self.xn(rm);
                // Apply extend/shift per option field
                let offset = match option {
                    0b010 => (rm_val as u32 as u64) << shift,        // UXTW
                    0b011 => rm_val << shift,                        // LSL (or UXTX)
                    0b110 => (rm_val as i32 as i64 as u64) << shift, // SXTW
                    0b111 => rm_val << shift,                        // SXTX
                    _ => rm_val,
                };
                (base.wrapping_add(offset), None)
            } else {
                let imm9 = sext((insn >> 12) & 0x1FF, 9) as u64;
                match idx {
                    0b00 => (base.wrapping_add(imm9), None),
                    0b01 => (base, Some(base.wrapping_add(imm9))),
                    0b11 => {
                        let a = base.wrapping_add(imm9);
                        (a, Some(a))
                    }
                    _ => (base, None),
                }
            };
            if opc == 0 {
                wr(mem, addr, self.xn(rt), sz)?;
            } else {
                self.set_xn(rt, rd(mem, addr, sz)?);
            }
            if let Some(w) = wb {
                if rn == 31 {
                    self.regs.sp = w;
                } else {
                    self.set_xn(rn, w);
                }
            }
            return Ok(());
        }
        // Load literal
        if (insn >> 24) & 0x3F == 0b011000 {
            let rt = (insn & 0x1F) as u16;
            let imm19 = sext((insn >> 5) & 0x7FFFF, 19) as u64;
            let addr = self.regs.pc.wrapping_add(imm19 << 2);
            let sz = if size == 0 { 4 } else { 8 };
            self.set_xn(rt, rd(mem, addr, sz)?);
            return Ok(());
        }
        Ok(())
    }

    fn exec_pair(&mut self, insn: u32, mem: &mut AddressSpace) -> HelmResult<()> {
        let opc = (insn >> 30) & 0x3;
        let l = (insn >> 22) & 1;
        let idx = (insn >> 23) & 0x3;
        let imm7 = sext((insn >> 15) & 0x7F, 7);
        let rt2 = ((insn >> 10) & 0x1F) as u16;
        let rn = ((insn >> 5) & 0x1F) as u16;
        let rt = (insn & 0x1F) as u16;
        let scale: u64 = if opc == 0b10 { 8 } else { 4 };
        let offset = (imm7 * scale as i64) as u64;
        let base = if rn == 31 { self.regs.sp } else { self.xn(rn) };
        let (addr, wb) = match idx {
            0b01 => (base, Some(base.wrapping_add(offset))),
            0b10 => (base.wrapping_add(offset), None),
            0b11 => {
                let a = base.wrapping_add(offset);
                (a, Some(a))
            }
            _ => (base, None),
        };
        let sz = scale as usize;
        if l == 1 {
            self.set_xn(rt, rd(mem, addr, sz)?);
            self.set_xn(rt2, rd(mem, addr.wrapping_add(scale), sz)?);
        } else {
            wr(mem, addr, self.xn(rt), sz)?;
            wr(mem, addr.wrapping_add(scale), self.xn(rt2), sz)?;
        }
        if let Some(w) = wb {
            if rn == 31 {
                self.regs.sp = w;
            } else {
                self.set_xn(rn, w);
            }
        }
        Ok(())
    }

    fn exec_exclusive(&mut self, insn: u32, mem: &mut AddressSpace) -> HelmResult<()> {
        let size = (insn >> 30) & 0x3;
        let l = (insn >> 22) & 1;
        let rn = ((insn >> 5) & 0x1F) as u16;
        let rt = (insn & 0x1F) as u16;
        let base = if rn == 31 { self.regs.sp } else { self.xn(rn) };
        let sz = 1usize << size;
        if l == 1 {
            self.set_xn(rt, rd(mem, base, sz)?);
        } else {
            let rs = ((insn >> 16) & 0x1F) as u16;
            wr(mem, base, self.xn(rt), sz)?;
            self.set_xn(rs, 0); // always succeeds in SE
        }
        Ok(())
    }

    fn exec_atomic(&mut self, insn: u32, mem: &mut AddressSpace) -> HelmResult<()> {
        let size = (insn >> 30) & 0x3;
        let rs = ((insn >> 16) & 0x1F) as u16;
        let o3 = (insn >> 15) & 1;
        let opc = (insn >> 12) & 0x7;
        let rn = ((insn >> 5) & 0x1F) as u16;
        let rt = (insn & 0x1F) as u16;
        let base = if rn == 31 { self.regs.sp } else { self.xn(rn) };
        let sz = 1usize << size;
        let old = rd(mem, base, sz)?;
        let op = self.xn(rs);
        let new = if o3 == 1 {
            op
        }
        // SWP
        else {
            match opc {
                0 => old.wrapping_add(op),
                1 => old & !op,
                2 => old ^ op,
                3 => old | op,
                4 => {
                    let a = sext64(old, (sz * 8) as u32) as i64;
                    let b = sext64(op, (sz * 8) as u32) as i64;
                    if a > b {
                        old
                    } else {
                        op
                    }
                }
                5 => {
                    let a = sext64(old, (sz * 8) as u32) as i64;
                    let b = sext64(op, (sz * 8) as u32) as i64;
                    if a < b {
                        old
                    } else {
                        op
                    }
                }
                6 => {
                    if old > op {
                        old
                    } else {
                        op
                    }
                }
                7 => {
                    if old < op {
                        old
                    } else {
                        op
                    }
                }
                _ => old,
            }
        };
        wr(mem, base, new, sz)?;
        self.set_xn(rt, old);
        Ok(())
    }

    // === SIMD/FP Loads and Stores (subset for memset/memcpy) ===
    fn exec_ldst_simd(&mut self, insn: u32, mem: &mut AddressSpace) -> HelmResult<()> {
        let size = (insn >> 30) & 0x3;
        let top5 = (insn >> 27) & 0x1F;

        // STP/LDP Q (128-bit pair): opc=10 101 V=1 ...
        if top5 == 0b10101 || top5 == 0b00101 {
            let opc = (insn >> 30) & 0x3;
            let l = (insn >> 22) & 1;
            let idx = (insn >> 23) & 0x3;
            let imm7 = sext((insn >> 15) & 0x7F, 7);
            let rt2 = ((insn >> 10) & 0x1F) as usize;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rt = (insn & 0x1F) as usize;
            let scale: u64 = if opc == 0b10 { 16 } else { 4 << opc }; // Q=16, D=8, S=4
            let offset = (imm7 * scale as i64) as u64;
            let base = self.xn_sp(rn);
            let (addr, wb) = match idx {
                0b01 => (base, Some(base.wrapping_add(offset))),
                0b10 => (base.wrapping_add(offset), None),
                0b11 => {
                    let a = base.wrapping_add(offset);
                    (a, Some(a))
                }
                _ => (base, None),
            };
            if l == 1 {
                // LDP Q
                let lo = rd128(mem, addr)?;
                let hi = rd128(mem, addr.wrapping_add(scale))?;
                self.regs.v[rt] = lo;
                self.regs.v[rt2] = hi;
            } else {
                // STP Q
                wr128(mem, addr, self.regs.v[rt])?;
                wr128(mem, addr.wrapping_add(scale), self.regs.v[rt2])?;
            }
            if let Some(w) = wb {
                self.set_xn_sp(rn, w);
            }
            return Ok(());
        }

        // STR/LDR Q (128-bit, unsigned offset): size 111101 opc imm12 Rn Rt
        if (insn >> 24) & 0x3F == 0b111101 {
            let opc = (insn >> 22) & 0x3;
            let imm12 = ((insn >> 10) & 0xFFF) as u64;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rt = (insn & 0x1F) as usize;
            // SIMD register size: size + opc[1] determine B/H/S/D/Q
            let is_q = (opc >> 1) & 1 == 1 && size == 0;
            let scale: u64 = if is_q { 16 } else { 1u64 << size };
            let offset = imm12 * scale;
            let base = self.xn_sp(rn);
            let addr = base.wrapping_add(offset);
            let is_load = opc & 1 == 1;
            if is_q {
                if is_load {
                    self.regs.v[rt] = rd128(mem, addr)?;
                } else {
                    wr128(mem, addr, self.regs.v[rt])?;
                }
            } else {
                // Scalar SIMD: B/H/S/D
                let sz = scale as usize;
                if is_load {
                    let val = rd(mem, addr, sz.max(1))?;
                    self.regs.v[rt] = val as u128;
                } else {
                    let val = self.regs.v[rt] as u64;
                    wr(mem, addr, val, sz.max(1))?;
                }
            }
            return Ok(());
        }

        // STR/LDR Q (pre/post/unscaled): size 111100 opc ...
        if (insn >> 24) & 0x3F == 0b111100 {
            let opc = (insn >> 22) & 0x3;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rt = (insn & 0x1F) as usize;
            let idx_type = (insn >> 10) & 0x3;
            let base = self.xn_sp(rn);
            let imm9 = sext((insn >> 12) & 0x1FF, 9) as u64;
            let (addr, wb) = match idx_type {
                0b00 => (base.wrapping_add(imm9), None),
                0b01 => (base, Some(base.wrapping_add(imm9))),
                0b11 => {
                    let a = base.wrapping_add(imm9);
                    (a, Some(a))
                }
                _ => (base, None),
            };
            if opc == 0 {
                wr128(mem, addr, self.regs.v[rt])?;
            } else {
                self.regs.v[rt] = rd128(mem, addr)?;
            }
            if let Some(w) = wb {
                self.set_xn_sp(rn, w);
            }
            return Ok(());
        }

        Ok(()) // other SIMD load/store — NOP for now
    }

    // === Data Processing — Register ===
    fn exec_dp_reg(&mut self, insn: u32) -> HelmResult<()> {
        let sf = (insn >> 31) & 1;
        // Add/sub shifted register
        if (insn >> 24) & 0x1F == 0b01011 && (insn >> 21) & 1 == 0 {
            let op = (insn >> 30) & 1;
            let s = (insn >> 29) & 1;
            let sht = (insn >> 22) & 0x3;
            let rm = ((insn >> 16) & 0x1F) as u16;
            let imm6 = ((insn >> 10) & 0x3F) as u32;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rd = (insn & 0x1F) as u16;
            let a = self.xn(rn);
            let b = shft(self.xn(rm), sht, imm6, sf == 1);
            let (r, c, v) = if op == 0 {
                awc(a, b, false, sf == 1)
            } else {
                awc(a, !b, true, sf == 1)
            };
            let r = mask(r, sf);
            if s == 1 {
                self.flags(r, c, v, sf == 1);
            }
            self.set_xn(rd, r);
            return Ok(());
        }
        // Add/sub extended register
        if (insn >> 24) & 0x1F == 0b01011 && (insn >> 21) & 1 == 1 {
            let op = (insn >> 30) & 1;
            let s = (insn >> 29) & 1;
            let rm = ((insn >> 16) & 0x1F) as u16;
            let option = (insn >> 13) & 0x7;
            let imm3 = ((insn >> 10) & 0x7) as u32;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rd = (insn & 0x1F) as u16;
            let a = self.xn_sp(rn);
            let mut b = self.xn(rm);
            b = match option {
                0 => b & 0xFF,
                1 => b & 0xFFFF,
                2 => b & 0xFFFF_FFFF,
                3 => b,
                4 => sext64(b & 0xFF, 8),
                5 => sext64(b & 0xFFFF, 16),
                6 => sext64(b & 0xFFFF_FFFF, 32),
                _ => b,
            };
            b = b.wrapping_shl(imm3);
            let (r, c, v) = if op == 0 {
                awc(a, b, false, sf == 1)
            } else {
                awc(a, !b, true, sf == 1)
            };
            let r = mask(r, sf);
            if s == 1 {
                self.flags(r, c, v, sf == 1);
            }
            self.set_xn(rd, r);
            return Ok(());
        }
        // Logical shifted register
        if (insn >> 24) & 0x1F == 0b01010 {
            let opc = (insn >> 29) & 0x3;
            let n = (insn >> 21) & 1;
            let sht = (insn >> 22) & 0x3;
            let rm = ((insn >> 16) & 0x1F) as u16;
            let imm6 = ((insn >> 10) & 0x3F) as u32;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rd = (insn & 0x1F) as u16;
            let a = self.xn(rn);
            let mut b = shft(self.xn(rm), sht, imm6, sf == 1);
            if n == 1 {
                b = !b;
            }
            let r = match opc {
                0 => a & b,
                1 => a | b,
                2 => a ^ b,
                3 => {
                    let r = mask(a & b, sf);
                    self.flags(r, false, false, sf == 1);
                    r
                }
                _ => a,
            };
            self.set_xn(rd, mask(r, sf));
            return Ok(());
        }
        // Multiply 3-source
        if (insn >> 24) & 0x1F == 0b11011 {
            let op31 = (insn >> 21) & 0x7;
            let rm = ((insn >> 16) & 0x1F) as u16;
            let o0 = (insn >> 15) & 1;
            let ra = ((insn >> 10) & 0x1F) as u16;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rd = (insn & 0x1F) as u16;
            match op31 {
                0 => {
                    let p = if sf == 1 {
                        self.xn(rn).wrapping_mul(self.xn(rm))
                    } else {
                        (self.wn(rn).wrapping_mul(self.wn(rm))) as u64
                    };
                    let r = if o0 == 0 {
                        self.xn(ra).wrapping_add(p)
                    } else {
                        self.xn(ra).wrapping_sub(p)
                    };
                    self.set_xn(rd, mask(r, sf));
                }
                1 => {
                    let p = (self.wn(rn) as i32 as i64).wrapping_mul(self.wn(rm) as i32 as i64);
                    let r = if o0 == 0 {
                        (self.xn(ra) as i64).wrapping_add(p)
                    } else {
                        (self.xn(ra) as i64).wrapping_sub(p)
                    };
                    self.set_xn(rd, r as u64);
                }
                2 => {
                    let r = ((self.xn(rn) as i64 as i128) * (self.xn(rm) as i64 as i128)) >> 64;
                    self.set_xn(rd, r as u64);
                }
                5 => {
                    let p = (self.wn(rn) as u64).wrapping_mul(self.wn(rm) as u64);
                    let r = if o0 == 0 {
                        self.xn(ra).wrapping_add(p)
                    } else {
                        self.xn(ra).wrapping_sub(p)
                    };
                    self.set_xn(rd, r);
                }
                6 => {
                    let r = ((self.xn(rn) as u128) * (self.xn(rm) as u128)) >> 64;
                    self.set_xn(rd, r as u64);
                }
                _ => {}
            }
            return Ok(());
        }
        // 2-source: UDIV/SDIV/LSLV/LSRV/ASRV
        if (insn >> 21) & 0x7FF == 0b0_11010110 {
            let rm = ((insn >> 16) & 0x1F) as u16;
            let op2 = (insn >> 10) & 0x3F;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rd = (insn & 0x1F) as u16;
            let a = self.xn(rn);
            let b = self.xn(rm);
            let bits = if sf == 1 { 64u32 } else { 32 };
            let r = match op2 {
                2 => {
                    if sf == 1 {
                        if b == 0 {
                            0
                        } else {
                            a / b
                        }
                    } else {
                        (if self.wn(rm) == 0 {
                            0
                        } else {
                            self.wn(rn) / self.wn(rm)
                        }) as u64
                    }
                }
                3 => {
                    if sf == 1 {
                        if b == 0 {
                            0
                        } else {
                            (a as i64).wrapping_div(b as i64) as u64
                        }
                    } else {
                        (if self.wn(rm) as i32 == 0 {
                            0
                        } else {
                            (self.wn(rn) as i32).wrapping_div(self.wn(rm) as i32)
                        }) as u32 as u64
                    }
                }
                8 => a.wrapping_shl(b as u32 % bits),
                9 => {
                    if sf == 1 {
                        a.wrapping_shr(b as u32 % bits)
                    } else {
                        (self.wn(rn).wrapping_shr(b as u32 % bits)) as u64
                    }
                }
                10 => {
                    if sf == 1 {
                        ((a as i64).wrapping_shr(b as u32 % bits)) as u64
                    } else {
                        ((self.wn(rn) as i32).wrapping_shr(b as u32 % bits)) as u32 as u64
                    }
                }
                11 => a.rotate_right(b as u32 % bits),
                _ => a,
            };
            self.set_xn(rd, mask(r, sf));
            return Ok(());
        }
        // 1-source: RBIT/REV/CLZ/CLS
        if (insn >> 21) & 0x7FF == 0b1_11010110 {
            let op2 = (insn >> 10) & 0x3F;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rd = (insn & 0x1F) as u16;
            let a = self.xn(rn);
            let r = match op2 {
                0 => {
                    if sf == 1 {
                        a.reverse_bits()
                    } else {
                        (self.wn(rn).reverse_bits()) as u64
                    }
                }
                2 => (a as u32).swap_bytes() as u64,
                3 => {
                    if sf == 1 {
                        a.swap_bytes()
                    } else {
                        (self.wn(rn).swap_bytes()) as u64
                    }
                }
                4 => {
                    if sf == 1 {
                        a.leading_zeros() as u64
                    } else {
                        self.wn(rn).leading_zeros() as u64
                    }
                }
                5 => {
                    let v = if sf == 1 { a } else { self.wn(rn) as u64 };
                    let b = if sf == 1 { 64 } else { 32 };
                    let s = if v >> (b - 1) == 1 {
                        (!v).leading_zeros()
                    } else {
                        v.leading_zeros()
                    };
                    s.saturating_sub(1) as u64
                }
                _ => a,
            };
            self.set_xn(rd, mask(r, sf));
            return Ok(());
        }
        // Conditional select
        if (insn >> 21) & 0x7FE == 0b1101010100_0 {
            let op = (insn >> 30) & 1;
            let rm = ((insn >> 16) & 0x1F) as u16;
            let cond = ((insn >> 12) & 0xF) as u8;
            let op2 = (insn >> 10) & 0x3;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rd = (insn & 0x1F) as u16;
            let r = if self.cond(cond) {
                self.xn(rn)
            } else {
                let v = self.xn(rm);
                match (op, op2 & 1) {
                    (0, 0) => v,
                    (0, 1) => v.wrapping_add(1),
                    (1, 0) => !v,
                    (1, 1) => (!v).wrapping_add(1),
                    _ => v,
                }
            };
            self.set_xn(rd, mask(r, sf));
            return Ok(());
        }
        // CCMP/CCMN
        if (insn >> 21) & 0x1FF == 0b111010010 {
            let op = (insn >> 30) & 1;
            let rm = ((insn >> 16) & 0x1F) as u16;
            let cond = ((insn >> 12) & 0xF) as u8;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let nzcv_imm = (insn & 0xF) as u32;
            if self.cond(cond) {
                let a = self.xn(rn);
                let b = self.xn(rm);
                let (r, c, v) = if op == 1 {
                    awc(a, !b, true, sf == 1)
                } else {
                    awc(a, b, false, sf == 1)
                };
                self.flags(mask(r, sf), c, v, sf == 1);
            } else {
                self.regs.nzcv = nzcv_imm << 28;
            }
            return Ok(());
        }
        // ADC/SBC
        if (insn >> 21) & 0x7FF == 0b0_11010000 {
            let op = (insn >> 30) & 1;
            let s = (insn >> 29) & 1;
            let rm = ((insn >> 16) & 0x1F) as u16;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rd = (insn & 0x1F) as u16;
            let (r, c, v) = if op == 0 {
                awc(self.xn(rn), self.xn(rm), self.regs.c(), sf == 1)
            } else {
                awc(self.xn(rn), !self.xn(rm), self.regs.c(), sf == 1)
            };
            let r = mask(r, sf);
            if s == 1 {
                self.flags(r, c, v, sf == 1);
            }
            self.set_xn(rd, r);
            return Ok(());
        }
        Ok(())
    }

    // === SIMD/FP Data Processing (minimal subset) ===
    fn exec_simd_dp(&mut self, insn: u32) -> HelmResult<()> {
        // DUP Vd.T, Wn: 0 Q 00 1110 000 imm5 0 0001 1 Rn Rd
        if insn & 0xBFE0_FC00 == 0x0E00_0C00 {
            let q = (insn >> 30) & 1;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rd = (insn & 0x1F) as usize;
            let val = self.xn(rn) as u8;
            // Fill the vector register with the byte value
            let mut v: u128 = 0;
            let bytes = if q == 1 { 16 } else { 8 };
            for i in 0..bytes {
                v |= (val as u128) << (i * 8);
            }
            self.regs.v[rd] = v;
            return Ok(());
        }

        // ORR Vd.16B, Vn.16B, Vm.16B (MOV vector): 0 Q 00 1110 10 1 Rm 0 00011 1 Rn Rd
        if insn & 0xBFE0_FC00 == 0x0EA0_1C00 {
            let rm = ((insn >> 16) & 0x1F) as usize;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            self.regs.v[rd] = self.regs.v[rn] | self.regs.v[rm];
            return Ok(());
        }

        // Other SIMD/FP — NOP for now
        Ok(())
    }

    // === Helpers ===
    fn flags(&mut self, r: u64, c: bool, v: bool, is64: bool) {
        let n = if is64 {
            r >> 63 != 0
        } else {
            (r >> 31) & 1 != 0
        };
        let z = if is64 { r == 0 } else { r & 0xFFFF_FFFF == 0 };
        self.regs.set_nzcv(n, z, c, v);
    }
    fn cond(&self, c: u8) -> bool {
        let base = match c >> 1 {
            0 => self.regs.z(),
            1 => self.regs.c(),
            2 => self.regs.n(),
            3 => self.regs.v(),
            4 => self.regs.c() && !self.regs.z(),
            5 => self.regs.n() == self.regs.v(),
            6 => self.regs.n() == self.regs.v() && !self.regs.z(),
            7 => true,
            _ => false,
        };
        if c & 1 != 0 && c != 0xF {
            !base
        } else {
            base
        }
    }
}

impl Default for Aarch64Cpu {
    fn default() -> Self {
        Self::new()
    }
}

fn sext(val: u32, bits: u32) -> i64 {
    let s = 32 - bits;
    ((val << s) as i32 >> s) as i64
}
fn sext64(val: u64, bits: u32) -> u64 {
    if bits >= 64 {
        val
    } else {
        let s = 64 - bits;
        ((val << s) as i64 >> s) as u64
    }
}
fn mask(v: u64, sf: u32) -> u64 {
    if sf == 1 {
        v
    } else {
        v & 0xFFFF_FFFF
    }
}

fn awc(a: u64, b: u64, cin: bool, is64: bool) -> (u64, bool, bool) {
    if is64 {
        let (s1, c1) = a.overflowing_add(b);
        let (s2, c2) = s1.overflowing_add(cin as u64);
        let carry = c1 || c2;
        let ov = {
            let sa = (a >> 63) & 1;
            let sb = (b >> 63) & 1;
            let sr = (s2 >> 63) & 1;
            sa == sb && sa != sr
        };
        (s2, carry, ov)
    } else {
        let a = a as u32;
        let b = b as u32;
        let (s1, c1) = a.overflowing_add(b);
        let (s2, c2) = s1.overflowing_add(cin as u32);
        let carry = c1 || c2;
        let ov = {
            let sa = (a >> 31) & 1;
            let sb = (b >> 31) & 1;
            let sr = (s2 >> 31) & 1;
            sa == sb && sa != sr
        };
        (s2 as u64, carry, ov)
    }
}

fn shft(val: u64, t: u32, amt: u32, is64: bool) -> u64 {
    if amt == 0 {
        return val;
    }
    match t {
        0 => val.wrapping_shl(amt),
        1 => {
            if is64 {
                val.wrapping_shr(amt)
            } else {
                ((val as u32).wrapping_shr(amt)) as u64
            }
        }
        2 => {
            if is64 {
                ((val as i64).wrapping_shr(amt)) as u64
            } else {
                ((val as i32).wrapping_shr(amt)) as u32 as u64
            }
        }
        3 => val.rotate_right(amt),
        _ => val,
    }
}

fn rd(mem: &AddressSpace, addr: Addr, sz: usize) -> HelmResult<u64> {
    let mut b = [0u8; 8];
    mem.read(addr, &mut b[..sz])?;
    Ok(match sz {
        1 => b[0] as u64,
        2 => u16::from_le_bytes([b[0], b[1]]) as u64,
        4 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64,
        8 => u64::from_le_bytes(b),
        _ => 0,
    })
}
fn wr(mem: &mut AddressSpace, addr: Addr, val: u64, sz: usize) -> HelmResult<()> {
    mem.write(addr, &val.to_le_bytes()[..sz])
}

fn decode_bitmask(n: u32, imms: u32, immr: u32, is64: bool) -> u64 {
    let len = hsb((n << 6) | (!imms & 0x3F), 7);
    if len < 1 {
        return 0;
    }
    let levels = (1u32 << len) - 1;
    let s = imms & levels;
    let r = immr & levels;
    let esize = 1u64 << len;
    let welem = if s + 1 >= 64 {
        u64::MAX
    } else {
        (1u64 << (s + 1)) - 1
    };
    let emask = if esize >= 64 {
        u64::MAX
    } else {
        (1u64 << esize) - 1
    };
    let elem = welem.rotate_right(r) & emask;
    let rsz = if is64 { 64u64 } else { 32 };
    let mut result = 0u64;
    let mut pos = 0u64;
    while pos < rsz {
        result |= elem << pos;
        pos += esize;
    }
    if !is64 {
        result &= 0xFFFF_FFFF;
    }
    result
}
fn hsb(val: u32, width: u32) -> u32 {
    for i in (0..width).rev() {
        if (val >> i) & 1 != 0 {
            return i;
        }
    }
    0
}

fn rd128(mem: &AddressSpace, addr: Addr) -> HelmResult<u128> {
    let mut b = [0u8; 16];
    mem.read(addr, &mut b)?;
    Ok(u128::from_le_bytes(b))
}
fn wr128(mem: &mut AddressSpace, addr: Addr, val: u128) -> HelmResult<()> {
    mem.write(addr, &val.to_le_bytes())
}
