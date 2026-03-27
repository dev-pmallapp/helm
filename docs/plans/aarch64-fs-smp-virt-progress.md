# Plan: AArch64 FS SMP Virt Progress

> **Status:** Design pass started — 2026-03-21
> **Goal:** Define what must be in place for `helm-system-aarch64` to run an SMP `arm-virt` VM that actually makes forward progress
> **Reference points:** `runtime/helm-engine/src/lib.rs`, `runtime/helm-engine/src/platform/arm_virt.rs`, `hw/helm-hw-intc/src/gicv2/*`, and QEMU `../helm.git/assets/qemu/hw/arm/virt.c`
> **Completion gate:** Linux can bring up more than one CPU on the virt machine and continue running with working private interrupts and inter-CPU coordination

---

## Current State

helm-ng already has the outer shell of SMP FS support:

| Area | Current state |
|------|---------------|
| DTB examples | `examples/fs/boot_rpi_full.py` and `examples/fs/virt.py` accept `--smp` and emit multiple CPU nodes with `enable-method = "psci"` |
| Boot setup | `setup_arm_virt_boot_with_cpus()` creates one `Aarch64ArchState` + `FsState` per CPU |
| CPU power model | only vCPU0 starts `powered_on`; secondaries are off until PSCI `CPU_ON` |
| Engine scheduler | `step_aarch64_system()` round-robins `powered_on` vCPUs in one host thread |
| PSCI path | engine-side `handle_fs_psci_call()` implements `CPU_ON`, `CPU_OFF`, `AFFINITY_INFO`, `SYSTEM_OFF`, `SYSTEM_RESET`, and basic feature probing |
| GIC constructor | `build_gicv2_mp()` already returns per-CPU IRQ lines and per-CPU CPU-interface state |
| GICC access | the banked MMIO CPU interface follows `shared.active_cpu_idx`, so a single mapped GICC device can represent the currently stepped CPU |

This is enough to claim "SMP scaffolding exists".
It is not enough to claim "an SMP virt VM can make progress".

---

## Working Pieces

### 1. Secondary CPU bring-up path exists

`handle_fs_psci_call()` in `runtime/helm-engine/src/lib.rs` already powers on secondaries:

- locates the target CPU by `MPIDR_EL1`
- sets the target PC to the PSCI entry address
- writes the PSCI context value into `x0`
- marks the target vCPU `powered_on = true`

So the machine already has a real path from:

- DTB CPU node with `enable-method = "psci"`
- guest PSCI `CPU_ON`
- secondary vCPU becoming runnable in the engine

### 2. The engine is already multi-vCPU-shaped

`Aarch64FsMachine` already owns:

- `vcpus: Vec<Aarch64Vcpu>`
- `next_vcpu`
- `irq_lines: Vec<Arc<AtomicBool>>`
- shared GIC state

And `pick_next_fs_vcpu()` already performs round-robin selection over `powered_on` CPUs.

This means the engine does **not** need a full scheduler redesign just to start supporting SMP progress.

### 3. The GIC implementation is partially SMP-aware

`build_gicv2_mp()` already creates:

- one `GicCpuState` per CPU
- one IRQ line per CPU
- one CPU interface per CPU

So the interrupt controller already has the right outer data shape.

---

## Core Problem

The current FS SMP path is structurally multi-CPU, but functionally still mostly **UP semantics with multiple register files**.

That matters because Linux SMP progress depends less on "can I execute a second CPU?" and more on:

- can each CPU receive its own PPIs?
- can CPUs send each other SGIs/IPIs?
- can the engine observe the right IRQ line for the CPU it is currently stepping?

Right now, the answer is not yet yes.

---

## Main Blockers

## 1. The engine polls the wrong IRQ line

In `step_aarch64_system()`:

```rust
fs_state.irq_pending = machine.irq_lines
    .first()
    .map_or(false, |l| l.load(...));
```

That ignores `vcpu_idx` and always reads CPU0's line.

### Consequence

- vCPU1+ can be stepped, but their `irq_pending` flag is sourced from CPU0
- a private interrupt targeted at a secondary CPU can be completely invisible to the core that should take it
- a CPU may observe another CPU's interrupt state

### Required fix

Use `machine.irq_lines[vcpu_idx]`, not `.first()`.

This is the smallest and most immediate engine-level SMP correctness fix.

---

