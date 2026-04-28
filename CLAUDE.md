# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project State

Active implementation. AArch64 SE+FS pipeline working. RISC-V RV64I+Zicsr decode/execute implemented. **`LinuxRiscv64SyscallHandler` and `helm-riscv64` are in tree** (`runtime/helm-engine/src/se/linux_riscv64.rs`, `runtime/helm-cli/src/bin/helm_riscv64.rs`). **Current focus:** Extend RISC-V syscall coverage, M/A/F/D, and `riscv-tests` gate — see `docs/plans/cursor-plan-00-roadmap.md` (§ RISC-V SE).

Read `AGENT.md` (400 lines) for the authoritative agent onboarding guide before working on this project.

## Control Plane Model

Helm does **not** need a separate QEMU-monitor analogue by default.

- Python is the first-class user interface for configure / start / stop / query / debug.
- `HelmSystem` / `HelmSpy` together act as the programmatic control and observation surface.
- All durable implementation belongs in Rust crates (`helm-engine`, `helm-debug`, `helm-spy`, `helm-report`, etc.).

In practice:

- prefer adding capabilities as Python-facing methods backed by Rust implementation
- do not move core control logic into Python scripts
- do not introduce a separate monitor shell or protocol unless there is a concrete external-tooling need that Python cannot satisfy

## Key Documentation

- `AGENT.md` — Agent onboarding: crate map, design rules, execution modes, object model
- `docs/ARCHITECTURE.md` — Full system architecture and type hierarchy
- `docs/design/HLD.md` — Canonical top-level design doc and crate DAG
- `docs/object-model.md` — SimObject lifecycle, wiring rules, checkpoint protocol
- `docs/traits.md` — All trait definitions (ExecContext, SimObject, Device, TimingModel, etc.)
- `docs/api.md` — Rust and Python API reference
- `docs/testing.md` — Testing strategy
- `docs/TODO.md` — Open work items
- `docs/plans/cursor-plan-00-roadmap.md` — Cursor execution hub (includes RISC-V SE status)
- `docs/design/<crate>/` — Per-crate HLD + LLD-*.md + TEST.md

## Build Commands

```bash
cargo build --workspace
cargo test --workspace
cargo test --package helm-arch          # ISA tests only
cargo test --lib --workspace            # Unit tests only
cargo clippy --all --all-targets -- -D warnings
cargo fmt --check
cargo doc --no-deps --open
```

## Debugging Discipline

**Never** add `eprintln!` / `println!` / ad-hoc `dbg!` to the simulator hot path. The repo has a coherent observability stack and adding scratch prints either silently regresses other workflows (the engine emits structured diagnostics that consumers parse) or gets reverted in review. Use the layered tools below in the order they appear.

### Layer 1: structured diagnostics (`helm-diag`)

`sim_stub!()`, `sim_warn!()`, and `sim_info!()` carry simulation time and instruction count. Every guest-visible warning (low-address abort, stub instruction, GIC illegal write, ...) goes through these. To turn the diagnostic stream on for a run, route the engine's `DiagMonitor` to stderr or a file (the `helm-aarch64` binary does this when `--diag` / similar flags are set; Python users get it via `HelmSpy`). New code that wants to emit a one-shot architectural warning belongs here, not behind `eprintln!`.

### Layer 2: probes (`helm-probe`)

`Probe<T>` is the primary low-overhead observation point. `CpuProbes` carries `pre_step`, `post_step`, `fault`, `mem`, `mmu`, `branch`; `JitProbes` carries block/trace compile + execute + cache + guard-exit; `GicProbes` carries IRQ assert/deassert/EOI. `CpuProbes::any_active()` returns `false` when no listener is registered, so subscribing only when actually needed costs nothing on the hot loop. Reach for a new probe (and a matching event struct in `framework/helm-probe/src/events.rs`) when you need a typed signal at a fixed point in the engine.

### Layer 3: built-in plugins (callback compatibility)

`helm-plugin` is the legacy callback layer; new long-term observation work should prefer probes + `helm-spy`. The built-in plugins are still the right tool for short-lived investigation because they install in one line from Python:

```python
sim.add_plugin("hvc-trace", "kind=both,max=128")        # HVC + SMC entries with x0..x7
sim.add_plugin("pc-trace", "pc=0x4eac0c,regs=full,dump=both")
sim.add_plugin("trace-window-fault", "before=64,after=8")
sim.add_plugin("fault-detect", "history=64")
sim.add_plugin("watchpoint", "addr=0x4000_0000,size=8,kind=write")
sim.add_plugin("stub-tracer")
sim.add_plugin("register-dump", "vcpu=0")
sim.add_plugin("execlog")        # per-instruction execution log
sim.add_plugin("syscall-trace")  # SE-mode SVC trace
sim.add_plugin("branch-trace")
sim.add_plugin("hotblocks")      # hottest PC ranges
sim.add_plugin("howvec")         # opcode mix
sim.add_plugin("insn-count")
sim.add_plugin("mem-trace")
sim.add_plugin("cache")          # cache-sim model
sim.add_plugin("jit-execlog")
```

The catalogue is wired in [system.rs](/home/pmallapp/proj/personal/helm-ng/runtime/helm-python/src/system.rs) (`HelmSystem::add_plugin`); each plugin lives under [framework/helm-plugin/src/builtins/](/home/pmallapp/proj/personal/helm-ng/framework/helm-plugin/src/builtins/) and ships its own `args` schema (`key=value,...`). Most emit through `sim_info!` so output appears on the diag stream — there's no separate sink to wire up.

Notable plugins worth knowing:

- `hvc-trace` (debug): HVC/SMC trap entries with imm16 + x0..x7 + ELR + EL transition. Lives on the new `on_exception` hook so it has zero per-instruction cost. Use this before debugging anything in EL2-bound payloads (PSCI, L4 IPC, KVM hypercalls) — `kind=both` for SMC + HVC.
- `pc-trace` (debug): repeated executions at a specific PC plus the memory accesses that instruction generated; supports range mode (`pc_start`/`pc_end`) and dumps on fault or atexit.
- `trace-window-fault` (debug): retains the last N instructions before any fault and dumps a window around the failing PC. The first thing to install when chasing an unknown fault.
- `watchpoint` (debug): traps on read/write/either at a memory address.
- `fault-detect` (debug): rolling history of recent instructions + syscalls; surfaces them when a fault fires.
- `stub-tracer` (debug): records every silently-skipped unimplemented stub instruction with PC and opcode name. Run this when guest progress stalls without an obvious abort.
- `register-dump` (debug): periodic full register snapshot for a chosen vCPU.
- `execlog` / `syscall-trace` / `branch-trace` (trace): per-event logs for guest execution, SE-mode SVC, and branches.

### Layer 4: spy + report (`helm-spy` + `helm-report`)

For analyses that span more than a single run or need durable output, `HelmSpy` consumes probes/events and `helm-report` formats them (JSON, CSV). This is the Python-first path the [helm-observability-debug-roadmap](/home/pmallapp/proj/personal/helm-ng/docs/plans/helm-observability-debug-roadmap.md) is converging on. New analysis work should land here rather than as a new built-in plugin.

### Layer 5: `helm-debug` (control plane)

`helm-debug` owns the synchronous debug-control surface: `GdbServer` (RSP stub), `WatchpointEngine` (the same machinery the `watchpoint` plugin uses), `CheckpointManager`, replay anchors, and trace-window deferred logging. Reach for these when you need *interactive* control rather than passive observation.

### When to instrument vs. when to print

| Symptom | Reach for |
|---|---|
| Need to know what a specific PC is doing repeatedly | `pc-trace` |
| Guest stuck without an abort | `stub-tracer`, then `trace-window-fault` |
| Unknown fault — need context before it | `trace-window-fault`, `fault-detect` |
| EL2 payload misbehaving (PSCI / L4 IPC / KVM HVC) | `hvc-trace` |
| Bad memory write to a known address | `watchpoint`, `mem-trace` |
| Guest emits surprising syscalls | `syscall-trace`, `fault-detect` (it logs syscalls in the rolling history) |
| Hot-loop perf regression | `hotblocks`, `howvec`, `insn-count` |
| New typed signal at a fixed engine point | new `Probe<T>` event + `helm-spy` consumer |
| One-shot guest-visible warning from the engine | `sim_warn!` / `sim_stub!` |

If none of those fit, prefer adding a new event to `helm-probe` over inserting a stray print.

## Workspace Layout (domain-based)

**`framework/`** — stable APIs and shared primitives

