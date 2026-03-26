# helm-ng Agent Onboarding

> Read this first. This document gives any AI agent everything needed to work on helm-ng
> without reading all 60+ design documents.

---

## What Is helm-ng?

A next-generation CPU/system simulator. **Rust core, Python config.** Multi-ISA (RISC-V RV64GC first, then ARM AArch64/AArch32). Inspired by:

| Simulator | What we take |
|-----------|-------------|
| **Gem5** | Python-drives-Rust two-phase config, typed param system, port-based memory interface |
| **SIMICS** | Object model (all state through attributes), named interface registry, HAP event bus, attribute = checkpoint |
| **QEMU** | MemoryRegion tree (unified RAM/MMIO/ROM/Alias), IOThread for async host IO |
| **Higan/ares** | Absolute scheduler (scalar normalization), JIT sync on device-register access, cooperative coroutine model |
| **Sniper** | Interval simulation (10–20% MAPE at 10× gem5 speed), CPI stack output |
| **PTLsim** | Cycle-accurate OoO pipeline depth (~5% IPC error ceiling) |

**Current state:** Active implementation. AArch64 SE+FS pipeline working. RISC-V RV64I+Zicsr decode/execute implemented. See Phase Build Plan below for what is complete.

---

## Crate Architecture (domain-based layout)

Workspace members: `framework/*`, `runtime/*`, `hw/*`, `debug/*`.

### `framework/` — stable APIs, shared primitives

| Crate | Key Types | Notes |
|-------|-----------|-------|
| `helm-core` | `ArchState` (trait), `ExecContext` (hot), `ThreadContext` (cold), `HartException`, `MemFault`, `MemInterface` | Zero helm-* deps. |
| `helm-memory` | `MemoryRegion`, `MemoryMap`, `FlatView`, `CacheModel`, `TlbModel` | QEMU-inspired MemoryRegion tree. |
| `helm-event` | `EventQueue`, `EventClass`, `PendingEvent` | Time-ordered discrete events. BinaryHeap. |
| `helm-devices` | `Device` trait, `InterruptPin`, `DeviceRegistry`, `HelmEventBus`, `Bus`, `BusDevice` | SDK only. HelmEventBus + AMBA/I2C/SPI bus controllers live here. |
| `helm-timing` | `VirtualTiming`, `IntervalTiming`, `AccurateTiming`, `TimingModel` trait, `MicroarchProfile` | Three timing models. |
| `helm-stats` | `PerfCounter` (AtomicU64), `PerfHistogram`, `StatsRegistry` | Dot-path namespaced stats. |
| `helm-plugin` | `HelmPluginRegistry`, `PluginDescriptor` | Engine extension/plugin system. |
| `helm-decode` | `DecodeTree`, `Pattern`, `Field` | QEMU-style .decode file parser + code generator. |

### `runtime/` — execution engine and frontends

| Crate | Key Types | Notes |
|-------|-----------|-------|
| `helm-arch` | `RiscvArchState`, `Aarch64ArchState`, `Instruction` (per-ISA enum) | `src/{riscv,aarch64,aarch32}/`. Decode + execute only. |
| `helm-engine` | `HelmEngine<T>`, `HelmSim`, `ExecMode`, `Isa`, `FlatMem`, `StopReason` | Kernel. `se/` has syscall handlers; `fs.rs` has FS-mode step loop. |
| `helm-platform` | `ArmVirtPlatform`, platform wiring | ARM virt machine builder. Loads kernel/DTB/initrd. |
| `helm-debug` | `GdbServer` (RSP), `TraceLogger`, `CheckpointManager` | Built in from Phase 0. |
| `helm-python` | PyO3 bindings → `_helm_ng` module | `python/helm/` package. SimObject, System, CPU, RAM. |
| `helm-cli` | `helm-aarch64` binary | CLI launcher with embedded Python. |

### `hw/` — concrete hardware implementations

| Crate | Devices |
|-------|---------|
| `helm-hw-char` | PL011 UART |
| `helm-hw-timer` | SP804 dual timer |
| `helm-hw-rtc` | PL031 RTC |
| `helm-hw-dma` | DMA engine |
| `helm-hw-intc` | GICv2 (distributor + CPU interface) |
| `helm-hw-pci` | PCI ECAM host bridge |
| `helm-hw-virtio` | VirtIO MMIO transport |

