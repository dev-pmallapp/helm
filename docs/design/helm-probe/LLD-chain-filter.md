# helm-probe / helm-plugin — Low-Level Design: Chain, Filter, and Delivery

> This document specifies the chain, filter, and `TraceSink` delivery mechanisms
> that sit between the `PluginRegistry` callback dispatch and the `sim_trace` backend.
> Read the [Instrumentation Stack HLD](HLD.md) first.

---

## 1. Problem Statement

The current `PluginRegistry` fires every registered callback unconditionally for every
event. Three things are missing:

1. **Filter**: no way to suppress a callback based on event content (PC range, class,
   sampling rate, time window).
2. **Chain**: no way to sequence stages so that one stage's decision (e.g. "keep this
   event") controls the next (e.g. "now format it and write it").
3. **Delivery abstraction**: plugins hardcode `eprintln!` in `atexit()`. There is no
   way to redirect output to a file, TCP stream, or in-process buffer without modifying
   each plugin.

This document defines the data types and APIs that fix all three.

---

## 2. FilteredCb — Predicate-Gated Callback

`FilteredCb<T>` wraps a callback with a list of predicate functions. The callback fires
only when ALL predicates return `true` (AND semantics).

```rust
// framework/helm-plugin/src/runtime/filter.rs

/// A callback with zero or more filter predicates (AND semantics).
///
/// Predicates are checked in registration order. Short-circuits on first `false`.
pub struct FilteredCb<T> {
    filters:  Vec<Box<dyn Fn(&T) -> bool + Send + Sync>>,
    callback: Box<dyn Fn(&T) + Send + Sync>,
}

impl<T: 'static> FilteredCb<T> {
    /// Create from a bare callback (no filters — always fires).
    pub fn new(f: impl Fn(&T) + Send + Sync + 'static) -> Self {
        Self { filters: Vec::new(), callback: Box::new(f) }
    }

    /// Add a predicate (builder pattern).
    pub fn filter(mut self, pred: impl Fn(&T) -> bool + Send + Sync + 'static) -> Self {
        self.filters.push(Box::new(pred));
        self
    }

    /// `true` if all predicates pass (or no predicates exist).
    pub fn matches(&self, val: &T) -> bool {
        self.filters.iter().all(|f| f(val))
    }

    /// Check predicates; if all pass, call the callback.
    pub fn call(&self, val: &T) {
        if self.matches(val) {
            (self.callback)(val);
        }
    }
}
```

### 2.1 Integration with `PluginRegistry`

`PluginRegistry` gains a filtered variant of each registration method. The existing
unfiltered methods are preserved for compatibility.

```rust
// In PluginRegistry:
pub insn_exec_filtered: Vec<FilteredCb<InsnInfo>>,
pub branch_filtered:    Vec<FilteredCb<BranchInfo>>,
pub mem_filtered:       Vec<FilteredCb<MemInfo>>,
// … etc.

pub fn on_insn_exec_filtered(&mut self, cb: FilteredCb<InsnInfo>) {
    self.insn_exec_filtered.push(cb);
}

// fire path:
pub fn fire_insn_exec(&self, vcpu: usize, insn: &InsnInfo) {
    for cb in &self.insn_exec { cb(vcpu, insn); }
    for cb in &self.insn_exec_filtered { cb.call(insn); }
}
```

**Note**: `vcpu` is not available inside `FilteredCb<InsnInfo>` in the current codebase
because it is passed alongside the event as a separate parameter, not embedded in it.
Adding `vcpu_idx` to `InsnInfo` (see §2.3) resolves this and is required before the
`vcpu()` stock filter can work. The HLD enrichment table and TEST.md already reflect the
post-addition state.

### 2.2 Existing `MemFilter` compatibility

`MemFilter` (All/ReadsOnly/WritesOnly) is preserved as-is for the unfiltered path. For
the filtered path, the same behaviour is achieved via a predicate:

```rust
reg.on_mem_access_filtered(
    FilteredCb::new(move |info: &MemInfo| { … })
        .filter(|info| info.is_store)   // WritesOnly equivalent
);
```

### 2.3 Adding `vcpu_idx` to event types

To support per-vCPU filtering, `InsnInfo` and `BranchInfo` gain `vcpu_idx: usize`:

```rust
pub struct InsnInfo {
    pub vcpu_idx: usize,   // ← new
    pub pc: u64,
    // … rest unchanged
}
```

