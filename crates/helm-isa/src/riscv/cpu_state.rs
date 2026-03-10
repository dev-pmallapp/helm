//! RISC-V 64-bit CPU state implementing CpuState trait.

use helm_core::cpu::CpuState;
use helm_core::types::{Addr, RegId};

/// RV64 architectural state: 32 integer regs + PC + CSRs.
pub struct Rv64CpuState {
    pub pc: Addr,
    /// x0 is always zero; x1-x31 are general-purpose.
    pub x: [u64; 32],
    /// Privilege mode: 0=User, 1=Supervisor, 3=Machine.
    pub priv_mode: u8,
    /// mstatus register.
    pub mstatus: u64,
    /// Machine-mode trap vector.
    pub mtvec: u64,
    pub mepc: u64,
    pub mcause: u64,
    pub mtval: u64,
    pub mscratch: u64,
    /// Supervisor-mode registers.
    pub stvec: u64,
    pub sepc: u64,
    pub scause: u64,
    pub stval: u64,
    pub sscratch: u64,
    pub satp: u64,
}

impl Rv64CpuState {
    pub fn new() -> Self {
        Self {
            pc: 0,
            x: [0; 32],
            priv_mode: 3, // start in Machine mode
            mstatus: 0,
            mtvec: 0,
            mepc: 0,
            mcause: 0,
            mtval: 0,
            mscratch: 0,
            stvec: 0,
            sepc: 0,
            scause: 0,
            stval: 0,
            sscratch: 0,
            satp: 0,
        }
    }
}

impl Default for Rv64CpuState {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuState for Rv64CpuState {
    fn pc(&self) -> Addr {
        self.pc
    }

    fn set_pc(&mut self, pc: Addr) {
        self.pc = pc;
    }

    /// x0 always reads as zero.
    fn gpr(&self, id: RegId) -> u64 {
        if id == 0 { 0 } else { self.x[id as usize & 31] }
    }

    /// x0 writes are silently dropped.
    fn set_gpr(&mut self, id: RegId, val: u64) {
        if id != 0 {
            self.x[id as usize & 31] = val;
        }
    }

    fn sysreg(&self, enc: u32) -> u64 {
        match enc {
            0x300 => self.mstatus,
            0x305 => self.mtvec,
            0x341 => self.mepc,
            0x342 => self.mcause,
            0x343 => self.mtval,
            0x340 => self.mscratch,
            0x105 => self.stvec,
            0x141 => self.sepc,
            0x142 => self.scause,
            0x143 => self.stval,
            0x140 => self.sscratch,
            0x180 => self.satp,
            _ => 0,
        }
    }

    fn set_sysreg(&mut self, enc: u32, val: u64) {
        match enc {
            0x300 => self.mstatus = val,
            0x305 => self.mtvec = val,
            0x341 => self.mepc = val,
            0x342 => self.mcause = val,
            0x343 => self.mtval = val,
            0x340 => self.mscratch = val,
            0x105 => self.stvec = val,
            0x141 => self.sepc = val,
            0x142 => self.scause = val,
            0x143 => self.stval = val,
            0x140 => self.sscratch = val,
            0x180 => self.satp = val,
            _ => {}
        }
    }

    fn flags(&self) -> u64 {
        self.mstatus
    }

    fn set_flags(&mut self, flags: u64) {
        self.mstatus = flags;
    }

    fn privilege_level(&self) -> u8 {
        self.priv_mode
    }
}