### `debug/` — analysis and delivery

| Crate | Purpose |
|-------|---------|
| `helm-spy` | Analysis models, `HelmSpy` |
| `helm-report` | Output sinks (JSON, CSV) |

---

## Key Design Rules (Never Violate These)

1. **No dark state.** Every field that must survive checkpoint must be registered as an `AttrDescriptor`. Unregistered state = lost on restore. SIMICS invariant.

2. **Monomorphize only timing.** `HelmEngine<T: TimingModel>` — T is the only generic parameter. ISA and mode dispatch via enum (never generic). Reason: PyO3 cannot pass generic type params across FFI; `HelmSim` enum is the boundary.

3. **Device knows no base address.** `MemoryMap` owns placement. Device only sees `offset` within its region. Never put a base address in `DeviceParams`.

4. **Device knows no IRQ number.** `InterruptPin::assert()` fires the signal. Platform config (`World::wire_interrupt()`) owns routing to controller + line number.

5. **HelmEventBus is synchronous, not checkpointed.** All subscribers called before `fire()` returns. Subscribers re-register in `init()` on every load (initial + restore). Lives in `helm-devices/src/bus/event_bus`.

6. **`ExecContext` is hot-path only.** Called billions of times. Must be statically dispatched (generic parameter, not `dyn`). `ThreadContext` (GDB, syscalls, Python) is cold — `&mut dyn ThreadContext` is fine.

7. **JIT synchronization before device register access.** When `mmio_read()` accesses a device, drain `EventQueue` and IO completions first. Higan principle: sync only at shared-state access boundaries.

---

## Execution Modes and Timing Models

### ExecMode (cold-path enum)
```rust
pub enum ExecMode {
    Functional,   // FE — pure instruction execution, no OS; fastest correctness check
    Syscall,      // SE — intercept syscalls, dispatch to host OS (user-space binaries)
    System,       // FS — boot real kernel, full privilege model (Phase 3)
    Hardware,     // HAE — KVM/HVMX hardware-assisted; CPU runs on host hardware, devices modeled
}
```

**Mode summary:**

| Mode | CPU execution | OS model | Use case |
|------|--------------|----------|----------|
| FE — Functional | Interpreted in Rust | None | ISA correctness, riscv-tests, boot tracing |
| SE — Syscall | Interpreted in Rust | Syscall intercept → host OS | User-space Linux binaries without a kernel |
| FS — Full System | Interpreted in Rust | Real guest kernel booted | Boot Linux, driver development (Phase 3) |
| HAE — Hardware Assisted | KVM/HVMX (host hardware) | Real guest kernel | Near-native speed; devices still modeled in Rust |

HAE mode (`ExecMode::Hardware`) uses the host's hardware virtualization (KVM on Linux, Hypervisor.framework/HVMX on macOS) to run the guest CPU at near-native speed. The device model is unchanged — `VcpuFd::run()` returns a `VcpuExit::MmioRead`/`MmioWrite` on device access, which is routed through the same `MemoryMap` and `Device` infrastructure as software-interpreted modes. HAE lives in `helm-engine/src/kvm/` and is gated behind a `cfg(target_os = "linux")` feature.

`HardwareEngine` (for HAE) wraps `kvm_ioctls::{Kvm, VmFd, VcpuFd}` and implements the same `SimObject` lifecycle as `HelmEngine<T>`, but does **not** implement `TimingModel` — timing is real hardware, not modeled.

### TimingModel (hot-path generic)
```rust
// T is monomorphized — zero overhead
pub enum HelmSim {                                    // PyO3 boundary
    VirtualTiming(HelmEngine<VirtualTiming>),         // event-driven clock, >100 MIPS
    IntervalTiming(HelmEngine<IntervalTiming>),       // Sniper-style, <15% MAPE, >10 MIPS
    AccurateTiming(HelmEngine<AccurateTiming>),       // cycle-accurate, <10% IPC err, >200 KIPS
    Hardware(HardwareEngine),                         // KVM/HVMX — real hardware timing
}
```

