# Exception Delivery

How exceptions are taken and returned in `Aarch64Cpu`.

## take_exception

When a synchronous or asynchronous exception is detected:

1. Determine the target exception level (EL1, EL2, or EL3).
2. Save `PSTATE` → `SPSR_ELx` (packs EL, SP, DAIF, NZCV).
3. Save `PC` → `ELR_ELx`.
4. Write `ESR_ELx` with the exception class (EC) and instruction-
   specific syndrome (ISS).
5. For data/instruction aborts: write `FAR_ELx` with the faulting VA.
6. Mask interrupts: set DAIF.{D,A,I,F} as appropriate.
7. Set `SP` to `SP_ELx`.
8. Branch to `VBAR_ELx + offset`:

| Offset | Exception | Source |
|--------|-----------|--------|
| 0x000 | Synchronous | Current EL, SP_EL0 |
| 0x080 | IRQ | Current EL, SP_EL0 |
| 0x200 | Synchronous | Current EL, SP_ELx |
| 0x280 | IRQ | Current EL, SP_ELx |
| 0x400 | Synchronous | Lower EL, AArch64 |
| 0x480 | IRQ | Lower EL, AArch64 |
| 0x500 | FIQ | Lower EL, AArch64 |
| 0x580 | SError | Lower EL, AArch64 |

## check_irq

At the top of `step()` (or at TCG block boundaries):

1. Check `IrqSignal::is_raised()`.
2. If DAIF.I is clear (interrupts enabled), take an IRQ exception
   with the appropriate vector offset.

## ERET

The `ERET` instruction:

1. Read `SPSR_ELx` → extract target EL, SP selection, DAIF, NZCV.
2. Set `current_el`, `sp_sel`, `daif`, `nzcv` from restored PSTATE.
3. Branch to `ELR_ELx`.

## SPSR Packing

SPSR layout (bits):
- [3:0] — M field (exception level + AArch state).
- [6] — FIQ mask.
- [7] — IRQ mask.
- [8] — SError mask.
- [9] — Debug mask.
- [31:28] — NZCV flags.
