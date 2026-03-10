# Sysreg Sync

The dual-state problem: CPU registers vs interpreter sysreg array.

## The Problem

In the TCG path, guest architectural state exists in two places:

1. **`Aarch64Regs`** — named struct fields in the CPU.
2. **`TcgInterp::sysregs`** — flat `Vec<u64>` indexed by 15-bit
   sysreg encoding.

The TCG interpreter reads/writes the sysreg array directly. The
FS session's outer loop (IRQ checking, timer updates, MMU operations)
reads/writes `Aarch64Regs`. These two copies must stay synchronised.

## Sync Points

Synchronisation happens at block boundaries:

1. **Before TCG execution** (`regs_to_array`):
   - Copy GP registers (X0–X30, SP, PC, NZCV) from `Aarch64Regs`
     to the register array.
   - Copy hot sysregs (SCTLR_EL1, VBAR_EL1, ELR_EL1, etc.) from
     `Aarch64Regs` to the sysreg array.

2. **After TCG execution** (`array_to_regs`):
   - Copy GP registers back from the array to `Aarch64Regs`.
   - Copy modified sysregs back.

## Hot Sysregs

Only ~30 system registers are frequently accessed. These are synced
explicitly. The remaining 32K entries in the sysreg array are cold
and only synced on demand (MSR/MRS execution).

## Pitfalls

- **Timer registers** must be synced before checking timer conditions
  in the outer loop.
- **MMU registers** (TCR, TTBR0/1, MAIR) must be synced before any
  page-table walk.
- **DAIF** must be synced before IRQ checking.
- The JIT path has the same sync requirements, handled via helper
  function callbacks.
