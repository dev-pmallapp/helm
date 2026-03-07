#![allow(clippy::unusual_byte_groupings)]
//! AArch64 instruction executor for SE/FE mode.
//!
//! Fetch-decode-execute loop operating directly on `Aarch64Regs`
//! and `AddressSpace`.  No pipeline modelling — this is the FE path.

#![allow(clippy::unnecessary_cast, clippy::identity_op)]

use crate::arm::aarch64::sysreg;
use crate::arm::regs::Aarch64Regs;
use helm_core::types::Addr;
use helm_core::{HelmError, HelmResult};
use helm_memory::address_space::AddressSpace;
use helm_memory::mmu::{self, TranslationConfig, TranslationFault, TtbrSelect};
use helm_memory::tlb::Tlb;
use helm_timing::InsnClass;
use std::collections::HashSet;

/// A single memory access recorded during `step()`.
#[derive(Debug, Clone)]
pub struct MemAccess {
    pub addr: u64,
    pub size: usize,
    pub is_write: bool,
}

/// Trace of a single instruction's execution, returned by `step()`.
///
/// Contains the instruction word, classification for timing, all memory
/// accesses performed, and whether a branch was taken.
#[derive(Debug, Clone)]
pub struct StepTrace {
    pub pc: u64,
    pub insn_word: u32,
    pub class: InsnClass,
    pub mem_accesses: Vec<MemAccess>,
    pub branch_taken: Option<bool>,
}

impl Default for StepTrace {
    fn default() -> Self {
        Self {
            pc: 0,
            insn_word: 0,
            class: InsnClass::Nop,
            mem_accesses: Vec::new(),
            branch_taken: None,
        }
    }
}

include!(concat!(env!("OUT_DIR"), "/decode_aarch64_simd.rs"));

/// Execution state for one AArch64 vCPU.
pub struct Aarch64Cpu {
    pub regs: Aarch64Regs,
    pub halted: bool,
    pub exit_code: u64,
    pc_written: bool,
    /// Current instruction word (set during step for unimpl diagnostics).
    cur_insn: u32,
    /// Track which SIMD instruction classes have been logged (dedup).
    simd_seen: HashSet<&'static str>,
    pub insn_count: u64,
    /// Trace accumulator — populated during step(), returned to caller.
    trace: StepTrace,
    /// TLB for address translation (256 entries).
    tlb: Tlb,
    /// Whether MMU is currently enabled (cached from SCTLR_EL1 bit 0).
    mmu_enabled: bool,
    /// SE mode: SVC returns HelmError::Syscall instead of taking an exception.
    se_mode: bool,
}

impl Aarch64Cpu {
    pub fn new() -> Self {
        Self {
            regs: Aarch64Regs::default(),
            halted: false,
            exit_code: 0,
            pc_written: false,
            cur_insn: 0,
            simd_seen: HashSet::new(),
            insn_count: 0,
            trace: StepTrace::default(),
            tlb: Tlb::new(256),
            mmu_enabled: false,
            se_mode: false,
        }
    }

    pub fn set_se_mode(&mut self, enabled: bool) {
        self.se_mode = enabled;
    }

    pub fn xn(&self, n: u16) -> u64 {
        if n >= 31 {
            0
        } else {
            self.regs.x[n as usize]
        }
    }
    /// Read Xn or SP. Reg 31 = current SP (respects SPSel).
    pub fn xn_sp(&self, n: u16) -> u64 {
        if n == 31 {
            self.current_sp()
        } else {
            self.regs.x[n as usize]
        }
    }
    pub fn set_xn(&mut self, n: u16, val: u64) {
        if n < 31 {
            self.regs.x[n as usize] = val;
        }
    }
    /// Write Xn or SP (respects SPSel).
    pub fn set_xn_sp(&mut self, n: u16, val: u64) {
        if n == 31 {
            self.set_current_sp(val);
        } else if n < 31 {
            self.regs.x[n as usize] = val;
        }
    }

    /// Read current stack pointer (respects SPSel and current EL).
    pub fn current_sp(&self) -> u64 {
        if self.regs.sp_sel == 0 || self.regs.current_el == 0 {
            self.regs.sp
        } else {
            match self.regs.current_el {
                1 => self.regs.sp_el1,
                2 => self.regs.sp_el2,
                3 => self.regs.sp_el3,
                _ => self.regs.sp,
            }
        }
    }

    /// Write current stack pointer (respects SPSel and current EL).
    pub fn set_current_sp(&mut self, val: u64) {
        if self.regs.sp_sel == 0 || self.regs.current_el == 0 {
            self.regs.sp = val;
        } else {
            match self.regs.current_el {
                1 => self.regs.sp_el1 = val,
                2 => self.regs.sp_el2 = val,
                3 => self.regs.sp_el3 = val,
                _ => self.regs.sp = val,
            }
        }
    }
    pub fn wn(&self, n: u16) -> u32 {
        self.xn(n) as u32
    }
    pub fn set_wn(&mut self, n: u16, val: u32) {
        self.set_xn(n, val as u64);
    }

    // ── MMU address translation ────────────────────────────────────────

    /// Translate VA → PA using the MMU page tables (if enabled).
    /// On fault, takes a data/instruction abort exception and returns Err.
    fn translate_va(
        &mut self, va: u64, is_write: bool, is_fetch: bool, mem: &mut AddressSpace,
    ) -> HelmResult<u64> {
        if !self.mmu_enabled {
            return Ok(va); // MMU off → identity map
        }

        let asid = self.current_asid();

        // TLB fast path
        if let Some((pa, perms)) = self.tlb.lookup(va, asid) {
            if !perms.check(self.regs.current_el, is_write, is_fetch) {
                return self.raise_translation_fault(
                    va, is_write, is_fetch,
                    TranslationFault::PermissionFault { level: 3 },
                );
            }
            return Ok(pa);
        }

        // TLB miss → page table walk
        let tcr = TranslationConfig::parse(self.regs.tcr_el1);
        let ttbr0 = self.regs.ttbr0_el1;
        let ttbr1 = self.regs.ttbr1_el1;

        let result = mmu::translate(va, &tcr, ttbr0, ttbr1, &mut |pa| {
            let mut buf = [0u8; 8];
            mem.read_phys(pa, &mut buf).unwrap_or(());
            u64::from_le_bytes(buf)
        });

        match result {
            Ok((walk, sel)) => {
                // Permission check
                if !walk.perms.check(self.regs.current_el, is_write, is_fetch) {
                    return self.raise_translation_fault(
                        va, is_write, is_fetch,
                        TranslationFault::PermissionFault { level: walk.level },
                    );
                }

                // Insert into TLB
                let global = !walk.ng;
                let entry = Tlb::make_entry(
                    va, walk.pa, walk.block_size,
                    walk.perms, walk.attr_indx, asid, global,
                );
                self.tlb.insert(entry);

                Ok(walk.pa)
            }
            Err(fault) => {
                self.raise_translation_fault(va, is_write, is_fetch, fault)
            }
        }
    }

    /// Get current ASID from TTBR (depends on TCR.A1).
    fn current_asid(&self) -> u16 {
        let tcr = self.regs.tcr_el1;
        let a1 = (tcr >> 22) & 1 != 0;
        let ttbr = if a1 { self.regs.ttbr1_el1 } else { self.regs.ttbr0_el1 };
        (ttbr >> 48) as u16
    }

    /// Raise a translation fault → data abort or instruction abort exception.
    fn raise_translation_fault(
        &mut self, va: u64, is_write: bool, is_fetch: bool, fault: TranslationFault,
    ) -> HelmResult<u64> {
        // EC: 0x20/0x21 = instruction abort (lower/current EL)
        //     0x24/0x25 = data abort (lower/current EL)
        let ec = if is_fetch {
            if self.regs.current_el == 0 { 0x20 } else { 0x21 }
        } else {
            if self.regs.current_el == 0 { 0x24 } else { 0x25 }
        };
        let fsc = fault.to_fsc();
        let wnr = if is_write && !is_fetch { 1u32 << 6 } else { 0 };
        let iss = fsc | wnr;
        self.regs.far_el1 = va;
        self.take_exception_to_el1(ec, iss);
        // Return a special error so the step loop knows an exception was taken
        Err(HelmError::Memory {
            addr: va,
            reason: format!("translation fault: {:?}", fault),
        })
    }

    // ── step + traced memory access ─────────────────────────────────────

    pub fn step(&mut self, mem: &mut AddressSpace) -> HelmResult<StepTrace> {
        let va = self.regs.pc;
        // Translate instruction fetch VA → PA
        let pc = match self.translate_va(va, false, true, mem) {
            Ok(pa) => pa,
            Err(_) => {
                // Exception taken — PC is now at the exception vector.
                // Return a trace for the faulting instruction.
                self.trace.pc = va;
                self.trace.insn_word = 0;
                self.trace.class = InsnClass::Nop;
                self.trace.mem_accesses.clear();
                self.trace.branch_taken = None;
                return Ok(std::mem::take(&mut self.trace));
            }
        };

        let mut buf = [0u8; 4];
        mem.read(pc, &mut buf)?;
        let insn = u32::from_le_bytes(buf);
        self.cur_insn = insn;
        self.pc_written = false;
        self.insn_count += 1;

        // Reset trace for this instruction (reuse existing Vec capacity)
        self.trace.pc = va;
        self.trace.insn_word = insn;
        self.trace.class = InsnClass::Nop;
        self.trace.mem_accesses.clear();
        self.trace.branch_taken = None;

        self.exec(pc, insn, mem)?;
        if !self.pc_written {
            self.regs.pc += 4;
        }

        // Record branch outcome for branch instructions
        if matches!(self.trace.class, InsnClass::Branch | InsnClass::CondBranch) {
            self.trace.branch_taken = Some(self.pc_written);
        }

        Ok(std::mem::take(&mut self.trace))
    }

    // -- Traced memory access wrappers (with VA→PA translation) --

    fn trace_rd(&mut self, mem: &mut AddressSpace, va: Addr, sz: usize) -> HelmResult<u64> {
        let pa = match self.translate_va(va, false, false, mem) {
            Ok(pa) => pa,
            Err(_) => { self.pc_written = true; return Ok(0); } // data abort taken
        };
        self.trace.mem_accesses.push(MemAccess { addr: pa, size: sz, is_write: false });
        rd(mem, pa, sz)
    }

    fn trace_wr(&mut self, mem: &mut AddressSpace, va: Addr, val: u64, sz: usize) -> HelmResult<()> {
        let pa = match self.translate_va(va, true, false, mem) {
            Ok(pa) => pa,
            Err(_) => { self.pc_written = true; return Ok(()); } // data abort taken
        };
        self.trace.mem_accesses.push(MemAccess { addr: pa, size: sz, is_write: true });
        wr(mem, pa, val, sz)
    }

    fn trace_rd128(&mut self, mem: &mut AddressSpace, va: Addr) -> HelmResult<u128> {
        let pa = match self.translate_va(va, false, false, mem) {
            Ok(pa) => pa,
            Err(_) => { self.pc_written = true; return Ok(0); }
        };
        self.trace.mem_accesses.push(MemAccess { addr: pa, size: 16, is_write: false });
        rd128(mem, pa)
    }

