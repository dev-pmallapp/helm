# SMP Gap Analysis + Timing Model Interaction with Cooperative Scheduling

*Generated: 2026-03-22*

---

## Overview

This document covers two related topics:

1. **SMP capability gaps** — what exists vs. what is needed for a fully functional multi-hart Linux SMP boot
2. **Timing model mechanics** — how Virtual/Interval/Accurate interact with cooperative scheduling and what changes when running multiple harts

---

## Part 1 — The Three Timing Models

### `TimingModel` trait (`helm-timing/src/lib.rs:44–60`)

```rust
pub trait TimingModel: Send + 'static {
    fn on_insn(&mut self, info: &InsnInfo) -> u64;          // hot path: once per instruction
    fn on_mem_access(&mut self, access: &MemAccess);        // cache hit/miss outcome
    fn on_branch(&mut self, taken: bool, predicted: bool);  // branch outcome
    fn current_cycles(&self) -> Tick;                       // absolute cycle count
    fn on_boundary(&mut self, eq: &mut EventQueue);         // interval end; drain EventQueue
}
```

`HelmEngine<T: TimingModel>` is **monomorphized** — no vtable, zero per-instruction dynamic dispatch. `HelmSim` is the PyO3 boundary enum (`Virtual | Interval | Accurate`) that dispatches once per Python call.

### Virtual (`helm-timing/src/lib.rs:63–101`)

- Every instruction costs exactly `cycles_per_insn` cycles (derived from IPC parameter, min 1)
- `on_mem_access()` and `on_branch()` are no-ops — no latency modeling
- `on_boundary()` is a no-op in Phase 0 (EventQueue drained by the step driver, not the model)
- **Speed:** 1–10M insns/sec
- **"Cooperative" meaning:** EventQueue is checked at every step boundary; device timers and IRQ delivery driven purely by instruction count

### Interval (`helm-timing/src/lib.rs:103–143`)

- Accumulates `insns_in_interval`; fires `on_boundary()` only when `>= interval_len` (default 10K)
- **Currently delegates entirely to Virtual** — OoO window and CPI stack are stubbed with a `TODO(phase-1)` comment
- Interval boundaries are the natural yield/sync points for multi-hart temporal decoupling
- **Speed:** 10–100M insns/sec (goal; currently same as Virtual)
- **Phase 1 goal:** Add OoO window model + CPI stack (branch mispredicts, I-cache misses, D-cache misses) → ~5% IPC error vs real hardware

### Accurate (`helm-timing/src/lib.rs:145–163`)

- **Currently identical to Virtual** — 5-stage pipeline is a Phase 3 placeholder
- Documented architecture: IF→ID→EX→MEM→WB, load-use stall, flush on branch mispredict
- `on_boundary()` delegates to Virtual's no-op
- **Speed:** 0.1–1M insns/sec (goal)
- **Phase 3 goal:** Per-stage latency, functional unit modeling, pipeline hazard detection

### Timing model comparison

| Aspect | Virtual | Interval | Accurate |
|--------|---------|----------|----------|
| Cycles per insn | Fixed IPC | Fixed base + miss penalties (Phase 1) | Pipeline simulation (Phase 3) |
| Memory modeling | None | OoO window (Phase 1) | Pipeline stages (Phase 3) |
| Branch penalty | None | Mispredict penalty (Phase 1) | Flush (Phase 3) |
| Boundary trigger | Every insn | Every 10K insns | Per pipeline stage (Phase 3) |
| Speed | 1–10M/s | 10–100M/s | 0.1–1M/s |
| IPC accuracy | Ideal | ~5% error (Phase 1) | Cycle-accurate (Phase 3) |
| Multi-hart ready | Yes (with Scheduler) | Yes (natural quantum boundary) | Yes (pipeline per hart) |

---

## Part 2 — Cooperative Scheduling

### What "cooperative" actually means here

