# helm-ng Instrumentation Stack — High-Level Design

> **Scope:** This document covers the complete instrumentation architecture across three
> crates: `helm-probe`, `helm-plugin`, and `helm-diag`. It supersedes the
> earlier probe-only HLD.

---

## 1. Overview

helm-ng has three distinct instrumentation layers. Each has a different audience,
performance contract, and lifecycle. Understanding how they connect is the purpose of this
document.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  LAYER 1 — helm-probe (framework/helm-probe)                                │
│  Zero-cost typed probe points. Zero-sized in release. One branch in dev.    │
│  Audience: core engine maintainers wiring instrumentation call sites.       │
└──────────────────────────────┬──────────────────────────────────────────────┘
                               │  ProbePluginBridge (in helm-plugin)
                               │  Subscribes probe events, enriches to plugin types
                               ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  LAYER 2 — helm-plugin (framework/helm-plugin)                              │
│  Typed callbacks, chain/filter, rich event types (InsnInfo, BranchInfo…).  │
│  Audience: tool authors, researchers, Python users writing analysis scripts.│
└──────────────────────────────┬──────────────────────────────────────────────┘
                               │  TraceSink / sim_trace::Monitor
                               │  Routes output to configured backend
                               ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  LAYER 3 — helm-diag (framework/helm-diag)                                  │
│  Diagnostic log channel. Async mpsc queue → background drain → backend.     │
│  Audience: developers reading simulator diagnostics (stubs, warnings).      │
│  Macros: sim_stub!, sim_warn!, sim_info!  (sim_branch! DELETED — use probe) │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Rule**: Events flow downward only. helm-probe does not depend on helm-plugin.
helm-plugin does not depend on sim_trace (except to accept a Monitor handle). sim_trace
has no deps on either.

---

## 2. Layer 1 — `helm-probe`

### 2.1 What it is

A typed, zero-cost probe point. `Probe<T>` is a struct that holds a `Vec<Listener>` in
debug builds and is **zero-sized** in release. The `probe!()` macro wraps event
construction in `if has_listeners()`, so expensive event objects are never built in
release.

### 2.2 Three-tier build model

| Profile | `debug_assertions` | Struct size | `has_listeners()` | Event constructed |
|---|---|---|---|---|
| `--release` | false | 0 bytes (ZST) | `const false` | **never** |
| `cargo build` | true | 24 bytes (Vec) | `!vec.is_empty()` | only if subscribed |
| `--features probe-full` | true | 24 bytes + counters | `!vec.is_empty()` | only if subscribed |

### 2.3 Standard event types

| Type | Key fields | Who fires |
|---|---|---|
| `CpuStepEvent` | `pc: u64`, `raw: u32` | FS loop, SE loop |
| `CpuFaultEvent` | `pc`, `raw`, `kind: &'static str` | FS fault handlers |
| `MemAccessEvent` | `addr`, `size`, `is_store`, `pc` | SE InstrumentedMem |
| `BranchEvent` | `pc`, `target`, `taken`, `kind: BranchKind` | `branch.rs` executor (**replaces `sim_branch!`**) |
| `IrqEvent` | `irq_id: u32`, `asserted: bool` | GIC distributor |
| `MmioEvent` | `addr`, `size`, `val`, `is_write` | SystemMem dispatch |

`BranchEvent` is the Instrumentation-v2 replacement for `sim_branch!`. It is zero-cost
in release (ZST probe). `BranchKind` is defined here and re-exported by `helm-spy`.

### 2.4 Probe bundles

Components own their bundles as plain public structs:

```
HelmEngine<T>  → pub probes: CpuProbes
                   pre_step, post_step, fault, mem, branch   ← branch added in v2
GicState       → pub probes: GicProbes   (irq_asserted, irq_deasserted, eoi)
```

### 2.5 Enable / disable

**Release builds**: probes are structurally absent — cannot be subscribed or fired.
Calling `probe.subscribe()` in release is a compile error.

**Dev builds**: a probe is "enabled" by having at least one subscriber. An empty probe
(no subscribers) is effectively disabled — `has_listeners()` returns false and the
`probe!()` block is skipped.