| Crate | Key Types | Notes |
|---|---|---|
| `helm-core` | `ArchState`, `ExecContext`, `ThreadContext`, `MemInterface` | Zero helm-* deps |
| `helm-memory` | `FlatMem`, `HelmAddressSpace`, `SharedDmaPort`, experimental `MemoryMap` | `FlatMem` + `HelmAddressSpace` are the live runtime surfaces |
| `helm-timing` | `VirtualTiming`, `IntervalTiming`, `AccurateTiming`, `TimingModel` | Three timing models |
| `helm-event` | `EventQueue`, `EventClass`, `PendingEvent` | BinaryHeap, discrete-event |
| `helm-devices` | `Device` trait, `InterruptPin`, `HelmEventBus`, `DeviceRegistry` | SDK only; bus controllers here |
| `helm-stats` | `PerfCounter`, `PerfHistogram`, `StatsRegistry` | Dot-path namespaced |
| `helm-plugin` | `HelmPluginRegistry`, `PluginDescriptor` | Legacy callback-compatibility layer |
| `helm-decode` | `DecodeTree`, `Pattern`, `Field` | QEMU-style .decode parser + codegen |
| `helm-jit` | `JitBackend` trait, cache, dynasm/stencil backends | `jit-tiered` feature: stencil baseline + dynasm hot-tier |

**`runtime/`** — execution engine and frontends

| Crate | Key Types | Notes |
|---|---|---|
| `helm-arch` | `RiscvArchState`, `Aarch64ArchState`, `Instruction` | ISA decode+execute only |
| `helm-engine` | `HelmEngine<T>`, `HelmSim`, `ExecMode`, `FlatMem` | `se/` = syscall handlers; `fs.rs` = FS-mode step loop |
| `helm-platform` | `ArmVirtPlatform` | ARM virt machine builder, loads kernel/DTB/initrd |
| `helm-debug` | `GdbServer`, `TraceLogger`, `CheckpointManager` | GDB RSP stub |
| `helm-python` | PyO3 → `_helm_ng` module | `python/helm/` package; SimObject, System, CPU, RAM |
| `helm-cli` | `helm-aarch64` binary | CLI launcher with embedded Python |

**`hw/`** — concrete hardware implementations

| Crate | Devices |
|---|---|
| `helm-hw-char` | PL011 UART |
| `helm-hw-timer` | SP804 dual timer |
| `helm-hw-rtc` | PL031 RTC |
| `helm-hw-dma` | DMA engine |
| `helm-hw-intc` | GICv2 (distributor + CPU interface) |
| `helm-hw-pci` | PCI ECAM host bridge |
| `helm-hw-virtio` | VirtIO MMIO transport |

**`debug/`** — analysis and delivery

| Crate | Purpose |
|---|---|
| `helm-spy` | Analysis models, `HelmSpy` |
| `helm-report` | Output sinks (JSON, CSV) |

## Critical Design Rules (inviolable)

1. **Monomorphize timing only** — `HelmEngine<T: TimingModel>` is the sole generic parameter; ISA/mode dispatch via enum
2. **ISA/mode are enum-dispatched** — one `match` per Python call, zero per instruction
3. **No dark state** — every persistent field must be a registered `AttrDescriptor`; unregistered = lost on restore
4. **Device knows no base address** — the live address-space owner (`HelmAddressSpace` today) owns placement; the device sees only `offset`
5. **Device knows no IRQ number** — `InterruptPin::assert()` fires the signal; platform owns routing
6. **No dynamic lookup in the hot loop** — store all cross-component `Arc` refs during `elaborate()`
7. **Python describes; Rust simulates** — config frozen after `build_simulator()`; no mutation during sim
8. **Determinism by default** — no wall-clock, no background threads in the hot loop
9. **`HelmEventBus` is synchronous** — not checkpointed; subscribers re-register in `init()` on every load
10. **`init()` is self-contained** — no cross-component access; that happens in `elaborate(system)`

## SimObject Lifecycle

```
CONSTRUCT → init() → elaborate(system) → startup() → RUN → reset() / checkpoint_save/restore
```

- `init()`: internal state only, no cross-component
- `elaborate(system)`: register MMIO, store `Arc` refs, wire interrupts
- `startup()`: schedule initial events, assert initial signals
- `reset()`: return to post-startup state, idempotent
- `checkpoint_save/restore`: architectural state only — no perf counters

## Irreducible Core

Every execution path reduces to:
1. **ArchState** — register file + PC
2. **Decoder** — bytes → Instruction
3. **Executor** — (ArchState, Insn, MemInterface) → ΔArchState
4. **MemInterface** — read/write(addr, size) ↔ bytes