    fn trace_wr128(&mut self, mem: &mut AddressSpace, va: Addr, val: u128) -> HelmResult<()> {
        let pa = match self.translate_va(va, true, false, mem) {
            Ok(pa) => pa,
            Err(_) => { self.pc_written = true; return Ok(()); }
        };
        self.trace.mem_accesses.push(MemAccess { addr: pa, size: 16, is_write: true });
        wr128(mem, pa, val)
    }

    /// Return an error for unimplemented instructions (no panic).
    fn unimpl(&self, ctx: &str) -> HelmResult<()> {
        let pc = self.regs.pc;
        let insn = self.cur_insn;
        Err(HelmError::Isa(format!(
            "unimplemented {ctx} at PC={pc:#x}: insn={insn:#010x} ({insn:032b})"
        )))
    }

    fn exec(&mut self, pc: Addr, insn: u32, mem: &mut AddressSpace) -> HelmResult<()> {
        let op0 = (insn >> 25) & 0xF;
        match op0 {
            0b1000 | 0b1001 => { self.trace.class = InsnClass::IntAlu; self.exec_dp_imm(pc, insn) }
            0b1010 | 0b1011 => self.exec_branch_sys(pc, insn, mem),
            0b0100 | 0b0110 | 0b1100 | 0b1110 => self.exec_ldst(insn, mem),
            0b0101 | 0b1101 => { self.trace.class = InsnClass::IntAlu; self.exec_dp_reg(insn) }
            0b0111 | 0b1111 => { self.trace.class = InsnClass::Simd; self.exec_simd_dp(insn) }
            _ => self.unimpl("encoding group"),
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
                        self.flags(r, self.regs.c(), self.regs.v(), sf == 1);
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
                    0 => self.set_xn(rd, mask(!(imm16 << shift), sf)),
                    2 => self.set_xn(rd, mask(imm16 << shift, sf)),
                    3 => {
                        let old = self.xn(rd);
                        let m = !(0xFFFFu64 << shift);
                        self.set_xn(rd, mask((old & m) | (imm16 << shift), sf));
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
                    1 => {
                        // BFM: bit field move — insert bits from Xn into Xd.
                        // Unlike SBFM/UBFM, BFM reads Rd and merges.
                        let dst = self.xn(rd);
                        let esize = if sf == 1 { 64u32 } else { 32 };
                        // ROR(src, immr) selects source bits
                        let rotated = if immr == 0 {
                            src
                        } else {
                            (src >> immr) | (src << (esize - immr))
                        };
                        // wmask selects which bits come from rotated source
                        let wmask =
                            decode_bitmask(if sf == 1 { 1 } else { 0 }, imms, immr, sf == 1);
                        (dst & !wmask) | (rotated & wmask)
                    }
                    _ => {
                        return self.unimpl("bitfield opc=3 (reserved)");
                    }
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
            _ => {
                return self.unimpl("dp_imm");
            }
        }
        Ok(())
    }

    // === Branches, Exception, System ===
    fn exec_branch_sys(&mut self, pc: Addr, insn: u32, mem: &mut AddressSpace) -> HelmResult<()> {
        self.trace.class = InsnClass::Branch;
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
            self.trace.class = InsnClass::CondBranch;
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
            self.trace.class = InsnClass::CondBranch;
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
            self.trace.class = InsnClass::CondBranch;
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
        // BR / BLR / RET and ERET
        if (insn >> 25) & 0x7F == 0b1101011 {
            let opc = (insn >> 21) & 0xF;
            // ERET: 1101011_0100_11111_000000_11111_00000 = 0xD69F03E0
            if opc == 0b0100 {
                return self.exec_eret();
            }
            let rn = ((insn >> 5) & 0x1F) as u16;
            let target = self.xn(rn);
            if opc & 0x3 == 1 {
                self.set_xn(30, pc + 4);
            }
            self.regs.pc = target;
            self.pc_written = true;
            return Ok(());
        }
        // SVC
        if insn & 0xFFE0_001F == 0xD400_0001 {
            self.trace.class = InsnClass::Syscall;
            let imm16 = (insn >> 5) & 0xFFFF;
            // In FS mode, EL0 code calling into kernel takes an exception.
            // In SE mode the engine handles syscalls, so always signal.
            if !self.se_mode && self.regs.current_el == 0 {
                self.take_exception_to_el1(0x15, imm16); // EC=0x15 = SVC from AArch64
                return Ok(());
            }
            // SE mode or SVC from EL1+: signal to engine for handling
            return Err(HelmError::Syscall {
                number: self.xn(8),
                reason: "SVC".into(),
            });
        }
        // HVC
        if insn & 0xFFE0_001F == 0xD400_0002 {
            let imm16 = (insn >> 5) & 0xFFFF;
            self.take_exception_to_el2(0x16, imm16); // EC=0x16 = HVC
            return Ok(());
        }
        // SMC
        if insn & 0xFFE0_001F == 0xD400_0003 {
            // NOP — no EL3 firmware in simulation
            return Ok(());
        }
        // BRK — breakpoint
        if insn & 0xFFE0_001F == 0xD420_0000 {
            let imm16 = (insn >> 5) & 0xFFFF;
            return Err(HelmError::Decode {
                addr: pc,
                reason: format!("BRK #{imm16} (breakpoint/assertion failure)"),
            });
        }
        // HLT
        if insn & 0xFFE0_001F == 0xD440_0000 {
            self.halted = true;
            return Ok(());
        }
        // All system instructions: 1101_0101_00...
        // Match top 10 bits: 1101010100
        if insn >> 22 == 0b1101_0101_00 {
            self.trace.class = InsnClass::IntAlu;
            return self.exec_system(insn, mem);
        }
        self.unimpl("branch/sys")
    }

    // === System instruction dispatcher ===
    fn exec_system(&mut self, insn: u32, mem: &mut AddressSpace) -> HelmResult<()> {
        let l = (insn >> 21) & 1;       // 0=MSR/SYS, 1=MRS/SYSL
        let op0 = (insn >> 19) & 3;
        let op1 = (insn >> 16) & 7;
        let crn = (insn >> 12) & 0xF;
        let crm = (insn >> 8) & 0xF;
        let op2 = (insn >> 5) & 7;
        let rt = (insn & 0x1F) as u16;

        // MSR (immediate) to PSTATE fields: op0=0, L=0, CRn=0100
        if op0 == 0 && l == 0 && crn == 4 {
            return self.exec_msr_imm(op1, crm, op2);
        }

        // Barriers: op0=0, L=0, CRn=0011 — NOP
        if op0 == 0 && l == 0 && crn == 3 {
            return Ok(());
        }

        // Hints: op0=0, L=0, CRn=0010 — NOP (NOP/YIELD/WFE/WFI/SEV/PAC*)
        if op0 == 0 && l == 0 && crn == 2 {
            return Ok(());
        }

        // CLREX: op0=0, L=0, CRn=0011, op2=010 — already caught above
        // PSTATE flag manipulation: op0=0, L=0, CRn=0100 — already caught above

        // SYS/SYSL: op0=1 — cache/TLB maintenance, AT, DC, IC, TLBI
        if op0 == 1 {
            // DC ZVA: op1=3, CRn=7, CRm=4, op2=1, L=0
            // Zeroes a cache-line-sized block; must write memory for correctness.
            if l == 0 && op1 == 3 && crn == 7 && crm == 4 && op2 == 1 {
                let addr = self.xn(rt);
                let bs = (self.regs.dczid_el0 & 0xF) as u64;
                let block_size = 4u64 << bs;
                let aligned = addr & !(block_size - 1);
                let zeros = vec![0u8; block_size as usize];
                mem.write(aligned, &zeros)?;
                return Ok(());
            }
            // Other DC, IC, AT, etc. — NOP in simulation
            return Ok(());
        }

        // MRS/MSR (register): op0 ∈ {2,3}
        // Encode the full sysreg ID
        let sysreg_id = (op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2;

        if l == 1 {
            // MRS Xt, <sysreg>
            let val = self.read_sysreg(sysreg_id);
            self.set_xn(rt, val);
        } else {
            // MSR <sysreg>, Xt
            let val = self.xn(rt);
            self.write_sysreg(sysreg_id, val);
        }
        Ok(())
    }

    // === MSR immediate to PSTATE fields ===
    fn exec_msr_imm(&mut self, op1: u32, crm: u32, op2: u32) -> HelmResult<()> {
        let imm = crm; // 4-bit immediate in CRm field
        match (op1, op2) {
            (3, 6) => { // DAIFSet — set (mask) interrupt bits
                self.regs.daif |= (imm << 6) as u32;
            }
            (3, 7) => { // DAIFClr — clear (unmask) interrupt bits
                self.regs.daif &= !((imm << 6) as u32);
            }
            (0, 5) => { // SPSel
                self.regs.sp_sel = (imm & 1) as u8;
            }
            _ => {} // NOP for PAN, UAO, DIT, TCO, etc.
        }
        Ok(())
    }

    // === System register read ===
    fn read_sysreg(&self, id: u32) -> u64 {
        match id {
            // EL1 control
            sysreg::SCTLR_EL1      => self.regs.sctlr_el1,
            sysreg::ACTLR_EL1      => self.regs.actlr_el1,
            sysreg::CPACR_EL1      => self.regs.cpacr_el1,
            // Translation
            sysreg::TTBR0_EL1      => self.regs.ttbr0_el1,
            sysreg::TTBR1_EL1      => self.regs.ttbr1_el1,
            sysreg::TCR_EL1        => self.regs.tcr_el1,
            // Fault
            sysreg::ESR_EL1        => self.regs.esr_el1 as u64,
            sysreg::AFSR0_EL1      => self.regs.afsr0_el1,
            sysreg::AFSR1_EL1      => self.regs.afsr1_el1,
            sysreg::FAR_EL1        => self.regs.far_el1,
            sysreg::PAR_EL1        => self.regs.par_el1,
            // Memory attributes
            sysreg::MAIR_EL1       => self.regs.mair_el1,
            sysreg::AMAIR_EL1      => self.regs.amair_el1,
            // Vector / exception
            sysreg::VBAR_EL1       => self.regs.vbar_el1,
            sysreg::CONTEXTIDR_EL1 => self.regs.contextidr_el1,
            // Thread ID
            sysreg::TPIDR_EL0      => self.regs.tpidr_el0,
            sysreg::TPIDR_EL1      => self.regs.tpidr_el1,
            sysreg::TPIDRRO_EL0    => 0, // read-only thread pointer (not set)
            // SP / exception state
            sysreg::SP_EL0         => self.regs.sp,
            sysreg::SP_EL1         => self.regs.sp_el1,
            sysreg::ELR_EL1        => self.regs.elr_el1,
            sysreg::SPSR_EL1       => self.regs.spsr_el1 as u64,
            sysreg::CURRENT_EL     => (self.regs.current_el as u64) << 2,
            sysreg::DAIF           => self.regs.daif as u64,
            sysreg::NZCV           => self.regs.nzcv as u64,
            sysreg::SPSEL          => self.regs.sp_sel as u64,
            // Debug
            sysreg::MDSCR_EL1      => self.regs.mdscr_el1 as u64,
            sysreg::MDCCSR_EL0     => 0,
            // Cache
            sysreg::CSSELR_EL1     => self.regs.csselr_el1,
            sysreg::CCSIDR_EL1     => 0x700F_E01A, // 32KB 4-way (dummy)
            sysreg::CLIDR_EL1      => 0x0A20_0023, // L1 I+D, L2 unified
            // Timer
            sysreg::CNTFRQ_EL0     => self.regs.cntfrq_el0,
            sysreg::CNTVCT_EL0     => self.insn_count, // approximate timer
            sysreg::CNTV_CTL_EL0   => self.regs.cntv_ctl_el0,
            sysreg::CNTV_CVAL_EL0  => self.regs.cntv_cval_el0,
            sysreg::CNTP_CTL_EL0   => self.regs.cntp_ctl_el0,
            sysreg::CNTP_CVAL_EL0  => self.regs.cntp_cval_el0,
            sysreg::CNTP_TVAL_EL0  => 0,
            sysreg::CNTKCTL_EL1    => self.regs.cntkctl_el1,
            // Counter / cache type (read-only)
            sysreg::CTR_EL0        => self.regs.ctr_el0,
            sysreg::DCZID_EL0      => self.regs.dczid_el0,
            // FP
            sysreg::FPCR           => self.regs.fpcr as u64,
            sysreg::FPSR           => self.regs.fpsr as u64,
            // ID registers (read-only)
            sysreg::MIDR_EL1       => self.regs.midr_el1,
            sysreg::MPIDR_EL1      => self.regs.mpidr_el1,
            sysreg::REVIDR_EL1     => self.regs.revidr_el1,
            sysreg::ID_AA64PFR0_EL1  => self.regs.id_aa64pfr0_el1,
            sysreg::ID_AA64PFR1_EL1  => self.regs.id_aa64pfr1_el1,
            sysreg::ID_AA64MMFR0_EL1 => self.regs.id_aa64mmfr0_el1,
            sysreg::ID_AA64MMFR1_EL1 => self.regs.id_aa64mmfr1_el1,
            sysreg::ID_AA64MMFR2_EL1 => self.regs.id_aa64mmfr2_el1,
            sysreg::ID_AA64ISAR0_EL1 => self.regs.id_aa64isar0_el1,
            sysreg::ID_AA64ISAR1_EL1 => self.regs.id_aa64isar1_el1,
            sysreg::ID_AA64ISAR2_EL1 => self.regs.id_aa64isar2_el1,
            sysreg::ID_AA64DFR0_EL1  => self.regs.id_aa64dfr0_el1,
            sysreg::ID_AA64DFR1_EL1  => 0,
            sysreg::ID_AA64AFR0_EL1  => 0,
            sysreg::ID_AA64AFR1_EL1  => 0,
            // Legacy AArch32 ID regs (read as zero)
            sysreg::ID_PFR0_EL1 | sysreg::ID_PFR1_EL1 | sysreg::ID_PFR2_EL1
            | sysreg::ID_DFR0_EL1 | sysreg::ID_AFR0_EL1
            | sysreg::ID_MMFR0_EL1 | sysreg::ID_MMFR1_EL1
            | sysreg::ID_MMFR2_EL1 | sysreg::ID_MMFR3_EL1 | sysreg::ID_MMFR4_EL1
            | sysreg::ID_ISAR0_EL1 | sysreg::ID_ISAR1_EL1 | sysreg::ID_ISAR2_EL1
            | sysreg::ID_ISAR3_EL1 | sysreg::ID_ISAR4_EL1 | sysreg::ID_ISAR5_EL1
            | sysreg::ID_ISAR6_EL1 => 0,
            // EL2
            sysreg::HCR_EL2       => self.regs.hcr_el2,
            sysreg::SCTLR_EL2     => self.regs.sctlr_el2,
            sysreg::VBAR_EL2      => self.regs.vbar_el2,
            sysreg::ELR_EL2       => self.regs.elr_el2,
            sysreg::SPSR_EL2      => self.regs.spsr_el2 as u64,
            sysreg::VTTBR_EL2     => self.regs.vttbr_el2,
            sysreg::CNTVOFF_EL2   => self.regs.cntvoff_el2,
            // EL3
            sysreg::SCR_EL3       => self.regs.scr_el3,
            sysreg::ELR_EL3       => self.regs.elr_el3,
            sysreg::SPSR_EL3      => self.regs.spsr_el3 as u64,
            // Performance monitors — stub
            sysreg::PMCR_EL0 | sysreg::PMCNTENSET_EL0 | sysreg::PMCNTENCLR_EL0
            | sysreg::PMOVSCLR_EL0 | sysreg::PMUSERENR_EL0 | sysreg::PMCCNTR_EL0
            | sysreg::PMCCFILTR_EL0 | sysreg::PMSELR_EL0
            | sysreg::PMXEVTYPER_EL0 | sysreg::PMXEVCNTR_EL0 => 0,
            // OS lock
            sysreg::OSLSR_EL1     => 0,
            sysreg::OSDLR_EL1     => 0,
            // Unknown: return 0 (RAZ)
            _ => {
                log::trace!("MRS: unknown sysreg {id:#06x} → 0 (RAZ)");
                0
            }
        }
    }

    // === System register write ===
    fn write_sysreg(&mut self, id: u32, val: u64) {
        match id {
            // EL1 control
            sysreg::SCTLR_EL1      => {
                let was_enabled = self.mmu_enabled;
                self.regs.sctlr_el1 = val;
                self.mmu_enabled = val & 1 != 0;
                if self.mmu_enabled && !was_enabled {
                    self.tlb.flush_all();
                }
            }
            sysreg::ACTLR_EL1      => self.regs.actlr_el1 = val,
            sysreg::CPACR_EL1      => self.regs.cpacr_el1 = val,
            // Translation — flush TLB on table base / config changes
            sysreg::TTBR0_EL1      => { self.regs.ttbr0_el1 = val; self.tlb.flush_all(); }
            sysreg::TTBR1_EL1      => { self.regs.ttbr1_el1 = val; self.tlb.flush_all(); }
            sysreg::TCR_EL1        => { self.regs.tcr_el1 = val; self.tlb.flush_all(); }
            // Fault
            sysreg::ESR_EL1        => self.regs.esr_el1 = val as u32,
            sysreg::AFSR0_EL1      => self.regs.afsr0_el1 = val,
            sysreg::AFSR1_EL1      => self.regs.afsr1_el1 = val,
            sysreg::FAR_EL1        => self.regs.far_el1 = val,
            sysreg::PAR_EL1        => self.regs.par_el1 = val,
            // Memory attributes
            sysreg::MAIR_EL1       => self.regs.mair_el1 = val,
            sysreg::AMAIR_EL1      => self.regs.amair_el1 = val,
            // Vector / exception
            sysreg::VBAR_EL1       => {
                self.regs.vbar_el1 = val;
                log::info!("MSR VBAR_EL1 = {val:#x} at insn #{}", self.insn_count);
            }
            sysreg::CONTEXTIDR_EL1 => self.regs.contextidr_el1 = val,
            // Thread ID
            sysreg::TPIDR_EL0      => self.regs.tpidr_el0 = val,
            sysreg::TPIDR_EL1      => self.regs.tpidr_el1 = val,
            sysreg::TPIDRRO_EL0    => {} // read-only, ignore
            // SP / exception state
            sysreg::SP_EL0         => self.regs.sp = val,
            sysreg::SP_EL1         => self.regs.sp_el1 = val,
            sysreg::ELR_EL1        => self.regs.elr_el1 = val,
            sysreg::SPSR_EL1       => self.regs.spsr_el1 = val as u32,
            sysreg::DAIF           => self.regs.daif = val as u32 & 0x3C0,
            sysreg::NZCV           => self.regs.nzcv = val as u32 & 0xF000_0000,
            sysreg::SPSEL          => self.regs.sp_sel = (val & 1) as u8,
            // Debug
            sysreg::MDSCR_EL1      => self.regs.mdscr_el1 = val as u32,
            // Cache
            sysreg::CSSELR_EL1     => self.regs.csselr_el1 = val,
            // Timer
            sysreg::CNTFRQ_EL0     => self.regs.cntfrq_el0 = val,
            sysreg::CNTV_CTL_EL0   => self.regs.cntv_ctl_el0 = val,
            sysreg::CNTV_CVAL_EL0  => self.regs.cntv_cval_el0 = val,
            sysreg::CNTP_CTL_EL0   => self.regs.cntp_ctl_el0 = val,
            sysreg::CNTP_CVAL_EL0  => self.regs.cntp_cval_el0 = val,
            sysreg::CNTP_TVAL_EL0  => {} // computed, ignore
            sysreg::CNTKCTL_EL1    => self.regs.cntkctl_el1 = val,
            // FP
            sysreg::FPCR           => self.regs.fpcr = val as u32,
            sysreg::FPSR           => self.regs.fpsr = val as u32,
            // EL2
            sysreg::HCR_EL2       => self.regs.hcr_el2 = val,
            sysreg::SCTLR_EL2     => self.regs.sctlr_el2 = val,
            sysreg::VBAR_EL2      => self.regs.vbar_el2 = val,
            sysreg::ELR_EL2       => self.regs.elr_el2 = val,
            sysreg::SPSR_EL2      => self.regs.spsr_el2 = val as u32,
            sysreg::VTTBR_EL2     => self.regs.vttbr_el2 = val,
            sysreg::CNTVOFF_EL2   => self.regs.cntvoff_el2 = val,
            // EL3
            sysreg::SCR_EL3       => self.regs.scr_el3 = val,
            sysreg::ELR_EL3       => self.regs.elr_el3 = val,
            sysreg::SPSR_EL3      => self.regs.spsr_el3 = val as u32,
            // Performance monitors — stub, ignore writes
            sysreg::PMCR_EL0 | sysreg::PMCNTENSET_EL0 | sysreg::PMCNTENCLR_EL0
            | sysreg::PMOVSCLR_EL0 | sysreg::PMUSERENR_EL0 | sysreg::PMCCNTR_EL0
            | sysreg::PMCCFILTR_EL0 | sysreg::PMSELR_EL0
            | sysreg::PMXEVTYPER_EL0 | sysreg::PMXEVCNTR_EL0 => {}
            // OS lock
            sysreg::OSLAR_EL1 | sysreg::OSDLR_EL1 => {}
            // ID registers — read-only, ignore writes
            sysreg::MIDR_EL1 | sysreg::MPIDR_EL1 | sysreg::REVIDR_EL1
            | sysreg::CTR_EL0 | sysreg::DCZID_EL0 => {}
            // Unknown: WI (write-ignored)
            _ => {
                log::trace!("MSR: unknown sysreg {id:#06x} ← {val:#x} (WI)");
            }
        }
    }

    // === Exception entry to EL1 ===
    fn take_exception_to_el1(&mut self, exception_class: u32, syndrome: u32) {
        // Save return address and PSTATE
        self.regs.elr_el1 = self.regs.pc;
        self.regs.spsr_el1 = self.save_pstate();
        self.regs.esr_el1 = (exception_class << 26) | (syndrome & 0x01FF_FFFF);

        // Vector offset depends on source EL and SP selection
        let vector_offset: u64 = match (self.regs.current_el, self.regs.sp_sel) {
            (0, _) => 0x400, // from lower EL, AArch64
            (1, 0) => 0x000, // from current EL, SP_EL0
            (1, 1) => 0x200, // from current EL, SP_ELx
            _ => 0x400,
        };

        if log::log_enabled!(log::Level::Debug) {
            log::debug!(
                "exception EC={:#x} ISS={:#x} from PC={:#x} → VBAR({:#x})+{:#x}={:#x} insn#{}",
                exception_class, syndrome, self.regs.pc,
                self.regs.vbar_el1, vector_offset,
                self.regs.vbar_el1.wrapping_add(vector_offset),
                self.insn_count,
            );
        }

        self.regs.pc = self.regs.vbar_el1.wrapping_add(vector_offset);
        self.regs.current_el = 1;
        self.regs.sp_sel = 1; // use SP_ELx on exception entry
        self.regs.daif = 0x3C0; // mask all (D, A, I, F)
        self.pc_written = true;
    }

    // === Exception entry to EL2 ===
    fn take_exception_to_el2(&mut self, exception_class: u32, syndrome: u32) {
        self.regs.elr_el2 = self.regs.pc;
        self.regs.spsr_el2 = self.save_pstate();
        let vector_offset: u64 = if self.regs.current_el < 2 { 0x400 } else { 0x200 };
        self.regs.pc = self.regs.vbar_el2.wrapping_add(vector_offset);
        self.regs.current_el = 2;
        self.regs.sp_sel = 1;
        self.regs.daif = 0x3C0;
        let _ = (exception_class, syndrome); // stored if needed
        self.pc_written = true;
    }

    // === ERET — exception return ===
    fn exec_eret(&mut self) -> HelmResult<()> {
        match self.regs.current_el {
            1 => {
                self.regs.pc = self.regs.elr_el1;
                self.restore_pstate(self.regs.spsr_el1);
            }
            2 => {
                self.regs.pc = self.regs.elr_el2;
                self.restore_pstate(self.regs.spsr_el2);
            }
            3 => {
                self.regs.pc = self.regs.elr_el3;
                self.restore_pstate(self.regs.spsr_el3);
            }
            _ => {
                return Err(HelmError::Isa("ERET from EL0".into()));
            }
        }
        self.pc_written = true;
        Ok(())
    }

    // === PSTATE save/restore ===
    fn save_pstate(&self) -> u32 {
        let mut spsr: u32 = 0;
        spsr |= self.regs.nzcv & 0xF000_0000;  // NZCV in bits [31:28]
        spsr |= self.regs.daif & 0x3C0;         // DAIF in bits [9:6]
        spsr |= (self.regs.current_el as u32) << 2; // EL in bits [3:2]
        spsr |= self.regs.sp_sel as u32;         // SP in bit [0]
        spsr
    }

    fn restore_pstate(&mut self, spsr: u32) {
        self.regs.nzcv = spsr & 0xF000_0000;
        self.regs.daif = spsr & 0x3C0;
        self.regs.current_el = ((spsr >> 2) & 3) as u8;
        self.regs.sp_sel = (spsr & 1) as u8;
    }

    // === TLBI dispatch ===
    fn exec_tlbi(&mut self, op1: u32, crm: u32, op2: u32, rt: u16) -> HelmResult<()> {
        match (op1, crm, op2) {
            (0, 3, 0) | (0, 7, 0) | (4, 3, 4) | (4, 7, 4) | (4, 3, 0) | (4, 7, 0) => {
                self.tlb.flush_all();
            }
            (0, 3, 1) | (0, 7, 1) | (0, 3, 5) | (0, 7, 5)
            | (0, 3, 3) | (0, 7, 3) | (0, 3, 7) | (0, 7, 7) => {
                let va = self.xn(rt) << 12;
                self.tlb.flush_va(va);
            }
            (0, 3, 2) | (0, 7, 2) => {
                let asid = (self.xn(rt) >> 48) as u16;
                self.tlb.flush_asid(asid);
            }
            _ => {
                self.tlb.flush_all();
            }
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

        if (insn >> 22) & 1 == 1 {
            self.trace.class = InsnClass::Load;
        } else {
            self.trace.class = InsnClass::Store;
        }

        // LDP/STP
        let top5 = (insn >> 27) & 0x1F;
        if top5 == 0b10101 || top5 == 0b00101 {
            return self.exec_pair(insn, mem);
        }
        // Exclusive
        // Exclusive: size xx 001000 (match all sizes)
        if (insn >> 24) & 0x3F == 0b001000 {
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
            let base = self.xn_sp(rn);
            let addr = base.wrapping_add(offset);
            let sz = 1usize << size;
            match opc {
                0 => {
                    self.trace_wr(mem, addr, self.xn(rt), sz)?;
                }
                1 => {
                    let val = self.trace_rd(mem, addr, sz)?;
                    self.set_xn(rt, val);
                }
                2 => {
                    let v = self.trace_rd(mem, addr, sz)?;
                    self.set_xn(rt, sext64(v, (sz * 8) as u32));
                }
                3 => {
                    let v = self.trace_rd(mem, addr, sz)?;
                    self.set_xn(rt, sext64(v, (sz * 8) as u32) & 0xFFFF_FFFF);
                }
                _ => {
                    return self.unimpl("ldst unsigned offset opc");
                }
            }
            return Ok(());
        }
        // Pre/post/unscaled/reg: size 111000 opc ...
        if (insn >> 24) & 0x3F == 0b111000 {
            let opc = (insn >> 22) & 0x3;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rt = (insn & 0x1F) as u16;
            let base = self.xn_sp(rn);
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
                self.trace_wr(mem, addr, self.xn(rt), sz)?;
            } else if opc == 1 {
                let val = self.trace_rd(mem, addr, sz)?;
                self.set_xn(rt, val);
            } else if opc == 2 {
                let v = self.trace_rd(mem, addr, sz)?;
                self.set_xn(rt, sext64(v, (sz * 8) as u32));
            } else {
                let v = self.trace_rd(mem, addr, sz)?;
                self.set_xn(rt, sext64(v, (sz * 8) as u32) & 0xFFFF_FFFF);
            }
            if let Some(w) = wb {
                self.set_xn_sp(rn, w);
            }
            return Ok(());
        }
        // Load literal
        if (insn >> 24) & 0x3F == 0b011000 {
            let rt = (insn & 0x1F) as u16;
            let imm19 = sext((insn >> 5) & 0x7FFFF, 19) as u64;
            let addr = self.regs.pc.wrapping_add(imm19 << 2);
            let sz = if size == 0 { 4 } else { 8 };
            let val = self.trace_rd(mem, addr, sz)?;
            self.set_xn(rt, val);
            return Ok(());
        }
        self.unimpl("ldst")
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
        let sz = scale as usize;
        if l == 1 {
            let v0 = self.trace_rd(mem, addr, sz)?;
            self.set_xn(rt, v0);
            let v1 = self.trace_rd(mem, addr.wrapping_add(scale), sz)?;
            self.set_xn(rt2, v1);
        } else {
            self.trace_wr(mem, addr, self.xn(rt), sz)?;
            self.trace_wr(mem, addr.wrapping_add(scale), self.xn(rt2), sz)?;
        }
        if let Some(w) = wb {
            self.set_xn_sp(rn, w);
        }
        Ok(())
    }

    fn exec_exclusive(&mut self, insn: u32, mem: &mut AddressSpace) -> HelmResult<()> {
        let size = (insn >> 30) & 0x3;
        let l = (insn >> 22) & 1;
        let rn = ((insn >> 5) & 0x1F) as u16;
        let rt = (insn & 0x1F) as u16;
        let base = self.xn_sp(rn);
        let sz = 1usize << size;
        if l == 1 {
            let val = self.trace_rd(mem, base, sz)?;
            self.set_xn(rt, val);
        } else {
            let rs = ((insn >> 16) & 0x1F) as u16;
            self.trace_wr(mem, base, self.xn(rt), sz)?;
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
        let base = self.xn_sp(rn);
        let sz = 1usize << size;
        let old = self.trace_rd(mem, base, sz)?;
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
        self.trace_wr(mem, base, new, sz)?;
        self.set_xn(rt, old);
        Ok(())
    }

    // === SIMD/FP Loads and Stores (subset for memset/memcpy) ===
    fn exec_ldst_simd(&mut self, insn: u32, mem: &mut AddressSpace) -> HelmResult<()> {
        let size = (insn >> 30) & 0x3;
        let top5 = (insn >> 27) & 0x1F;
        let opc = (insn >> 22) & 0x3;
        let is_load = opc & 1 == 1;
        let ldst_kind: &'static str = match (top5 & 0b00111, (insn >> 24) & 0x3F, is_load) {
            (0b00101, _, true) => "STP/LDP_simd_pair_L",
            (0b00101, _, false) => "STP/LDP_simd_pair_S",
            (_, 0b111101, true) => "LDR_simd_uimm",
            (_, 0b111101, false) => "STR_simd_uimm",
            (_, 0b111100, true) if (insn >> 21) & 1 == 1 => "LDR_simd_reg",
            (_, 0b111100, false) if (insn >> 21) & 1 == 1 => "STR_simd_reg",
            (_, 0b111100, true) => "LDR_simd_imm9",
            (_, 0b111100, false) => "STR_simd_imm9",
            _ => "simd_ldst_UNKNOWN",
        };
        if self.simd_seen.insert(ldst_kind) {
            log::warn!("SIMD ldst encountered: {} (insn={:#010x} PC={:#x})", ldst_kind, insn, self.regs.pc);
        }

        // STP/LDP SIMD pair (S/D/Q): opc xx 101 V=1 ...
        if top5 & 0b00111 == 0b00101 {
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
                if opc == 0b10 {
                    self.regs.v[rt] = self.trace_rd128(mem, addr)?;
                    self.regs.v[rt2] = self.trace_rd128(mem, addr.wrapping_add(scale))?;
                } else {
                    let sz = scale as usize;
                    let lo = self.trace_rd(mem, addr, sz)?;
                    let hi = self.trace_rd(mem, addr.wrapping_add(scale), sz)?;
                    self.regs.v[rt] = lo as u128;
                    self.regs.v[rt2] = hi as u128;
                }
            } else {
                if opc == 0b10 {
                    self.trace_wr128(mem, addr, self.regs.v[rt])?;
                    self.trace_wr128(mem, addr.wrapping_add(scale), self.regs.v[rt2])?;
                } else {
                    let sz = scale as usize;
                    self.trace_wr(mem, addr, self.regs.v[rt] as u64, sz)?;
                    self.trace_wr(mem, addr.wrapping_add(scale), self.regs.v[rt2] as u64, sz)?;
                }
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
                    self.regs.v[rt] = self.trace_rd128(mem, addr)?;
                } else {
                    self.trace_wr128(mem, addr, self.regs.v[rt])?;
                }
            } else {
                // Scalar SIMD: B/H/S/D
                let sz = scale as usize;
                if is_load {
                    let val = self.trace_rd(mem, addr, sz.max(1))?;
                    self.regs.v[rt] = val as u128;
                } else {
                    let val = self.regs.v[rt] as u64;
                    self.trace_wr(mem, addr, val, sz.max(1))?;
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
            let (addr, wb) = if (insn >> 21) & 1 == 1 {
                let rm = ((insn >> 16) & 0x1F) as u16;
                let option = (insn >> 13) & 0x7;
                let s_bit = (insn >> 12) & 1;
                let is_q = size == 0 && opc >= 2;
                let shift = if s_bit == 1 { if is_q { 4 } else { size } } else { 0 };
                let rm_val = self.xn(rm);
                let offset = match option {
                    0b010 => (rm_val as u32 as u64) << shift,
                    0b011 => rm_val << shift,
                    0b110 => (rm_val as i32 as i64 as u64) << shift,
                    0b111 => rm_val << shift,
                    _ => rm_val,
                };
                (base.wrapping_add(offset), None)
            } else {
                let imm9 = sext((insn >> 12) & 0x1FF, 9) as u64;
                match idx_type {
                    0b00 => (base.wrapping_add(imm9), None),
                    0b01 => (base, Some(base.wrapping_add(imm9))),
                    0b11 => {
                        let a = base.wrapping_add(imm9);
                        (a, Some(a))
                    }
                    _ => (base, None),
                }
            };
            let is_q = size == 0 && opc >= 2;
            let is_store = opc & 1 == 0;
            if is_store {
                if is_q {
                    self.trace_wr128(mem, addr, self.regs.v[rt])?;
                } else {
                    let sz = (1usize << size).max(1);
                    let val = self.regs.v[rt] as u64;
                    self.trace_wr(mem, addr, val, sz)?;
                }
            } else {
                if is_q {
                    self.regs.v[rt] = self.trace_rd128(mem, addr)?;
                } else {
                    let sz = (1usize << size).max(1);
                    let val = self.trace_rd(mem, addr, sz)?;
                    self.regs.v[rt] = val as u128;
                }
            }
            if let Some(w) = wb {
                self.set_xn_sp(rn, w);
            }
            return Ok(());
        }

        // LD1/ST1 multiple structures: 0 Q 001100 L 0 00000 opcode size Rn Rt
        // Also post-index form:        0 Q 001100 L 1 Rm    opcode size Rn Rt
        if (insn >> 24) & 0x3E == 0b001100 {
            let q = (insn >> 30) & 1;
            let l = (insn >> 22) & 1;
            let rm = ((insn >> 16) & 0x1F) as u16;
            let post_index = (insn >> 23) & 1 == 1;
            let opcode = (insn >> 12) & 0xF;
            let elem_size = (insn >> 10) & 0x3;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rt = (insn & 0x1F) as usize;
            let reg_bytes: usize = if q == 1 { 16 } else { 8 };
            let nregs: usize = match opcode {
                0b0111 => 1,
                0b1010 => 2,
                0b0110 => 3,
                0b0010 => 4,
                _ => return self.unimpl("simd_ldst_multi (interleave)"),
            };
            let base = self.xn_sp(rn);
            let mut addr = base;
            if l == 1 {
                for i in 0..nregs {
                    let vr = (rt + i) % 32;
                    if reg_bytes == 16 {
                        self.regs.v[vr] = self.trace_rd128(mem, addr)?;
                    } else {
                        let lo = self.trace_rd(mem, addr, 8)?;
                        self.regs.v[vr] = lo as u128;
                    }
                    addr = addr.wrapping_add(reg_bytes as u64);
                }
            } else {
                for i in 0..nregs {
                    let vr = (rt + i) % 32;
                    if reg_bytes == 16 {
                        self.trace_wr128(mem, addr, self.regs.v[vr])?;
                    } else {
                        self.trace_wr(mem, addr, self.regs.v[vr] as u64, 8)?;
                    }
                    addr = addr.wrapping_add(reg_bytes as u64);
                }
            }
            if post_index {
                let offset = if rm == 31 {
                    (nregs * reg_bytes) as u64
                } else {
                    self.xn(rm)
                };
                self.set_xn_sp(rn, base.wrapping_add(offset));
            }
            return Ok(());
        }

        self.unimpl("simd_ldst")
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
            if s == 0 {
                self.set_xn_sp(rd, r);
            } else {
                self.set_xn(rd, r);
            }
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
                    self.flags(r, self.regs.c(), self.regs.v(), sf == 1);
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
                _ => {
                    return self.unimpl("dp3_multiply");
                }
            }
            return Ok(());
        }
        // 2-source: UDIV/SDIV/LSLV/LSRV/ASRV
        if (insn >> 21) & 0x3FF == 0xD6 {
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
        if (insn >> 21) & 0x3FF == 0x2D6 {
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
                1 => {
                    let swap16 = |v: u16| -> u16 { v.swap_bytes() };
                    if sf == 1 {
                        let b = a.to_le_bytes();
                        u64::from_le_bytes([b[1],b[0],b[3],b[2],b[5],b[4],b[7],b[6]])
                    } else {
                        let w = self.wn(rn);
                        let lo = swap16(w as u16) as u32;
                        let hi = swap16((w >> 16) as u16) as u32;
                        ((hi << 16) | lo) as u64
                    }
                }
                2 => {
                    if sf == 1 {
                        let b = a.to_le_bytes();
                        u64::from_le_bytes([b[3],b[2],b[1],b[0],b[7],b[6],b[5],b[4]])
                    } else {
                        (self.wn(rn).swap_bytes()) as u64
                    }
                }
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
                    if sf == 1 {
                        let s = if a >> 63 == 1 { (!a).leading_zeros() } else { a.leading_zeros() };
                        s.saturating_sub(1) as u64
                    } else {
                        let w = self.wn(rn);
                        let s = if w >> 31 == 1 { (!w).leading_zeros() } else { w.leading_zeros() };
                        s.saturating_sub(1) as u64
                    }
                }
                _ => a,
            };
            self.set_xn(rd, mask(r, sf));
            return Ok(());
        }
        // Conditional select
        if (insn >> 21) & 0x1FF == 0xD4 {
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
            let cond = ((insn >> 12) & 0xF) as u8;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let nzcv_imm = (insn & 0xF) as u32;
            let is_imm = (insn >> 11) & 1 == 1;
            if self.cond(cond) {
                let a = self.xn(rn);
                let b = if is_imm {
                    ((insn >> 16) & 0x1F) as u64
                } else {
                    self.xn(((insn >> 16) & 0x1F) as u16)
                };
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
        if (insn >> 21) & 0xFF == 0xD0 {
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
        let mnemonic = { let q = decode_a64(insn); if q != "UNKNOWN" { q } else { decode_aarch64_simd(insn) } };
        if self.simd_seen.insert(mnemonic) {
            log::warn!(
                "SIMD insn encountered: {} (insn={:#010x} PC={:#x})",
                mnemonic, insn, self.regs.pc
            );
        }
        // DUP Vd.T, Wn/Xn: 0 Q 00 1110 000 imm5 0 0001 1 Rn Rd
        if insn & 0xBFE0_FC00 == 0x0E00_0C00 {
            let q = (insn >> 30) & 1;
            let imm5 = (insn >> 16) & 0x1F;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rd = (insn & 0x1F) as usize;
            let (esize, val) = if imm5 & 1 != 0 {
                (8, self.xn(rn) as u8 as u128)
            } else if imm5 & 2 != 0 {
                (16, self.xn(rn) as u16 as u128)
            } else if imm5 & 4 != 0 {
                (32, self.xn(rn) as u32 as u128)
            } else {
                (64, self.xn(rn) as u128)
            };
            let total_bits = if q == 1 { 128 } else { 64 };
            let mut v: u128 = 0;
            for i in 0..(total_bits / esize) {
                v |= val << (i * esize);
            }
            self.regs.v[rd] = v;
            return Ok(());
        }

        // INS Vd.Ts[idx], Wn/Xn: 0 1 0 01110 000 imm5 0 00111 Rn Rd
        if insn & 0xFFE0_FC00 == 0x4E00_1C00 {
            let imm5 = (insn >> 16) & 0x1F;
            let rn = ((insn >> 5) & 0x1F) as u16;
            let rd = (insn & 0x1F) as usize;
            let val = self.xn(rn);
            if imm5 & 1 != 0 {
                let idx = (imm5 >> 1) as usize;
                let shift = idx * 8;
                let mask = !(0xFFu128 << shift);
                self.regs.v[rd] = (self.regs.v[rd] & mask) | ((val as u128 & 0xFF) << shift);
            } else if imm5 & 2 != 0 {
                let idx = (imm5 >> 2) as usize;
                let shift = idx * 16;
                let mask = !(0xFFFFu128 << shift);
                self.regs.v[rd] = (self.regs.v[rd] & mask) | ((val as u128 & 0xFFFF) << shift);
            } else if imm5 & 4 != 0 {
                let idx = (imm5 >> 3) as usize;
                let shift = idx * 32;
                let mask = !(0xFFFF_FFFFu128 << shift);
                self.regs.v[rd] = (self.regs.v[rd] & mask) | ((val as u128 & 0xFFFF_FFFF) << shift);
            } else if imm5 & 8 != 0 {
                let idx = (imm5 >> 4) as usize;
                let shift = idx * 64;
                let mask = !(0xFFFF_FFFF_FFFF_FFFFu128 << shift);
                self.regs.v[rd] = (self.regs.v[rd] & mask) | ((val as u128 & 0xFFFF_FFFF_FFFF_FFFF) << shift);
            }
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

        // FP <-> integer conversions + FMOV: sf 00 11110 ftype 1 rmode opcode 000000 Rn Rd
        if insn & 0x5F20_FC00 == 0x1E20_0000 {
            let sf = (insn >> 31) & 1;
            let ftype = (insn >> 22) & 0x3;
            let rmode = (insn >> 19) & 0x3;
            let opcode = (insn >> 16) & 0x7;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;

            match (rmode, opcode) {
                // FMOV Sd, Wn
                (0, 7) if sf == 0 => {
                    self.regs.v[rd] = self.xn(rn as u16) as u32 as u128;
                }
                // FMOV Wd, Sn
                (0, 6) if sf == 0 => {
                    self.set_xn(rd as u16, (self.regs.v[rn] as u32) as u64);
                }
                // FMOV Dd, Xn
                (0, 7) if sf == 1 => {
                    self.regs.v[rd] = self.xn(rn as u16) as u128;
                }
                // FMOV Xd, Dn
                (0, 6) if sf == 1 => {
                    self.set_xn(rd as u16, self.regs.v[rn] as u64);
                }
                // SCVTF: signed int -> FP
                (0, 2) => {
                    let ival = if sf == 1 { self.xn(rn as u16) as i64 } else { self.xn(rn as u16) as i32 as i64 };
                    if ftype == 0 {
                        self.regs.v[rd] = (ival as f32).to_bits() as u128;
                    } else {
                        self.regs.v[rd] = (ival as f64).to_bits() as u128;
                    }
                }
                // UCVTF: unsigned int -> FP
                (0, 3) => {
                    let uval = if sf == 1 { self.xn(rn as u16) } else { self.xn(rn as u16) as u32 as u64 };
                    if ftype == 0 {
                        self.regs.v[rd] = (uval as f32).to_bits() as u128;
                    } else {
                        self.regs.v[rd] = (uval as f64).to_bits() as u128;
                    }
                }
                // FCVTZS: FP -> signed int (round toward zero)
                (3, 0) => {
                    let val = if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        if sf == 1 { f as i64 as u64 } else { f as i32 as u32 as u64 }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        if sf == 1 { f as i64 as u64 } else { f as i32 as u32 as u64 }
                    };
                    self.set_xn(rd as u16, val);
                }
                // FCVTZU: FP -> unsigned int (round toward zero)
                (3, 1) => {
                    let val = if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        if sf == 1 { f as u64 } else { f as u32 as u64 }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        if sf == 1 { f as u64 } else { f as u32 as u64 }
                    };
                    self.set_xn(rd as u16, val);
                }
                // FCVTNS: FP -> signed int (round nearest, ties to even)
                (0, 0) => {
                    let val = if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        let r = f.round_ties_even();
                        if sf == 1 { r as i64 as u64 } else { r as i32 as u32 as u64 }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        let r = f.round_ties_even();
                        if sf == 1 { r as i64 as u64 } else { r as i32 as u32 as u64 }
                    };
                    self.set_xn(rd as u16, val);
                }
                // FCVTNU: FP -> unsigned int (round nearest, ties to even)
                (0, 1) => {
                    let val = if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        let r = f.round_ties_even();
                        if sf == 1 { r as u64 } else { r as u32 as u64 }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        let r = f.round_ties_even();
                        if sf == 1 { r as u64 } else { r as u32 as u64 }
                    };
                    self.set_xn(rd as u16, val);
                }
                // FCVTMS: FP -> signed int (round toward -inf)
                (2, 0) => {
                    let val = if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        let r = f.floor();
                        if sf == 1 { r as i64 as u64 } else { r as i32 as u32 as u64 }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        let r = f.floor();
                        if sf == 1 { r as i64 as u64 } else { r as i32 as u32 as u64 }
                    };
                    self.set_xn(rd as u16, val);
                }
                // FCVTPS: FP -> signed int (round toward +inf)
                (1, 0) => {
                    let val = if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        let r = f.ceil();
                        if sf == 1 { r as i64 as u64 } else { r as i32 as u32 as u64 }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        let r = f.ceil();
                        if sf == 1 { r as i64 as u64 } else { r as i32 as u32 as u64 }
                    };
                    self.set_xn(rd as u16, val);
                }
                // FCVTAS: FP -> signed int (round to nearest, ties away)
                (0, 4) => {
                    let val = if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        let r = f.round();
                        if sf == 1 { r as i64 as u64 } else { r as i32 as u32 as u64 }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        let r = f.round();
                        if sf == 1 { r as i64 as u64 } else { r as i32 as u32 as u64 }
                    };
                    self.set_xn(rd as u16, val);
                }
                _ => {
                    log::warn!("Unhandled FP<->int conv: sf={sf} ftype={ftype} rmode={rmode} opcode={opcode} insn={insn:#010x} PC={:#x}", self.regs.pc);
                }
            }
            return Ok(());
        }

        // MOVI / MVNI (advanced SIMD modified immediate)
        // 0 Q op 0111100000 a:b:c:d:e:f:g:h cmode 01 Rd
        if insn & 0x9FF8_0400 == 0x0F00_0400 {
            let q = (insn >> 30) & 1;
            let op = (insn >> 29) & 1;
            let rd = (insn & 0x1F) as usize;
            let cmode = (insn >> 12) & 0xF;
            let abc = ((insn >> 16) & 0x7) as u8;
            let defgh = ((insn >> 5) & 0x1F) as u8;
            let imm8 = (abc << 5) | defgh;
            let mut val: u128 = 0;
            if cmode == 0b1110 && op == 1 {
                let mut imm64: u64 = 0;
                for i in 0..8 {
                    if (imm8 >> i) & 1 != 0 {
                        imm64 |= 0xFFu64 << (i * 8);
                    }
                }
                val = if q == 1 { ((imm64 as u128) << 64) | imm64 as u128 } else { imm64 as u128 };
            } else if cmode == 0b1110 && op == 0 {
                let byte_val = imm8 as u128;
                let bytes = if q == 1 { 16 } else { 8 };
                for i in 0..bytes {
                    val |= byte_val << (i * 8);
                }
            } else {
                let shift = ((cmode >> 1) & 3) * 8;
                let base = (imm8 as u64) << shift;
                let elem_size = if cmode < 4 { 4usize } else if cmode < 8 { 4 } else { 2 };
                let elem_mask = if elem_size == 4 { 0xFFFF_FFFFu64 } else { 0xFFFFu64 };
                let elem = if op == 1 { !base & elem_mask } else { base & elem_mask };
                let total = if q == 1 { 16 } else { 8 };
                for i in 0..(total / elem_size) {
                    val |= (elem as u128) << (i * elem_size * 8);
                }
            }
            self.regs.v[rd] = val;
            return Ok(());
        }



        // SIMD across lanes: 0 Q U 01110 size 11000 opcode 10 Rn Rd
        if (insn >> 17) & 0x7FFF == 0b0_01110_00_11000u32 >> 0 {
            let across_check = (insn >> 17) & 0x7FFF;
            let _ = across_check;
        }
        if insn & 0x9F3E_0C00 == 0x0E30_0800 && (insn >> 17) & 0x1F == 0b11000 {
            let q = (insn >> 30) & 1;
            let u = (insn >> 29) & 1;
            let size = (insn >> 22) & 0x3;
            let opcode = (insn >> 12) & 0x1F;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            let bytes = if q == 1 { 16usize } else { 8 };
            let esize = 1usize << size;
            let ebits = esize * 8;
            let emask: u128 = if esize >= 16 { u128::MAX } else { (1u128 << ebits) - 1 };
            let count = bytes / esize;
            let a = self.regs.v[rn];
            let mut acc = (a >> 0) & emask;
            for i in 1..count {
                let ea = (a >> (i * ebits)) & emask;
                acc = match (u, opcode) {
                    (_, 0b11011) => (acc + ea) & emask,
                    (0, 0b01010) => {
                        let sa = acc as i128 - if acc >> (ebits-1) != 0 { 1i128 << ebits } else { 0 };
                        let sb = ea as i128 - if ea >> (ebits-1) != 0 { 1i128 << ebits } else { 0 };
                        if sa >= sb { acc } else { ea }
                    }
                    (1, 0b01010) => if acc >= ea { acc } else { ea },
                    (0, 0b11010) => {
                        let sa = acc as i128 - if acc >> (ebits-1) != 0 { 1i128 << ebits } else { 0 };
                        let sb = ea as i128 - if ea >> (ebits-1) != 0 { 1i128 << ebits } else { 0 };
                        if sa <= sb { acc } else { ea }
                    }
                    (1, 0b11010) => if acc <= ea { acc } else { ea },
                    _ => {
                        return self.unimpl("simd_across_lanes");
                    }
                };
            }
            self.regs.v[rd] = acc;
            return Ok(());
        }

        // UMOV Wd/Xd, Vn.T[idx]: 0 Q 00 1110 000 imm5 0 01111 Rn Rd
        if insn & 0xBFE0_FC00 == 0x0E00_3C00 {
            let imm5 = (insn >> 16) & 0x1F;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as u16;
            let v = self.regs.v[rn];
            let (val, esize) = if imm5 & 1 != 0 {
                let idx = (imm5 >> 1) as usize;
                ((v >> (idx * 8)) as u64 & 0xFF, 1)
            } else if imm5 & 2 != 0 {
                let idx = (imm5 >> 2) as usize;
                ((v >> (idx * 16)) as u64 & 0xFFFF, 2)
            } else if imm5 & 4 != 0 {
                let idx = (imm5 >> 3) as usize;
                ((v >> (idx * 32)) as u64 & 0xFFFF_FFFF, 4)
            } else {
                let idx = (imm5 >> 4) as usize;
                ((v >> (idx * 64)) as u64, 8)
            };
            self.set_xn(rd, val);
            return Ok(());
        }

        // Advanced SIMD three-same: 0 Q U 01110 size 1 Rm opcode 1 Rn Rd
        if (insn >> 24) & 0x1F == 0b01110 && (insn >> 21) & 1 == 1 && (insn >> 10) & 1 == 1 {
            let q = (insn >> 30) & 1;
            let u = (insn >> 29) & 1;
            let size = (insn >> 22) & 0x3;
            let rm = ((insn >> 16) & 0x1F) as usize;
            let opcode = (insn >> 11) & 0x1F;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            let a = self.regs.v[rn];
            let b = self.regs.v[rm];
            let bytes = if q == 1 { 16usize } else { 8 };

            if opcode == 0b00011 {
                let r = match (u, size) {
                    (0, 0) => a & b,
                    (0, 1) => a & !b,
                    (0, 2) => a | b,
                    (0, 3) => a | !b,
                    (1, 0) => a ^ b,
                    (1, 1) => (a & !b) | (self.regs.v[rd] & b),
                    (1, 2) => (a & b) | (self.regs.v[rd] & !b),
                    (1, 3) => (!a & b) | (self.regs.v[rd] & !b),
                    _ => a,
                };
                self.regs.v[rd] = if q == 0 { r & ((1u128 << 64) - 1) } else { r };
                return Ok(());
            }

            let esize = 1usize << size;
            let ebits = esize * 8;
            let emask: u128 = if esize >= 16 { u128::MAX } else { (1u128 << ebits) - 1 };
            let mut result: u128 = 0;
            for i in 0..(bytes / esize) {
                let shift = i * ebits;
                let ea = (a >> shift) & emask;
                let eb = (b >> shift) & emask;
                let sa = ea as i128 - if ea >> (ebits - 1) != 0 { 1i128 << ebits } else { 0 };
                let sb = eb as i128 - if eb >> (ebits - 1) != 0 { 1i128 << ebits } else { 0 };
                let er = match (u, opcode) {
                    (0, 0b10000) => ea.wrapping_add(eb) & emask,
                    (1, 0b10000) => ea.wrapping_sub(eb) & emask,
                    (0, 0b00110) => if sa > sb { emask } else { 0 },
                    (1, 0b00110) => if ea > eb { emask } else { 0 },
                    (0, 0b00111) => if sa >= sb { emask } else { 0 },
                    (1, 0b00111) => if ea >= eb { emask } else { 0 },
                    (1, 0b10001) => if ea == eb { emask } else { 0 },
                    (0, 0b10001) => if ea & eb != 0 { emask } else { 0 },
                    (0, 0b01100) => if sa >= sb { ea } else { eb },
                    (1, 0b01100) => if ea >= eb { ea } else { eb },
                    (0, 0b01101) => if sa <= sb { ea } else { eb },
                    (1, 0b01101) => if ea <= eb { ea } else { eb },
                    (0, 0b10011) => ea.wrapping_mul(eb) & emask,
                    (0, 0b10010) => {
                        let d = (self.regs.v[rd] >> shift) & emask;
                        d.wrapping_add(ea.wrapping_mul(eb)) & emask
                    }
                    _ => {
                        return self.unimpl("simd_three_same");
                    }
                };
                result |= er << shift;
            }
            self.regs.v[rd] = result;
            return Ok(());
        }

        // SIMD two-reg misc: 0 Q U 01110 size 10000 opcode 10 Rn Rd
        if insn & 0x9F3E_0C00 == 0x0E20_0800 {
            let q = (insn >> 30) & 1;
            let u = (insn >> 29) & 1;
            let size = (insn >> 22) & 0x3;
            let opcode = (insn >> 12) & 0x1F;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            let bytes = if q == 1 { 16usize } else { 8 };
            let esize = 1usize << size;
            let ebits = esize * 8;
            let emask: u128 = if esize >= 16 { u128::MAX } else { (1u128 << ebits) - 1 };
            let a = self.regs.v[rn];
            let mut result: u128 = 0;
            for i in 0..(bytes / esize) {
                let shift = i * ebits;
                let ea = (a >> shift) & emask;
                let sign = ea >> (ebits - 1);
                let er = match (u, opcode) {
                    (0, 8) => if sign == 0 && ea != 0 { emask } else { 0 },
                    (0, 9) => if ea == 0 { emask } else { 0 },
                    (0, 10) => if sign != 0 { emask } else { 0 },
                    (1, 8) => if sign == 0 { emask } else { 0 },
                    (1, 9) => if sign != 0 || ea == 0 { emask } else { 0 },
                    (0, 11) => {
                        let sa = ea as i128 - if sign != 0 { 1i128 << ebits } else { 0 };
                        (sa.unsigned_abs() as u128) & emask
                    }
                    (1, 11) => {
                        let sa = ea as i128 - if sign != 0 { 1i128 << ebits } else { 0 };
                        ((-sa) as u128) & emask
                    }
                    (0, 5) if size == 0 => ea.reverse_bits() >> (128 - ebits) & emask, // RBIT_v (size=0 only)
                    (0, 5) => (ea.count_ones() as u128) & emask, // CNT_v (size!=0)
                    (1, 5) if size == 0 => (!ea) & emask, // NOT_v (size=00)
                    (0, 15) if size >= 2 => {
                        if size == 2 {
                            let f = f32::from_bits(ea as u32);
                            (f.abs().to_bits() as u128) & emask
                        } else {
                            let f = f64::from_bits(ea as u64);
                            (f.abs().to_bits() as u128) & emask
                        }
                    }
                    (1, 15) if size >= 2 => {
                        if size == 2 {
                            let f = f32::from_bits(ea as u32);
                            ((-f).to_bits() as u128) & emask
                        } else {
                            let f = f64::from_bits(ea as u64);
                            ((-f).to_bits() as u128) & emask
                        }
                    }
                    (1, 31) if size >= 2 => {
                        if size == 2 {
                            let f = f32::from_bits(ea as u32);
                            (f.sqrt().to_bits() as u128) & emask
                        } else {
                            let f = f64::from_bits(ea as u64);
                            (f.sqrt().to_bits() as u128) & emask
                        }
                    }
                    _ => {
                        return self.unimpl("simd_2reg_misc");
                    }
                };
                result |= er << shift;
            }
            self.regs.v[rd] = result;
            return Ok(());
        }

        // SIMD shift by immediate: 0 Q U 011110 immh:immb opcode 1 Rn Rd
        if (insn >> 24) & 0x1F == 0b01111 && (insn >> 10) & 1 == 1 && (insn >> 19) & 0xF != 0 {
            let q = (insn >> 30) & 1;
            let u = (insn >> 29) & 1;
            let immh = (insn >> 19) & 0xF;
            let immb = (insn >> 16) & 0x7;
            let opcode = (insn >> 11) & 0x1F;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            let shift_val = ((immh << 3) | immb) as usize;
            let src_esize = if immh & 8 != 0 { 64usize }
                else if immh & 4 != 0 { 32 }
                else if immh & 2 != 0 { 16 }
                else { 8 };

            if opcode == 0b10100 {
                let amt = shift_val - src_esize;
                let dst_esize = src_esize * 2;
                let src_mask: u128 = (1u128 << src_esize) - 1;
                let dst_mask: u128 = (1u128 << dst_esize) - 1;
                let src_start = if q == 1 { 64 } else { 0 };
                let count = 64 / src_esize;
                let mut result: u128 = 0;
                for i in 0..count {
                    let src_val = (self.regs.v[rn] >> (src_start + i * src_esize)) & src_mask;
                    let widened = if u == 0 {
                        let sign = src_val >> (src_esize - 1);
                        if sign != 0 {
                            (src_val | (dst_mask & !src_mask)) << amt
                        } else {
                            src_val << amt
                        }
                    } else {
                        src_val << amt
                    };
                    result |= (widened & dst_mask) << (i * dst_esize);
                }
                self.regs.v[rd] = result;
                return Ok(());
            }

            let esize = src_esize;
            let emask: u128 = if esize >= 128 { u128::MAX } else { (1u128 << esize) - 1 };
            let bytes = if q == 1 { 16usize } else { 8 };
            let a = self.regs.v[rn];
            let mut result: u128 = 0;
            for i in 0..(bytes * 8 / esize) {
                let bit_shift = i * esize;
                let ea = (a >> bit_shift) & emask;
                let er = match (u, opcode) {
                    (1, 0b00000) => {
                        let amt = esize * 2 - shift_val;
                        if amt >= esize { 0 } else { (ea >> amt) & emask }
                    }
                    (0, 0b00000) => {
                        let amt = esize * 2 - shift_val;
                        if amt >= esize {
                            if ea >> (esize - 1) != 0 { emask } else { 0 }
                        } else {
                            let sign_bit = ea >> (esize - 1);
                            let shifted = ea >> amt;
                            if sign_bit != 0 {
                                (shifted | (emask << (esize - amt))) & emask
                            } else {
                                shifted & emask
                            }
                        }
                    }
                    (0, 0b01010) | (1, 0b01010) => {
                        let amt = shift_val - esize;
                        (ea << amt) & emask
                    }
                    _ => {
                        return self.unimpl("simd_shift_imm");
                    }
                };
                result |= er << bit_shift;
            }
            self.regs.v[rd] = result;
            return Ok(());
        }


        // INS Vd.Ts[idx1], Vn.Ts[idx2]: 0 1 1 01110 000 imm5 0 imm4 1 Rn Rd
        if insn & 0xFFE0_8400 == 0x6E00_0400 {
            let imm5 = ((insn >> 16) & 0x1F) as usize;
            let imm4 = ((insn >> 11) & 0xF) as usize;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            let (esize, dst_idx, src_idx) = if imm5 & 1 != 0 {
                (8, imm5 >> 1, imm4)
            } else if imm5 & 2 != 0 {
                (16, imm5 >> 2, imm4 >> 1)
            } else if imm5 & 4 != 0 {
                (32, imm5 >> 3, imm4 >> 2)
            } else {
                (64, imm5 >> 4, imm4 >> 3)
            };
            let emask: u128 = if esize >= 128 { u128::MAX } else { (1u128 << esize) - 1 };
            let src_val = (self.regs.v[rn] >> (src_idx * esize)) & emask;
            let dst_shift = dst_idx * esize;
            self.regs.v[rd] = (self.regs.v[rd] & !(emask << dst_shift)) | (src_val << dst_shift);
            return Ok(());
        }

        // EXT Vd.T, Vn.T, Vm.T, #imm: 0 Q 10 1110 00 0 Rm 0 imm4 0 Rn Rd
        if insn & 0xBFE0_8400 == 0x2E00_0000 && (insn >> 24) & 0x1F == 0b01110 {
            let q = (insn >> 30) & 1;
            let rm = ((insn >> 16) & 0x1F) as usize;
            let imm4 = ((insn >> 11) & 0xF) as usize;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            let total = if q == 1 { 16usize } else { 8 };
            let a = self.regs.v[rn];
            let b = self.regs.v[rm];
            let mut result: u128 = 0;
            for i in 0..total {
                let idx = imm4 + i;
                let byte = if idx < total {
                    ((a >> (idx * 8)) & 0xFF) as u8
                } else {
                    ((b >> ((idx - total) * 8)) & 0xFF) as u8
                };
                result |= (byte as u128) << (i * 8);
            }
            self.regs.v[rd] = result;
            return Ok(());
        }

        // SIMD permute: 0 Q 0 01110 size 0 Rm 0 opcode 10 Rn Rd
        if (insn >> 24) & 0x1F == 0b01110 && (insn >> 21) & 1 == 0 && (insn >> 10) & 3 == 2 {
            let q = (insn >> 30) & 1;
            let size = (insn >> 22) & 0x3;
            let rm = ((insn >> 16) & 0x1F) as usize;
            let opcode = (insn >> 12) & 0x7;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            let esize = 1usize << size;
            let ebits = esize * 8;
            let emask: u128 = if esize >= 16 { u128::MAX } else { (1u128 << ebits) - 1 };
            let elems = if q == 1 { 128 / ebits } else { 64 / ebits };
            let a = self.regs.v[rn];
            let b = self.regs.v[rm];
            let mut result: u128 = 0;
            match opcode {
                1 | 5 => {
                    let step = if opcode == 1 { 0 } else { 1 };
                    let mut ri = 0;
                    for i in (step..elems).step_by(2) {
                        result |= ((a >> (i * ebits)) & emask) << (ri * ebits);
                        ri += 1;
                    }
                    for i in (step..elems).step_by(2) {
                        result |= ((b >> (i * ebits)) & emask) << (ri * ebits);
                        ri += 1;
                    }
                }
                3 | 7 => {
                    let half = elems / 2;
                    let base = if opcode == 3 { 0 } else { half };
                    for i in 0..half {
                        let ai = (a >> ((base + i) * ebits)) & emask;
                        let bi = (b >> ((base + i) * ebits)) & emask;
                        result |= ai << (i * 2 * ebits);
                        result |= bi << ((i * 2 + 1) * ebits);
                    }
                }
                2 | 6 => {
                    let step = if opcode == 2 { 0 } else { 1 };
                    for i in 0..(elems / 2) {
                        let ai = (a >> ((i * 2 + step) * ebits)) & emask;
                        let bi = (b >> ((i * 2 + step) * ebits)) & emask;
                        result |= ai << (i * 2 * ebits);
                        result |= bi << ((i * 2 + 1) * ebits);
                    }
                }
                _ => {
                    return self.unimpl("simd_permute");
                }
            }
            self.regs.v[rd] = result;
            return Ok(());
        }

        // Advanced SIMD three-different: 0 Q U 01110 size 1 Rm opcode 00 Rn Rd
        if (insn >> 24) & 0x1F == 0b01110 && (insn >> 21) & 1 == 1 && (insn >> 10) & 3 == 0 {
            let q = (insn >> 30) & 1;
            let u = (insn >> 29) & 1;
            let size = (insn >> 22) & 0x3;
            let rm = ((insn >> 16) & 0x1F) as usize;
            let opcode = (insn >> 12) & 0xF;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            let src_esize = 1usize << size;
            let dst_esize = src_esize * 2;
            let src_ebits = src_esize * 8;
            let dst_ebits = dst_esize * 8;
            let src_mask: u128 = (1u128 << src_ebits) - 1;
            let dst_mask: u128 = (1u128 << dst_ebits) - 1;
            let src_start = if q == 1 { 64 } else { 0 };
            let count = 64 / src_esize;
            let a = self.regs.v[rn];
            let b = self.regs.v[rm];
            let mut result: u128 = 0;
            let is_wide = opcode & 1 != 0 && opcode < 8;
            for i in 0..count {
                let ea = if is_wide {
                    a.wrapping_shr((i * dst_ebits) as u32) & dst_mask
                } else {
                    let shift = (src_start + i * src_ebits) as u32;
                    let raw = a.wrapping_shr(shift) & src_mask;
                    if u == 0 && src_ebits > 0 && raw.wrapping_shr((src_ebits - 1) as u32) != 0 {
                        raw | (dst_mask & !src_mask)
                    } else { raw }
                };
                let shift = (src_start + i * src_ebits) as u32;
                let eb_raw = b.wrapping_shr(shift) & src_mask;
                let eb = if u == 0 && src_ebits > 0 && eb_raw.wrapping_shr((src_ebits - 1) as u32) != 0 {
                    eb_raw | (dst_mask & !src_mask)
                } else { eb_raw };
                let er = match opcode >> 1 {
                    0 => ea.wrapping_add(eb) & dst_mask,
                    1 => ea.wrapping_sub(eb) & dst_mask,
                    2 => ea.wrapping_add(eb) & dst_mask,
                    3 => ea.wrapping_sub(eb) & dst_mask,
                    5 => ea.wrapping_mul(eb) & dst_mask,
                    _ => {
                        return self.unimpl("simd_three_diff");
                    }
                };
                result |= er.wrapping_shl((i * dst_ebits) as u32);
            }
            self.regs.v[rd] = result;
            return Ok(());
        }

        // Scalar ADDP Dd, Vn.2D: 01 01 1110 11 11000 11011 10 Rn Rd
        if insn & 0xFFFF_FC00 == 0x5EF1_B800 {
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            let lo = self.regs.v[rn] as u64;
            let hi = (self.regs.v[rn] >> 64) as u64;
            self.regs.v[rd] = lo.wrapping_add(hi) as u128;
            return Ok(());
        }

        // FMOV (immediate to scalar): sf 00 11110 type 1 imm8 100 00000 Rd
        if insn & 0x5F20_FC00 == 0x1E20_1000 {
            let ftype = (insn >> 22) & 0x3;
            let imm8 = ((insn >> 13) & 0xFF) as u8;
            let rd = (insn & 0x1F) as usize;
            if ftype == 0 {
                let sign = (imm8 >> 7) & 1;
                let exp = ((!(imm8 >> 6) & 1) << 7) | (if (imm8 >> 6) & 1 != 0 { 0x7C } else { 0 }) | ((imm8 >> 4) & 0x3);
                let frac = ((imm8 & 0xF) as u32) << 19;
                let bits = ((sign as u32) << 31) | ((exp as u32) << 23) | frac as u32;
                self.regs.v[rd] = bits as u128;
            } else if ftype == 1 {
                let sign = ((imm8 >> 7) & 1) as u64;
                let exp6 = (imm8 >> 6) & 1;
                let exp = ((((!exp6) & 1) as u64) << 10) | (if exp6 != 0 { 0x3FCu64 } else { 0u64 }) | (((imm8 >> 4) & 0x3) as u64);
                let frac = ((imm8 & 0xF) as u64) << 48;
                let bits = (sign << 63) | (exp << 52) | frac;
                self.regs.v[rd] = bits as u128;
            }
            return Ok(());
        }

        // CNT Vd.T, Vn.T (popcount bytes): 0 Q 00 1110 size 10000 00101 10 Rn Rd
        if insn & 0xBF3F_FC00 == 0x0E20_5800 {
            let q = (insn >> 30) & 1;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            let bytes = if q == 1 { 16usize } else { 8 };
            let a = self.regs.v[rn];
            let mut result: u128 = 0;
            for i in 0..bytes {
                let byte = ((a >> (i * 8)) & 0xFF) as u8;
                result |= (byte.count_ones() as u128) << (i * 8);
            }
            self.regs.v[rd] = result;
            return Ok(());
        }

        // Scalar FP 2-source: 0 0 0 11110 ftype 1 Rm 0pcode 10 Rn Rd
        if insn & 0xFF20_0C00 == 0x1E20_0800 {
            let ftype = (insn >> 22) & 0x3;
            let rm = ((insn >> 16) & 0x1F) as usize;
            let opcode = (insn >> 12) & 0xF;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            if ftype == 0 {
                let a = f32::from_bits(self.regs.v[rn] as u32);
                let b = f32::from_bits(self.regs.v[rm] as u32);
                let r = match opcode {
                    0 => a * b,     // FMUL
                    1 => a / b,     // FDIV
                    2 => a + b,     // FADD
                    3 => a - b,     // FSUB
                    4 => a.max(b),  // FMAX
                    5 => a.min(b),  // FMIN
                    _ => return self.unimpl("fp_2source opcode"),
                };
                self.regs.v[rd] = r.to_bits() as u128;
            } else {
                let a = f64::from_bits(self.regs.v[rn] as u64);
                let b = f64::from_bits(self.regs.v[rm] as u64);
                let r = match opcode {
                    0 => a * b,
                    1 => a / b,
                    2 => a + b,
                    3 => a - b,
                    4 => a.max(b),
                    5 => a.min(b),
                    _ => return self.unimpl("fp_2source opcode"),
                };
                self.regs.v[rd] = r.to_bits() as u128;
            }
            return Ok(());
        }

        // Scalar FP 1-source: 0 0 0 11110 ftype 1 0000 opcode 10000 Rn Rd
        if insn & 0xFF3E_0C00 == 0x1E20_0000 {
            let ftype = (insn >> 22) & 0x3;
            let opcode = (insn >> 15) & 0x3F;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            match opcode {
                0 => {
                    // FMOV same type
                    self.regs.v[rd] = self.regs.v[rn];
                }
                1 => {
                    // FABS
                    if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32).abs();
                        self.regs.v[rd] = f.to_bits() as u128;
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64).abs();
                        self.regs.v[rd] = f.to_bits() as u128;
                    }
                }
                2 => {
                    // FNEG
                    if ftype == 0 {
                        let f = -f32::from_bits(self.regs.v[rn] as u32);
                        self.regs.v[rd] = f.to_bits() as u128;
                    } else {
                        let f = -f64::from_bits(self.regs.v[rn] as u64);
                        self.regs.v[rd] = f.to_bits() as u128;
                    }
                }
                3 => {
                    // FSQRT
                    if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32).sqrt();
                        self.regs.v[rd] = f.to_bits() as u128;
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64).sqrt();
                        self.regs.v[rd] = f.to_bits() as u128;
                    }
                }
                4 => {
                    // FCVT single->double
                    let f = f32::from_bits(self.regs.v[rn] as u32) as f64;
                    self.regs.v[rd] = f.to_bits() as u128;
                }
                5 => {
                    // FCVT double->single
                    let f = f64::from_bits(self.regs.v[rn] as u64) as f32;
                    self.regs.v[rd] = f.to_bits() as u128;
                }
                _ => return self.unimpl("fp_1source opcode"),
            }
            return Ok(());
        }

        // Scalar FP compare: FCMP / FCMPE
        if insn & 0xFF20_FC07 == 0x1E20_2000 {
            let ftype = (insn >> 22) & 0x3;
            let rm = ((insn >> 16) & 0x1F) as usize;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let opc = (insn >> 3) & 0x3;
            let (n_val, z_val, c_val, v_val) = if ftype == 0 {
                let a = f32::from_bits(self.regs.v[rn] as u32);
                let b = if opc & 1 == 1 { 0.0f32 } else { f32::from_bits(self.regs.v[rm] as u32) };
                if a.is_nan() || b.is_nan() {
                    (false, false, true, true)
                } else if a == b {
                    (false, true, true, false)
                } else if a < b {
                    (true, false, false, false)
                } else {
                    (false, false, true, false)
                }
            } else {
                let a = f64::from_bits(self.regs.v[rn] as u64);
                let b = if opc & 1 == 1 { 0.0f64 } else { f64::from_bits(self.regs.v[rm] as u64) };
                if a.is_nan() || b.is_nan() {
                    (false, false, true, true)
                } else if a == b {
                    (false, true, true, false)
                } else if a < b {
                    (true, false, false, false)
                } else {
                    (false, false, true, false)
                }
            };
            let nzcv = ((n_val as u32) << 3) | ((z_val as u32) << 2) | ((c_val as u32) << 1) | (v_val as u32);
            self.regs.nzcv = nzcv << 28;
            return Ok(());
        }

        // FCSEL: 0 0 0 11110 ftype 1 Rm cond 11 Rn Rd
        if insn & 0xFF20_0C00 == 0x1E20_0C00 {
            let ftype = (insn >> 22) & 0x3;
            let rm = ((insn >> 16) & 0x1F) as usize;
            let cond = ((insn >> 12) & 0xF) as u8;
            let rn = ((insn >> 5) & 0x1F) as usize;
            let rd = (insn & 0x1F) as usize;
            let src = if self.cond(cond) { rn } else { rm };
            if ftype == 0 {
                self.regs.v[rd] = (self.regs.v[src] as u32) as u128;
            } else {
                self.regs.v[rd] = (self.regs.v[src] as u64) as u128;
            }
            return Ok(());
        }

        // Other SIMD/FP — fail fast
        self.unimpl("simd_fp_dp")
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

fn rd(mem: &mut AddressSpace, addr: Addr, sz: usize) -> HelmResult<u64> {
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
    let elem = if r == 0 {
        welem
    } else if esize >= 64 {
        welem.rotate_right(r)
    } else {
        ((welem >> r) | (welem << (esize as u32 - r))) & emask
    };
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

fn rd128(mem: &mut AddressSpace, addr: Addr) -> HelmResult<u128> {
    let mut b = [0u8; 16];
    mem.read(addr, &mut b)?;
    Ok(u128::from_le_bytes(b))
}
fn wr128(mem: &mut AddressSpace, addr: Addr, val: u128) -> HelmResult<()> {
    mem.write(addr, &val.to_le_bytes())
}
include!(concat!(env!("OUT_DIR"), "/decode_a64.rs"));