## 2. GIC private interrupt routing is still hardcoded to CPU0

In `hw/helm-hw-intc/src/gicv2/mod.rs`:

```rust
if irq < 32 {
    // Preserve current UP semantics until SGI/PPI routing is modeled.
    return cpu_idx == 0;
}
```

This makes all SGIs and PPIs effectively CPU0-only.

### Consequence

Linux SMP secondaries may be powered on, but they cannot correctly receive:

- per-CPU timer interrupts
- reschedule IPIs
- TLB shootdown IPIs
- other SGI-based cross-call traffic

That is a hard forward-progress blocker for real SMP kernel behavior.

### Required fix

Implement proper per-CPU routing semantics for:

- SGIs (`INTID 0..15`)
- PPIs (`INTID 16..31`)

This is the single most important SMP milestone after the IRQ-line polling fix.

---

## 3. GIC private interrupt state is not banked per CPU

`GicSharedState` currently stores:

- `enabled`
- `pending`
- `active`

as distributor-global arrays.

That is acceptable for SPIs.
It is not correct for SGIs/PPIs, which are CPU-private in GICv2.

### Consequence

Even if routing were fixed, SGI/PPI behavior would still be wrong because:

- one CPU's acknowledge can clear another CPU's pending private interrupt
- one CPU's active state can suppress another CPU's private interrupt
- enable bits for private interrupts are not independent per CPU

### Required fix

Split interrupt state by class:

- distributor-global state for SPIs
- banked per-CPU state for SGIs and PPIs

At minimum:

- per-CPU enabled mask for `0..31`
- per-CPU pending mask for `0..31`
- per-CPU active mask for `0..31`

Without this, SGI/PPI routing remains only cosmetically SMP-capable.

---

## 4. Timer PPIs are currently modeled with UP assumptions

`arm_virt::inject_timers()` writes PPI pending bits through GICD `ISPENDR` / `ICPENDR`.

That was the right fix for single-core timer storms.
For SMP it is incomplete because the PPI state is not banked.

### Consequence

- timer delivery semantics for secondaries are not correct
- Linux may bring up CPUs but fail once it expects local timer interrupts on each CPU

### Required fix

Once SGI/PPI state is banked:

- timer injection must target the currently stepped CPU's private interrupt state
- or the GIC model must expose an explicit helper for asserting CPU-local PPIs

The goal is not "poke a global pending bit", but "assert the private timer interrupt for this CPU".

---

## 5. The DTB timer description is not SMP-correct for GICv2

The Python-generated virt DTB uses:

```dts
interrupts = <1 13 4>, <1 14 4>, <1 11 4>, <1 10 4>;
```

QEMU adjusts GICv2 timer PPI flags with the CPU mask bits for SMP.

### Consequence

Even if the engine and GIC are fixed, the guest can still be told the wrong interrupt topology for timer PPIs on a GICv2 machine.

### Required fix

Match QEMU's GICv2 SMP behavior:

- include the PPI CPU mask bits in the timer interrupt flags
- generate those flags from `num_cpus`

This becomes cleaner once DTB generation moves into Rust.

---

## 6. SGI generation path is absent

I do not see a modeled SGI send path in the current GIC distributor implementation.
The distributor handles enables, pend/clear, priorities, targets, and config, but there is no visible `GICD_SGIR` handling or equivalent CPU-targeted SGI injection flow.

### Consequence

Linux SMP can boot a secondary CPU through PSCI and still fail later when it needs:

- reschedule IPIs
- TLB maintenance IPIs
- stop-machine style coordination

### Required fix

Implement at least the GICv2 SGI path Linux actually uses:

- software-generated interrupt write
- target list decoding
- per-target per-CPU SGI pending state

This is a correctness requirement, not an optimization.

---

## 7. There is almost no SMP-focused test coverage

Current explicit SMP-ish test coverage is essentially:

- `psci_cpu_on_powers_secondary_vcpu`

That proves only that a secondary CPU can be marked `powered_on`.
It does not prove:

- that a secondary can take an interrupt
- that a timer PPI can reach CPU1
- that SGIs are routed correctly
- that round-robin stepping plus banked GICC state works under load

### Required fix

Add narrow, deterministic tests before trying to boot a full SMP guest as the only validation method.

---

## What Does Not Need To Change First

### 1. The one-host-thread engine model

The current FS engine steps vCPUs sequentially in one host thread.

