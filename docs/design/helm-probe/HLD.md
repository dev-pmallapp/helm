# helm-ng Instrumentation Stack — High-Level Design

> **Status:** Reflects the state of the codebase as of 2026-03-19.
> helm-probe (Layer 1) and helm-diag (Layer 1b) are implemented.
> helm-spy (Layer 2) and helm-report (Layer 3) are planned.

---

## 1. Overview

The instrumentation stack has three layers. Each has a different audience, performance
contract, and lifecycle.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  LAYER 1 — helm-probe (framework/helm-probe)                                │
│  Zero-cost typed probe points. Zero-sized in release. One branch in dev.   │
│  Audience: core engine maintainers wiring call sites.                      │
│  STATUS: IMPLEMENTED                                                        │
└──────────────────────────────┬──────────────────────────────────────────────┘
                               │  ProbePluginBridge (planned, Phase 2)
                               │  Subscribes probe events, enriches to plugin types
                               ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  LAYER 2 — helm-spy (runtime/helm-debug or framework/)                      │
│  Analysis primitives, SpySession, chain/filter. Replaces helm-plugin.      │
│  Audience: tool authors and researchers writing analysis scripts.           │
│  STATUS: PLANNED (Phase 2/3)                                                │
└──────────────────────────────┬──────────────────────────────────────────────┘
                               │  Sink trait
                               ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  LAYER 3 — helm-report (planned)                                            │
│  Sink trait, formatters, Report. Routes output to backends.                 │
│  STATUS: PLANNED (Phase 3)                                                  │
└─────────────────────────────────────────────────────────────────────────────┘

LAYER 1b — helm-diag (framework/helm-diag)
│  Diagnostic log channel for simulator internals.
│  Macros: sim_stub!, sim_warn!, sim_info!
│  STATUS: IMPLEMENTED (separate from probe — always-on diagnostic channel)
```

**Rule**: Events flow downward only. helm-probe does not depend on helm-spy.
helm-spy does not depend on helm-report (except to accept a Sink handle).

---

## 2. Crate Dependency Graph

```
helm-probe  (zero deps — no_std compatible)
    │
    ├── runtime/helm-engine   (always; owns CpuProbes field on HelmEngine<T>)
    └── hw/helm-hw-intc       (optional feature = "probe"; owns GicProbes on GicState)

helm-diag  (framework/helm-diag; deps: helm-core, thiserror, log)
    └── runtime/helm-engine   (sim_stub!/sim_warn!/sim_info! call sites)

helm-spy   (planned; will depend on helm-probe for event types)
helm-report (planned; will depend on helm-spy for Sink trait)
```

**Note**: `helm-plugin` (the old Layer 2) is **deprecated** in favor of helm-spy, but
it still exists in the codebase and its `PluginRegistry` is still used in `HelmEngine<T>`.
The `ProbePluginBridge` that would connect probe events to helm-plugin/helm-spy is
**planned but not yet implemented** in source code.

---

## 3. Layer 1 — `helm-probe`

### 3.1 What it is

A typed, zero-cost probe point. `Probe<T>` is a struct that holds a
`Vec<Box<dyn Fn(&T) + Send + Sync>>` in debug builds and is **zero-sized** in release.
The `probe!()` macro wraps event construction in `if has_listeners()`, so event objects
are never built when no subscribers are attached.

### 3.2 Build profile model

| Profile | `debug_assertions` | Struct size | `has_listeners()` | `subscribe()` |
|---|---|---|---|---|
| `--release` | false | 0 bytes (ZST) | `const false` | **compile error** |
| `cargo build` (dev) | true | 24 bytes (Vec) | `!vec.is_empty()` | available |
| `--features probe-full` | true | 24 bytes + extra fields | `!vec.is_empty()` | available |

The `probe-full` feature adds `insn_count: u64` to `CpuStepEvent`. It does not change
the `Probe<T>` struct size.

### 3.3 Event types (all defined in `framework/helm-probe/src/events.rs`)

| Type | Key fields | Who fires |
|---|---|---|
| `CpuStepEvent` | `pc: u64`, `raw: u32` (+ `insn_count` with probe-full) | FS and SE step loops |
| `CpuFaultEvent` | `pc`, `raw`, `kind: &'static str` | FS fault handlers |
| `MemAccessEvent` | `addr`, `size`, `is_store`, `pc` | SE via `InstrumentedMem` |
| `BranchEvent` | `pc`, `target`, `taken`, `kind: BranchKind` | SE `step_aarch64()` after execute |
| `IrqEvent` | `irq_id: u32`, `asserted: bool` | GIC distributor (feature "probe") |
| `MmioEvent` | `addr`, `size`, `val`, `is_write` | Defined; not yet wired to call sites |

`CpuFaultEvent.kind` string values used by the FS loop: `"insn-abort"`, `"data-abort"`,
`"store-abort"`, `"svc"`.

`BranchKind` is an enum defined in `events.rs`:
`DirectCond | DirectUncond | Call | Return | IndirectJump | IndirectCall`.

### 3.4 Probe bundles

