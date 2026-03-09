# State Sync

Functions that synchronise CPU state between representations.

## regs_to_array

Copies `Aarch64Regs` fields into the flat register array used by the
TCG interpreter:

- X0–X30 → array[0..31]
- SP → array[31]
- PC → array[32]
- NZCV → array[33]
- DAIF, ELR_EL1, SPSR_EL1, etc. → subsequent indices.

Also copies hot sysregs into the `TcgInterp::sysregs` array.

## array_to_regs

Inverse of `regs_to_array`: copies the register array back into
`Aarch64Regs` after TCG execution.

## sync_mmu_to_cpu

Ensures MMU-related registers (TCR, TTBR0/1, SCTLR, MAIR) are
current in `Aarch64Regs` before a page-table walk.

## sync_sysregs_*

Per-subsystem sync helpers for timer registers, debug registers,
and exception state.

## When Sync Happens

- **Before TCG block**: `regs_to_array`.
- **After TCG block**: `array_to_regs`.
- **Before timer check**: sync timer sysregs.
- **Before MMU walk**: sync translation registers.
- **Before IRQ check**: sync DAIF.
