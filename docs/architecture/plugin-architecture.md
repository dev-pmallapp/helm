# Plugin Architecture

How helm-ng supports instrumentation, analysis, and extensibility
through plugins and probes.

## Two Systems

helm-ng has two complementary instrumentation mechanisms:

| System | Crate | Purpose | Overhead |
|--------|-------|---------|----------|
| **Plugins** | `helm-plugin` | Analysis callbacks (instruction trace, cache sim) | Function call per event |
| **Probes** | `helm-probe` | Typed observation points | Zero-cost when inactive |

## Plugin System (helm-plugin)

### HelmPlugin Trait

```rust
pub trait HelmPlugin: Send {
    fn name(&self) -> &str;
    fn on_init(&mut self, args: &HelmPluginArgs);
    fn on_insn(&mut self, pc: u64, insn_bytes: u32);
    fn on_mem(&mut self, addr: u64, size: usize, is_write: bool);
    fn on_syscall(&mut self, num: u64);
    fn on_exit(&mut self);
}
```

### HelmPluginRegistry

`HelmPluginRegistry` manages plugin lifecycle:

```rust
// Register a plugin
registry.register(Box::new(MyPlugin::new()));

// Plugins receive callbacks during simulation
// (called by HelmEngine at appropriate points)
```

### HelmPluginArgs

Initialization arguments passed to `on_init()`:

| Field | Description |
|-------|-------------|
| ISA | Current ISA (AArch64, RISC-V) |
| Mode | Execution mode (FE, SE, FS) |
| Config | Plugin-specific configuration map |

### Built-in Plugins

When the `builtins` feature is enabled, `helm-plugin` includes
pre-built instrumentation plugins.

## Probe Framework (helm-probe)

### Design Goal

Probes provide typed, zero-cost observation points. In release builds
with no active listeners, probe checks compile to a single
branch-not-taken instruction.

### Probe\<T\>

```rust
pub struct Probe<T> {
    // In release: zero-sized (no listeners → no cost)
    // In dev: Vec<Box<dyn Fn(&T)>>
}
```

### Pre-defined Probe Bundles

**CpuProbes** — per-CPU observation points:

| Probe | Event Type | Fires When |
|-------|-----------|------------|
| `pre_step` | `CpuStepEvent` | Before each instruction |
| `post_step` | `CpuStepEvent` | After each instruction |
| `fault` | `CpuFaultEvent` | On exception or fault |
| `mem` | `MemAccessEvent` | On load or store |
| `branch` | `BranchEvent` | On branch instruction |

**GicProbes** — interrupt controller observation:

| Probe | Fires When |
|-------|------------|
| `irq_asserted` | Interrupt line asserted |
| `irq_deasserted` | Interrupt line deasserted |
| `eoi` | End of interrupt acknowledged |

### Instruction Classification

`InsnClass` categorizes instructions for probe consumers:

```text
IntAlu, IntMul, Branch, Load, Store, FpAlu, SimdAlu,
System, Nop, Atomic, Unknown
```

### Performance

`CpuProbes::any_active()` returns `false` when no listeners are
registered, allowing the hot loop to skip all probe overhead with a
single check.

The `probe-full` feature flag enables richer event fields at the cost
of additional data collection.

## Diagnostic Channel (helm-diag)

`helm-diag` provides a separate structured logging system:

| Type | Purpose |
|------|---------|
| `DiagEntry` | Structured log entry (message, level, context) |
| `DiagLevel` | Error, Warn, Stub, Info |
| `DiagContext` | Simulation time (ns, insns) |
| `DiagMonitor` | Cloneable sender handle (thread-local) |

The `sim_stub!()`, `sim_warn!()`, and `sim_info!()` macros emit
diagnostics with simulation context. Unlike `log` crate macros, these
carry simulation time and instruction count.

## Comparison

| Aspect | QEMU | gem5 | Simics | helm-ng |
|--------|------|------|--------|---------|
| Plugin API | TCG plugin (C) | Probes (C++) | HAPs (C) | `HelmPlugin` (Rust) |
| Zero-cost probes | No | No | No | Yes (`Probe<T>`) |
| Diagnostics | `error_report()` | `DPRINTF` | `SIM_log` | `DiagMonitor` |
| Built-in plugins | icount, lockstep | Various | — | `builtins` feature |
| Dynamic loading | `.so` plugins | Compile-time | `.so` modules | Trait objects |