## Two Distinct Event Systems

- **EventQueue** (`helm-event`): schedule callbacks at future tick T — **asynchronous/deferred**
- **HelmEventBus** (`helm-devices/bus`): observable named events — **synchronous/inline, not checkpointed**

## `HelmSim` — PyO3 Boundary

```rust
pub enum HelmSim {
    VirtualTiming(HelmEngine<VirtualTiming>),    // >100 MIPS
    IntervalTiming(HelmEngine<IntervalTiming>),  // Sniper-style, <15% MAPE, >10 MIPS
    AccurateTiming(HelmEngine<AccurateTiming>),  // cycle-accurate, <10% IPC err, >200 KIPS
    Hardware(HardwareEngine),                    // KVM/HVMX — real hardware timing
}
```

`HelmSim` is the sole object exposed to Python. All Python calls enter here; ISA and mode dispatched once per call, not per instruction.

## Primary Device Modeling Primitive

`register_bank!` macro — replaces manual MMIO switch statements:

```rust
register_bank! {
    UartRegs for Uart16550 at offset 0x0 {
        reg RHR @ 0x00 is read_only  { field DATA [7:0] }
        reg THR @ 0x00 is write_only { field DATA [7:0] }
        reg LSR @ 0x14 is read_only  { field THRE [5]; field DR [0] }
    }
}
// Generates: MmioHandler impl, serde checkpoint, AttrDescriptors, Python introspection
```

## Quick Reference

| Need to... | Look at |
|---|---|
| Add a new ISA | `helm-arch/src/{new_isa}/` + implement `Hart` trait from `helm-core` |
| Add a new device | Implement `Device` trait from `helm-devices`, use `register_bank!` |
| Add a new timing model | Implement `TimingModel` from `helm-timing`, add variant to `HelmSim` |
| Add a RISC-V syscall | `helm-engine/src/se/linux_riscv64.rs` |
| Add an AArch64 syscall | `helm-engine/src/se/linux_aarch64.rs` |
| Add a GDB packet | `helm-debug/src/gdb_server.rs` |
| Change Python API | `helm-python/src/` + `python/helm/` |
| Debug a checkpoint | All persistent state must be in `AttrStore` with `AttrKind::Required` |

## Python-First Rule

When adding simulator control or observability features:

1. Python should expose the feature first-class.
2. Rust should implement the feature.
3. Python should orchestrate, not contain the simulator logic.

Examples:

- good: `HelmSystem.breakpoint()` backed by `helm-debug`
- good: `HelmSpy.write_report()` backed by `helm-report`
- bad: a Python-only monitor workflow that cannot be reproduced through Rust APIs

## Naming Reference (use these exact names)

| Correct Name | Location |
|---|---|
| `HelmEngine<T>` | helm-engine |
| `HelmSim` | helm-engine |
| `VirtualTiming` / `IntervalTiming` / `AccurateTiming` | helm-timing |
| `ExecMode::Functional` / `::Syscall` / `::System` / `::Hardware` | helm-engine |
| `HardwareEngine` | helm-engine/src/kvm/ |
| `HelmAddressSpace` | helm-memory/src/address_space.rs |
| `HelmSpy` | helm-spy |
| `DiagContext` | helm-diag |
| `DeviceContext` | helm-devices |
| `HelmPluginRegistry` | helm-plugin |
| `TimingInsnInfo` | helm-timing |
| `HelmSystem` (Rust) / `"System"` (Python) | helm-python |

## Phased Build Plan

| Phase | Deliverables | Status |
|---|---|---|
| **0 — MVP** | RISC-V SE simulator, riscv-tests pass | In progress — RV64I+Zicsr done; handler + `helm-riscv64` in tree |
| **1** | AArch64 SE+FS, GDB stub, ARM virt platform, timing | Largely done |
| **2** | RISC-V SE completion, riscv-tests gate | **Current** |
| **3** | Boot Linux RISC-V, OoO pipeline, AArch32, JIT | Future |

## Testing Strategy

- **ISA correctness**: official riscv-tests vectors + AArch64 torture tests
- **Differential testing**: QEMU/Spike traces vs. helm-ng execution
- **Property-based**: `proptest` for memory layouts and instruction sequences
- **Benchmarks**: `criterion` for IPC accuracy regressions

See `docs/testing.md` for the full strategy and planned test locations per crate.