Timing models are in `helm-timing`. `HardwareEngine` is in `helm-engine/src/kvm/`. `World::run()` with no `HelmEngine` = device-only mode (no CPU needed).

---

## The World and Object Model

```rust
// helm-engine — owns the simulation
pub struct World {
    objects:     SlotMap<HelmObjectId, HelmObject>,
    by_name:     HashMap<String, HelmObjectId>,   // dot-path: "board.cpu0.uart"
    interfaces:  InterfaceRegistry,               // named runtime interface discovery
    memory:      MemoryMap,
    events:      EventQueue,
    event_bus:   Arc<HelmEventBus>,
    clock:       VirtualClock,
    scheduler:   Option<Scheduler>,               // None = device-only mode
    io_thread:   Option<IoThread>,                // async host IO backend
}
```

**Two-phase config (Python → Rust):**
```
Phase 1 (Python, no side effects):
  uart = PendingObject::new("uart", "uart16550").set("clock_hz", 1_843_200)
  world.add(uart)

Phase 2 (Rust, atomic):
  world.instantiate()   →   alloc() → set_attrs() → finalize() → all_finalized()
```

**Lifecycle:** `alloc → init → [attrs set] → finalize → all_finalized → run → deinit`

Cross-object calls forbidden in `init()` — peers not yet initialized. Safe from `finalize()` onward.

---

## Python Config API

```python
from helm_ng import Simulation, Cpu, Memory, Uart16550, Plic, Isa, TimingModel

cpu  = Cpu(isa=Isa.RiscV, timing=TimingModel.Interval)
mem  = Memory(size="512MiB", base_addr=0x80000000)
uart = Uart16550(clock_hz=1_843_200)   # no base_addr — platform concern
plic = Plic(num_sources=64)

sim = Simulation(components=[cpu, mem, uart, plic])
sim.map_device(uart, base=0x10000000)
sim.wire_interrupt(uart.irq_out, plic.input(10))
sim.elaborate()

# Subscribe to events
sim.event_bus.subscribe("Exception", lambda e: print(f"Exception {e.vector:#x}"))

sim.run(n_instructions=1_000_000_000)
print(sim.stats())
```

---

## Memory System

```rust
pub enum MemoryRegion {
    Ram     { data: Vec<u8> },
    Rom     { data: &'static [u8] },
    Mmio    { handler: Box<dyn Device>, size: u64 },
    Alias   { target: Arc<RwLock<MemoryRegion>>, offset: u64, size: u64 },
    Container { subregions: BTreeMap<u64, (u64, MemoryRegion)> },
    Reserved { size: u64 },
}
```

**Three access modes (cannot coexist for Timing+Atomic):**
- `Atomic` — synchronous + estimated latency. Used by FE, fast-forward.
- `Functional` — instantaneous, no cache side effects. Used by GDB, ELF load, checkpoint.
- `Timing` — async callbacks with actual modeled latency. Used by Interval + Accurate.

`FlatView` = sorted `Vec<FlatRange>`, O(log n) lookup, recomputed lazily on structural change.

---

## Interrupt Model

```
Device ──assert()──► InterruptPin
                           │ (InterruptWire, wired at elaborate() by platform config)
                           ▼
                    InterruptSink (e.g., Plic)
                           │ on_assert(wire_id)
                           ▼
                    PLIC sets pending bit
                           │ fires HelmEvent::ExternalIrq
                           ▼
                    HelmEngine checks pending_irq → CPU exception entry
```

Platform config: `world.wire_interrupt("uart.irq_out", "plic.IRQ[10]")`
Device: never knows about PLIC or IRQ number 10.

---

## HelmEventBus — Observability

```rust
pub enum HelmEvent {
    Exception     { cpu: ObjectRef, vector: u32, tval: u64, pc: u64 },
    CsrWrite      { cpu: ObjectRef, csr: u16, old: u64, new: u64 },
    ExternalIrq   { cpu: ObjectRef, irq_num: u32 },
    Breakpoint    { cpu: ObjectRef, addr: u64, bp_id: u32 },
    MagicInsn     { cpu: ObjectRef, pc: u64, value: u64 },
    SimulationStop{ reason: StopReason },
    MemWrite      { addr: u64, size: usize, val: u64, cycle: u64 },
    SyscallEnter  { nr: u64, args: [u64; 6] },
    SyscallReturn { nr: u64, ret: u64 },
    DeviceSignal  { device: ObjectRef, port: String, asserted: bool },
    Custom        { name: &'static str, data: Arc<dyn Any + Send + Sync> },
    // + 4 more
}
```

