# helm-ng Research Notes

Design research informing helm-ng's architecture, implementation, and testing strategy.
Each file documents what existing simulators do, followed by explicit Helm design choices.

---

## Files

| File | Size | Coverage |
|------|------|----------|
| `simics-object-model.md` | 13K | SIMICS: conf_object_t, class lifecycle, interface system, attribute system, component config → **Helm**: HelmObject, ClassDescriptor, InterfaceRegistry, HelmAttr, PendingObject/World::instantiate() |
| `simics-haps-timing-devices.md` | 24K | SIMICS: HAP system, temporal decoupling, event posting, CPU model integration, DML patterns, port objects, signal interface → **Helm**: HelmEventBus, Execute+Scheduler, EventQueue+EventClass, Hart, `register_bank!` macro, Connect<T>/Port<T>/SignalInterface |
| `simics-raw-research.md` | 51K | Full 1300-line raw SIMICS API research (all function signatures, struct defs) |
| `qom-qmp.md` | 17K | QEMU QOM: TypeInfo, two-phase lifecycle, DEFINE_PROP_*, realize/unrealize, Resettable, Interfaces. QEMU QMP: QAPI schema, command/event protocol, Python client. → **Helm**: `register_bank!` macro, DeviceConfig/Device::realize(), HelmProtocol, inventory::submit! |
| `gem5-qemu-rust-patterns.md` | 40K | Gem5: SimObject, Python config, port system, event queue, Classic/Ruby memory, stats. QEMU: TCG, QOM, MemoryRegion. Rust simulators: rrs, rvemu, riscv-rust, rv8 |
| `simulator-accuracy.md` | 13K | Quantitative IPC accuracy: gem5 (1.4–458% default, 6% calibrated), Sniper (9.5–20% MAPE), PTLsim (5%), ZSim (24%), FireSim (cycle-exact). Root causes of inaccuracy. Helm accuracy targets by ISA and mode. |
| `accuracy-design.md` | 13K | Helm timing model accuracy architecture: Virtual/Interval/Accurate internals, MicroarchProfile spec, `helm validate` CLI, OoOWindow model, CPI stack output, per-ISA accuracy targets |
| `helm-engine/LLD-world.md` | ~30K | Headless device/bus simulation (World (no HelmEngine)): World Rust API, Python config, Bus trait, PCI/I2C/SPI without CPU, fuzzing with libFuzzer, co-simulation with Verilator RTL, testing patterns |
| `memory-system.md` | ~25K | Memory system design: MemoryRegion tree, FlatView computation, three access modes (Atomic/Functional/Timing), cache model (set-associative + MSHR), TLB, Sv39/Sv48 page walk, AArch64 4K walk, MMIO dispatch, endianness, MemFault types |
| `riscv-isa-implementation.md` | ~30K | RV64GC ISA: encoding formats (R/I/S/B/U/J), register file (ABI names, CSRs), privilege levels, trap handling, Sv39/Sv48, interrupt model (CLINT/PLIC), atomics (LR/SC/AMO), compressed (C extension), decode/execute in Rust, 10 common bugs |
| `arm-aarch64-implementation.md` | ~25K | AArch64 ISA: register file (X0–X30, PSTATE, SIMD V0–V31), exception levels (EL0–EL3), instruction encoding (fixed 32-bit), addressing modes, system registers, barriers, SIMD/FP, deku crate decode, AArch32 interworking, 8 common bugs |

---

## Key Design Decisions Derived from Research

### Object Model (from SIMICS + QOM)
- **`HelmObject`** = universal handle, dot-path name (`board.cpu0.icache`)
- **`ClassDescriptor`** = `alloc/init/finalize/all_finalized/deinit` lifecycle (SIMICS 5-phase)
- **`InterfaceRegistry`** = named runtime-discoverable interfaces (SIMICS pattern, better than QOM strings)
- **`HelmAttr` system** = ALL persistent state flows through attributes; checkpoint = serialize attrs (SIMICS invariant)
- **`PendingObject` → `World::instantiate()`** = describe topology first, instantiate atomically (SIMICS pre_conf_object)
- **`DeviceConfig` → `Device::realize()`** = two-phase infallible/fallible lifecycle (QOM insight)
- **`inventory::submit!`** = self-registration without central dispatch (QOM type_init equivalent)

