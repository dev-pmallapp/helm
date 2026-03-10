//! x86-64 register map for the TCG flat register array.

pub const RAX: u16 = 0;
pub const RCX: u16 = 1;
pub const RDX: u16 = 2;
pub const RBX: u16 = 3;
pub const RSP: u16 = 4;
pub const RBP: u16 = 5;
pub const RSI: u16 = 6;
pub const RDI: u16 = 7;
pub const R8: u16 = 8;
pub const R9: u16 = 9;
pub const R10: u16 = 10;
pub const R11: u16 = 11;
pub const R12: u16 = 12;
pub const R13: u16 = 13;
pub const R14: u16 = 14;
pub const R15: u16 = 15;

/// Instruction pointer.
pub const RIP: u16 = 16;
/// Flags register (RFLAGS).
pub const RFLAGS: u16 = 17;
/// Segment selectors (CS, DS, ES, FS, GS, SS).
pub const CS: u16 = 18;
pub const DS: u16 = 19;
pub const ES: u16 = 20;
pub const FS: u16 = 21;
pub const GS: u16 = 22;
pub const SS: u16 = 23;
/// CR0 — control register 0 (PE, PG, etc.).
pub const CR0: u16 = 24;
/// CR3 — page table base.
pub const CR3: u16 = 25;
/// CR4 — extended control.
pub const CR4: u16 = 26;
/// EFER — extended feature enable register.
pub const EFER: u16 = 27;

/// Total number of register slots for x86-64.
pub const NUM_REGS: usize = 28;
