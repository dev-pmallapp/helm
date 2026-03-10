#![allow(clippy::unusual_byte_groupings)]
//! AArch64 instruction executor for SE/FE mode.
//!
//! Fetch-decode-execute loop operating directly on `Aarch64Regs`
//! and `AddressSpace`.  No pipeline modelling — this is the FE path.

#![allow(clippy::unnecessary_cast, clippy::identity_op)]

use crate::arm::aarch64::hcr;
use crate::arm::aarch64::mem_bridge::ExecMem;
use crate::arm::aarch64::sysreg;
use crate::arm::regs::Aarch64Regs;
use helm_core::insn::InsnClass;
use helm_core::types::Addr;
use helm_core::{HelmError, HelmResult};
use helm_memory::mmu::{self, TranslationConfig, TranslationFault};
use helm_memory::tlb::Tlb;
use std::collections::HashSet;

/// Pluggable MMU debug hook — attach to an `Aarch64Cpu` to observe
/// translation faults, TLB flushes, and page table walks without
/// modifying the core execution code.
pub trait MmuDebugHook: Send {
    /// Called when a translation fault occurs (before the exception is taken).
    #[allow(unused_variables)]
    fn on_translation_fault(
        &mut self,
        va: u64,
        pa_walk: Option<u64>,
        fault: &TranslationFault,
        el: u8,
        is_write: bool,
        is_fetch: bool,
        insn_count: u64,
    ) {
    }

    /// Called on every TLBI instruction.
    #[allow(unused_variables)]
    fn on_tlbi(&mut self, va: Option<u64>, flush_all: bool, insn_count: u64) {}