`TraceLogger` is a `HelmEventBus` subscriber — not a separate system. `GdbServer` subscribes to `Breakpoint` and `SimulationStop`. Python callbacks subscribe via PyO3.

---

## IO Thread Model (3 Layers)

```
Layer 1: Simulation world  (single thread, EventQueue-driven)
         Device models, timing, state, register access

         ↕ IoThread::submit(req) / drain_completions()

Layer 2: IO bridge  (async_channel — thread-safe, non-blocking)

         ↕ tokio async runtime

Layer 3: IO backend  (dedicated OS thread)
         tokio::fs for disk images, tokio::net for network
```

JIT sync rule: `World::mmio_read()` calls `drain_io()` before dispatching to device — device state is always current at the moment of CPU access. (Higan: `while(peer.clock < my.clock) yield(peer)`)

---

## register_bank! Macro

The primary device modeling primitive — replaces manual MMIO switch statements:

```rust
register_bank! {
    UartRegs for Uart16550 at offset 0x0 {
        reg RHR @ 0x00 is read_only  { field DATA [7:0] }
        reg THR @ 0x00 is write_only { field DATA [7:0] }
        reg LSR @ 0x14 is read_only  {
            field THRE [5]   // TX holding register empty
            field DR   [0]   // data ready
        }
    }
}
// Generates: MmioHandler impl, serde checkpoint, AttrDescriptors, Python introspection
```

---

## Accuracy Targets

| Mode | RISC-V (simple) | ARM (in-order) | Speed |
|------|----------------|----------------|-------|
| Virtual | correctness only | correctness only | >100 MIPS |
| Interval | <12% MAPE | <12% MAPE | >10 MIPS |
| Accurate (default) | <10% IPC err | <10% IPC err | >200 KIPS |
| Accurate (calibrated) | <5% IPC err | <7% IPC err | — |

Calibration via `MicroarchProfile` JSON (per real target core). Validation: Spike (RISC-V oracle), QEMU (ARM), real HW (SiFive, RPi4).

---

## Phase Build Plan

| Phase | Deliverables | Status |
|-------|-------------|--------|
| **0 — MVP** | RISC-V SE simulator, runs static binaries, riscv-tests pass | **In progress** — RV64I+Zicsr done; M/A/F/D + syscall handler + CLI pending |
| **1** | AArch64 SE+FS, EventQueue, GDB stub, Stats, ARM virt platform, timing | **Largely done** — AArch64 SE+FS working; GICv2, PL011, SP804; PyO3 bindings |
| **2** | RISC-V SE completion, helm-riscv64 binary, riscv-tests gate | **Next** |
| **3** | Full system (boot Linux RISC-V), OoO pipeline, AArch32, JIT | Future |

**Current focus:** Complete RISC-V SE (`LinuxRiscv64SyscallHandler`) and ship `helm-riscv64` binary.
See `docs/plans/riscv64-se-emulation.md` for the implementation plan.

---

## File Structure

