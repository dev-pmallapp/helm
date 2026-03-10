//! AArch64 register map for the TCG flat register array.
//!
//! Maps architectural registers to slots in a `[u64; NUM_REGS]` array
//! used by the TCG interpreter. The slot assignments must match what
//! the A64 emitter uses in `ReadReg`/`WriteReg` ops.

/// General-purpose registers X0–X30 occupy slots 0–30.
pub const X0: u16 = 0;
pub const X30: u16 = 30;

/// Stack pointer (SP_EL0).
pub const SP: u16 = 31;
/// Program counter.
pub const PC: u16 = 32;
/// Condition flags (NZCV packed in bits [31:28]).
pub const NZCV: u16 = 33;
/// DAIF interrupt mask (bits [9:6]).
pub const DAIF: u16 = 34;
/// ELR_EL1 — exception link register.
pub const ELR_EL1: u16 = 35;
/// SPSR_EL1 — saved program status register.
pub const SPSR_EL1: u16 = 36;
/// ESR_EL1 — exception syndrome register.
pub const ESR_EL1: u16 = 37;
/// VBAR_EL1 — vector base address register.
pub const VBAR_EL1: u16 = 38;
/// CurrentEL — current exception level (stored << 2).
pub const CURRENT_EL: u16 = 39;
/// SPSel — stack pointer selection (0=SP_EL0, 1=SP_ELx).
pub const SPSEL: u16 = 40;
/// SP_EL1 — the kernel stack pointer.
pub const SP_EL1: u16 = 41;
/// TPIDR_EL0 — thread-local storage pointer (hot in user-space TLS).
/// Mirrored in the regs array so MRS/MSR TPIDR_EL0 avoid sysreg calls.
pub const TPIDR_EL0: u16 = 42;

/// Total number of register slots for AArch64.
pub const NUM_REGS: usize = 43;