This is a source-level breaking change to `InsnInfo` but requires no ABI change (no
extern C). All callers are in-crate.

---

## 3. Stock Filters

Standard predicate factories in `helm-plugin::filter`:

```rust
// framework/helm-plugin/src/runtime/filter.rs  (continued)

use crate::runtime::{BranchInfo, InsnInfo, InsnClass, MemInfo};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ── InsnInfo filters ─────────────────────────────────────────────────────────

/// Only fire for instructions with PC in [start, end).
pub fn pc_range(start: u64, end: u64) -> impl Fn(&InsnInfo) -> bool + Send + Sync {
    move |i| i.pc >= start && i.pc < end
}

/// Only fire for a specific instruction class.
pub fn insn_class(cls: InsnClass) -> impl Fn(&InsnInfo) -> bool + Send + Sync {
    move |i| i.class == cls
}

/// Only fire for stub instructions (unimplemented; returned default value).
pub fn is_stub() -> impl Fn(&InsnInfo) -> bool + Send + Sync {
    |i| i.is_stub
}

/// Fire for every Nth instruction (deterministic sampling).
///
/// Uses a shared `AtomicU64` counter, safe for multi-vcpu use.
pub fn sample_every(n: u64) -> impl Fn(&InsnInfo) -> bool + Send + Sync {
    let counter = Arc::new(AtomicU64::new(0));
    move |_| counter.fetch_add(1, Ordering::Relaxed) % n == 0
}

/// Only fire while the instruction count is in [start, end).
///
/// `InsnInfo` must have `insn_count` (available with `--features probe-full`).
/// Without that feature this filter is always-true.
pub fn insn_window(start: u64, end: u64) -> impl Fn(&InsnInfo) -> bool + Send + Sync {
    move |i| {
        #[cfg(feature = "probe-full")]
        { i.insn_count >= start && i.insn_count < end }
        #[cfg(not(feature = "probe-full"))]
        { let _ = (start, end); true }
    }
}

/// Only fire for a specific vCPU index.
pub fn vcpu(idx: usize) -> impl Fn(&InsnInfo) -> bool + Send + Sync {
    move |i| i.vcpu_idx == idx
}

// ── BranchInfo filters ───────────────────────────────────────────────────────

/// Only fire for taken branches.
pub fn taken_only() -> impl Fn(&BranchInfo) -> bool + Send + Sync {
    |b| b.taken
}

/// Only fire for branches to a target PC in [start, end).
pub fn branch_target_range(start: u64, end: u64) -> impl Fn(&BranchInfo) -> bool + Send + Sync {
    move |b| b.target >= start && b.target < end
}

/// Only fire for a specific branch kind.
pub fn branch_kind(kind: crate::runtime::BranchKind) -> impl Fn(&BranchInfo) -> bool + Send + Sync {
    move |b| b.kind == kind
}

// ── MemInfo filters ──────────────────────────────────────────────────────────

/// Only fire for accesses to addresses in [start, end).
pub fn mem_addr_range(start: u64, end: u64) -> impl Fn(&MemInfo) -> bool + Send + Sync {
    move |m| m.vaddr >= start && m.vaddr < end
}

/// Only fire for stores.
pub fn stores_only() -> impl Fn(&MemInfo) -> bool + Send + Sync {
    |m| m.is_store
}

/// Only fire for loads.
pub fn loads_only() -> impl Fn(&MemInfo) -> bool + Send + Sync {
    |m| !m.is_store
}

/// Only fire for atomic accesses.
pub fn atomics_only() -> impl Fn(&MemInfo) -> bool + Send + Sync {
    |m| m.is_atomic
}
```

### 3.1 Usage example — filtered subscription

```rust
use helm_plugin::{filter, FilteredCb};
use helm_plugin::runtime::{InsnClass};

// Only trace branch instructions in the kernel text (0xffff_8000_0000_0000 …)
reg.on_branch_filtered(
    FilteredCb::new(|info: &BranchInfo| {
        println!("branch pc={:#x} → {:#x}", info.pc, info.target);
    })
    .filter(filter::branch_target_range(0xffff_8000_0000_0000, 0xffff_ffff_ffff_ffff))
    .filter(filter::taken_only())
);

// Sample 1-in-100 instructions, only IntAlu class
reg.on_insn_exec_filtered(
    FilteredCb::new(|info: &InsnInfo| { /* profile this */ })
        .filter(filter::insn_class(InsnClass::IntAlu))
        .filter(filter::sample_every(100))
);
```