```
helm-ng/
├── AGENT.md              ← this file (agent onboarding)
├── CLAUDE.md             ← project instructions for Claude Code
├── Cargo.toml            ← workspace root
├── framework/            ← helm-core, helm-memory, helm-event, helm-devices, helm-timing, ...
├── runtime/              ← helm-arch, helm-engine, helm-platform, helm-python, helm-cli
├── hw/                   ← helm-hw-char, helm-hw-intc, helm-hw-timer, helm-hw-rtc, ...
├── debug/                ← helm-spy, helm-report
├── assets/
│   ├── aarch64/alpine/   ← Alpine Linux boot assets (kernel, DTB, rootfs)
│   └── riscv/bin/        ← static RISC-V test binaries (busybox, static-sh)
├── examples/
│   └── fs/boot_rpi_full.py  ← AArch64 FS boot example
├── python/helm/          ← helm Python package (ISA-namespaced)
└── docs/
    ├── ARCHITECTURE.md   ← detailed system architecture
    ├── api.md            ← public API reference
    ├── traits.md         ← all traits documented
    ├── testing.md        ← testing strategy
    ├── object-model.md   ← SimObject lifecycle
    ├── plans/            ← implementation plans (active)
    │   └── riscv64-se-emulation.md  ← RISC-V SE plan (helm-riscv64)
    └── design/           ← 60+ design documents
        ├── HLD.md                ← system-wide HLD
        ├── helm-core/            ← HLD + LLD × 3 + TEST
        ├── helm-engine/          ← HLD + LLD × 9 + TEST × 3
        ├── helm-arch/            ← HLD + LLD × 4 + TEST
        ├── helm-memory/          ← HLD + LLD × 3 + TEST
        ├── helm-devices/         ← HLD + LLD × 6 + TEST + bus-event/
        ├── helm-timing/          ← HLD + LLD × 2 + TEST
        ├── helm-event/           ← HLD + LLD + TEST
        ├── helm-stats/           ← HLD + LLD + TEST
        ├── helm-debug/           ← HLD + LLD × 3 + TEST
        └── helm-python/          ← HLD + LLD × 3 + TEST
```

---

## Naming Reference

| Old / Alternative | Correct Name | Location |
|-------------------|-------------|----------|
| `SimKernel` | `HelmEngine<T>` | helm-engine |
| `HelmSimulator` | `HelmSim` | helm-engine |
| `AnySimulator` | `HelmSim` | helm-engine |
| `SimulatedTime` | `VirtualTiming` | helm-timing |
| `IntervalTimed` | `IntervalTiming` | helm-timing |
| `AccurateTimed` | `AccurateTiming` | helm-timing |
| `FunctionalEmulation` | `ExecMode::Functional` | helm-engine |
| `SyscallEmulation` | `ExecMode::Syscall` | helm-engine |
| `FullSystem` | `ExecMode::System` | helm-engine |
| `KvmMode` / `HardwareEmulation` | `ExecMode::Hardware` | helm-engine |
| `KvmEngine` | `HardwareEngine` | helm-engine/src/kvm/ |
| `Counter/Histogram` | `PerfCounter/PerfHistogram` | helm-stats |
| `HapBus` / `HelmBus` | `HelmEventBus` | helm-devices/bus |
| `DeviceWorld` | `World` (no HelmEngine) | helm-engine |
| `helm-world` | dissolved → helm-core + helm-devices + helm-engine | — |
| `helm-eventbus` | dissolved → helm-devices/src/bus/event_bus | — |
| `helm-se` | dissolved → helm-engine/src/se/ | — |
| `helm-py` | `helm-python` | helm-python |

---

## Quick Reference: Where Things Live

| Need to... | Look at |
|-----------|---------|
| Add a new ISA | `helm-arch/src/{new_isa}/` + implement `Hart` trait from `helm-core` |
| Add a new device | Implement `Device` trait from `helm-devices`, use `register_bank!` |
| Add a new timing model | Implement `TimingModel` trait from `helm-timing`, add variant to `HelmSim` |
| Add a RISC-V syscall | `helm-engine/src/se/linux_riscv64.rs` (create per plan) |
| Add an AArch64 syscall | `helm-engine/src/se/linux_aarch64.rs` |
| Add a GDB packet | `helm-debug/src/gdb_server.rs`, implement `GdbTarget` method |
| Change Python API | `helm-python/src/` + `python/helm/` |
| Add a bus type | `helm-devices/src/bus/{new_bus}/` |
| Add a stat counter | `helm-stats::StatsRegistry::perf_counter("path.name", "desc")` |
| Debug a checkpoint | All persistent state must be in `AttrStore` with `AttrKind::Required` |
| Understand timing models | `docs/design/helm-timing/HLD.md` + `LLD-timing-models.md` |
| Understand device design | `docs/design/helm-devices/HLD.md` + `LLD-device-sdk.md` |
| Understand RISC-V SE plan | `docs/plans/riscv64-se-emulation.md` |