The timing models are cooperative in the **event-driven** sense, not the OS-preemption sense. There is no wall clock, no background thread, no preemption. The EventQueue is the sole source of simulated time. `on_boundary()` is the only place device timers fire and IRQs get delivered.

### The current FS-mode step loop (`helm-engine/src/lib.rs:879–950`)

```
loop:
  vcpu_idx = pick_next_fs_vcpu()         // round-robin over powered_on CPUs
  gic.set_active_cpu(vcpu_idx)           // route banked MMIO to correct CPU
  every 16 insns:
    poll irq_lines[vcpu_idx]             // inject IRQ if asserted (recently fixed from .first())
  every 1024 insns:
    inject_timers()                      // write GICD_ISPENDR[0] globally (BUG — see below)
  fetch → decode → execute one insn
  timing.on_insn()                       // advance cycle counter
  on_boundary() if triggered             // drain EventQueue, fire callbacks
```

CPUs switch every **single instruction** (maximally fine-grained round-robin). This is functionally correct but inefficient — switching 1000× more often than necessary.

### The planned Scheduler architecture (from `docs/design/helm-engine/HLD.md:121–154`)

```
Quantum loop (Scheduler):

  for each quantum:
    hart0.run(quantum=1000)   → 1000 instructions, local tick advances
    hart1.run(quantum=1000)   → 1000 instructions, local tick advances
    hart2.run(quantum=1000)   → 1000 instructions, local tick advances
    hart3.run(quantum=1000)   → 1000 instructions, local tick advances
    synchronize()             → drain shared EventQueue, fire IRQs, flush shared memory
```

The quantum boundary is the temporal decoupling point — within a quantum, no synchronization occurs between harts. Interrupts and timer events are only delivered at boundaries.

### `on_boundary()` takes `&mut EventQueue` — architectural implication

Because `on_boundary(eq: &mut EventQueue)` borrows the EventQueue externally, the EventQueue **cannot** live inside `HelmEngine` and also be shared. The `Scheduler<T>` struct must own the EventQueue and pass a mutable borrow into each hart's boundary call:

```rust
pub struct Scheduler<T: TimingModel> {
    harts: Vec<HelmEngine<T>>,
    quantum_size: u64,
    current_tick: Tick,
    shared_events: EventQueue,  // owned here, not inside HelmEngine
}
```

This is a structural change to `HelmEngine` — move `events: EventQueue` out.

---

## Part 3 — SMP Gap Analysis

### What already works

| Feature | Code location |
|---------|---------------|
| Multi-vCPU storage (`Vec<Aarch64Vcpu>`) | `lib.rs:52–70` |
| Round-robin scheduler (`pick_next_fs_vcpu`) | `lib.rs:469–490` |
| PSCI CPU_ON / CPU_OFF / AFFINITY_INFO | `lib.rs:493–605` |
| Per-CPU banked GICv2 state (IRQ 0–31 enabled/pending/active/priority) | `gicv2/mod.rs:56–90` |
| SGI generation with per-CPU target routing | `gicv2/mod.rs:307–350` |
| Per-CPU IRQ line assertion + polling (`irq_lines[vcpu_idx]`) | `lib.rs:914–916` |
| MPIDR assignment + independent stacks per vCPU | `arm_virt.rs:151–227` |
| `build_gicv2_mp(n_irqs, num_cpus)` constructor | `gicv2/mod.rs:367–389` |
| Test: banked PPI state is independent per CPU | `distributor.rs:343–377` |
| Test: SGI from CPU0 delivered to CPU1 only | `distributor.rs:379–412` |
| Test: PSCI CPU_ON powers secondary vCPU | `lib.rs:1838–1874` |
| Test: IRQ line polling uses selected vCPU's line | `lib.rs:1877–1926` |

### Structurally present but semantically incomplete

**1. `routes_to_cpu()` for private IRQs (`gicv2/mod.rs:119–124`)**

```rust
fn routes_to_cpu(&self, irq: u32, cpu_idx: usize) -> bool {
    if irq < 32 {
        // Preserve current UP semantics until SGI/PPI routing is modeled
        return false;  // ← STUB
    }
    // SPI affinity check (works correctly)
}
```