---

## 4. Chain — Sequential Stages

A `Chain<T>` sequences multiple `FilteredCb<T>` stages. All stages see every event
(each with its own predicates). No early-exit between stages — chains are not pipelines
with stop-propagation semantics. For stop-propagation, use a single `FilteredCb` that
internally does conditional work.

```rust
// framework/helm-plugin/src/runtime/chain.rs

use super::filter::FilteredCb;

/// A sequential chain of filtered callbacks for event type `T`.
///
/// All stages fire (subject to their own predicates). Stage order is stable.
pub struct Chain<T> {
    stages: Vec<FilteredCb<T>>,
}

impl<T: 'static> Chain<T> {
    pub fn new() -> Self { Self { stages: Vec::new() } }

    /// Append a stage (builder pattern).
    pub fn stage(mut self, cb: FilteredCb<T>) -> Self {
        self.stages.push(cb);
        self
    }

    /// Append a bare callback as a stage (no predicates).
    pub fn then(self, f: impl Fn(&T) + Send + Sync + 'static) -> Self {
        self.stage(FilteredCb::new(f))
    }

    /// Fire all stages in order.
    pub fn fire(&self, val: &T) {
        for stage in &self.stages {
            stage.call(val);
        }
    }
}

impl<T: 'static> Default for Chain<T> {
    fn default() -> Self { Self::new() }
}
```

### 4.1 Usage example — chain of stages

```rust
use helm_plugin::{filter, Chain, FilteredCb};

// Stage 1: sample every 1000 insns
// Stage 2 (only for branches): log the branch target
// Stage 3: always count
let mut branch_count = 0u64;
let chain: Chain<BranchInfo> = Chain::new()
    .stage(
        FilteredCb::new(|info: &BranchInfo| {
            eprintln!("branch → {:#x}", info.target);
        })
        .filter(filter::taken_only())
        .filter(filter::branch_target_range(0x4000_0000, 0x8000_0000))
    )
    .then(move |_| { branch_count += 1; });

// Install the chain as a single callback:
reg.on_branch(Box::new(move |_vcpu, info| chain.fire(info)));
```

### 4.2 Type-transforming chains

When stages have different types (e.g. `InsnInfo → String → TraceSink`), write an
explicit adapter rather than a generic `Chain<A, B>`. The plugin framework does not
provide a typed-transform chain — the overhead of a trait object per stage is not
justified when a simple closure captures all the state.

---

## 5. `TraceSink` — Delivery Abstraction

`TraceSink` decouples **event generation** (collect data, format a string) from
**event delivery** (write to file, stderr, TCP, or in-memory buffer).

```rust
// framework/helm-plugin/src/runtime/sink.rs

use helm_debug::sim_trace;

/// Where a plugin or chain stage writes its output.
///
/// Cloneable so that multiple plugins can share a sink (e.g. all route to the
/// same log file via a shared `MonitorSink`).
#[derive(Clone)]
pub enum TraceSink {
    /// Write to stderr (default — current atexit! behavior).
    Stderr,

    /// Route via the sim_trace async channel.
    ///
    /// Output is mixed with engine diagnostics (STUB, WARN, BRNC) and routed
    /// to the configured MonitorSink backend (file, TCP, null).
    SimTrace {
        monitor:   sim_trace::Monitor,
        component: &'static str,
        level:     sim_trace::Level,
    },

    /// Append to an in-memory buffer. Use for tests or in-process consumers.
    Buffer(std::sync::Arc<std::sync::Mutex<Vec<String>>>),

    /// Discard silently (benchmarking or collector-only plugins).
    Null,
}

impl TraceSink {
    /// Write a single formatted line to the sink.
    pub fn write_line(&self, line: &str) {
        match self {
            TraceSink::Stderr => eprintln!("{line}"),

            TraceSink::SimTrace { monitor, component, level } => {
                let (sim_ns, sim_insns) = sim_trace::SIM_CTX.with(|c| {
                    let ctx = c.borrow();
                    (ctx.sim_ns, ctx.sim_insns)
                });
                monitor.try_send(sim_trace::MonitorEntry {
                    sim_ns,
                    sim_insns,
                    component,
                    level: *level,
                    pc: None,
                    message: line.to_string(),
                });
            }

            TraceSink::Buffer(buf) => {
                buf.lock().unwrap().push(line.to_string());
            }

            TraceSink::Null => {}
        }
    }

    /// Write a line with an explicit PC annotation (used by SimTrace path).
    pub fn write_line_pc(&self, line: &str, pc: u64) {
        match self {
            TraceSink::SimTrace { monitor, component, level } => {
                let (sim_ns, sim_insns) = sim_trace::SIM_CTX.with(|c| {
                    let ctx = c.borrow();
                    (ctx.sim_ns, ctx.sim_insns)
                });
                monitor.try_send(sim_trace::MonitorEntry {
                    sim_ns, sim_insns, component,
                    level: *level,
                    pc: Some(pc),
                    message: line.to_string(),
                });
            }
            other => other.write_line(line),
        }
    }
}
```