### Interface & Device Model (from SIMICS + QOM)
- **`register_bank!` macro** = first-class bank/register/field hierarchy (DML insight, QOM's biggest weakness)
- **`SignalInterface`** = canonical interrupt output pin (SIMICS signal_interface_t)
- **`Connect<T>` / `Port<T>`** = typed wiring at elaborate() time (DML connect/port pattern)
- **`InterruptPin`** = device asserts signal only; routing is platform config (no IRQ numbers on device)
- **Three-phase reset** = `assert → hold → release` via `Resettable` trait (QOM Resettable insight)

### Timing & Scheduling (from SIMICS + Sniper + PTLsim)
- **`HelmEngine<T: TimingModel>`** = monomorphized timing (generic param), enum ISA/mode dispatch
- **`Execute` trait + `Scheduler`** = temporal decoupling with quantum (SIMICS model)
- **`EventQueue`** = `BinaryHeap<Reverse<PendingEvent>>` with typed `EventClass` (SIMICS event posting)
- **`HelmEventBus`** = named typed pub-sub; synchronous; NOT checkpointed (SIMICS HAPs)
- **`Virtual` / `Interval` / `Accurate`** = three timing model structs (PTLsim depth + Sniper interval + event-driven)
- **`MicroarchProfile`** = pluggable JSON config per real µarch target (calibration strategy)
- **`CpiStack`** = cycle breakdown output in Interval mode (Sniper CPI stack insight)

### Memory System (from QEMU + Gem5)
- **`MemoryRegion` tree** = unified RAM/MMIO/ROM/Alias/Container (QEMU MemoryRegion)
- **`FlatView`** = sorted non-overlapping ranges, O(log n) lookup (QEMU FlatView)
- **Three access modes** = Atomic / Functional / Timing (Gem5 port model)
- **`CacheModel`** = set-associative + LRU + MSHR (standard, validated against gem5 lessons)
- **`TlbModel`** = per-hart, ASID-aware, Sv39/Sv48/AArch64 page walks

### ISA Implementation (from RISC-V spec + ARM ARM)
- **RISC-V first** = RV64GC, no µ-op complexity, regular encoding, abundant references
- **AArch64 second** = `deku` crate for bit-field parsing, starts EL1-only, defers AArch32
- **Separate `ArchState` from `MicroState`** = enables adding timing without refactoring functional core

### Control Protocol (from QMP)
- **`HelmProtocol`** = typed `HelmCommand` enum + `HelmEvent` enum over Unix socket/TCP
- **Schema introspection** = `DeviceDescriptor::param_schema()` machine-readable property list
- **`HelmServer`** = replacement for QMP; typed, not stringly-typed

### Device Testing (new capability)
- **`World`** = headless device simulation (`World (no HelmEngine)`) — no CPU, no OS
- **Fuzzing** = `World` + `cargo-fuzz` / libFuzzer for device MMIO fuzzing
- **Co-simulation** = `TlmBridge` connects `World` to Verilator RTL for scoreboard testing

### Accuracy Targets
| Mode | RISC-V (simple) | RISC-V (OoO) | ARM (in-order) | ARM (OoO) |
|------|----------------|--------------|----------------|-----------|
| Virtual | correctness | correctness | correctness | correctness |
| Interval | <12% MAPE | <18% MAPE | <12% MAPE | <18% MAPE |
| Accurate (default profile) | <10% IPC err | <15% IPC err | <10% IPC err | <15% IPC err |
| Accurate (calibrated) | <5% IPC err | <10% IPC err | <7% IPC err | <12% IPC err |

| `higan-accuracy.md` | 14K | Higan/ares: absolute scheduler (Second=2^63-1, scalar=Second/freq), cooperative threading, JIT sync, catch-up while-loop, run-ahead. → **Helm**: AccuratePipeline step granularity, JIT drain on device-register access, AbsoluteClock for multi-freq, IO thread bridge |