There is no global enable/disable switch at the probe layer. Enable/disable at the probe
layer is implicit: subscribe → enabled; no subscribers → disabled. Coarse enable/disable
belongs at Layer 2 (plugin) or Layer 3 (sim_trace URI).

---

## 3. Layer 2 — `helm-plugin`

### 3.1 What it is

A typed callback registry that user plugins subscribe to. Callbacks are typed closures
stored in `Vec` fields on `PluginRegistry`. A `has_*_callbacks()` flag enables fast-path
skipping in the engine when no plugins are installed.

### 3.2 Current callback taxonomy

```
PluginRegistry {
    insn_exec:   Vec<InsnExecCb>          // (vcpu, &InsnInfo)
    mem_access:  Vec<(MemFilter, MemAccessCb)>  // filtered: All/ReadsOnly/WritesOnly
    branch:      Vec<BranchCb>            // (vcpu, &BranchInfo)
    syscall:     Vec<SyscallCb>           // (&SyscallInfo)
    syscall_ret: Vec<SyscallRetCb>        // (&SyscallRetInfo)
    fault:       Vec<FaultCb>             // (&FaultInfo)
    vcpu_init:   Vec<VcpuInitCb>          // (vcpu)
    vcpu_exit:   Vec<VcpuExitCb>          // (vcpu)
    timer:       Vec<(u64, TimerCb)>      // (interval_insns, (vcpu, insn_count))
}
```

### 3.3 Rich event types vs probe events

The plugin layer uses **richer** event types than `helm-probe`:

| helm-probe type | helm-plugin type | Extra fields |
|---|---|---|
| `CpuStepEvent` | `InsnInfo` | `vcpu_idx`, `class`, `opcode_name`, `is_stub`, `ArchContext` |
| `CpuFaultEvent` | `FaultInfo` | `vcpu_idx`, `kind` (enum), `message`, `insn_count`, `ArchContext` |
| `MemAccessEvent` | `MemInfo` | `is_atomic` (set from `InstrumentedMem` access type) |
| `IrqEvent` | *(not yet in plugin layer)* | — |

