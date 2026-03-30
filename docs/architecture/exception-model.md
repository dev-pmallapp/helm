# Exception Model

How helm-ng handles AArch64 exceptions — levels, entry, return, and
interrupt delivery.

## AArch64 Exception Levels

ARM defines four exception levels (EL0–EL3). helm-ng implements EL0
and EL1 for Linux workloads:

| Level | Typical Use | helm-ng Status |
|-------|-------------|----------------|
| EL0 | User-space applications | Fully implemented |
| EL1 | OS kernel | Fully implemented |
| EL2 | Hypervisor | Stub (HVC dispatches PSCI inline) |
| EL3 | Secure monitor | Stub (SMC dispatches PSCI inline) |

## Exception Entry

When an exception occurs (synchronous fault, SVC, or asynchronous
IRQ), `helm-arch::aarch64::exception` handles the transition:

```text
1. Save PSTATE → SPSR_EL1
2. Save PC → ELR_EL1
3. Set PSTATE.{DAIF} masking bits
4. Look up vector base: VBAR_EL1 + offset
   ├── Offset depends on: current EL, SP selection, exception type
   └── Four vector groups × four exception types = 16 vectors
5. Branch to vector address
```

The vector offset calculation follows the ARMv8 VBAR layout:

| Source | SP_EL0 vectors | SP_ELx vectors |
|--------|---------------|----------------|
| Same EL | +0x000 (sync), +0x080 (IRQ), +0x100 (FIQ), +0x180 (SError) | +0x200, +0x280, +0x300, +0x380 |
| Lower EL (AArch64) | +0x400, +0x480, +0x500, +0x580 | — |
| Lower EL (AArch32) | +0x600, +0x680, +0x700, +0x780 | — |

## Exception Return

`ERET` restores execution state:

```text
1. Read SPSR_EL1 → restore PSTATE (NZCV, DAIF, mode)
2. Read ELR_EL1 → restore PC
3. Resume execution at restored PC with restored PSTATE
```

## IRQ Delivery (FS Mode)

In full-system mode, `step_aarch64_fs()` checks for pending IRQs
at the start of each instruction:

```text
if irq_pending && !PSTATE.I (IRQ not masked):
    1. Write ESR_EL1 with IRQ syndrome
    2. Exception entry (save SPSR/ELR, jump to VBAR+0x280)
    3. Clear irq_pending
```

The `irq_pending` flag is set by the GIC CPU interface when an
interrupt reaches sufficient priority. The flag lives in `FsState`
and is polled on every step — no asynchronous delivery.

## PSCI (Power State Coordination Interface)

HVC and SMC instructions in `branch.rs` dispatch PSCI function IDs
inline rather than raising exceptions:

| Function ID | Behavior |
|-------------|----------|
| `PSCI_VERSION` | Returns version (0.2) |
| `CPU_ON` | Schedules secondary CPU start |
| `CPU_OFF` | Stops current CPU |
| `SYSTEM_RESET` | Triggers simulation reset |
| `SYSTEM_OFF` | Triggers simulation exit |

This avoids the need for a full EL2/EL3 firmware implementation.

## Syscall Handling (SE Mode)

In syscall emulation mode, `SVC #0` is intercepted by the engine
before it reaches exception.rs. The `SyscallHandler` trait routes
the call to `LinuxAarch64SyscallHandler` or
`LinuxRiscv64SyscallHandler`, which emulate Linux syscalls on the
host.

## System Registers

`sysreg.rs` handles ~40 system registers via `read_sysreg` and
`write_sysreg`:

| Category | Registers |
|----------|-----------|
| Exception control | ELR_EL1, SPSR_EL1, ESR_EL1, FAR_EL1, VBAR_EL1 |
| Stack pointers | SP_EL0, SP_EL1 |
| Translation | SCTLR_EL1, TCR_EL1, TTBR0_EL1, TTBR1_EL1, MAIR_EL1 |
| FP/SIMD control | CPACR_EL1, FPCR, FPSR |
| Timers | CNTFRQ_EL0, CNTVCT_EL0, CNTP_CTL_EL0, CNTP_CVAL_EL0 |
| Performance | PMCR_EL0, PMCCNTR_EL0 (stubs) |
| Cache/TLB ID | CTR_EL0, DCZID_EL0, CCSIDR_EL1, CLIDR_EL1 |
| CPU identification | MIDR_EL1, MPIDR_EL1, REVIDR_EL1 |
| Feature ID | ID_AA64ISAR0/1_EL1, ID_AA64MMFR0/1_EL1, ID_AA64PFR0_EL1 |

## Comparison

| Aspect | QEMU | gem5 | helm-ng |
|--------|------|------|---------|
| Exception levels | EL0–EL3 full | EL0–EL1 typical | EL0–EL1, EL2/3 stubs |
| IRQ delivery | Check at TB boundaries | Event-driven | Per-instruction poll |
| PSCI | Firmware emulation | Optional firmware | Inline stubs |
| Sysreg count | ~200 | ~150 | ~40 (growing) |