### 5.1 Plugin trait extension

```rust
pub trait HelmPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn install(&mut self, reg: &mut PluginRegistry, args: &PluginArgs);
    fn atexit(&mut self) {}

    /// Set delivery sink. Called before `install()`. Default: `TraceSink::Stderr`.
    fn set_sink(&mut self, _sink: TraceSink) {}
}
```

Plugins store the sink as a field and use it in `atexit()` (for summary output) or
inside their callbacks (for per-event output):

```rust
// ExecLog updated:
pub struct ExecLog {
    lines: Arc<Mutex<Vec<String>>>,
    sink:  TraceSink,   // ← new field
}

impl HelmPlugin for ExecLog {
    fn set_sink(&mut self, sink: TraceSink) { self.sink = sink; }

    fn atexit(&mut self) {
        let guard = self.lines.lock().unwrap();
        for line in guard.iter() {
            self.sink.write_line(&format!("[execlog] {line}"));
        }
    }
}
```

### 5.2 `TraceSink` construction helpers

```rust
impl TraceSink {
    /// Construct a SimTrace sink using the thread-local monitor (if installed).
    ///
    /// Falls back to Stderr if no MonitorSink is running.
    pub fn from_thread_local(component: &'static str, level: sim_trace::Level) -> Self {
        let monitor = sim_trace::SIM_MONITOR.with(|cell| cell.borrow().clone());
        match monitor {
            Some(m) => TraceSink::SimTrace { monitor: m, component, level },
            None    => TraceSink::Stderr,
        }
    }
}
```

---

## 6. Level Filtering in `sim_trace`

`MonitorSink` gains a `min_level` field. Entries below `min_level` are discarded before
`try_send()`.

### 6.1 Level ordering

```rust
impl Level {
    /// Numeric severity. Higher = more severe.
    fn severity(self) -> u8 {
        match self {
            Level::Branch => 0,
            Level::Info   => 1,
            Level::Stub   => 2,
            Level::Warn   => 3,
            Level::Error  => 4,
        }
    }
}

impl PartialOrd for Level {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.severity().cmp(&other.severity()))
    }
}
```

### 6.2 `emit()` with level gate

```rust
pub fn emit(level: Level, component: &'static str, pc: Option<u64>, message: String) {
    // … build MonitorEntry …
    let sent = SIM_MONITOR.with(|cell| {
        if let Some(ref m) = *cell.borrow() {
            // Only send if level meets the sink's minimum
            // (min_level is stored in Monitor, not MonitorSink, to avoid Arc overhead)
            if level >= m.min_level {
                m.try_send(entry.clone());
                return true;
            }
        }
        false
    });
    if !sent && level != Level::Branch {
        eprintln!("{}", entry.format());
    }
}
```

`Monitor` gains `pub min_level: Level` (default `Level::Branch` = pass all).

### 6.3 CLI flag

```
--sim-trace=stderr:              # output destination (default)
--sim-trace=file:/tmp/trace.log  # output to file
--sim-trace=tcp:localhost:9000   # stream to socket
--sim-trace=null:                # discard (benchmarking)

--sim-trace-level=branch         # pass everything (default)
--sim-trace-level=info           # suppress BRNC
--sim-trace-level=warn           # suppress BRNC + INFO
--sim-trace-level=stub           # suppress BRNC + INFO + WARN
--sim-trace-level=error          # only errors
```

---

## 7. Plugin Pause / Resume

An `AtomicBool` gate on `PluginRegistry` allows temporarily silencing all callbacks
without unregistering them:

```rust
pub struct PluginRegistry {
    // … existing fields …
    paused: std::sync::atomic::AtomicBool,
}

impl PluginRegistry {
    /// Temporarily disable all callback dispatch. Thread-safe.
    pub fn pause(&self) {
        self.paused.store(true, std::sync::atomic::Ordering::Release);
    }

    /// Resume callback dispatch.
    pub fn resume(&self) {
        self.paused.store(false, std::sync::atomic::Ordering::Release);
    }

    pub fn fire_insn_exec(&self, vcpu: usize, insn: &InsnInfo) {
        if self.paused.load(std::sync::atomic::Ordering::Acquire) { return; }
        for cb in &self.insn_exec { cb(vcpu, insn); }
        for cb in &self.insn_exec_filtered { cb.call(insn); }
    }
    // … same for other fire_* methods …
}
```

`pause()`/`resume()` use acquire/release ordering. The `paused` check costs one
atomic load per event type per fire — acceptable because `has_*_callbacks()` already
gates the entire fire block.

Python API:
```python
sim.plugins.pause()
sim.run(1_000_000)
sim.plugins.resume()
```

---

## 8. `sim_branch!` ↔ Plugin Bridge Connection

`sim_branch!` and `Probe<BranchInfo>` fire from the same sites in `branch.rs` but serve
different consumers. They are not unified into one call.

`sim_branch!` always invokes `emit()` and `try_send()` in both debug and release builds
— it has a small constant cost even in release (one function call, dropped immediately
if no monitor is installed). This is intentional: it is an always-on diagnostic channel.

`Probe<BranchInfo>` is zero-sized in release — the entire `probe!(...)` block is
eliminated. It is the right choice when zero overhead in release is required.

### Connection diagram

```
branch.rs execute path
    │
    ├── sim_branch!(pc=pc, target=target)          ← always fires (drop if no monitor)
    │       └──► MonitorSink backend (file/TCP)    ← consumed by branch_trace.py
    │
    └── probe!(probes.branch, BranchInfo{...})     ← zero cost in release
            └──► ProbePluginBridge.fire_branch()
                    └──► PluginRegistry.fire_branch()
                            └──► BranchTrace plugin (in-process analysis)
```

**Note**: `CpuProbes` does not currently include a `branch` field — only `pre_step`,
`post_step`, `fault`, `mem`. Adding `branch: Probe<BranchInfo>` to `CpuProbes` is a
Phase 2 item. When added, the bridge subscribes to it and routes to `fire_branch()`,
making the sim_branch! + plugin connection explicit.

`BranchInfo` definition for the probe:
```rust
// In helm-probe/src/events.rs (Phase 2 addition)
#[derive(Debug, Clone)]
pub struct BranchEvent {
    pub pc:     u64,
    pub target: u64,
    pub taken:  bool,
    pub kind:   BranchKind,  // re-exported from helm-plugin::BranchKind
}
```

Or equivalently, the bridge constructs a `helm_plugin::BranchInfo` directly from
the raw `pc` and `target` captured in branch.rs.

---

## 9. File Summary

| File | Status | Contents |
|---|---|---|
| `framework/helm-probe/src/probe.rs` | implement | `Probe<T>` + impl |
| `framework/helm-probe/src/events.rs` | implement | 5 event types |
| `framework/helm-probe/src/macros.rs` | implement | `probe!()` |
| `framework/helm-probe/src/lib.rs` | implement | re-exports |
| `framework/helm-plugin/src/runtime/filter.rs` | **new** | `FilteredCb<T>`, stock filters |
| `framework/helm-plugin/src/runtime/chain.rs` | **new** | `Chain<T>` |
| `framework/helm-plugin/src/runtime/sink.rs` | **new** | `TraceSink` |
| `framework/helm-plugin/src/runtime/registry.rs` | **extend** | filtered variants, pause/resume |
| `framework/helm-plugin/src/bridge.rs` | **new** | `ProbePluginBridge` |
| `runtime/helm-debug/src/sim_trace.rs` | **extend** | `min_level`, level ordering |
| `runtime/helm-engine/src/lib.rs` | **extend** | `CpuProbes` field, bridge install |
| `runtime/helm-engine/src/fs.rs` | **extend** | `probe!()` call sites |
| `hw/helm-hw-intc/src/gicv2/mod.rs` | **extend** | `GicProbes` field (feature-gated) |
| `hw/helm-hw-intc/src/gicv2/distributor.rs` | **extend** | `probe!()` call sites |