    /// Called after a successful VA→PA translation (TLB hit or walk).
    #[allow(unused_variables)]
    fn on_translate(&mut self, va: u64, pa: u64, el: u8, is_write: bool, insn_count: u64) {}
}

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
    /// TLB for address translation (256 slow + 1024 fast entries).
    pub tlb: Tlb,
    /// SE mode: SVC returns HelmError::Syscall instead of taking an exception.
    se_mode: bool,
    /// Optional MMU debug hook for observing translation events.
    mmu_hook: Option<Box<dyn MmuDebugHook>>,
    /// Shared IRQ pending signal from the interrupt controller.
    irq_signal: Option<helm_core::IrqSignal>,
    /// WFI is pending — CPU is waiting for an interrupt.
    pub wfi_pending: bool,
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
            se_mode: false,
            mmu_hook: None,
            irq_signal: None,
            wfi_pending: false,
        }
    }

    /// Attach an IRQ signal for checking pending interrupts in `step()`.
    pub fn set_irq_signal(&mut self, signal: helm_core::IrqSignal) {
        self.irq_signal = Some(signal);
    }

    /// Attach an MMU debug hook for observing translations, faults, and TLBI.
    pub fn set_mmu_hook(&mut self, hook: Box<dyn MmuDebugHook>) {
        self.mmu_hook = Some(hook);
    }

    /// Flush all TLB entries.  Exposed for JIT helpers that handle TLBI
    /// instructions outside the normal interpretive path.
    pub fn flush_tlb_all(&mut self) {
        self.tlb.flush_all();
    }

    /// Flush TLB entries matching a specific virtual address.
    pub fn flush_tlb_va(&mut self, va: u64) {
        self.tlb.flush_va(va);
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

    /// Fast VA→PA for JIT helpers. Returns None on fault instead of Err.
    pub fn translate_va_jit(
        &mut self,
        va: u64,
        is_write: bool,
        is_fetch: bool,
        mem: &mut impl ExecMem,
    ) -> Option<u64> {
        self.translate_va(va, is_write, is_fetch, mem).ok()
    }

    /// Translate VA → PA using the MMU page tables (if enabled).
    /// Selects translation regime based on current exception level.
    fn translate_va(
        &mut self,
        va: u64,
        is_write: bool,
        is_fetch: bool,
        mem: &mut impl ExecMem,
    ) -> HelmResult<u64> {
        match self.regs.current_el {
            0 | 1 => self.translate_el01(va, is_write, is_fetch, mem),
            2 => self.translate_el2(va, is_write, is_fetch, mem),
            3 => self.translate_el3(va, is_write, is_fetch, mem),
            _ => Ok(va),
        }
    }

    /// EL0/EL1 translation: stage-1 via SCTLR_EL1 + optional stage-2 via HCR_EL2.VM.
    fn translate_el01(
        &mut self,
        va: u64,
        is_write: bool,
        is_fetch: bool,
        mem: &mut impl ExecMem,
    ) -> HelmResult<u64> {
        let stage1_enabled = self.regs.sctlr_el1 & 1 != 0;
        let stage2_enabled = self.regs.hcr_el2 & hcr::HCR_VM != 0;

        let ipa = if !stage1_enabled {
            va // MMU off → VA = IPA
        } else {
            self.walk_stage1_el1(va, is_write, is_fetch, mem)?
        };

        if stage2_enabled {
            self.walk_stage2(ipa, is_write, is_fetch, mem)
        } else {
            Ok(ipa)
        }
    }

    /// EL2 translation: stage-1 via SCTLR_EL2 (no stage-2 for EL2).
    fn translate_el2(
        &mut self,
        va: u64,
        is_write: bool,
        is_fetch: bool,
        mem: &mut impl ExecMem,
    ) -> HelmResult<u64> {
        if self.regs.sctlr_el2 & 1 == 0 {
            return Ok(va); // EL2 MMU off
        }

        let vhe = self.regs.hcr_el2 & hcr::HCR_E2H != 0;

        if vhe {
            // VHE mode: EL2 uses split VA space like EL1 (TTBR0_EL2/TTBR1_EL2)
            let tcr = TranslationConfig::parse(self.regs.tcr_el2);
            let ttbr0 = self.regs.ttbr0_el2;
            let ttbr1 = self.regs.ttbr1_el2;
            self.walk_and_cache(va, &tcr, ttbr0, ttbr1, is_write, is_fetch, mem)
        } else {
            // Non-VHE: single VA space from TTBR0_EL2, TCR_EL2 has only T0SZ
            let tcr = TranslationConfig::parse_single(self.regs.tcr_el2);
            let ttbr0 = self.regs.ttbr0_el2;
            self.walk_and_cache(va, &tcr, ttbr0, 0, is_write, is_fetch, mem)
        }
    }

    /// EL3 translation: stage-1 via SCTLR_EL3 (single VA space, no stage-2).
    fn translate_el3(
        &mut self,
        va: u64,
        is_write: bool,
        is_fetch: bool,
        mem: &mut impl ExecMem,
    ) -> HelmResult<u64> {
        if self.regs.sctlr_el3 & 1 == 0 {
            return Ok(va); // EL3 MMU off
        }
        let tcr = TranslationConfig::parse_single(self.regs.tcr_el3);
        let ttbr0 = self.regs.ttbr0_el3;
        self.walk_and_cache(va, &tcr, ttbr0, 0, is_write, is_fetch, mem)
    }

    /// EL0/EL1 stage-1 walk using SCTLR_EL1 translation tables.
    fn walk_stage1_el1(
        &mut self,
        va: u64,
        is_write: bool,
        is_fetch: bool,
        mem: &mut impl ExecMem,
    ) -> HelmResult<u64> {
        let tcr = TranslationConfig::parse(self.regs.tcr_el1);
        let ttbr0 = self.regs.ttbr0_el1;
        let ttbr1 = self.regs.ttbr1_el1;
        self.walk_and_cache(va, &tcr, ttbr0, ttbr1, is_write, is_fetch, mem)
    }

    /// Common page table walk + TLB caching for any stage-1 translation.
    fn walk_and_cache(
        &mut self,
        va: u64,
        tcr: &TranslationConfig,
        ttbr0: u64,
        ttbr1: u64,
        is_write: bool,
        is_fetch: bool,
        mem: &mut impl ExecMem,
    ) -> HelmResult<u64> {
        let asid = self.current_asid();

        // Fast TLB (O(1) direct-mapped, 4KB pages only)
        if let Some((pa, perms)) = self.tlb.lookup_fast(va, asid) {
            if !perms.check(self.regs.current_el, is_write, is_fetch) {
                return self.raise_translation_fault(
                    va,
                    is_write,
                    is_fetch,
                    TranslationFault::PermissionFault { level: 3 },
                );
            }
            return Ok(pa);
        }

        // Slow TLB (O(n) fully-associative, all page sizes)
        if let Some((pa, perms)) = self.tlb.lookup(va, asid) {
            if !perms.check(self.regs.current_el, is_write, is_fetch) {
                return self.raise_translation_fault(
                    va,
                    is_write,
                    is_fetch,
                    TranslationFault::PermissionFault { level: 3 },
                );
            }
            return Ok(pa);
        }

        // Both TLBs miss → page table walk
        let result = mmu::translate(va, tcr, ttbr0, ttbr1, &mut |pa| {
            let mut buf = [0u8; 8];
            mem.read_phys(pa, &mut buf).unwrap_or(());
            u64::from_le_bytes(buf)
        });

        match result {
            Ok((walk, _sel)) => {
                // Permission check
                if !walk.perms.check(self.regs.current_el, is_write, is_fetch) {
                    return self.raise_translation_fault(
                        va,
                        is_write,
                        is_fetch,
                        TranslationFault::PermissionFault { level: walk.level },
                    );
                }

                // Notify MMU debug hook on TLB-miss walks
                if let Some(ref mut hook) = self.mmu_hook {
                    hook.on_translate(va, walk.pa, self.regs.current_el, is_write, self.insn_count);
                }

                // Insert into slow TLB
                let global = !walk.ng;
                let entry = Tlb::make_entry(
                    va,
                    walk.pa,
                    walk.block_size,
                    walk.perms,
                    walk.attr_indx,
                    asid,
                    global,
                );
                self.tlb.insert(entry.clone());

                // Insert into fast TLB with addend (4KB pages only)
                let host_ptr = mem.host_ptr_for_pa(entry.pa_page);
                self.tlb.insert_fast(&entry, host_ptr);

                Ok(walk.pa)
            }
            Err(fault) => self.raise_translation_fault(va, is_write, is_fetch, fault),
        }
    }

    /// Stage-2 walk (IPA → PA) via VTTBR_EL2 + VTCR_EL2.
    fn walk_stage2(
        &mut self,
        ipa: u64,
        is_write: bool,
        is_fetch: bool,
        mem: &mut impl ExecMem,
    ) -> HelmResult<u64> {
        let s2cfg = mmu::Stage2Config::parse(self.regs.vtcr_el2);
        let vttbr = self.regs.vttbr_el2;

        let result = mmu::walk_stage2(ipa, vttbr, &s2cfg, &mut |pa| {
            let mut buf = [0u8; 8];
            mem.read_phys(pa, &mut buf).unwrap_or(());
            u64::from_le_bytes(buf)
        });

        match result {
            Ok(walk) => {
                // Stage-2 permission check
                if !walk.perms.check(self.regs.current_el, is_write, is_fetch) {
                    // Stage-2 fault: set HPFAR_EL2 and route to EL2
                    self.regs.hpfar_el2 = (ipa >> 12) << 4;
                    self.regs.far_el2 = ipa;
                    let ec = if is_fetch { 0x20 } else { 0x24 };
                    let fsc = TranslationFault::PermissionFault { level: walk.level }.to_fsc();
                    let wnr = if is_write && !is_fetch { 1u32 << 6 } else { 0 };
                    self.take_exception_to_el2(ec, fsc | wnr);
                    return Err(HelmError::Memory {
                        addr: ipa,
                        reason: "stage-2 permission fault".into(),
                    });
                }
                Ok(walk.pa)
            }
            Err(fault) => {
                // Stage-2 translation fault → route to EL2
                self.regs.hpfar_el2 = (ipa >> 12) << 4;
                self.regs.far_el2 = ipa;
                let ec = if is_fetch { 0x20 } else { 0x24 };
                let fsc = fault.to_fsc();
                let wnr = if is_write && !is_fetch { 1u32 << 6 } else { 0 };
                self.take_exception_to_el2(ec, fsc | wnr);
                Err(HelmError::Memory {
                    addr: ipa,
                    reason: format!("stage-2 fault: {:?}", fault),
                })
            }
        }
    }

    /// Get current ASID from TTBR (depends on TCR.A1).
    pub fn current_asid(&self) -> u16 {
        let tcr = self.regs.tcr_el1;
        let a1 = (tcr >> 22) & 1 != 0;
        let ttbr = if a1 {
            self.regs.ttbr1_el1
        } else {
            self.regs.ttbr0_el1
        };
        (ttbr >> 48) as u16
    }

    // ── VHE register redirection ────────────────────────────────────────

    /// Redirect EL1-named system registers to EL2 when VHE is active (E2H=1 at EL2).
    fn vhe_redirect(&self, id: u32) -> u32 {
        if self.regs.current_el != 2 || (self.regs.hcr_el2 & hcr::HCR_E2H == 0) {
            return id;
        }
        match id {
            sysreg::SCTLR_EL1 => sysreg::SCTLR_EL2,
            sysreg::CPACR_EL1 => sysreg::CPTR_EL2,
            sysreg::TTBR0_EL1 => sysreg::TTBR0_EL2,
            sysreg::TTBR1_EL1 => sysreg::TTBR1_EL2,
            sysreg::TCR_EL1 => sysreg::TCR_EL2,
            sysreg::ESR_EL1 => sysreg::ESR_EL2,
            sysreg::AFSR0_EL1 => sysreg::AFSR0_EL2,
            sysreg::AFSR1_EL1 => sysreg::AFSR1_EL2,
            sysreg::FAR_EL1 => sysreg::FAR_EL2,
            sysreg::MAIR_EL1 => sysreg::MAIR_EL2,
            sysreg::AMAIR_EL1 => sysreg::AMAIR_EL2,
            sysreg::VBAR_EL1 => sysreg::VBAR_EL2,
            sysreg::CONTEXTIDR_EL1 => sysreg::CONTEXTIDR_EL2,
            sysreg::CNTKCTL_EL1 => sysreg::CNTHCTL_EL2,
            sysreg::ELR_EL1 => sysreg::ELR_EL2,
            sysreg::SPSR_EL1 => sysreg::SPSR_EL2,
            _ => id,
        }
    }

    // ── HCR_EL2.TVM trap check ─────────────────────────────────────────

    /// Check if a sysreg write at EL1 should be trapped to EL2 (TVM).
    fn check_tvm_trap(&self, id: u32) -> bool {
        if self.regs.hcr_el2 & hcr::HCR_TVM == 0 {
            return false;
        }
        matches!(
            id,
            sysreg::SCTLR_EL1
                | sysreg::TTBR0_EL1
                | sysreg::TTBR1_EL1
                | sysreg::TCR_EL1
                | sysreg::ESR_EL1
                | sysreg::FAR_EL1
                | sysreg::AFSR0_EL1
                | sysreg::AFSR1_EL1
                | sysreg::MAIR_EL1
                | sysreg::AMAIR_EL1
                | sysreg::CONTEXTIDR_EL1
        )
    }

    /// Encode ISS for MSR/MRS trap (EC=0x18).
    fn msr_trap_iss(l: u32, op0: u32, op1: u32, crn: u32, crm: u32, op2: u32, rt: u16) -> u32 {
        // ISS encoding for MSR/MRS trap:
        // [24] = Direction (0=MSR, 1=MRS)
        // [21:20] = Op0
        // [19:17] = Op2
        // [16:14] = Op1
        // [13:10] = CRn
        // [4:1] = CRm
        // [9:5] = Rt
        (l << 24)
            | (op0 << 20)
            | (op2 << 17)
            | (op1 << 14)
            | (crn << 10)
            | ((rt as u32) << 5)
            | (crm << 1)
    }

    /// Raise a translation fault → data abort or instruction abort exception.
    fn raise_translation_fault(
        &mut self,
        va: u64,
        is_write: bool,
        is_fetch: bool,
        fault: TranslationFault,
    ) -> HelmResult<u64> {
        // EC: 0x20/0x21 = instruction abort (lower/current EL)
        //     0x24/0x25 = data abort (lower/current EL)
        let ec = if is_fetch {
            if self.regs.current_el == 0 {
                0x20
            } else {
                0x21
            }
        } else {
            if self.regs.current_el == 0 {
                0x24
            } else {
                0x25
            }
        };
        let fsc = fault.to_fsc();
        let wnr = if is_write && !is_fetch { 1u32 << 6 } else { 0 };
        let iss = fsc | wnr;

        // Notify MMU debug hook
        if let Some(ref mut hook) = self.mmu_hook {
            hook.on_translation_fault(
                va,
                None,
                &fault,
                self.regs.current_el,
                is_write,
                is_fetch,
                self.insn_count,
            );
        }

        // Route to correct EL and set FAR at target EL
        let target = self.route_sync_exception(ec);
        match target {
            2 => self.regs.far_el2 = va,
            3 => self.regs.far_el3 = va,
            _ => self.regs.far_el1 = va,
        }
        log::debug!(
            "translation fault: EC={ec:#x} ISS={iss:#x} VA={va:#x} → EL{target} {:?} insn#{}",
            fault,
            self.insn_count,
        );
        self.take_exception(target, ec, iss);
        Err(HelmError::Memory {
            addr: va,
            reason: format!("translation fault: {:?}", fault),
        })
    }

    // ── step + traced memory access ─────────────────────────────────────

    pub fn step(&mut self, mem: &mut impl ExecMem) -> HelmResult<StepTrace> {
        // Check for pending IRQs before executing the next instruction
        if self.check_irq() {
            self.wfi_pending = false;
            self.pc_written = true;
            // Return a minimal trace for the IRQ exception entry
            self.trace.pc = self.regs.pc;
            self.trace.insn_word = 0;
            self.trace.class = InsnClass::Nop;
            self.trace.mem_accesses.clear();
            self.trace.branch_taken = None;
            return Ok(std::mem::take(&mut self.trace));
        }

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
        mem.read_bytes(pc, &mut buf)?;
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

        match self.exec(va, insn, mem) {
            Ok(()) => {}
            Err(HelmError::Pipeline(_)) => {
                // Data abort taken — exception handler already set PC.
                // Don't propagate the error, just return the trace.
                return Ok(std::mem::take(&mut self.trace));
            }
            Err(e) => return Err(e),
        }
        if !self.pc_written {
            self.regs.pc += 4;
        }

        // Record branch outcome for branch instructions
        if matches!(self.trace.class, InsnClass::Branch | InsnClass::CondBranch) {
            self.trace.branch_taken = Some(self.pc_written);
        }

        Ok(std::mem::take(&mut self.trace))
    }

    /// Fast step — no trace allocation. Returns Ok(()) on success.
    /// Used by FsSession in FE-timing mode where trace data is not needed.
    pub fn step_fast(&mut self, mem: &mut impl ExecMem) -> HelmResult<()> {
        if self.check_irq() {
            self.wfi_pending = false;
            return Ok(());
        }

        let va = self.regs.pc;
        let pc = match self.translate_va(va, false, true, mem) {
            Ok(pa) => pa,
            Err(_) => return Ok(()), // exception taken
        };

        let mut buf = [0u8; 4];
        mem.read_bytes(pc, &mut buf)?;
        let insn = u32::from_le_bytes(buf);
        self.cur_insn = insn;
        self.pc_written = false;
        self.insn_count += 1;

        // Skip trace setup — just execute
        self.trace.mem_accesses.clear();

        match self.exec(va, insn, mem) {
            Ok(()) => {}
            Err(HelmError::Pipeline(_)) => return Ok(()),
            Err(e) => return Err(e),
        }
        if !self.pc_written {
            self.regs.pc += 4;
        }
        Ok(())
    }

    // -- Traced memory access wrappers (with VA→PA translation) --

    /// Sentinel error for data aborts — aborts the current instruction.
    fn data_abort_err() -> HelmError {
        HelmError::Pipeline("data abort".into())
    }

    fn trace_rd(&mut self, mem: &mut impl ExecMem, va: Addr, sz: usize) -> HelmResult<u64> {
        let pa = match self.translate_va(va, false, false, mem) {
            Ok(pa) => pa,
            Err(_) => return Err(Self::data_abort_err()),
        };
        self.trace.mem_accesses.push(MemAccess {
            addr: pa,
            size: sz,
            is_write: false,
        });
        rd(mem, pa, sz)
    }

    fn trace_wr(
        &mut self,
        mem: &mut impl ExecMem,
        va: Addr,
        val: u64,
        sz: usize,
    ) -> HelmResult<()> {
        let pa = match self.translate_va(va, true, false, mem) {
            Ok(pa) => pa,
            Err(_) => return Err(Self::data_abort_err()),
        };
        self.trace.mem_accesses.push(MemAccess {
            addr: pa,
            size: sz,
            is_write: true,
        });
        wr(mem, pa, val, sz)
    }

    fn trace_rd128(&mut self, mem: &mut impl ExecMem, va: Addr) -> HelmResult<u128> {
        let pa = match self.translate_va(va, false, false, mem) {
            Ok(pa) => pa,
            Err(_) => return Err(Self::data_abort_err()),
        };
        self.trace.mem_accesses.push(MemAccess {
            addr: pa,
            size: 16,
            is_write: false,
        });
        rd128(mem, pa)
    }

    fn trace_wr128(&mut self, mem: &mut impl ExecMem, va: Addr, val: u128) -> HelmResult<()> {
        let pa = match self.translate_va(va, true, false, mem) {
            Ok(pa) => pa,
            Err(_) => return Err(Self::data_abort_err()),
        };
        self.trace.mem_accesses.push(MemAccess {
            addr: pa,
            size: 16,
            is_write: true,
        });
        wr128(mem, pa, val)
    }

    /// Return an error for unimplemented instructions (no panic).
    fn unimpl(&mut self, ctx: &str) -> HelmResult<()> {
        let pc = self.regs.pc;
        let insn = self.cur_insn;

        // In FS mode, take an Undefined Instruction exception so the
        // kernel's undef handler can deal with it (or at least report
        // the faulting instruction accurately).  In SE mode there is no
        // exception vector, so propagate the error to the runner.
        if !self.se_mode {
            log::warn!("UNDEF exception: {ctx} at PC={pc:#x} insn={insn:#010x}",);
            let target = self.route_sync_exception(0x00);
            self.take_exception(target, 0x00, 0);
            return Err(HelmError::Pipeline("undefined instruction".into()));
        }

        Err(HelmError::Isa(format!(
            "unimplemented {ctx} at PC={pc:#x}: insn={insn:#010x} ({insn:032b})"
        )))
    }

    fn exec(&mut self, pc: Addr, insn: u32, mem: &mut impl ExecMem) -> HelmResult<()> {
        let op0 = (insn >> 25) & 0xF;
        match op0 {
            0b1000 | 0b1001 => {
                self.trace.class = InsnClass::IntAlu;
                self.exec_dp_imm(pc, insn)
            }
            0b1010 | 0b1011 => self.exec_branch_sys(pc, insn, mem),
            0b0100 | 0b0110 | 0b1100 | 0b1110 => self.exec_ldst(insn, mem),
            0b0101 | 0b1101 => {
                self.trace.class = InsnClass::IntAlu;
                self.exec_dp_reg(insn)
            }
            0b0111 | 0b1111 => {
                self.trace.class = InsnClass::SimdAlu;
                self.exec_simd_dp(insn)
            }
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
                // For 32-bit operations, use only the lower 32 bits of the source
                let src = if sf == 1 {
                    self.xn(rn)
                } else {
                    self.wn(rn) as u64
                };
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
    fn exec_branch_sys(&mut self, pc: Addr, insn: u32, mem: &mut impl ExecMem) -> HelmResult<()> {
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
            // FS mode from EL0: route to kernel via exception
            if !self.se_mode && self.regs.current_el == 0 {
                let target = self.route_sync_exception(0x15);
                self.take_exception(target, 0x15, imm16);
                return Ok(());
            }
            // SE mode or SVC from EL1+: signal to engine for handling
            return Err(HelmError::Syscall {
                number: self.xn(8),
                reason: "SVC".into(),
            });
        }
        // HVC — Hypervisor Call
        if insn & 0xFFE0_001F == 0xD400_0002 {
            let imm16 = (insn >> 5) & 0xFFFF;
            // HVC is UNDEFINED at EL0 and EL2 (and when HCR_EL2.HCD=1 from EL1)
            if self.regs.current_el == 0 || self.regs.current_el == 2 {
                self.take_exception_to_el1(0x00, 0);
                return Ok(());
            }
            if self.regs.current_el == 1 && (self.regs.hcr_el2 & hcr::HCR_HCD != 0) {
                self.take_exception_to_el1(0x00, 0);
                return Ok(());
            }
            // PSCI interception: handle PSCI calls directly instead of trapping to EL2.
            // This avoids needing a full EL2 firmware implementation.
            if self.handle_psci_call() {
                return Ok(());
            }
            // EC=0x16 = HVC from AArch64, always routes to EL2
            self.take_exception_to_el2(0x16, imm16);
            return Ok(());
        }
        // SMC — Secure Monitor Call
        if insn & 0xFFE0_001F == 0xD400_0003 {
            let imm16 = (insn >> 5) & 0xFFFF;
            // SMC is UNDEFINED at EL0
            if self.regs.current_el == 0 {
                self.take_exception_to_el1(0x00, 0);
                return Ok(());
            }
            // SCR_EL3.SMD=1: SMC is UNDEFINED
            if self.regs.scr_el3 & hcr::SCR_SMD != 0 {
                let target = if self.regs.current_el == 1 {
                    1
                } else {
                    self.regs.current_el
                };
                self.take_exception(target, 0x00, 0);
                return Ok(());
            }
            // HCR_EL2.TSC=1 and from EL1: trap SMC to EL2
            if self.regs.current_el == 1 && (self.regs.hcr_el2 & hcr::HCR_TSC != 0) {
                self.take_exception_to_el2(0x17, imm16);
                return Ok(());
            }
            // Otherwise route to EL3
            self.take_exception_to_el3(0x17, imm16);
            return Ok(());
        }
        // BRK — software breakpoint exception (EC=0x3C)
        if insn & 0xFFE0_001F == 0xD420_0000 {
            let imm16 = (insn >> 5) & 0xFFFF;
            // In SE mode, BRK is fatal (musl a_crash). In FS mode, route to handler.
            if self.se_mode {
                return Err(HelmError::Decode {
                    addr: pc,
                    reason: format!("BRK #{imm16} (breakpoint/assertion failure)"),
                });
            }
            // EC=0x3C (BRK from AArch64), ISS=imm16
            self.regs.pc = pc + 4; // ELR points past the BRK
            let target = self.route_sync_exception(0x3C);
            self.take_exception(target, 0x3C, imm16);
            return Ok(());
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
    fn exec_system(&mut self, insn: u32, mem: &mut impl ExecMem) -> HelmResult<()> {
        let l = (insn >> 21) & 1; // 0=MSR/SYS, 1=MRS/SYSL
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

        // Hints: op0=0, L=0, CRn=0010 — NOP/YIELD/WFE/WFI/SEV/PAC*
        if op0 == 0 && l == 0 && crn == 2 {
            // WFI: CRm=0, op2=3
            if crm == 0 && op2 == 3 {
                self.wfi_pending = true;
            }
            return Ok(());
        }

        // CLREX: op0=0, L=0, CRn=0011, op2=010 — already caught above
        // PSTATE flag manipulation: op0=0, L=0, CRn=0100 — already caught above

        // SYS/SYSL: op0=1 — cache/TLB maintenance, AT, DC, IC, TLBI
        if op0 == 1 {
            // DC ZVA: op1=3, CRn=7, CRm=4, op2=1, L=0
            // Zeroes a cache-line-sized block. Uses VIRTUAL address → must translate.
            if l == 0 && op1 == 3 && crn == 7 && crm == 4 && op2 == 1 {
                let va = self.xn(rt);
                let bs = (self.regs.dczid_el0 & 0xF) as u64;
                let block_size = 4u64 << bs;
                let aligned_va = va & !(block_size - 1);
                // Translate VA → PA (DC ZVA is a data write)
                let pa = match self.translate_va(aligned_va, true, false, mem) {
                    Ok(pa) => pa,
                    Err(_) => return Err(Self::data_abort_err()),
                };
                let zeros = vec![0u8; block_size as usize];
                mem.write_bytes(pa, &zeros)?;
                return Ok(());
            }
            // TLBI: CRn=8
            if crn == 8 {
                return self.exec_tlbi(op1, crm, op2, rt);
            }
            // AT: CRn=7, CRm=8
            if crn == 7 && crm == 8 {
                return self.exec_at(op1, op2, rt, mem);
            }
            // Other DC, IC, etc. — NOP in simulation
            return Ok(());
        }

        // MRS/MSR (register): op0 ∈ {2,3}
        // Encode the full sysreg ID
        let raw_id = (op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2;

        // VHE register redirection: at EL2 with E2H=1, EL1-named regs → EL2
        let sysreg_id = self.vhe_redirect(raw_id);

        // HCR_EL2.TVM trap: writes to VM control regs at EL1 → trap to EL2
        if l == 0 && self.regs.current_el == 1 {
            if self.check_tvm_trap(sysreg_id) {
                // EC=0x18 (MSR/MRS/System instruction trap), ISS encodes the sysreg
                let iss = Self::msr_trap_iss(l, op0, op1, crn, crm, op2, rt);
                self.take_exception_to_el2(0x18, iss);
                return Ok(());
            }
        }

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
            (3, 6) => {
                // DAIFSet — set (mask) interrupt bits
                self.regs.daif |= (imm << 6) as u32;
            }
            (3, 7) => {
                // DAIFClr — clear (unmask) interrupt bits
                self.regs.daif &= !((imm << 6) as u32);
            }
            (0, 5) => {
                // SPSel
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
            sysreg::SCTLR_EL1 => self.regs.sctlr_el1,
            sysreg::ACTLR_EL1 => self.regs.actlr_el1,
            sysreg::CPACR_EL1 => self.regs.cpacr_el1,
            // Translation
            sysreg::TTBR0_EL1 => self.regs.ttbr0_el1,
            sysreg::TTBR1_EL1 => self.regs.ttbr1_el1,
            sysreg::TCR_EL1 => self.regs.tcr_el1,
            // Fault
            sysreg::ESR_EL1 => self.regs.esr_el1 as u64,
            sysreg::AFSR0_EL1 => self.regs.afsr0_el1,
            sysreg::AFSR1_EL1 => self.regs.afsr1_el1,
            sysreg::FAR_EL1 => self.regs.far_el1,
            sysreg::PAR_EL1 => self.regs.par_el1,
            // Memory attributes
            sysreg::MAIR_EL1 => self.regs.mair_el1,
            sysreg::AMAIR_EL1 => self.regs.amair_el1,
            // Vector / exception
            sysreg::VBAR_EL1 => self.regs.vbar_el1,
            sysreg::CONTEXTIDR_EL1 => self.regs.contextidr_el1,
            // Thread ID
            sysreg::TPIDR_EL0 => self.regs.tpidr_el0,
            sysreg::TPIDR_EL1 => self.regs.tpidr_el1,
            sysreg::TPIDRRO_EL0 => 0, // read-only thread pointer (not set)
            // SP / exception state
            sysreg::SP_EL0 => self.regs.sp,
            sysreg::SP_EL1 => self.regs.sp_el1,
            sysreg::ELR_EL1 => self.regs.elr_el1,
            sysreg::SPSR_EL1 => self.regs.spsr_el1 as u64,
            sysreg::CURRENT_EL => (self.regs.current_el as u64) << 2,
            sysreg::DAIF => self.regs.daif as u64,
            sysreg::NZCV => self.regs.nzcv as u64,
            sysreg::SPSEL => self.regs.sp_sel as u64,
            // Debug
            sysreg::MDSCR_EL1 => self.regs.mdscr_el1 as u64,
            sysreg::MDCCSR_EL0 => 0,
            // Cache
            sysreg::CSSELR_EL1 => self.regs.csselr_el1,
            sysreg::CCSIDR_EL1 => 0x700F_E01A, // 32KB 4-way (dummy)
            sysreg::CLIDR_EL1 => 0x0A20_0023,  // L1 I+D, L2 unified
            // Timer
            sysreg::CNTFRQ_EL0 => self.regs.cntfrq_el0,
            sysreg::CNTVCT_EL0 => self.insn_count, // approximate timer
            sysreg::CNTV_CTL_EL0 => {
                let mut ctl = self.regs.cntv_ctl_el0;
                // ISTATUS (bit 2): set when timer condition met
                if ctl & 1 != 0 && self.insn_count >= self.regs.cntv_cval_el0 {
                    ctl |= 1 << 2;
                } else {
                    ctl &= !(1 << 2);
                }
                ctl
            }
            sysreg::CNTV_CVAL_EL0 => self.regs.cntv_cval_el0,
            sysreg::CNTP_CTL_EL0 => {
                let mut ctl = self.regs.cntp_ctl_el0;
                if ctl & 1 != 0 && self.insn_count >= self.regs.cntp_cval_el0 {
                    ctl |= 1 << 2;
                } else {
                    ctl &= !(1 << 2);
                }
                ctl
            }
            sysreg::CNTP_CVAL_EL0 => self.regs.cntp_cval_el0,
            sysreg::CNTP_TVAL_EL0 => {
                self.regs.cntp_cval_el0.wrapping_sub(self.insn_count) as i32 as i64 as u64
            }
            sysreg::CNTV_TVAL_EL0 => {
                self.regs.cntv_cval_el0.wrapping_sub(self.insn_count) as i32 as i64 as u64
            }
            sysreg::CNTKCTL_EL1 => self.regs.cntkctl_el1,
            // Counter / cache type (read-only)
            sysreg::CTR_EL0 => self.regs.ctr_el0,
            sysreg::DCZID_EL0 => self.regs.dczid_el0,
            // FP
            sysreg::FPCR => self.regs.fpcr as u64,
            sysreg::FPSR => self.regs.fpsr as u64,
            // ID registers (read-only)
            sysreg::MIDR_EL1 => self.regs.midr_el1,
            sysreg::MPIDR_EL1 => self.regs.mpidr_el1,
            sysreg::REVIDR_EL1 => self.regs.revidr_el1,
            sysreg::ID_AA64PFR0_EL1 => self.regs.id_aa64pfr0_el1,
            sysreg::ID_AA64PFR1_EL1 => self.regs.id_aa64pfr1_el1,
            sysreg::ID_AA64MMFR0_EL1 => self.regs.id_aa64mmfr0_el1,
            sysreg::ID_AA64MMFR1_EL1 => self.regs.id_aa64mmfr1_el1,
            sysreg::ID_AA64MMFR2_EL1 => self.regs.id_aa64mmfr2_el1,
            sysreg::ID_AA64ISAR0_EL1 => self.regs.id_aa64isar0_el1,
            sysreg::ID_AA64ISAR1_EL1 => self.regs.id_aa64isar1_el1,
            sysreg::ID_AA64ISAR2_EL1 => self.regs.id_aa64isar2_el1,
            sysreg::ID_AA64DFR0_EL1 => self.regs.id_aa64dfr0_el1,
            sysreg::ID_AA64DFR1_EL1 => 0,
            sysreg::ID_AA64AFR0_EL1 => 0,
            sysreg::ID_AA64AFR1_EL1 => 0,
            // Legacy AArch32 ID regs (read as zero)
            sysreg::ID_PFR0_EL1
            | sysreg::ID_PFR1_EL1
            | sysreg::ID_PFR2_EL1
            | sysreg::ID_DFR0_EL1
            | sysreg::ID_AFR0_EL1
            | sysreg::ID_MMFR0_EL1
            | sysreg::ID_MMFR1_EL1
            | sysreg::ID_MMFR2_EL1
            | sysreg::ID_MMFR3_EL1
            | sysreg::ID_MMFR4_EL1
            | sysreg::ID_ISAR0_EL1
            | sysreg::ID_ISAR1_EL1
            | sysreg::ID_ISAR2_EL1
            | sysreg::ID_ISAR3_EL1
            | sysreg::ID_ISAR4_EL1
            | sysreg::ID_ISAR5_EL1
            | sysreg::ID_ISAR6_EL1 => 0,
            // EL2 — control
            sysreg::HCR_EL2 => self.regs.hcr_el2,
            sysreg::SCTLR_EL2 => self.regs.sctlr_el2,
            sysreg::ACTLR_EL2 => self.regs.actlr_el2,
            sysreg::CPTR_EL2 => self.regs.cptr_el2,
            sysreg::HACR_EL2 => self.regs.hacr_el2,
            sysreg::MDCR_EL2 => self.regs.mdcr_el2,
            // EL2 — translation
            sysreg::TCR_EL2 => self.regs.tcr_el2,
            sysreg::TTBR0_EL2 => self.regs.ttbr0_el2,
            sysreg::TTBR1_EL2 => self.regs.ttbr1_el2,
            sysreg::VTTBR_EL2 => self.regs.vttbr_el2,
            sysreg::VTCR_EL2 => self.regs.vtcr_el2,
            sysreg::MAIR_EL2 => self.regs.mair_el2,
            sysreg::AMAIR_EL2 => self.regs.amair_el2,
            // EL2 — fault
            sysreg::ESR_EL2 => self.regs.esr_el2 as u64,
            sysreg::FAR_EL2 => self.regs.far_el2,
            sysreg::HPFAR_EL2 => self.regs.hpfar_el2,
            sysreg::AFSR0_EL2 => self.regs.afsr0_el2,
            sysreg::AFSR1_EL2 => self.regs.afsr1_el2,
            // EL2 — exception state
            sysreg::VBAR_EL2 => self.regs.vbar_el2,
            sysreg::ELR_EL2 => self.regs.elr_el2,
            sysreg::SPSR_EL2 => self.regs.spsr_el2 as u64,
            sysreg::SP_EL2 => self.regs.sp_el2,
            // EL2 — virtualized ID
            sysreg::VMPIDR_EL2 => self.regs.vmpidr_el2,
            sysreg::VPIDR_EL2 => self.regs.vpidr_el2,
            // EL2 — context / thread
            sysreg::CONTEXTIDR_EL2 => self.regs.contextidr_el2,
            sysreg::TPIDR_EL2 => self.regs.tpidr_el2,
            // EL2 — timers
            sysreg::CNTHCTL_EL2 => self.regs.cnthctl_el2,
            sysreg::CNTHP_CTL_EL2 => self.regs.cnthp_ctl_el2,
            sysreg::CNTHP_CVAL_EL2 => self.regs.cnthp_cval_el2,
            sysreg::CNTHP_TVAL_EL2 => 0,
            sysreg::CNTVOFF_EL2 => self.regs.cntvoff_el2,
            // EL3 — control
            sysreg::SCR_EL3 => self.regs.scr_el3,
            sysreg::SCTLR_EL3 => self.regs.sctlr_el3,
            sysreg::ACTLR_EL3 => self.regs.actlr_el3,
            sysreg::CPTR_EL3 => self.regs.cptr_el3,
            sysreg::MDCR_EL3 => self.regs.mdcr_el3,
            // EL3 — translation
            sysreg::TCR_EL3 => self.regs.tcr_el3,
            sysreg::TTBR0_EL3 => self.regs.ttbr0_el3,
            sysreg::MAIR_EL3 => self.regs.mair_el3,
            sysreg::AMAIR_EL3 => self.regs.amair_el3,
            // EL3 — fault
            sysreg::ESR_EL3 => self.regs.esr_el3 as u64,
            sysreg::FAR_EL3 => self.regs.far_el3,
            sysreg::AFSR0_EL3 => self.regs.afsr0_el3,
            sysreg::AFSR1_EL3 => self.regs.afsr1_el3,
            // EL3 — exception state
            sysreg::VBAR_EL3 => self.regs.vbar_el3,
            sysreg::ELR_EL3 => self.regs.elr_el3,
            sysreg::SPSR_EL3 => self.regs.spsr_el3 as u64,
            sysreg::SP_EL3 => self.regs.sp_el3,
            // EL3 — thread
            sysreg::TPIDR_EL3 => self.regs.tpidr_el3,
            // Performance monitors — stub
            sysreg::PMCR_EL0
            | sysreg::PMCNTENSET_EL0
            | sysreg::PMCNTENCLR_EL0
            | sysreg::PMOVSCLR_EL0
            | sysreg::PMUSERENR_EL0
            | sysreg::PMCCNTR_EL0
            | sysreg::PMCCFILTR_EL0
            | sysreg::PMSELR_EL0
            | sysreg::PMXEVTYPER_EL0
            | sysreg::PMXEVCNTR_EL0 => 0,
            // OS lock
            sysreg::OSLSR_EL1 => 0,
            sysreg::OSDLR_EL1 => 0,
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
            sysreg::SCTLR_EL1 => {
                let was_enabled = self.regs.sctlr_el1 & 1 != 0;
                self.regs.sctlr_el1 = val;
                if val & 1 != 0 && !was_enabled {
                    self.tlb.flush_all();
                }
            }
            sysreg::ACTLR_EL1 => self.regs.actlr_el1 = val,
            sysreg::CPACR_EL1 => self.regs.cpacr_el1 = val,
            // Translation — flush TLB on table base / config changes
            sysreg::TTBR0_EL1 => {
                self.regs.ttbr0_el1 = val;
                self.tlb.flush_all();
            }
            sysreg::TTBR1_EL1 => {
                self.regs.ttbr1_el1 = val;
                self.tlb.flush_all();
            }
            sysreg::TCR_EL1 => {
                self.regs.tcr_el1 = val;
                self.tlb.flush_all();
            }
            // Fault
            sysreg::ESR_EL1 => self.regs.esr_el1 = val as u32,
            sysreg::AFSR0_EL1 => self.regs.afsr0_el1 = val,
            sysreg::AFSR1_EL1 => self.regs.afsr1_el1 = val,
            sysreg::FAR_EL1 => self.regs.far_el1 = val,
            sysreg::PAR_EL1 => self.regs.par_el1 = val,
            // Memory attributes
            sysreg::MAIR_EL1 => self.regs.mair_el1 = val,
            sysreg::AMAIR_EL1 => self.regs.amair_el1 = val,
            // Vector / exception
            sysreg::VBAR_EL1 => {
                self.regs.vbar_el1 = val;
                log::info!("MSR VBAR_EL1 = {val:#x} at insn #{}", self.insn_count);
            }
            sysreg::CONTEXTIDR_EL1 => self.regs.contextidr_el1 = val,
            // Thread ID
            sysreg::TPIDR_EL0 => self.regs.tpidr_el0 = val,
            sysreg::TPIDR_EL1 => self.regs.tpidr_el1 = val,
            sysreg::TPIDRRO_EL0 => {} // read-only, ignore
            // SP / exception state
            sysreg::SP_EL0 => self.regs.sp = val,
            sysreg::SP_EL1 => self.regs.sp_el1 = val,
            sysreg::ELR_EL1 => self.regs.elr_el1 = val,
            sysreg::SPSR_EL1 => self.regs.spsr_el1 = val as u32,
            sysreg::DAIF => self.regs.daif = val as u32 & 0x3C0,
            sysreg::NZCV => self.regs.nzcv = val as u32 & 0xF000_0000,
            sysreg::SPSEL => self.regs.sp_sel = (val & 1) as u8,
            // Debug
            sysreg::MDSCR_EL1 => self.regs.mdscr_el1 = val as u32,
            // Cache
            sysreg::CSSELR_EL1 => self.regs.csselr_el1 = val,
            // Timer
            sysreg::CNTFRQ_EL0 => self.regs.cntfrq_el0 = val,
            sysreg::CNTV_CTL_EL0 => self.regs.cntv_ctl_el0 = val,
            sysreg::CNTV_CVAL_EL0 => self.regs.cntv_cval_el0 = val,
            sysreg::CNTP_CTL_EL0 => self.regs.cntp_ctl_el0 = val,
            sysreg::CNTP_CVAL_EL0 => self.regs.cntp_cval_el0 = val,
            sysreg::CNTP_TVAL_EL0 => {
                // TVAL write sets CVAL = CNTVCT + sign_extend(TVAL, 64)
                self.regs.cntp_cval_el0 = self.insn_count.wrapping_add(val as i32 as i64 as u64);
            }
            sysreg::CNTV_TVAL_EL0 => {
                self.regs.cntv_cval_el0 = self.insn_count.wrapping_add(val as i32 as i64 as u64);
            }
            sysreg::CNTKCTL_EL1 => self.regs.cntkctl_el1 = val,
            // FP
            sysreg::FPCR => self.regs.fpcr = val as u32,
            sysreg::FPSR => self.regs.fpsr = val as u32,
            // EL2 — control
            sysreg::HCR_EL2 => self.regs.hcr_el2 = val,
            sysreg::SCTLR_EL2 => {
                self.regs.sctlr_el2 = val;
                self.tlb.flush_all();
            }
            sysreg::ACTLR_EL2 => self.regs.actlr_el2 = val,
            sysreg::CPTR_EL2 => self.regs.cptr_el2 = val,
            sysreg::HACR_EL2 => self.regs.hacr_el2 = val,
            sysreg::MDCR_EL2 => self.regs.mdcr_el2 = val,
            // EL2 — translation (flush TLB on table/config changes)
            sysreg::TCR_EL2 => {
                self.regs.tcr_el2 = val;
                self.tlb.flush_all();
            }
            sysreg::TTBR0_EL2 => {
                self.regs.ttbr0_el2 = val;
                self.tlb.flush_all();
            }
            sysreg::TTBR1_EL2 => {
                self.regs.ttbr1_el2 = val;
                self.tlb.flush_all();
            }
            sysreg::VTTBR_EL2 => {
                self.regs.vttbr_el2 = val;
                self.tlb.flush_all();
            }
            sysreg::VTCR_EL2 => {
                self.regs.vtcr_el2 = val;
                self.tlb.flush_all();
            }
            sysreg::MAIR_EL2 => self.regs.mair_el2 = val,
            sysreg::AMAIR_EL2 => self.regs.amair_el2 = val,
            // EL2 — fault
            sysreg::ESR_EL2 => self.regs.esr_el2 = val as u32,
            sysreg::FAR_EL2 => self.regs.far_el2 = val,
            sysreg::HPFAR_EL2 => self.regs.hpfar_el2 = val,
            sysreg::AFSR0_EL2 => self.regs.afsr0_el2 = val,
            sysreg::AFSR1_EL2 => self.regs.afsr1_el2 = val,
            // EL2 — exception state
            sysreg::VBAR_EL2 => self.regs.vbar_el2 = val,
            sysreg::ELR_EL2 => self.regs.elr_el2 = val,
            sysreg::SPSR_EL2 => self.regs.spsr_el2 = val as u32,
            sysreg::SP_EL2 => self.regs.sp_el2 = val,
            // EL2 — virtualized ID
            sysreg::VMPIDR_EL2 => self.regs.vmpidr_el2 = val,
            sysreg::VPIDR_EL2 => self.regs.vpidr_el2 = val,
            // EL2 — context / thread
            sysreg::CONTEXTIDR_EL2 => self.regs.contextidr_el2 = val,
            sysreg::TPIDR_EL2 => self.regs.tpidr_el2 = val,
            // EL2 — timers
            sysreg::CNTHCTL_EL2 => self.regs.cnthctl_el2 = val,
            sysreg::CNTHP_CTL_EL2 => self.regs.cnthp_ctl_el2 = val,
            sysreg::CNTHP_CVAL_EL2 => self.regs.cnthp_cval_el2 = val,
            sysreg::CNTHP_TVAL_EL2 => {
                self.regs.cnthp_cval_el2 = self.insn_count.wrapping_add(val as i32 as i64 as u64);
            }
            sysreg::CNTVOFF_EL2 => self.regs.cntvoff_el2 = val,
            // EL3 — control
            sysreg::SCR_EL3 => self.regs.scr_el3 = val,
            sysreg::SCTLR_EL3 => {
                self.regs.sctlr_el3 = val;
                self.tlb.flush_all();
            }
            sysreg::ACTLR_EL3 => self.regs.actlr_el3 = val,
            sysreg::CPTR_EL3 => self.regs.cptr_el3 = val,
            sysreg::MDCR_EL3 => self.regs.mdcr_el3 = val,
            // EL3 — translation (flush TLB on table/config changes)
            sysreg::TCR_EL3 => {
                self.regs.tcr_el3 = val;
                self.tlb.flush_all();
            }
            sysreg::TTBR0_EL3 => {
                self.regs.ttbr0_el3 = val;
                self.tlb.flush_all();
            }
            sysreg::MAIR_EL3 => self.regs.mair_el3 = val,
            sysreg::AMAIR_EL3 => self.regs.amair_el3 = val,
            // EL3 — fault
            sysreg::ESR_EL3 => self.regs.esr_el3 = val as u32,
            sysreg::FAR_EL3 => self.regs.far_el3 = val,
            sysreg::AFSR0_EL3 => self.regs.afsr0_el3 = val,
            sysreg::AFSR1_EL3 => self.regs.afsr1_el3 = val,
            // EL3 — exception state
            sysreg::VBAR_EL3 => self.regs.vbar_el3 = val,
            sysreg::ELR_EL3 => self.regs.elr_el3 = val,
            sysreg::SPSR_EL3 => self.regs.spsr_el3 = val as u32,
            sysreg::SP_EL3 => self.regs.sp_el3 = val,
            // EL3 — thread
            sysreg::TPIDR_EL3 => self.regs.tpidr_el3 = val,
            // Performance monitors — stub, ignore writes
            sysreg::PMCR_EL0
            | sysreg::PMCNTENSET_EL0
            | sysreg::PMCNTENCLR_EL0
            | sysreg::PMOVSCLR_EL0
            | sysreg::PMUSERENR_EL0
            | sysreg::PMCCNTR_EL0
            | sysreg::PMCCFILTR_EL0
            | sysreg::PMSELR_EL0
            | sysreg::PMXEVTYPER_EL0
            | sysreg::PMXEVCNTR_EL0 => {}
            // OS lock
            sysreg::OSLAR_EL1 | sysreg::OSDLR_EL1 => {}
            // ID registers — read-only, ignore writes
            sysreg::MIDR_EL1
            | sysreg::MPIDR_EL1
            | sysreg::REVIDR_EL1
            | sysreg::CTR_EL0
            | sysreg::DCZID_EL0 => {}
            // Unknown: WI (write-ignored)
            _ => {
                log::trace!("MSR: unknown sysreg {id:#06x} ← {val:#x} (WI)");
            }
        }
    }

    // === Exception entry to EL1 ===
    fn take_exception_to_el1(&mut self, exception_class: u32, syndrome: u32) {
        // Save return address and PSTATE.
        // Per ARMv8 spec (D1.10.1), SVC/HVC/SMC/BRK from AArch64 set
        // ELR to the preferred return address PC+4 (the instruction
        // after the exception-generating instruction).  All other
        // synchronous exceptions use PC (the faulting instruction).
        self.regs.elr_el1 = match exception_class {
            0x15 | 0x16 | 0x17 | 0x3C => self.regs.pc.wrapping_add(4),
            _ => self.regs.pc,
        };
        self.regs.spsr_el1 = self.save_pstate();
        self.regs.esr_el1 = (exception_class << 26) | (1u32 << 25) | (syndrome & 0x01FF_FFFF);

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
                exception_class,
                syndrome,
                self.regs.pc,
                self.regs.vbar_el1,
                vector_offset,
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
        self.regs.elr_el2 = match exception_class {
            0x15 | 0x16 | 0x17 | 0x3C => self.regs.pc.wrapping_add(4),
            _ => self.regs.pc,
        };
        self.regs.spsr_el2 = self.save_pstate();
        self.regs.esr_el2 = (exception_class << 26) | (1u32 << 25) | (syndrome & 0x01FF_FFFF);

        let vector_offset: u64 = match (self.regs.current_el, self.regs.sp_sel) {
            (2, 0) => 0x000, // from current EL, SP_EL0
            (2, _) => 0x200, // from current EL, SP_ELx
            _ => 0x400,      // from lower EL, AArch64
        };

        if log::log_enabled!(log::Level::Debug) {
            log::debug!(
                "EL2 exception EC={:#x} ISS={:#x} from EL{} PC={:#x} → VBAR_EL2({:#x})+{:#x} insn#{}",
                exception_class, syndrome, self.regs.current_el, self.regs.pc,
                self.regs.vbar_el2, vector_offset, self.insn_count,
            );
        }

        self.regs.pc = self.regs.vbar_el2.wrapping_add(vector_offset);
        self.regs.current_el = 2;
        self.regs.sp_sel = 1;
        self.regs.daif = 0x3C0;
        self.pc_written = true;
    }

    // === Exception entry to EL3 ===
    fn take_exception_to_el3(&mut self, exception_class: u32, syndrome: u32) {
        self.regs.elr_el3 = match exception_class {
            0x15 | 0x16 | 0x17 | 0x3C => self.regs.pc.wrapping_add(4),
            _ => self.regs.pc,
        };
        self.regs.spsr_el3 = self.save_pstate();
        self.regs.esr_el3 = (exception_class << 26) | (1u32 << 25) | (syndrome & 0x01FF_FFFF);

        let vector_offset: u64 = match (self.regs.current_el, self.regs.sp_sel) {
            (3, 0) => 0x000, // from current EL, SP_EL0
            (3, _) => 0x200, // from current EL, SP_ELx
            _ => 0x400,      // from lower EL, AArch64
        };

        if log::log_enabled!(log::Level::Debug) {
            log::debug!(
                "EL3 exception EC={:#x} ISS={:#x} from EL{} PC={:#x} → VBAR_EL3({:#x})+{:#x} insn#{}",
                exception_class, syndrome, self.regs.current_el, self.regs.pc,
                self.regs.vbar_el3, vector_offset, self.insn_count,
            );
        }

        self.regs.pc = self.regs.vbar_el3.wrapping_add(vector_offset);
        self.regs.current_el = 3;
        self.regs.sp_sel = 1;
        self.regs.daif = 0x3C0;
        self.pc_written = true;
    }

    // === Route a synchronous exception to the correct target EL ===
    fn take_exception(&mut self, target_el: u8, exception_class: u32, syndrome: u32) {
        match target_el {
            1 => self.take_exception_to_el1(exception_class, syndrome),
            2 => self.take_exception_to_el2(exception_class, syndrome),
            3 => self.take_exception_to_el3(exception_class, syndrome),
            _ => self.take_exception_to_el1(exception_class, syndrome),
        }
    }

    /// Determine the target EL for a synchronous exception.
    fn route_sync_exception(&self, ec: u32) -> u8 {
        let from_el = self.regs.current_el;
        let hcr = self.regs.hcr_el2;
        let scr = self.regs.scr_el3;

        match from_el {
            0 => {
                // From EL0: if HCR_EL2.TGE=1 → EL2, else → EL1
                if hcr & hcr::HCR_TGE != 0 {
                    2
                } else {
                    1
                }
            }
            1 => {
                match ec {
                    0x16 => 2, // HVC from EL1 → always EL2
                    0x17 => {
                        // SMC from EL1
                        if hcr & hcr::HCR_TSC != 0 {
                            2 // TSC traps SMC to EL2
                        } else {
                            3 // SMC → EL3
                        }
                    }
                    _ => 1,
                }
            }
            2 => {
                match ec {
                    0x17 if scr & (1 << 7) != 0 => 3, // SMC from EL2 → EL3 (if SCR_EL3.SMD clear)
                    _ => 2,
                }
            }
            _ => from_el, // EL3 stays at EL3
        }
    }

    // === IRQ exception delivery ===

    /// Check for a pending IRQ and take the exception if unmasked.
    /// Returns `true` if an IRQ exception was taken.
    pub fn check_irq(&mut self) -> bool {
        let signal = match self.irq_signal {
            Some(ref s) => s,
            None => return false,
        };
        if !signal.is_raised() {
            return false;
        }
        // DAIF.I is bit 7 (PSTATE bit 7, stored in regs.daif)
        if self.regs.daif & 0x80 != 0 {
            return false; // IRQs masked
        }

        // Determine target EL for IRQ
        let target_el = match self.regs.current_el {
            0 => {
                if self.regs.hcr_el2 & hcr::HCR_TGE != 0 {
                    2
                } else {
                    1
                }
            }
            1 => {
                // HCR_EL2.IMO routes physical IRQs to EL2
                if self.regs.hcr_el2 & hcr::HCR_IMO != 0 {
                    2
                } else {
                    1
                }
            }
            2 => 2,
            _ => 3,
        };

        // Take IRQ exception — save state and jump to vector
        match target_el {
            1 => {
                self.regs.elr_el1 = self.regs.pc;
                self.regs.spsr_el1 = self.save_pstate();
                // IRQ vector offset
                let vector_offset: u64 = match (self.regs.current_el, self.regs.sp_sel) {
                    (0, _) => 0x480, // from lower EL, AArch64
                    (1, 0) => 0x080, // from current EL, SP_EL0
                    (1, _) => 0x280, // from current EL, SP_ELx
                    _ => 0x480,
                };
                self.regs.pc = self.regs.vbar_el1.wrapping_add(vector_offset);
                self.regs.current_el = 1;
                self.regs.sp_sel = 1;
                self.regs.daif |= 0x80; // mask IRQs
            }
            2 => {
                self.regs.elr_el2 = self.regs.pc;
                self.regs.spsr_el2 = self.save_pstate();
                let vector_offset: u64 = match (self.regs.current_el, self.regs.sp_sel) {
                    (2, 0) => 0x080,
                    (2, _) => 0x280,
                    _ => 0x480,
                };
                self.regs.pc = self.regs.vbar_el2.wrapping_add(vector_offset);
                self.regs.current_el = 2;
                self.regs.sp_sel = 1;
                self.regs.daif |= 0x80;
            }
            _ => {
                self.regs.elr_el3 = self.regs.pc;
                self.regs.spsr_el3 = self.save_pstate();
                let vector_offset: u64 = match (self.regs.current_el, self.regs.sp_sel) {
                    (3, 0) => 0x080,
                    (3, _) => 0x280,
                    _ => 0x480,
                };
                self.regs.pc = self.regs.vbar_el3.wrapping_add(vector_offset);
                self.regs.current_el = 3;
                self.regs.sp_sel = 1;
                self.regs.daif |= 0x80;
            }
        }

        log::debug!(
            "IRQ exception taken → EL{} PC={:#x} insn#{}",
            target_el,
            self.regs.pc,
            self.insn_count,
        );
        true
    }

    // === Timer checks ===

    /// Check generic timers and return which timer IRQs should fire.
    /// Returns (vtimer_fire, ptimer_fire) — the caller injects these into the GIC.
    pub fn check_timers(&self) -> (bool, bool) {
        let cnt = self.insn_count;
        // Virtual timer: IRQ 27
        let v_ctl = self.regs.cntv_ctl_el0;
        let v_fire = (v_ctl & 1 != 0)       // ENABLE
            && (v_ctl & 2 == 0)              // !IMASK
            && cnt >= self.regs.cntv_cval_el0;

        // Physical timer: IRQ 30
        let p_ctl = self.regs.cntp_ctl_el0;
        let p_fire = (p_ctl & 1 != 0) && (p_ctl & 2 == 0) && cnt >= self.regs.cntp_cval_el0;

        (v_fire, p_fire)
    }

    /// Advance the virtual counter to the nearest timer event (WFI fast-forward).
    /// Returns the number of ticks skipped.
    pub fn wfi_advance(&mut self) -> u64 {
        let cnt = self.insn_count;
        let mut next = u64::MAX;

        // Virtual timer
        let v_ctl = self.regs.cntv_ctl_el0;
        if v_ctl & 1 != 0 && v_ctl & 2 == 0 {
            next = next.min(self.regs.cntv_cval_el0);
        }
        // Physical timer
        let p_ctl = self.regs.cntp_ctl_el0;
        if p_ctl & 1 != 0 && p_ctl & 2 == 0 {
            next = next.min(self.regs.cntp_cval_el0);
        }

        if next > cnt && next != u64::MAX {
            let skip = next - cnt;
            self.insn_count = next;
            skip
        } else {
            0
        }
    }

    // === PSCI (Power State Coordination Interface) ===

    /// Handle PSCI function calls via HVC/SMC.
    /// Returns true if the call was a recognized PSCI function ID.
    fn handle_psci_call(&mut self) -> bool {
        let fid = self.xn(0) as u32;
        match fid {
            // PSCI_VERSION → return 1.1 (0x00010001)
            0x8400_0000 => {
                self.set_xn(0, 0x0001_0001);
                log::info!("PSCI: VERSION → 1.1");
            }
            // PSCI_FEATURES → return SUCCESS for known functions
            0x8400_000A => {
                let qfid = self.xn(1) as u32;
                let ret = match qfid {
                    0x8400_0000 | 0x8400_0001 | 0x8400_0002 | 0x8400_0003 | 0x8400_0008
                    | 0x8400_0009 | 0x8400_000A => 0i64, // SUCCESS
                    _ => -1i64, // NOT_SUPPORTED
                };
                self.set_xn(0, ret as u64);
            }
            // CPU_SUSPEND → return SUCCESS (wake immediately)
            0x8400_0001 => {
                self.set_xn(0, 0); // SUCCESS
            }
            // CPU_OFF → halt this CPU
            0x8400_0002 => {
                self.halted = true;
                log::info!("PSCI: CPU_OFF");
            }
            // CPU_ON → not supported (single-core), return ALREADY_ON
            0x8400_0003 => {
                self.set_xn(0, (-4i64) as u64); // ALREADY_ON
            }
            // SYSTEM_OFF → halt
            0x8400_0008 => {
                self.halted = true;
                log::info!("PSCI: SYSTEM_OFF");
            }
            // SYSTEM_RESET → halt (no actual reset)
            0x8400_0009 => {
                self.halted = true;
                log::info!("PSCI: SYSTEM_RESET");
            }
            // MIGRATE_INFO_TYPE → return NOT_SUPPORTED (no TOS)
            0x8400_0006 => {
                self.set_xn(0, 2); // TOS not present
            }
            _ => return false, // not a PSCI call
        }
        true
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
        spsr |= self.regs.nzcv & 0xF000_0000; // NZCV in bits [31:28]
        spsr |= self.regs.daif & 0x3C0; // DAIF in bits [9:6]
        spsr |= (self.regs.current_el as u32) << 2; // EL in bits [3:2]
        spsr |= self.regs.sp_sel as u32; // SP in bit [0]
        spsr
    }

    fn restore_pstate(&mut self, spsr: u32) {
        self.regs.nzcv = spsr & 0xF000_0000;
        self.regs.daif = spsr & 0x3C0;
        self.regs.current_el = ((spsr >> 2) & 3) as u8;
        self.regs.sp_sel = (spsr & 1) as u8;
    }

    /// Reconstruct the VA from a TLBI register value.
    /// TLBI instructions store VA[55:12] in Xt[43:0]. The result must be
    /// sign-extended from bit 55 so kernel VAs (bit 55 = 1) get the correct
    /// 0xFFxx upper byte.
    fn tlbi_va(xt: u64) -> u64 {
        let raw = xt << 12;
        // Sign-extend from bit 55
        if raw & (1u64 << 55) != 0 {
            raw | 0xFF00_0000_0000_0000
        } else {
            raw
        }
    }

    // === TLBI dispatch ===
    fn exec_tlbi(&mut self, op1: u32, crm: u32, op2: u32, rt: u16) -> HelmResult<()> {
        // Determine if this is a VA-based or all-entries flush for the hook
        let is_va_tlbi = matches!(
            (op1, op2),
            (0, 1) | (0, 3) | (0, 5) | (0, 7) | (4, 1) | (4, 5) | (6, 1) | (6, 5)
        );

        match (op1, crm, op2) {
            // VMALLE1(IS), VMALLE1OS — flush all EL1 (stage-1+2)
            (0, 3, 0) | (0, 7, 0) => self.tlb.flush_all(),
            // ALLE1(IS) — flush all EL1 entries
            (4, 3, 4) | (4, 7, 4) => self.tlb.flush_all(),
            // ALLE2(IS) — flush all EL2 entries
            (4, 3, 0) | (4, 7, 0) => self.tlb.flush_all(),
            // ALLE3(IS) — flush all EL3 entries
            (6, 3, 0) | (6, 7, 0) => self.tlb.flush_all(),
            // VMALLS12E1(IS) — flush all stage-1+2 for current VMID
            (4, 3, 6) | (4, 7, 6) => self.tlb.flush_all(),
            // VAE1(IS), VALE1(IS), VAAE1(IS), VAALE1(IS) — flush by VA
            (0, 3, 1)
            | (0, 7, 1)
            | (0, 3, 5)
            | (0, 7, 5)
            | (0, 3, 3)
            | (0, 7, 3)
            | (0, 3, 7)
            | (0, 7, 7) => {
                let va = Self::tlbi_va(self.xn(rt));
                self.tlb.flush_va(va);
            }
            // VAE2(IS), VALE2(IS) — flush EL2 by VA
            (4, 3, 1) | (4, 7, 1) | (4, 3, 5) | (4, 7, 5) => {
                let va = Self::tlbi_va(self.xn(rt));
                self.tlb.flush_va(va);
            }
            // VAE3(IS), VALE3(IS) — flush EL3 by VA
            (6, 3, 1) | (6, 7, 1) | (6, 3, 5) | (6, 7, 5) => {
                let va = Self::tlbi_va(self.xn(rt));
                self.tlb.flush_va(va);
            }
            // ASIDE1(IS) — flush by ASID
            (0, 3, 2) | (0, 7, 2) => {
                let asid = (self.xn(rt) >> 48) as u16;
                self.tlb.flush_asid(asid);
            }
            // IPAS2E1(IS), IPAS2LE1(IS) — flush stage-2 by IPA
            (4, 0, 1) | (4, 4, 1) | (4, 0, 5) | (4, 4, 5) => {
                // Currently flush all (no VMID-tagged entries yet)
                self.tlb.flush_all();
            }
            // Unknown TLBI → flush all (safe)
            _ => self.tlb.flush_all(),
        }
        // Notify MMU debug hook
        if self.mmu_hook.is_some() {
            let tlbi_target = if is_va_tlbi {
                Some(Self::tlbi_va(self.xn(rt)))
            } else {
                None
            };
            let insn_n = self.insn_count;
            if let Some(ref mut hook) = self.mmu_hook {
                hook.on_tlbi(tlbi_target, !is_va_tlbi, insn_n);
            }
        }
        Ok(())
    }

    // === AT (Address Translate) dispatch ===
    fn exec_at(&mut self, op1: u32, op2: u32, rt: u16, mem: &mut impl ExecMem) -> HelmResult<()> {
        let va = self.xn(rt);
        let is_write = op2 & 1 != 0; // op2 bit 0: 0=read, 1=write

        log::trace!("AT: op1={op1} op2={op2} va={va:#x} is_write={is_write}");
        let result = match (op1, op2) {
            // AT S1E1R/W — stage-1 EL1 translation
            (0, 0) | (0, 1) => {
                let tcr = TranslationConfig::parse(self.regs.tcr_el1);
                mmu::translate(
                    va,
                    &tcr,
                    self.regs.ttbr0_el1,
                    self.regs.ttbr1_el1,
                    &mut |pa| {
                        let mut buf = [0u8; 8];
                        mem.read_phys(pa, &mut buf).unwrap_or(());
                        u64::from_le_bytes(buf)
                    },
                )
                .map(|(w, _)| w)
            }
            // AT S1E2R/W — stage-1 EL2 translation
            (4, 0) | (4, 1) => {
                let tcr = TranslationConfig::parse_single(self.regs.tcr_el2);
                mmu::translate(va, &tcr, self.regs.ttbr0_el2, 0, &mut |pa| {
                    let mut buf = [0u8; 8];
                    mem.read_phys(pa, &mut buf).unwrap_or(());
                    u64::from_le_bytes(buf)
                })
                .map(|(w, _)| w)
            }
            // AT S1E3R/W — stage-1 EL3 translation
            (6, 0) | (6, 1) => {
                let tcr = TranslationConfig::parse_single(self.regs.tcr_el3);
                mmu::translate(va, &tcr, self.regs.ttbr0_el3, 0, &mut |pa| {
                    let mut buf = [0u8; 8];
                    mem.read_phys(pa, &mut buf).unwrap_or(());
                    u64::from_le_bytes(buf)
                })
                .map(|(w, _)| w)
            }
            // AT S12E1R/W — combined stage-1 + stage-2
            (0, 4) | (0, 5) => {
                // Stage-1 first
                let tcr = TranslationConfig::parse(self.regs.tcr_el1);
                let s1_result = mmu::translate(
                    va,
                    &tcr,
                    self.regs.ttbr0_el1,
                    self.regs.ttbr1_el1,
                    &mut |pa| {
                        let mut buf = [0u8; 8];
                        mem.read_phys(pa, &mut buf).unwrap_or(());
                        u64::from_le_bytes(buf)
                    },
                );
                match s1_result {
                    Ok((walk, _)) => {
                        if self.regs.hcr_el2 & hcr::HCR_VM != 0 {
                            // Stage-2
                            let s2cfg = mmu::Stage2Config::parse(self.regs.vtcr_el2);
                            mmu::walk_stage2(walk.pa, self.regs.vttbr_el2, &s2cfg, &mut |pa| {
                                let mut buf = [0u8; 8];
                                mem.read_phys(pa, &mut buf).unwrap_or(());
                                u64::from_le_bytes(buf)
                            })
                        } else {
                            Ok(walk)
                        }
                    }
                    Err(e) => Err(e),
                }
            }
            _ => {
                // Unknown AT variant — set PAR_EL1 to fault
                self.regs.par_el1 = 1; // F bit set
                return Ok(());
            }
        };

        match result {
            Ok(walk) => {
                // PAR_EL1 success: F=0, PA[47:12], ATTR from walk
                self.regs.par_el1 =
                    (walk.pa & 0x0000_FFFF_FFFF_F000) | ((walk.attr_indx as u64) << 56);
            }
            Err(fault) => {
                // PAR_EL1 failure: F=1, FST[6:1]
                let fsc = fault.to_fsc();
                self.regs.par_el1 = 1 | ((fsc as u64) << 1);
            }
        }
        Ok(())
    }

    // === Loads and Stores ===
    fn exec_ldst(&mut self, insn: u32, mem: &mut impl ExecMem) -> HelmResult<()> {
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

        // LDP/STP (and MTE STGP/LDGP — treat tag operations as regular pair)
        let top5 = (insn >> 27) & 0x1F;
        if top5 == 0b10101 || top5 == 0b00101 || top5 == 0b01101 {
            return self.exec_pair(insn, mem);
        }

        // MTE tag instructions: STG, LDG, STZG, ST2G, STZ2G — NOP (no tag support)
        // Encoding: 1101_1001_1xx0_xxxx_xxxx_xxxx_xxxx_xxxx
        if (insn >> 24) & 0xFF == 0xD9 && (insn >> 22) & 1 == 0 {
            return Ok(());
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
            // PRFM (prefetch): size=3, opc=2 → NOP in simulation
            if size == 3 && opc == 2 {
                return Ok(());
            }
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
            // PRFM variants (size=3, opc=2): prefetch → NOP
            if size == 3 && opc == 2 {
                return Ok(());
            }
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
        if (insn >> 24) & 0x3F == 0b011000 || (insn >> 24) & 0x3F == 0b011100 {
            // PRFM literal: size=3 → NOP
            if size == 3 {
                return Ok(());
            }
            let rt = (insn & 0x1F) as u16;
            let imm19 = sext((insn >> 5) & 0x7FFFF, 19) as u64;
            let addr = self.regs.pc.wrapping_add(imm19 << 2);
            let sz = if size == 2 {
                4
            } else if size == 0 {
                4
            } else {
                8
            };
            let val = self.trace_rd(mem, addr, sz)?;
            if size == 2 {
                // LDRSW literal: sign-extend 32-bit to 64-bit
                self.set_xn(rt, sext64(val, 32));
            } else {
                self.set_xn(rt, val);
            }
            return Ok(());
        }
        self.unimpl("ldst")
    }

    fn exec_pair(&mut self, insn: u32, mem: &mut impl ExecMem) -> HelmResult<()> {
        let opc = (insn >> 30) & 0x3;
        let l = (insn >> 22) & 1;
        let idx = (insn >> 23) & 0x3;
        let imm7 = sext((insn >> 15) & 0x7F, 7);
        let rt2 = ((insn >> 10) & 0x1F) as u16;
        let rn = ((insn >> 5) & 0x1F) as u16;
        let rt = (insn & 0x1F) as u16;
        // opc: 00=32-bit(4B), 01=STGP/LDPSW(8B), 10=64-bit(8B)
        let scale: u64 = if opc >= 0b01 { 8 } else { 4 };
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

    fn exec_exclusive(&mut self, insn: u32, mem: &mut impl ExecMem) -> HelmResult<()> {
        let size = (insn >> 30) & 0x3;
        let l = (insn >> 22) & 1;
        let o0 = (insn >> 21) & 1; // 1 = pair (LDXP/STXP), 0 = single
        let rn = ((insn >> 5) & 0x1F) as u16;
        let rt = (insn & 0x1F) as u16;
        let base = self.xn_sp(rn);
        let sz = 1usize << size;

        if o0 == 1 {
            // Exclusive pair: LDXP / STXP / LDAXP / STLXP
            let rt2 = ((insn >> 10) & 0x1F) as u16;
            if l == 1 {
                let v0 = self.trace_rd(mem, base, sz)?;
                let v1 = self.trace_rd(mem, base.wrapping_add(sz as u64), sz)?;
                self.set_xn(rt, v0);
                self.set_xn(rt2, v1);
            } else {
                let rs = ((insn >> 16) & 0x1F) as u16;
                self.trace_wr(mem, base, self.xn(rt), sz)?;
                self.trace_wr(mem, base.wrapping_add(sz as u64), self.xn(rt2), sz)?;
                self.set_xn(rs, 0); // always succeeds (single-core)
            }
        } else {
            // Exclusive single: LDXR / STXR / LDAXR / STLXR / LDAR / STLR
            if l == 1 {
                let val = self.trace_rd(mem, base, sz)?;
                self.set_xn(rt, val);
            } else {
                let rs = ((insn >> 16) & 0x1F) as u16;
                self.trace_wr(mem, base, self.xn(rt), sz)?;
                self.set_xn(rs, 0); // always succeeds (single-core)
            }
        }
        Ok(())
    }

    fn exec_atomic(&mut self, insn: u32, mem: &mut impl ExecMem) -> HelmResult<()> {
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
    fn exec_ldst_simd(&mut self, insn: u32, mem: &mut impl ExecMem) -> HelmResult<()> {
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
            log::warn!(
                "SIMD ldst encountered: {} (insn={:#010x} PC={:#x})",
                ldst_kind,
                insn,
                self.regs.pc
            );
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
                let shift = if s_bit == 1 {
                    if is_q {
                        4
                    } else {
                        size
                    }
                } else {
                    0
                };
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
            let elem_bytes: usize = 1 << elem_size;
            let reg_bytes: usize = if q == 1 { 16 } else { 8 };
            log::trace!("SIMD LD/ST multi: nregs elem_bytes={elem_bytes}");
            let nregs: usize = match opcode {
                0b0111 => 1,
                0b1010 => 2,
                0b0110 => 3,
                0b0010 => 4,
                // Interleave variants: LD2/ST2, LD3/ST3, LD4/ST4
                0b1000 => 2, // LD2/ST2
                0b0100 => 3, // LD3/ST3
                0b0000 => 4, // LD4/ST4
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
                // CRC32B/H/W/X (op2=16-19), CRC32CB/CH/CW/CX (op2=20-23)
                16..=23 => {
                    let crc_c = op2 >= 20; // CRC32C variants
                    let sz = (op2 & 3) as u32; // 0=B, 1=H, 2=W, 3=X
                    let mut crc = self.wn(rn); // CRC accumulator is 32-bit
                    let data = if sz == 3 {
                        self.xn(rm)
                    } else {
                        self.wn(rm) as u64
                    };
                    let nbytes = 1usize << sz;
                    for i in 0..nbytes {
                        let byte = ((data >> (i * 8)) & 0xFF) as u8;
                        crc = if crc_c {
                            crc32c_byte(crc, byte)
                        } else {
                            crc32_byte(crc, byte)
                        };
                    }
                    crc as u64
                }
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
                        u64::from_le_bytes([b[1], b[0], b[3], b[2], b[5], b[4], b[7], b[6]])
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
                        u64::from_le_bytes([b[3], b[2], b[1], b[0], b[7], b[6], b[5], b[4]])
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
                        let s = if a >> 63 == 1 {
                            (!a).leading_zeros()
                        } else {
                            a.leading_zeros()
                        };
                        s.saturating_sub(1) as u64
                    } else {
                        let w = self.wn(rn);
                        let s = if w >> 31 == 1 {
                            (!w).leading_zeros()
                        } else {
                            w.leading_zeros()
                        };
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
        let mnemonic = {
            let q = decode_a64(insn);
            if q != "UNKNOWN" {
                q
            } else {
                decode_aarch64_simd(insn)
            }
        };
        if self.simd_seen.insert(mnemonic) {
            log::warn!(
                "SIMD insn encountered: {} (insn={:#010x} PC={:#x})",
                mnemonic,
                insn,
                self.regs.pc
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
                self.regs.v[rd] =
                    (self.regs.v[rd] & mask) | ((val as u128 & 0xFFFF_FFFF_FFFF_FFFF) << shift);
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
                    let ival = if sf == 1 {
                        self.xn(rn as u16) as i64
                    } else {
                        self.xn(rn as u16) as i32 as i64
                    };
                    if ftype == 0 {
                        self.regs.v[rd] = (ival as f32).to_bits() as u128;
                    } else {
                        self.regs.v[rd] = (ival as f64).to_bits() as u128;
                    }
                }
                // UCVTF: unsigned int -> FP
                (0, 3) => {
                    let uval = if sf == 1 {
                        self.xn(rn as u16)
                    } else {
                        self.xn(rn as u16) as u32 as u64
                    };
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
                        if sf == 1 {
                            f as i64 as u64
                        } else {
                            f as i32 as u32 as u64
                        }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        if sf == 1 {
                            f as i64 as u64
                        } else {
                            f as i32 as u32 as u64
                        }
                    };
                    self.set_xn(rd as u16, val);
                }
                // FCVTZU: FP -> unsigned int (round toward zero)
                (3, 1) => {
                    let val = if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        if sf == 1 {
                            f as u64
                        } else {
                            f as u32 as u64
                        }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        if sf == 1 {
                            f as u64
                        } else {
                            f as u32 as u64
                        }
                    };
                    self.set_xn(rd as u16, val);
                }
                // FCVTNS: FP -> signed int (round nearest, ties to even)
                (0, 0) => {
                    let val = if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        let r = f.round_ties_even();
                        if sf == 1 {
                            r as i64 as u64
                        } else {
                            r as i32 as u32 as u64
                        }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        let r = f.round_ties_even();
                        if sf == 1 {
                            r as i64 as u64
                        } else {
                            r as i32 as u32 as u64
                        }
                    };
                    self.set_xn(rd as u16, val);
                }
                // FCVTNU: FP -> unsigned int (round nearest, ties to even)
                (0, 1) => {
                    let val = if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        let r = f.round_ties_even();
                        if sf == 1 {
                            r as u64
                        } else {
                            r as u32 as u64
                        }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        let r = f.round_ties_even();
                        if sf == 1 {
                            r as u64
                        } else {
                            r as u32 as u64
                        }
                    };
                    self.set_xn(rd as u16, val);
                }
                // FCVTMS: FP -> signed int (round toward -inf)
                (2, 0) => {
                    let val = if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        let r = f.floor();
                        if sf == 1 {
                            r as i64 as u64
                        } else {
                            r as i32 as u32 as u64
                        }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        let r = f.floor();
                        if sf == 1 {
                            r as i64 as u64
                        } else {
                            r as i32 as u32 as u64
                        }
                    };
                    self.set_xn(rd as u16, val);
                }
                // FCVTPS: FP -> signed int (round toward +inf)
                (1, 0) => {
                    let val = if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        let r = f.ceil();
                        if sf == 1 {
                            r as i64 as u64
                        } else {
                            r as i32 as u32 as u64
                        }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        let r = f.ceil();
                        if sf == 1 {
                            r as i64 as u64
                        } else {
                            r as i32 as u32 as u64
                        }
                    };
                    self.set_xn(rd as u16, val);
                }
                // FCVTAS: FP -> signed int (round to nearest, ties away)
                (0, 4) => {
                    let val = if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        let r = f.round();
                        if sf == 1 {
                            r as i64 as u64
                        } else {
                            r as i32 as u32 as u64
                        }
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        let r = f.round();
                        if sf == 1 {
                            r as i64 as u64
                        } else {
                            r as i32 as u32 as u64
                        }
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
                val = if q == 1 {
                    ((imm64 as u128) << 64) | imm64 as u128
                } else {
                    imm64 as u128
                };
            } else if cmode == 0b1110 && op == 0 {
                let byte_val = imm8 as u128;
                let bytes = if q == 1 { 16 } else { 8 };
                for i in 0..bytes {
                    val |= byte_val << (i * 8);
                }
            } else {
                let shift = ((cmode >> 1) & 3) * 8;
                let base = (imm8 as u64) << shift;
                let elem_size = if cmode < 4 {
                    4usize
                } else if cmode < 8 {
                    4
                } else {
                    2
                };
                let elem_mask = if elem_size == 4 {
                    0xFFFF_FFFFu64
                } else {
                    0xFFFFu64
                };
                let elem = if op == 1 {
                    !base & elem_mask
                } else {
                    base & elem_mask
                };
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
            let emask: u128 = if esize >= 16 {
                u128::MAX
            } else {
                (1u128 << ebits) - 1
            };
            let count = bytes / esize;
            let a = self.regs.v[rn];
            let mut acc = (a >> 0) & emask;
            for i in 1..count {
                let ea = (a >> (i * ebits)) & emask;
                acc = match (u, opcode) {
                    (_, 0b11011) => (acc + ea) & emask,
                    (0, 0b01010) => {
                        let sa = acc as i128
                            - if acc >> (ebits - 1) != 0 {
                                1i128 << ebits
                            } else {
                                0
                            };
                        let sb = ea as i128
                            - if ea >> (ebits - 1) != 0 {
                                1i128 << ebits
                            } else {
                                0
                            };
                        if sa >= sb {
                            acc
                        } else {
                            ea
                        }
                    }
                    (1, 0b01010) => {
                        if acc >= ea {
                            acc
                        } else {
                            ea
                        }
                    }
                    (0, 0b11010) => {
                        let sa = acc as i128
                            - if acc >> (ebits - 1) != 0 {
                                1i128 << ebits
                            } else {
                                0
                            };
                        let sb = ea as i128
                            - if ea >> (ebits - 1) != 0 {
                                1i128 << ebits
                            } else {
                                0
                            };
                        if sa <= sb {
                            acc
                        } else {
                            ea
                        }
                    }
                    (1, 0b11010) => {
                        if acc <= ea {
                            acc
                        } else {
                            ea
                        }
                    }
                    // SADDLV / UADDLV (add-across into wider element)
                    (0, 0b00011) | (1, 0b00011) => (acc + ea) & emask,
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
            log::trace!("UMOV: esize={esize} val={val:#x}");
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
            let emask: u128 = if esize >= 16 {
                u128::MAX
            } else {
                (1u128 << ebits) - 1
            };
            let mut result: u128 = 0;
            for i in 0..(bytes / esize) {
                let shift = i * ebits;
                let ea = (a >> shift) & emask;
                let eb = (b >> shift) & emask;
                let sa = ea as i128
                    - if ea >> (ebits - 1) != 0 {
                        1i128 << ebits
                    } else {
                        0
                    };
                let sb = eb as i128
                    - if eb >> (ebits - 1) != 0 {
                        1i128 << ebits
                    } else {
                        0
                    };
                let er = match (u, opcode) {
                    (0, 0b10000) => ea.wrapping_add(eb) & emask,
                    (1, 0b10000) => ea.wrapping_sub(eb) & emask,
                    (0, 0b00110) => {
                        if sa > sb {
                            emask
                        } else {
                            0
                        }
                    }
                    (1, 0b00110) => {
                        if ea > eb {
                            emask
                        } else {
                            0
                        }
                    }
                    (0, 0b00111) => {
                        if sa >= sb {
                            emask
                        } else {
                            0
                        }
                    }
                    (1, 0b00111) => {
                        if ea >= eb {
                            emask
                        } else {
                            0
                        }
                    }
                    (1, 0b10001) => {
                        if ea == eb {
                            emask
                        } else {
                            0
                        }
                    }
                    (0, 0b10001) => {
                        if ea & eb != 0 {
                            emask
                        } else {
                            0
                        }
                    }
                    (0, 0b01100) => {
                        if sa >= sb {
                            ea
                        } else {
                            eb
                        }
                    }
                    (1, 0b01100) => {
                        if ea >= eb {
                            ea
                        } else {
                            eb
                        }
                    }
                    (0, 0b01101) => {
                        if sa <= sb {
                            ea
                        } else {
                            eb
                        }
                    }
                    (1, 0b01101) => {
                        if ea <= eb {
                            ea
                        } else {
                            eb
                        }
                    }
                    (0, 0b10011) => ea.wrapping_mul(eb) & emask,
                    (0, 0b10010) => {
                        let d = (self.regs.v[rd] >> shift) & emask;
                        d.wrapping_add(ea.wrapping_mul(eb)) & emask
                    }
                    // SHADD / UHADD (halving add)
                    (0, 0b00000) => (sa.wrapping_add(sb) >> 1) as u128 & emask,
                    (1, 0b00000) => ea.wrapping_add(eb) >> 1 & emask,
                    // SHSUB / UHSUB (halving sub)
                    (0, 0b00100) => (sa.wrapping_sub(sb) >> 1) as u128 & emask,
                    (1, 0b00100) => ea.wrapping_sub(eb) >> 1 & emask,
                    // SQADD / UQADD (saturating add) — simplified, no saturation
                    (0, 0b00001) => ea.wrapping_add(eb) & emask,
                    (1, 0b00001) => {
                        let s = ea + eb;
                        if s > emask {
                            emask
                        } else {
                            s
                        }
                    }
                    // SQSUB / UQSUB (saturating sub) — simplified
                    (0, 0b00101) => ea.wrapping_sub(eb) & emask,
                    (1, 0b00101) => {
                        if ea >= eb {
                            ea - eb
                        } else {
                            0
                        }
                    }
                    // SABD / UABD (absolute difference)
                    (0, 0b01110) => (sa.wrapping_sub(sb)).unsigned_abs() & emask,
                    (1, 0b01110) => {
                        if ea >= eb {
                            ea - eb
                        } else {
                            eb - ea
                        }
                    }
                    // SABA / UABA (absolute difference accumulate)
                    (0, 0b10111) => {
                        let d = (self.regs.v[rd] >> shift) & emask;
                        d.wrapping_add((sa.wrapping_sub(sb)).unsigned_abs() & emask) & emask
                    }
                    (1, 0b10111) => {
                        let d = (self.regs.v[rd] >> shift) & emask;
                        let diff = if ea >= eb { ea - eb } else { eb - ea };
                        d.wrapping_add(diff) & emask
                    }
                    // ADDP (pairwise add)
                    (0, 0b10101) => {
                        let pair_idx = i / 2;
                        let src = if i % 2 == 0 { a } else { b };
                        let lo = (src >> (pair_idx * 2 * ebits)) & emask;
                        let hi = (src >> ((pair_idx * 2 + 1) * ebits)) & emask;
                        lo.wrapping_add(hi) & emask
                    }
                    // SMAXP / UMAXP (pairwise max)
                    (0, 0b10100) => {
                        let pair_idx = i / 2;
                        let src = if i % 2 == 0 { a } else { b };
                        let lo = (src >> (pair_idx * 2 * ebits)) & emask;
                        let hi = (src >> ((pair_idx * 2 + 1) * ebits)) & emask;
                        let slo = lo as i128
                            - if lo >> (ebits - 1) != 0 {
                                1i128 << ebits
                            } else {
                                0
                            };
                        let shi = hi as i128
                            - if hi >> (ebits - 1) != 0 {
                                1i128 << ebits
                            } else {
                                0
                            };
                        if slo >= shi {
                            lo
                        } else {
                            hi
                        }
                    }
                    (1, 0b10100) => {
                        let pair_idx = i / 2;
                        let src = if i % 2 == 0 { a } else { b };
                        let lo = (src >> (pair_idx * 2 * ebits)) & emask;
                        let hi = (src >> ((pair_idx * 2 + 1) * ebits)) & emask;
                        if lo >= hi {
                            lo
                        } else {
                            hi
                        }
                    }
                    // SMINP / UMINP (pairwise min)
                    (0, 0b10110) => {
                        let pair_idx = i / 2;
                        let src = if i % 2 == 0 { a } else { b };
                        let lo = (src >> (pair_idx * 2 * ebits)) & emask;
                        let hi = (src >> ((pair_idx * 2 + 1) * ebits)) & emask;
                        let slo = lo as i128
                            - if lo >> (ebits - 1) != 0 {
                                1i128 << ebits
                            } else {
                                0
                            };
                        let shi = hi as i128
                            - if hi >> (ebits - 1) != 0 {
                                1i128 << ebits
                            } else {
                                0
                            };
                        if slo <= shi {
                            lo
                        } else {
                            hi
                        }
                    }
                    (1, 0b10110) => {
                        let pair_idx = i / 2;
                        let src = if i % 2 == 0 { a } else { b };
                        let lo = (src >> (pair_idx * 2 * ebits)) & emask;
                        let hi = (src >> ((pair_idx * 2 + 1) * ebits)) & emask;
                        if lo <= hi {
                            lo
                        } else {
                            hi
                        }
                    }
                    // MLS (multiply-subtract): Vd = Vd - Vn * Vm
                    (1, 0b10010) => {
                        let d = (self.regs.v[rd] >> shift) & emask;
                        d.wrapping_sub(ea.wrapping_mul(eb) & emask) & emask
                    }
                    // SSHL / USHL (register shift)
                    (0, 0b01000) | (1, 0b01000) => {
                        let shift_amt = sb as i8;
                        if shift_amt >= 0 {
                            (ea << (shift_amt as u32 % ebits as u32)) & emask
                        } else {
                            (ea >> ((-shift_amt) as u32 % ebits as u32)) & emask
                        }
                    }
                    // SRSHL / URSHL (rounding register shift) — simplified to non-rounding
                    (0, 0b01010) | (1, 0b01010) => {
                        let shift_amt = sb as i8;
                        if shift_amt >= 0 {
                            (ea << (shift_amt as u32 % ebits as u32)) & emask
                        } else {
                            (ea >> ((-shift_amt) as u32 % ebits as u32)) & emask
                        }
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
            let emask: u128 = if esize >= 16 {
                u128::MAX
            } else {
                (1u128 << ebits) - 1
            };
            let a = self.regs.v[rn];
            let mut result: u128 = 0;
            for i in 0..(bytes / esize) {
                let shift = i * ebits;
                let ea = (a >> shift) & emask;
                let sign = ea >> (ebits - 1);
                let er = match (u, opcode) {
                    (0, 8) => {
                        if sign == 0 && ea != 0 {
                            emask
                        } else {
                            0
                        }
                    }
                    (0, 9) => {
                        if ea == 0 {
                            emask
                        } else {
                            0
                        }
                    }
                    (0, 10) => {
                        if sign != 0 {
                            emask
                        } else {
                            0
                        }
                    }
                    (1, 8) => {
                        if sign == 0 {
                            emask
                        } else {
                            0
                        }
                    }
                    (1, 9) => {
                        if sign != 0 || ea == 0 {
                            emask
                        } else {
                            0
                        }
                    }
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
                    (1, 5) if size == 0 => (!ea) & emask,        // NOT_v (size=00)
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
                    // REV (byte reverse per element)
                    (0, 0) if size < 3 => {
                        let mut rev = 0u128;
                        for b in 0..esize {
                            let byte = (ea >> (b * 8)) & 0xFF;
                            rev |= byte << ((esize - 1 - b) * 8);
                        }
                        rev & emask
                    }
                    // CLS (count leading sign bits)
                    (0, 4) => {
                        let sa = ea as i128 - if sign != 0 { 1i128 << ebits } else { 0 };
                        let leading = if sa >= 0 {
                            (ea << (128 - ebits)).leading_zeros() as u128
                        } else {
                            ((!ea & emask) << (128 - ebits)).leading_zeros() as u128
                        };
                        (leading.saturating_sub(1)) & emask
                    }
                    // CLZ (count leading zeros)
                    (1, 4) => {
                        let lz = if ea == 0 {
                            ebits as u128
                        } else {
                            (ea << (128 - ebits)).leading_zeros() as u128
                        };
                        lz & emask
                    }
                    // XTN / SQXTN (narrow — simplified to truncation)
                    (0, 18) | (1, 18) => ea & emask,
                    // SHLL (shift left long) — size determines shift
                    (1, 19) => (ea << ebits) & emask,
                    // SCVTF / UCVTF integer to FP vector
                    (0, 29) | (1, 29) if size >= 2 => {
                        if size == 2 {
                            let ival = if u == 0 {
                                let s = ea as i32;
                                s as f32
                            } else {
                                ea as u32 as f32
                            };
                            (ival.to_bits() as u128) & emask
                        } else {
                            let ival = if u == 0 {
                                let s = ea as i64;
                                s as f64
                            } else {
                                ea as u64 as f64
                            };
                            (ival.to_bits() as u128) & emask
                        }
                    }
                    // FCVTZS / FCVTZU FP to integer vector (round toward zero)
                    (0, 27) | (1, 27) if size >= 2 => {
                        if size == 2 {
                            let f = f32::from_bits(ea as u32);
                            let ival = if u == 0 { f as i32 as u32 } else { f as u32 };
                            (ival as u128) & emask
                        } else {
                            let f = f64::from_bits(ea as u64);
                            let ival = if u == 0 { f as i64 as u64 } else { f as u64 };
                            (ival as u128) & emask
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
            let src_esize = if immh & 8 != 0 {
                64usize
            } else if immh & 4 != 0 {
                32
            } else if immh & 2 != 0 {
                16
            } else {
                8
            };

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
            let emask: u128 = if esize >= 128 {
                u128::MAX
            } else {
                (1u128 << esize) - 1
            };
            let bytes = if q == 1 { 16usize } else { 8 };
            let a = self.regs.v[rn];
            let mut result: u128 = 0;
            for i in 0..(bytes * 8 / esize) {
                let bit_shift = i * esize;
                let ea = (a >> bit_shift) & emask;
                let er = match (u, opcode) {
                    (1, 0b00000) => {
                        let amt = esize * 2 - shift_val;
                        if amt >= esize {
                            0
                        } else {
                            (ea >> amt) & emask
                        }
                    }
                    (0, 0b00000) => {
                        let amt = esize * 2 - shift_val;
                        if amt >= esize {
                            if ea >> (esize - 1) != 0 {
                                emask
                            } else {
                                0
                            }
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
                    // SSRA / USRA (shift right and accumulate)
                    (0, 0b00010) | (1, 0b00010) => {
                        let amt = esize * 2 - shift_val;
                        let shifted = if u == 0 {
                            let sign_bit = ea >> (esize - 1);
                            let s = ea >> amt.min(esize - 1);
                            if sign_bit != 0 && amt < esize {
                                (s | (emask << (esize - amt))) & emask
                            } else {
                                s & emask
                            }
                        } else {
                            if amt >= esize {
                                0
                            } else {
                                (ea >> amt) & emask
                            }
                        };
                        let d = (self.regs.v[rd] >> bit_shift) & emask;
                        d.wrapping_add(shifted) & emask
                    }
                    // SRSHR / URSHR (rounding shift right) — simplified to non-rounding
                    (0, 0b00100) | (1, 0b00100) => {
                        let amt = esize * 2 - shift_val;
                        if amt >= esize {
                            0
                        } else {
                            (ea >> amt) & emask
                        }
                    }
                    // SRSRA / URSRA (rounding shift right + accumulate) — simplified
                    (0, 0b00110) | (1, 0b00110) => {
                        let amt = esize * 2 - shift_val;
                        let shifted = if amt >= esize { 0 } else { (ea >> amt) & emask };
                        let d = (self.regs.v[rd] >> bit_shift) & emask;
                        d.wrapping_add(shifted) & emask
                    }
                    // SRI (shift right and insert)
                    (1, 0b01000) => {
                        let amt = esize * 2 - shift_val;
                        let d = (self.regs.v[rd] >> bit_shift) & emask;
                        if amt >= esize {
                            d
                        } else {
                            let mask_hi = emask << (esize - amt) & emask;
                            (d & mask_hi) | ((ea >> amt) & !mask_hi & emask)
                        }
                    }
                    // SLI (shift left and insert)
                    (1, 0b01011) => {
                        let amt = shift_val - esize;
                        let d = (self.regs.v[rd] >> bit_shift) & emask;
                        let mask_lo = if amt == 0 { 0 } else { (1u128 << amt) - 1 };
                        (d & mask_lo) | ((ea << amt) & emask)
                    }
                    // SQSHL / UQSHL (saturating shift left imm) — simplified
                    (0, 0b01110) | (1, 0b01110) => {
                        let amt = shift_val - esize;
                        (ea << amt) & emask
                    }
                    // SQSHRN / UQSHRN / SQSHRUN (narrowing shift) — simplified
                    (0, 0b10010) | (1, 0b10010) | (0, 0b10000) => {
                        let amt = esize * 2 - shift_val;
                        if amt >= esize {
                            0
                        } else {
                            (ea >> amt) & emask
                        }
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
            let emask: u128 = if esize >= 128 {
                u128::MAX
            } else {
                (1u128 << esize) - 1
            };
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
            let emask: u128 = if esize >= 16 {
                u128::MAX
            } else {
                (1u128 << ebits) - 1
            };
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
                // EXT: opcode=0 handled separately via different encoding
                // remaining opcodes are TRN variants or reserved — treat as NOP
                _ => {
                    result = a; // fallback: pass through
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
                    } else {
                        raw
                    }
                };
                let shift = (src_start + i * src_ebits) as u32;
                let eb_raw = b.wrapping_shr(shift) & src_mask;
                let eb = if u == 0
                    && src_ebits > 0
                    && eb_raw.wrapping_shr((src_ebits - 1) as u32) != 0
                {
                    eb_raw | (dst_mask & !src_mask)
                } else {
                    eb_raw
                };
                let er = match opcode >> 1 {
                    0 => ea.wrapping_add(eb) & dst_mask,
                    1 => ea.wrapping_sub(eb) & dst_mask,
                    2 => ea.wrapping_add(eb) & dst_mask,
                    3 => ea.wrapping_sub(eb) & dst_mask,
                    5 => ea.wrapping_mul(eb) & dst_mask,
                    4 => ea.wrapping_add(eb) & dst_mask,
                    6 => ea.wrapping_sub(eb) & dst_mask,
                    7 => ea.wrapping_add(ea.wrapping_mul(eb) & dst_mask) & dst_mask,
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
                let exp = ((!(imm8 >> 6) & 1) << 7)
                    | (if (imm8 >> 6) & 1 != 0 { 0x7C } else { 0 })
                    | ((imm8 >> 4) & 0x3);
                let frac = ((imm8 & 0xF) as u32) << 19;
                let bits = ((sign as u32) << 31) | ((exp as u32) << 23) | frac as u32;
                self.regs.v[rd] = bits as u128;
            } else if ftype == 1 {
                let sign = ((imm8 >> 7) & 1) as u64;
                let exp6 = (imm8 >> 6) & 1;
                let exp = ((((!exp6) & 1) as u64) << 10)
                    | (if exp6 != 0 { 0x3FCu64 } else { 0u64 })
                    | (((imm8 >> 4) & 0x3) as u64);
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
                    0 => a * b,    // FMUL
                    1 => a / b,    // FDIV
                    2 => a + b,    // FADD
                    3 => a - b,    // FSUB
                    4 => a.max(b), // FMAX
                    5 => a.min(b), // FMIN
                    6 => a.max(b), // FMAXNM (≈ FMAX for non-NaN)
                    7 => a.min(b), // FMINNM (≈ FMIN for non-NaN)
                    8 => -(a * b), // FNMUL
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
                    6 => a.max(b), // FMAXNM
                    7 => a.min(b), // FMINNM
                    8 => -(a * b), // FNMUL
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
                6 => {
                    // FRINTN (round to nearest, ties to even)
                    if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        self.regs.v[rd] = f.round_ties_even().to_bits() as u128;
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        self.regs.v[rd] = f.round_ties_even().to_bits() as u128;
                    }
                }
                7 => {
                    // FRINTP (round toward +inf)
                    if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        self.regs.v[rd] = f.ceil().to_bits() as u128;
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        self.regs.v[rd] = f.ceil().to_bits() as u128;
                    }
                }
                8 => {
                    // FRINTM (round toward -inf)
                    if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        self.regs.v[rd] = f.floor().to_bits() as u128;
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        self.regs.v[rd] = f.floor().to_bits() as u128;
                    }
                }
                9 => {
                    // FRINTZ (round toward zero)
                    if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        self.regs.v[rd] = f.trunc().to_bits() as u128;
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        self.regs.v[rd] = f.trunc().to_bits() as u128;
                    }
                }
                10 => {
                    // FRINTA (round to nearest, ties away)
                    if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        self.regs.v[rd] = f.round().to_bits() as u128;
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        self.regs.v[rd] = f.round().to_bits() as u128;
                    }
                }
                14 => {
                    // FRINTX (round to current mode, signal inexact) — use round
                    if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        self.regs.v[rd] = f.round_ties_even().to_bits() as u128;
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        self.regs.v[rd] = f.round_ties_even().to_bits() as u128;
                    }
                }
                15 => {
                    // FRINTI (round to current mode) — use round-to-nearest
                    if ftype == 0 {
                        let f = f32::from_bits(self.regs.v[rn] as u32);
                        self.regs.v[rd] = f.round_ties_even().to_bits() as u128;
                    } else {
                        let f = f64::from_bits(self.regs.v[rn] as u64);
                        self.regs.v[rd] = f.round_ties_even().to_bits() as u128;
                    }
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
                let b = if opc & 1 == 1 {
                    0.0f32
                } else {
                    f32::from_bits(self.regs.v[rm] as u32)
                };
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
                let b = if opc & 1 == 1 {
                    0.0f64
                } else {
                    f64::from_bits(self.regs.v[rm] as u64)
                };
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
            let nzcv = ((n_val as u32) << 3)
                | ((z_val as u32) << 2)
                | ((c_val as u32) << 1)
                | (v_val as u32);
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

/// CRC32 (ISO 3309) one byte: polynomial 0x04C11DB7
fn crc32_byte(crc: u32, byte: u8) -> u32 {
    let mut c = crc ^ (byte as u32);
    for _ in 0..8 {
        c = if c & 1 != 0 {
            (c >> 1) ^ 0xEDB8_8320
        } else {
            c >> 1
        };
    }
    c
}

/// CRC32C (Castagnoli) one byte: reflected polynomial 0x82F63B78
fn crc32c_byte(crc: u32, byte: u8) -> u32 {
    let mut c = crc ^ (byte as u32);
    for _ in 0..8 {
        c = if c & 1 != 0 {
            (c >> 1) ^ 0x82F6_3B78
        } else {
            c >> 1
        };
    }
    c
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

fn rd(mem: &mut impl ExecMem, addr: Addr, sz: usize) -> HelmResult<u64> {
    let mut b = [0u8; 8];
    mem.read_bytes(addr, &mut b[..sz])?;
    Ok(match sz {
        1 => b[0] as u64,
        2 => u16::from_le_bytes([b[0], b[1]]) as u64,
        4 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64,
        8 => u64::from_le_bytes(b),
        _ => 0,
    })
}
fn wr(mem: &mut impl ExecMem, addr: Addr, val: u64, sz: usize) -> HelmResult<()> {
    mem.write_bytes(addr, &val.to_le_bytes()[..sz])
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

fn rd128(mem: &mut impl ExecMem, addr: Addr) -> HelmResult<u128> {
    let mut b = [0u8; 16];
    mem.read_bytes(addr, &mut b)?;
    Ok(u128::from_le_bytes(b))
}
fn wr128(mem: &mut impl ExecMem, addr: Addr, val: u128) -> HelmResult<()> {
    mem.write_bytes(addr, &val.to_le_bytes())
}
include!(concat!(env!("OUT_DIR"), "/decode_a64.rs"));
