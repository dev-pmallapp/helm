//! RISC-V 64-bit register map for the TCG flat register array.

/// Integer registers x0–x31 occupy slots 0–31.
/// x0 is hardwired to zero (the emitter should handle this).
pub const X0: u16 = 0;
pub const RA: u16 = 1; // x1 = return address
pub const SP: u16 = 2; // x2 = stack pointer
pub const GP: u16 = 3; // x3 = global pointer
pub const TP: u16 = 4; // x4 = thread pointer
pub const X31: u16 = 31;

/// Program counter.
pub const PC: u16 = 32;

/// Machine status register (mstatus).
pub const MSTATUS: u16 = 33;
/// Machine exception program counter.
pub const MEPC: u16 = 34;
/// Machine cause register.
pub const MCAUSE: u16 = 35;
/// Machine trap value.
pub const MTVAL: u16 = 36;
/// Supervisor status register.
pub const SSTATUS: u16 = 37;
/// Supervisor exception program counter.
pub const SEPC: u16 = 38;
/// Supervisor cause.
pub const SCAUSE: u16 = 39;
/// Supervisor trap value.
pub const STVAL: u16 = 40;
/// Supervisor trap vector base.
pub const STVEC: u16 = 41;
/// Supervisor address translation and protection.
pub const SATP: u16 = 42;

/// Total number of register slots for RISC-V 64.
pub const NUM_REGS: usize = 43;