SGIs generated by `generate_sgi()` correctly write to `cpu_states[target].private_pending`. But `highest_pending_for_cpu()` calls `routes_to_cpu()` which returns `false` for IRQ < 32, so those pending bits are never seen. The data is correct; the query logic is stubbed. **One-line fix** — remove the `return false` early exit and implement the banked lookup. The test at `distributor.rs:379–412` already validates the data path.

**2. `update_irq_line(cpu_idx)` call-site audit**

Every GIC state mutation must call `update_irq_line(cpu_idx)` for the affected CPU to propagate the new pending state to `irq_lines[cpu_idx]`. Call sites need auditing to ensure no mutation goes unpropagated under SMP.

### Broken / missing

**3. Timer PPI delivery is global, not per-CPU (`arm_virt.rs:43–66`)**

```rust
fn inject_timers(sys_mem: &mut SystemMem, ...) {
    // Writes GICD_ISPENDR[0] via MMIO — sets pending bit for ALL CPUs
    sys_mem.write(GICD_ISPENDR0, 4, timer_pending_mask)?;
}
```

Under SMP each vCPU has its own physical timer (`CNTP_CTL_EL0` / `CNTP_CVAL_EL0`). The timer check in `step_aarch64_fs()` already runs per-vCPU (reads the current vCPU's arch state), but delivery writes a global pending register so all CPUs see the same pending bit. Fix: add `GicSharedState::assert_ppi(cpu_idx, irq_id)` that writes directly to `cpu_states[cpu_idx].private_pending` and calls `update_irq_line(cpu_idx)`, bypassing the distributor MMIO path.

**4. DTB timer interrupt flags lack CPU mask for SMP**

Python-generated DTB has:
```
interrupts = <1 13 4>, <1 14 4>, <1 11 4>, <1 10 4>
```
QEMU adjusts the flags field to include a CPU mask for GICv2 SMP. Without this, the guest's interrupt controller view is misconfigured. Fix: parameterize Python `virt.py` per `--smp N`, or move DTB generation to Rust.

**5. No `Scheduler<T>` quantum wrapper**

The round-robin in `step_aarch64_system()` switches CPUs every instruction. This is functionally correct but:
- ~10× slower than quantum-based switching for the same functional result
- Prevents meaningful per-hart timing statistics under Interval mode
- `Scheduler<T>` struct (from HLD) doesn't exist yet

**6. EventQueue not shared across harts**

Today `HelmEngine` owns one `EventQueue`. This works because all vCPUs live in one engine instance. When `Scheduler<T>` owns multiple `HelmEngine<T>` instances, the EventQueue must be promoted out of `HelmEngine` into `Scheduler`. See the `on_boundary()` signature note in Part 2.

### GICv2 feature matrix for SMP

| Feature | Status | Notes |
|---------|--------|-------|
| Distributor global state (CTLR/TYPER/IIDR) | ✅ | |
| SPI enable/pending/active (IRQ 32+) | ✅ | |
| SPI affinity routing (ITARGETS) | ✅ | |
| Per-CPU interface (`GicCpuState` vector) | ✅ | |
| Private enable/pending/active (IRQ 0–31, banked) | ✅ data; ❌ query | `routes_to_cpu` stub |
| Private priority (IRQ 0–31, banked) | ✅ | |
| SGI generation with CPU target filters | ✅ | |
| Highest-pending selection per CPU | ✅ data; ❌ private query | same stub |
| Per-CPU IRQ line assertion | ✅ | |
| Per-CPU timer PPI delivery | ❌ | writes global ISPENDR[0] |
| GICv3 redistributors | ❌ | deferred; GICv2 is the milestone |

---

## Part 4 — Ordered Fix Sequence for Functional SMP

These are ordered by dependency. Steps 1–4 unblock Linux SMP boot. Steps 5–6 unlock meaningful timing under SMP.

### Step 1 — Fix `routes_to_cpu()` for private IRQs
**File:** `gicv2/mod.rs:119–124`
**Change:** Remove the `return false` UP stub. Return `true` for IRQ < 32 if `cpu_idx` matches the pending bit's owner in `cpu_states[cpu_idx].private_pending`. The banked data is already correct.
**Test exists:** `distributor.rs:379–412` validates SGI data path. Add a companion test that `highest_pending_for_cpu()` returns the SGI.

### Step 2 — Fix timer PPI delivery to be per-CPU
**File:** `arm_virt.rs:43–66`
**Change:** Replace MMIO write to `GICD_ISPENDR[0]` with a direct `gic.assert_ppi(vcpu_idx, PPI_TIMER_PHYS)` call. This writes to `cpu_states[vcpu_idx].private_pending` and calls `update_irq_line(vcpu_idx)`.
**New API needed:** `GicSharedState::assert_ppi(cpu_idx: usize, irq: u32)` and `deassert_ppi(cpu_idx: usize, irq: u32)`.

### Step 3 — Fix DTB timer interrupt flags
**File:** `examples/fs/virt.py` (and `boot_rpi_full.py`)
**Change:** Parameterize timer interrupt flags in DTB generation based on `--smp N`. For GICv2 SMP, the flags byte should encode the CPU mask. Reference QEMU's behavior for N-CPU virt machine.

### Step 4 — Add SMP test coverage
Tests missing today:
- Secondary CPU takes a timer interrupt (PPI 14 delivered to CPU1)
- SGI from CPU0 reaches CPU1 via `highest_pending_for_cpu(1)`
- Round-robin scheduling: CPU1 executes after CPU0 in a 2-CPU machine
- PSCI AFFINITY_INFO returns correct state after CPU_ON and CPU_OFF

### Step 5 — Add `Scheduler<T>` quantum wrapper
**File:** `runtime/helm-engine/src/scheduler.rs` (new)
**Change:** Extract `Vec<Aarch64Vcpu>` scheduling from `step_aarch64_system()` into a `Scheduler<T>` that runs each hart for `quantum_size` instructions before synchronizing. Move `EventQueue` out of `HelmEngine` into `Scheduler`. Pass `&mut EventQueue` to `on_boundary()` calls at synchronization points.

### Step 6 — Implement Interval CPI stack
**File:** `framework/helm-timing/src/lib.rs:103–143`
**Change:** Replace the `TODO(phase-1)` stub with an OoO window accumulator. Track per-interval: branch mispredicts, I-cache misses, D-cache miss cycles, OoO window occupancy. At boundary, adjust `cycles_per_insn` based on observed rates. Target: ~5% IPC error vs hardware counters.

---

## Key Insight Summary

1. **"Cooperative" = event-driven, not yielding.** No OS scheduler, no wall clock, no preemption. The EventQueue is the only time source. `on_boundary()` is the only delivery point for IRQs and device timer events.

2. **All three timing models are currently identical at runtime.** Virtual is fully implemented. Interval and Accurate delegate to Virtual with stubs. The architectural separation is correct; the implementations are placeholders.

3. **The biggest SMP bug is `routes_to_cpu()` returning false for IRQ < 32.** It's a one-line fix — all the data (banked pending bits, per-CPU SGI targeting) is already correct. This single stub blocks SGI-based IPI delivery.

4. **Timer PPI delivery is the second blocker.** The `inject_timers()` function writes a global pending register instead of a per-CPU one. Under SMP, all CPUs see the same timer pending bit.

5. **`Scheduler<T>` requires moving EventQueue out of `HelmEngine`.** This is forced by the `on_boundary(&mut EventQueue)` signature — the EventQueue must be externally owned to be shared. This is a breaking API change to `HelmEngine` but the right long-term architecture.

6. **Interval timing is the natural SMP timing mode.** Each quantum boundary is the synchronization point. Harts run independently for one quantum, then drain the shared EventQueue together. This matches the temporal decoupling model described in the engine HLD.
