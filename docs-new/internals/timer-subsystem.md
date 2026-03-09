# Timer Subsystem

AArch64 generic timer implementation.

## Registers

| Register | Description |
|----------|-------------|
| `CNTFRQ_EL0` | Counter frequency (default 62.5 MHz) |
| `CNTVCT_EL0` | Virtual counter value |
| `CNTV_CTL_EL0` | Virtual timer control (ENABLE, IMASK, ISTATUS) |
| `CNTV_CVAL_EL0` | Virtual timer compare value |
| `CNTP_CTL_EL0` | Physical timer control |
| `CNTP_CVAL_EL0` | Physical timer compare value |
| `CNTKCTL_EL1` | Kernel timer control (EL0 access enable) |

## Timer Checking

The FS session checks timers periodically (every N instructions):

1. Increment `CNTVCT_EL0` by the number of instructions executed.
2. For each timer (virtual and physical):
   - If ENABLE is set and IMASK is clear:
   - Compare `CNTVCT_EL0 ≥ CVAL`.
   - If condition met, set ISTATUS and raise the GIC timer IRQ.

## GIC Integration

Timer interrupts are routed through the GIC:
- Virtual timer → PPI 27 (IRQ 27).
- Physical timer → PPI 30 (IRQ 30).

The timer asserts the IRQ via `IrqSignal::raise()`. The CPU checks
this signal at block boundaries.

## Sysreg Sync

Timer registers exist in both `Aarch64Regs` and the TCG interpreter's
sysreg array. Sync must happen at block boundaries to avoid stale
values. See [sysreg-sync.md](sysreg-sync.md).