That is acceptable for now.
Linux SMP boot does **not** require host-parallel execution if:

- per-CPU state is preserved
- interrupts are routed correctly
- PSCI and timers work

QEMU's TCG also makes heavy use of serialized execution in many configurations.

### 2. Full GICv3 migration

The current blockers are not caused by lacking GICv3.
They are caused by incomplete SMP semantics in the existing GICv2 path.

Progress should come from making the current GICv2 path correct enough for SMP before jumping to redistributors and ITS.

### 3. Full virt-machine completeness

RTC, `fw_cfg`, flash, PCIe, and ACPI are useful virt-machine work.
They are not the first blockers to SMP forward progress.

The SMP-critical path is:

- CPU bring-up
- banked GICC/GICD semantics
- timers
- SGIs/IPIs

---

## Proposed Phases

## Phase 1: Fix the engine-side SMP wiring bug

### Work

- change IRQ polling in `step_aarch64_system()` to use `irq_lines[vcpu_idx]`
- add a regression test proving CPU1 does not observe CPU0's IRQ line

### Completion gate

- the currently stepped vCPU sees its own GIC line, not CPU0's

---

## Phase 2: Make GICv2 private interrupts truly per-CPU

### Work

- bank SGI/PPI enabled state per CPU
- bank SGI/PPI pending state per CPU
- bank SGI/PPI active state per CPU
- keep SPI state shared/global

### Completion gate

- CPU0 and CPU1 can each have independent PPI/SGI state
- acknowledging a private interrupt on one CPU does not clear another CPU's state

---

## Phase 3: Add SGI send and CPU-local timer delivery

### Work

- implement SGI generation path
- implement CPU-local PPI assertion for physical and virtual timers
- update timer injection to target the correct CPU-local state

### Completion gate

- a synthetic SGI sent to CPU1 is taken by CPU1
- a timer interrupt for CPU1 is taken by CPU1

---

## Phase 4: Fix the SMP DTB contract

### Work

- generate correct GICv2 timer PPI flags with CPU mask bits
- ensure CPU nodes, MPIDR values, and PSCI declaration match the SMP machine contract
- ideally move DTB generation into Rust so it cannot drift from the platform

### Completion gate

- the guest sees an SMP-correct virt DTB for the implemented GIC profile

---

## Phase 5: Boot and validate a real SMP guest

### Work

- boot Linux with `--smp 2`
- verify secondary CPU online path
- verify periodic timer interrupts on secondary
- verify basic IPI-driven coordination does not stall

### Completion gate

- Linux reports multiple CPUs online and continues making progress beyond early bring-up

---

## Test Plan

Add focused tests in this order:

1. engine test: `irq_lines[vcpu_idx]` polling uses the selected CPU
2. GIC unit test: CPU1 PPI pending state is independent from CPU0
3. GIC unit test: SGI to CPU1 is acknowledged only on CPU1
4. FS integration test: timer PPI reaches the currently stepped secondary CPU
5. FS integration test: PSCI `CPU_ON` + SGI/timer delivery on CPU1

Only after those should SMP Linux boot be treated as a milestone gate.

---

## File Map

Expected primary files:

| File | Change |
|------|--------|
| `runtime/helm-engine/src/lib.rs` | fix per-vCPU IRQ polling, add SMP integration tests |
| `runtime/helm-engine/src/platform/arm_virt.rs` | timer injection API may need to become CPU-local |
| `hw/helm-hw-intc/src/gicv2/mod.rs` | bank SGI/PPI state per CPU |
| `hw/helm-hw-intc/src/gicv2/distributor.rs` | SGI generation and per-CPU private interrupt semantics |
| `hw/helm-hw-intc/src/gicv2/cpu_interface.rs` | consume the corrected banked private interrupt state |
| `examples/fs/boot_rpi_full.py` | temporary SMP DTB fixes until DTB generation moves into Rust |
| `examples/fs/virt.py` | same temporary DTB fix path |

---

## Immediate Next Step

Start with **Phase 1 plus Phase 2**.

Reason:

- the current engine already knows how to step multiple vCPUs
- the GIC already has the right top-level SMP shape
- the first real progress comes from removing the remaining UP assumptions around private interrupts

Until that is done, an SMP virt VM may boot a second CPU, but it still does not have the interrupt semantics needed to keep that CPU useful.