Probe bundles are plain structs with `Default` implementations, defined in
`framework/helm-probe/src/lib.rs`:

```
CpuProbes  (pub field on HelmEngine<T>):
    pre_step:  Probe<CpuStepEvent>
    post_step: Probe<CpuStepEvent>
    fault:     Probe<CpuFaultEvent>
    mem:       Probe<MemAccessEvent>
    branch:    Probe<BranchEvent>

GicProbes  (pub field on GicState, only when feature "probe" is enabled):
    irq_asserted:   Probe<IrqEvent>
    irq_deasserted: Probe<IrqEvent>
    eoi:            Probe<IrqEvent>
```

### 3.5 Where probes are wired (current implementation)

**`runtime/helm-engine/src/lib.rs` — `step_aarch64()`** (SE mode):
- `pre_step` fires before fetch with `raw: 0`
- `mem` fires for each recorded access when `InstrumentedMem` path is active
- `post_step` fires after execute with actual `raw` word
- `branch` fires after execute when `insn.is_branch()` is true

**`runtime/helm-engine/src/fs.rs` — `step_aarch64_fs()`** (FS mode):
- Takes `probes: &CpuProbes` as 4th parameter
- `pre_step` fires before MMU fetch translation
- `fault` fires on insn-abort, data-abort, store-abort, svc
- `post_step` fires after successful execute

**`hw/helm-hw-intc/src/gicv2/mod.rs` — `GicState`**:
- `probes: GicProbes` field present when `feature = "probe"` is enabled on helm-hw-intc
- GIC methods fire `irq_asserted`, `irq_deasserted`, `eoi` probes

**Not yet wired**: `MmioEvent` — defined in events.rs but no `probe!()` call sites in
`SystemMem` dispatch.

### 3.6 Enable / disable

- **Release builds**: `subscribe()` is absent (compile error). The entire probe block
  is dead code — zero instructions emitted.
- **Dev builds**: a probe fires only when `has_listeners()` is true (at least one
  subscriber). Empty probes skip the event construction block.
- **GicProbes**: absent at compile time unless `features = ["probe"]` is set on the
  helm-hw-intc dependency.

There is no global on/off switch at the probe layer. Enable = subscribe. Disable =
no subscribers. Coarse control belongs at Layer 2 (helm-spy, planned).

---

## 4. Layer 1b — `helm-diag`

`helm-diag` is an always-on structured diagnostic log channel, separate from the probe
system. It is not zero-cost in release — it always emits (dropped to stderr or a
configured backend if no sink is installed).

Macros: `sim_stub!`, `sim_warn!`, `sim_info!`.

`sim_branch!` is **deleted** — branch events now use `probe!(probes.branch, BranchEvent{...})`.

---

## 5. Layer 2 — helm-spy (PLANNED)

`helm-spy` will provide analysis primitives and a `SpySession` for attaching structured
analysis to probe points. It is intended to replace the current `helm-plugin` / `PluginRegistry`
system.

**Current state**: not implemented. The `ProbePluginBridge` (Layer 1 → Layer 2 connector)
is designed in `LLD-probe-framework.md` §9 but is not present in source code.

---

## 6. Layer 3 — helm-report (PLANNED)

`helm-report` will provide a `Sink` trait with formatters (text, JSON, protobuf) and
a `Report` type for structured output. Not implemented.

---

## 7. Chain and Filter Mechanism (DESIGNED, NOT YET IMPLEMENTED)

The chain/filter design (`FilteredCb<T>`, `Chain<T>`, stock filters, `TraceSink`)
is fully specified in [LLD-chain-filter.md](LLD-chain-filter.md). It is Phase 3 work.

---

## 8. Phased Implementation Plan

| Phase | Deliverable | Status |
|---|---|---|
| **1 (current)** | `Probe<T>`, `probe!()`, CpuProbes/GicProbes wired in SE and FS loops | **DONE** |
| **2** | ProbePluginBridge: probe → helm-spy/plugin enrichment. helm-spy SpySession. | PLANNED |
| **3** | Chain/filter: FilteredCb, stock filters, Chain<T>, TraceSink. | PLANNED |
| **4** | Level filtering on helm-diag; `--sim-trace-level` CLI flag. | PLANNED |
| **5** | Pause/resume gate on analysis registry; Python API. | PLANNED |

---

## 9. Industry Comparison

| System | Probe layer | Analysis layer | Delivery layer |
|---|---|---|---|
| **gem5 ProbePoint** | `ProbePoint<T>` (our model) | `ProbeListener` subclasses | Direct call, synchronous |
| **QEMU TCG plugins** | TCG translation hooks | `qemu_plugin_register_*_cb` | Plugin writes directly |
| **Simics** | HAP system | Per-HAP subscription | Callbacks write directly |
| **helm-ng** | `Probe<T>` + `probe!()` | helm-spy (planned) | helm-report Sink (planned) |

Key difference from gem5: our release path emits zero instructions (ZST + `const false`),
versus gem5 which always has the `if (listeners.empty())` check.
Key difference from QEMU: zero overhead controlled by build profile (`debug_assertions`),
not a runtime flag.
