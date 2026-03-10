//! Executor trait implementation for AArch64.
//!
//! Wraps the existing `Aarch64Cpu::step()` behind the `Executor` trait
//! interface, using `TraitMemBridge` to adapt `dyn MemoryAccess` to
//! the byte-oriented `ExecMem` interface.

use crate::arm::aarch64::exec::Aarch64Cpu;
use crate::arm::aarch64::mem_bridge::TraitMemBridge;
use helm_core::cpu::CpuState;
use helm_core::exec::Executor;
use helm_core::insn::{DecodedInsn, ExceptionInfo, ExecOutcome, InsnClass, MemAccessInfo};
use helm_core::mem::MemoryAccess;

/// AArch64 executor that wraps the existing `Aarch64Cpu::step()`.
///
/// Owns an internal `Aarch64Cpu`. On each `execute()` call:
/// 1. Syncs state from `dyn CpuState` → internal CPU
/// 2. Executes one instruction via `step()` with `TraitMemBridge`
/// 3. Syncs state back to `dyn CpuState`
/// 4. Converts `StepTrace` → `ExecOutcome`
pub struct Aarch64TraitExecutor {
    cpu: Aarch64Cpu,
}

impl Aarch64TraitExecutor {
    pub fn new() -> Self {
        let mut cpu = Aarch64Cpu::new();
        cpu.set_se_mode(true);
        Self { cpu }
    }

    /// Access the internal CPU (e.g. for setting se_mode, irq_signal).
    pub fn inner(&self) -> &Aarch64Cpu {
        &self.cpu
    }

    pub fn inner_mut(&mut self) -> &mut Aarch64Cpu {
        &mut self.cpu
    }

    /// Sync register state from CpuState trait to internal Aarch64Cpu.
    fn sync_from(&mut self, cpu: &dyn CpuState) {
        self.cpu.regs.pc = cpu.pc();
        for i in 0..31u16 {
            self.cpu.regs.x[i as usize] = cpu.gpr(i);
        }
        self.cpu.regs.sp = cpu.gpr(31);

        let flags = cpu.flags();
        self.cpu.regs.nzcv = ((flags >> 28) as u32) << 28;
        self.cpu.regs.daif = ((flags >> 6) & 0xF) as u32;
        self.cpu.regs.current_el = ((flags >> 2) & 3) as u8;
        self.cpu.regs.sp_sel = (flags & 1) as u8;
    }

    /// Sync register state from internal Aarch64Cpu to CpuState trait.
    fn sync_to(&self, cpu: &mut dyn CpuState) {
        cpu.set_pc(self.cpu.regs.pc);
        for i in 0..31u16 {
            cpu.set_gpr(i, self.cpu.regs.x[i as usize]);
        }
        cpu.set_gpr(31, self.cpu.regs.sp);

        let nzcv = (self.cpu.regs.nzcv as u64) & 0xF000_0000;
        let daif = ((self.cpu.regs.daif as u64) & 0xF) << 6;
        let el = ((self.cpu.regs.current_el as u64) & 3) << 2;
        let sp = (self.cpu.regs.sp_sel as u64) & 1;
        cpu.set_flags(nzcv | daif | el | sp);
    }
}

impl Default for Aarch64TraitExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor for Aarch64TraitExecutor {
    fn execute(
        &mut self,
        _insn: &DecodedInsn,
        cpu: &mut dyn CpuState,
        mem: &mut dyn MemoryAccess,
    ) -> ExecOutcome {
        // Sync state from trait to internal CPU
        self.sync_from(cpu);

        // Execute one instruction through the existing step() path
        let mut bridge = TraitMemBridge(mem);
        let result = self.cpu.step(&mut bridge);

        // Sync state back
        self.sync_to(cpu);

        match result {
            Ok(trace) => {
                let mut outcome = ExecOutcome {
                    next_pc: self.cpu.regs.pc,
                    branch_taken: trace.branch_taken.unwrap_or(false),
                    ..ExecOutcome::default()
                };

                // Convert memory accesses
                for (i, ma) in trace.mem_accesses.iter().enumerate().take(2) {
                    outcome.mem_accesses[i] = MemAccessInfo {
                        addr: ma.addr,
                        size: ma.size as u8,
                        is_write: ma.is_write,
                    };
                    outcome.mem_access_count = (i + 1) as u8;
                }

                outcome
            }
            Err(helm_core::HelmError::Syscall { number, .. }) => {
                // SVC in SE mode — signal as exception for syscall handling
                ExecOutcome {
                    next_pc: self.cpu.regs.pc,
                    exception: Some(ExceptionInfo {
                        class: 0x15, // EC for SVC from AArch64
                        iss: number as u32,
                        vaddr: 0,
                        target_el: 1,
                    }),
                    ..ExecOutcome::default()
                }
            }
            Err(_) => {
                // Other errors (decode error, memory fault, etc.)
                ExecOutcome {
                    next_pc: self.cpu.regs.pc,
                    exception: Some(ExceptionInfo {
                        class: 0,
                        iss: 0,
                        vaddr: 0,
                        target_el: 0,
                    }),
                    ..ExecOutcome::default()
                }
            }
        }
    }
}