The enrichment (probe event → plugin event) happens in the **ProbePluginBridge**
(see §4 and [LLD-probe-framework.md §9](LLD-probe-framework.md#9-probepluginbridge-in-helm-plugin)).
`InsnInfo.class` and `.opcode_name` are populated via `classify_aarch64_opcode()` from
`helm-engine`. `InsnInfo.is_atomic` on `MemInfo` comes from the `AccessType` stored by
`InstrumentedMem` during the SE step.

### 3.4 Built-in plugins

| Plugin | Subscribes to | Output (current) |
|---|---|---|
| `InsnCount` | `insn_exec` | summary at atexit |
| `ExecLog` | `insn_exec` | one line per instruction |
| `HotBlocks` | `branch` | top-N basic blocks at atexit |
| `HowVec` | `insn_exec` | class histogram at atexit |
| `SyscallTrace` | `syscall`, `syscall_ret` | per-syscall log |
| `BranchTrace` | `branch` | per-PC taken/not-taken at atexit |
| `MemTrace` | `mem_access` | per-access log |
| `CacheSim` | `mem_access` | hit/miss statistics |
| `FaultDetect` | `fault` | fault log |
| `StubTracer` | `insn_exec` (is_stub) | stub site report |

### 3.5 Enable / disable

**Plugin-level enable/disable** is achieved by installing or not installing a plugin.
Once `plugin.install(&mut reg, args)` is called, the plugin is active for the lifetime
of the registry.

**Coarse disable without removal**: planned `reg.pause()` / `reg.resume()` — sets an
atomic bool that `has_*_callbacks()` checks first. Not yet implemented.

**Fine-grained disable**: handled by the chain/filter mechanism (§5).

---

## 4. `sim_branch!` and `sim_trace` — Layer 3

### 4.1 What sim_trace is

A structured, async log channel for simulator-internal diagnostics. It is **not** a
plugin system. It is the output substrate that plugins and the engine use for writing
human-readable or machine-parseable traces.

```
sim_stub!  }
sim_warn!  }── sim_trace::emit(level, component, pc, message)
sim_info!  }         │
sim_branch!}         │ thread-local SIM_MONITOR.try_send(MonitorEntry)
                     │
                 MonitorSink (background drain thread)
                     │
                 Backend: Stderr | File | Tcp | Null
```

### 4.2 `MonitorEntry` format

```
[STUB] sim_ns=000001234 insns=000025750 gicv2-dist   pc=0x0000000040201234 | MRS ID_AA64MMFR4_EL1 → 0
[BRNC] sim_ns=000012340 insns=000025760 branch        pc=0x0000000040202000 | -> 0x0000000040201000
```

Fields: `level` (STUB/WARN/INFO/ERR/BRNC), `sim_ns`, `sim_insns`, `component`, `pc`,
`message`. (Note: the rendered format above abbreviates `sim_insns` as `insns` for
column width; the actual struct field is `sim_insns`.)

### 4.3 Enable / disable

| Mechanism | Effect |
|---|---|
| No `MonitorSink` installed | Branch (BRNC) events dropped silently; others fall back to `eprintln!` |
| URI `null:` | All events discarded (benchmarking mode) |
| URI `stderr:` | All events written to stderr (default) |
| URI `file:/path` | All events appended to file |
| URI `tcp:host:port` | All events streamed to TCP listener |

Level-based filtering (e.g. suppress STUB, pass WARN+): see §5 (chain/filter).

### 4.4 `sim_branch!` vs `Probe<BranchInfo>`

These are **parallel systems with different purposes**:

| | `sim_branch!` | `Probe<BranchInfo>` |
|---|---|---|
| **Layer** | 3 — logging | 1 — typed probe |
| **Consumer** | External readers (branch_trace.py) | Internal plugins (BranchTrace) |
| **Data** | String message in MonitorEntry | Typed `BranchInfo` struct |
| **Cost in release** | `emit()` call always fires (drops if no monitor) | Zero (ZST probe — whole block eliminated) |
| **Enable/disable** | MonitorSink presence | subscriber presence |

**Connection**: Both fire from the same call sites in `branch.rs`. `sim_branch!` fires
unconditionally (drops if no monitor). `Probe<BranchInfo>` only fires if subscribed.
The `ProbePluginBridge` subscribes to `Probe<BranchInfo>` and calls
`registry.fire_branch(vcpu, info)`. A plugin (e.g. `BranchTrace`) then receives the
structured event without parsing string output.

The `branch_trace.py` example uses the `sim_branch!` path (reads from file, parses
BRNC lines, resolves symbols). A Rust plugin using the bridge uses the probe path.
Both can run simultaneously.

---

## 5. Connections Between Layers

### 5.1 ProbePluginBridge

`ProbePluginBridge` is a struct (in `helm-plugin`) that subscribes to `Probe<T>` events
and dispatches them to a `PluginRegistry`. It is the vertical connector between Layer 1
and Layer 2.

```
                    ┌────────────────────────────────────────┐
  CpuProbes         │          ProbePluginBridge              │
  .pre_step ────────►  subscribe → fire_insn_exec(InsnInfo)  │
  .post_step ───────►  subscribe → fire_insn_exec(InsnInfo)  │
  .fault ───────────►  subscribe → fire_fault(FaultInfo)     │  ───► PluginRegistry
  .mem ─────────────►  subscribe → fire_mem_access(MemInfo)  │
                    │                                          │
  GicProbes         │                                          │
  .irq_asserted ────►  subscribe → (future irq callbacks)    │
  .irq_deasserted ──►  subscribe → (future irq callbacks)    │
                    └────────────────────────────────────────┘
```

Enrichment happens inside the bridge: `CpuStepEvent { pc, raw }` → `InsnInfo { pc, raw,
class, opcode_name, is_stub, context }`. The `classify_aarch64_opcode()` function
(already in `helm-engine/lib.rs`) provides the class and name.

### 5.2 Plugin output → sim_trace

Currently plugins write to `eprintln!`. The planned connection routes plugin output
through sim_trace:

```
Plugin::atexit()
    ↓
TraceSink (see §6.3)
    ↓ if TraceSink::SimTrace
sim_trace::Monitor::try_send(MonitorEntry { level: Info, component: plugin.name() ... })
    ↓
MonitorSink backend (file / stderr / TCP)
```

This makes plugin output subject to the same routing, filtering, and async buffering as
the rest of sim_trace output.

### 5.3 Full data flow (annotated)

```
                         FS/SE hot loop
                              │
                    ┌─────────▼─────────┐
                    │    probe!(...)     │  ← zero cost in release
                    └─────────┬─────────┘
                              │ CpuStepEvent {pc, raw}
                    ┌─────────▼────────────────┐
                    │   ProbePluginBridge       │  ← dev only; installed by Python/CLI
                    │   enriches → InsnInfo     │
                    └─────────┬────────────────┘
                              │ InsnInfo {pc, raw, class, …}
                    ┌─────────▼────────────────┐
                    │   PluginRegistry          │
                    │   fire_insn_exec(…)       │
                    │   (also fire_branch etc.) │
                    └──────┬────────┬───────────┘
                           │        │
               ┌───────────▼─┐  ┌──▼──────────┐
               │  ExecLog    │  │  HotBlocks  │  ← builtin plugins
               │  (collect)  │  │  (collect)  │
               └──────┬──────┘  └──┬──────────┘
                      │             │
               ┌──────▼─────────────▼──────────┐
               │       TraceSink               │
               │  Stderr | SimTrace | Buffer   │
               └──────────────┬────────────────┘
                              │ (if SimTrace)
                    ┌─────────▼────────────────┐
                    │  sim_trace::Monitor       │  ← try_send, non-blocking
                    └─────────┬────────────────┘
                              │ mpsc channel
                    ┌─────────▼────────────────┐
                    │  MonitorSink (bg thread)  │
                    │  Backend: file/stderr/TCP │
                    └──────────────────────────┘

Also: device stubs / arch stubs (direct path, always-on):
    sim_stub!(…) / sim_warn!(…) / sim_branch!(…)
         └──────────────────────────────────────► same Monitor / MonitorSink
```

---

## 6. Chain and Filter Mechanism

### 6.1 Why chain and filter

The current plugin system is flat: all registered callbacks for a given event type fire
unconditionally. There is no way to:
- Suppress callbacks for a PC range (e.g. ignore boot code)
- Sample every Nth instruction without a counter inside every plugin
- Chain a formatter plugin to a writer plugin
- Stop propagation after the first matching handler

Chain and filter solve this at Layer 2, without touching Layer 1 or Layer 3.

See [LLD-chain-filter.md](LLD-chain-filter.md) for the full implementation specification.

### 6.2 Filter predicates

A filter is a predicate `Fn(&T) -> bool` applied before a callback fires. If the
predicate returns `false`, the callback is skipped.

Filters compose: multiple predicates are AND-ed.

```rust
/// A callback with associated filter predicates.
pub struct FilteredCb<T> {
    filters: Vec<Box<dyn Fn(&T) -> bool + Send + Sync>>,
    cb: Box<dyn Fn(&T) + Send + Sync>,
}

impl<T> FilteredCb<T> {
    pub fn new(cb: impl Fn(&T) + Send + Sync + 'static) -> Self {
        Self { filters: Vec::new(), cb: Box::new(cb) }
    }

    /// Add a filter predicate (AND semantics).
    pub fn filter(mut self, pred: impl Fn(&T) -> bool + Send + Sync + 'static) -> Self {
        self.filters.push(Box::new(pred));
        self
    }

    pub fn matches(&self, val: &T) -> bool {
        self.filters.iter().all(|f| f(val))
    }

    pub fn call(&self, val: &T) {
        if self.matches(val) {
            (self.cb)(val);
        }
    }
}
```

**Stock filters** (provided as free functions in `helm-plugin::filter`):

```rust
// Only fire for PCs in [start, end)
pub fn pc_range(start: u64, end: u64) -> impl Fn(&InsnInfo) -> bool {
    move |i| i.pc >= start && i.pc < end
}

// Only fire for instruction class
pub fn insn_class(cls: InsnClass) -> impl Fn(&InsnInfo) -> bool {
    move |i| i.class == cls
}

// Only fire every Nth event
pub fn sample_every(n: u64) -> impl Fn(&InsnInfo) -> bool {
    let counter = std::sync::atomic::AtomicU64::new(0);
    move |_| counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % n == 0
}

// Only fire for insn_count in [start, end)
pub fn insn_window(start: u64, end: u64) -> impl Fn(&InsnInfo) -> bool {
    move |i| i.insn_count >= start && i.insn_count < end
}
```

`MemFilter` (already implemented: All/ReadsOnly/WritesOnly) is an instance of this
pattern. The chain/filter design generalises it.

### 6.3 Chain (sequential stages)

A chain routes events through a sequence of stages. Each stage can consume, transform,
or pass through an event. This enables "pipeline" instrumentation:

```
Source (probe fire)
  → Stage 1: sample (every 100th)
  → Stage 2: enrich (add symbol name from ELF table)
  → Stage 3: format (render to string)
  → Stage 4: sink (write to file via sim_trace)
```

Simple chain (just sequenced callbacks, same type):
```rust
pub struct Chain<T> {
    stages: Vec<FilteredCb<T>>,
}

impl<T> Chain<T> {
    pub fn new() -> Self { Self { stages: Vec::new() } }

    pub fn stage(mut self, cb: FilteredCb<T>) -> Self {
        self.stages.push(cb);
        self
    }

    pub fn fire(&self, val: &T) {
        for stage in &self.stages {
            stage.call(val);
        }
    }
}
```

For type-transforming chains (e.g. `InsnInfo → String → file`), each stage has a
different `T` and must be wired manually. This is the responsibility of the plugin author,
not the framework.

### 6.4 `TraceSink` — delivery abstraction

The key missing abstraction. Every plugin currently hardcodes `eprintln!`. `TraceSink`
decouples generation from delivery:

```rust
/// Where a plugin writes its output.
pub enum TraceSink {
    /// Write to stderr (default, current behavior).
    Stderr,
    /// Route via sim_trace channel — same backend as engine diagnostics.
    SimTrace {
        monitor: helm_debug::sim_trace::Monitor,
        component: &'static str,
        level: helm_debug::sim_trace::Level,
    },
    /// Buffer in memory (for testing or in-process consumers).
    Buffer(std::sync::Arc<std::sync::Mutex<Vec<String>>>),
    /// Discard (benchmarking, or plugin that stores state but prints nothing).
    Null,
}

impl TraceSink {
    pub fn write_line(&self, line: &str) {
        match self {
            TraceSink::Stderr => eprintln!("{line}"),
            TraceSink::SimTrace { monitor, component, level } => {
                // Read sim context from the thread-local so timestamps are accurate.
                let (sim_ns, sim_insns) = helm_debug::sim_trace::SIM_CTX.with(|c| {
                    let ctx = c.borrow();
                    (ctx.sim_ns, ctx.sim_insns)
                });
                monitor.try_send(helm_debug::sim_trace::MonitorEntry {
                    sim_ns,
                    sim_insns,
                    component,
                    level: *level,
                    pc: None,
                    message: line.to_string(),
                });
            }
            TraceSink::Buffer(buf) => buf.lock().unwrap().push(line.to_string()),
            TraceSink::Null => {}
        }
    }
}
```

Plugins receive a `TraceSink` at install time (via `PluginArgs` or a dedicated setter).
The `HelmPlugin` trait gains an optional `set_sink()` method:

```rust
pub trait HelmPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn install(&mut self, reg: &mut PluginRegistry, args: &PluginArgs);
    fn atexit(&mut self) {}
    /// Optional: set where this plugin writes its output.
    /// Default: Stderr.
    fn set_sink(&mut self, _sink: TraceSink) {}
}
```

### 6.5 Level filtering in sim_trace

sim_trace currently delivers all levels that pass the "Branch only if monitor installed"
rule. A `LevelFilter` on `MonitorSink` enables coarse suppression:

```rust
pub struct MonitorSink {
    handle: Option<thread::JoinHandle<()>>,
    min_level: Level,   // new field; default = Info (pass all)
}
```

`emit()` checks `level >= min_level` before `try_send()`. The effective ordering is:
```
Error > Warn > Stub > Info > Branch
```

This lets a user capture only BRNC events (`--sim-trace-level=branch`) or suppress
STUB noise (`--sim-trace-level=warn`).

---

## 7. Enable / Disable — Full Picture

| Layer | How to disable entirely | How to pause temporarily |
|---|---|---|
| **helm-probe** (release) | Automatic — ZST, zero instructions emitted | N/A |
| **helm-probe** (dev) | Don't call `subscribe()` — empty probe skips | N/A |
| **GicProbes** | Omit `features = ["probe"]` on helm-hw-intc dep — field absent at compile time | N/A |
| **ProbePluginBridge** | Don't call `bridge.install_cpu()` — bridge not wired | N/A (bridge is all-or-nothing) |
| **helm-plugin** | Don't call `plugin.install()` | `reg.pause()` / `reg.resume()` (planned) |
| **sim_trace** | URI `null:` or no `MonitorSink` | No pause yet; close and reopen sink |
| **sim_branch! (Branch level)** | Don't install MonitorSink | Change `min_level` to suppress |
| **plugin output delivery** | Set `TraceSink::Null` | Change sink at runtime |

---

## 8. Dependency Graph (updated)

```
helm-probe   (zero deps)
    │
    ├── helm-engine    (always; CpuProbes; ProbePluginBridge instantiation)
    ├── helm-hw-intc   (optional feature = "probe"; GicProbes)
    └── helm-arch      (optional feature = "probe"; future decode probes)

helm-plugin  (no dep on helm-probe or sim_trace directly)
    │  ProbePluginBridge lives here; takes &mut Probe<T> refs
    └── helm-engine    (PluginRegistry field; bridge wired during build_simulator)

helm-diag  (runtime/helm-debug; deps: helm-core, thiserror, log)
    │  Monitor handle passed to TraceSink → plugins receive it
    └── helm-engine    (MonitorSink opened at startup; Monitor installed on sim thread)
```

`helm-core` is unchanged — zero deps, no probe or plugin dependency.

---

## 9. Phased Implementation

| Phase | Deliverable |
|---|---|
| **0 (current)** | sim_trace: MonitorSink, Level, macros. PluginRegistry: callbacks + fire_*. |
| **1 (helm-probe)** | `Probe<T>`, `probe!()`, CpuProbes/GicProbes. Wired in fs.rs + SE loop. |
| **2 (bridge)** | ProbePluginBridge: probe → InsnInfo/BranchInfo → PluginRegistry. |
| **3 (chain/filter)** | FilteredCb, stock filters, Chain<T>, TraceSink on all plugins. |
| **4 (level filter)** | `min_level` on MonitorSink; `--sim-trace-level` CLI flag. |
| **5 (pause/resume)** | AtomicBool gate on PluginRegistry; Python API. |

---

## 10. Industry Comparison

| System | Probe layer | Plugin layer | Delivery layer |
|---|---|---|---|
| **QEMU TCG plugins** | TCG translation hooks | `qemu_plugin_register_*_cb` closures | Plugin writes directly or uses scoreboard |
| **gem5 ProbePoint** | `ProbePoint<T>` (our model for helm-probe) | `ProbeListener` subclasses | Direct call, no buffering |
| **Simics** | HAP system | Per-HAP subscription | Hap callbacks write directly |
| **Pin** | `INS_InsertCall` | `IARG_*` typed arguments | `PIN_SafeCopy` / analysis routines |
| **helm-ng** | `Probe<T>` + `probe!()` | `PluginRegistry` + `HelmPlugin` trait | `TraceSink` → `sim_trace::MonitorSink` |

Key differences from gem5: our delivery layer is async (mpsc channel + background thread)
rather than synchronous. This means plugin output does not add to hot-loop latency.
Key difference from QEMU: we use build-cfg (`debug_assertions`) not runtime flags to
eliminate probe sites. QEMU plugins always have the `if (plugin_enabled)` branch.
