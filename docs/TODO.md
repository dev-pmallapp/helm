# helm-ng TODO

Consolidated work items extracted from exploratory design docs that were pruned.
Grouped by area. Items marked with a phase reflect the original phased build plan.

---

## Instrumentation / Observability

### helm-probe (framework/helm-probe)
- ~~Add `BranchEvent` to `src/events.rs`~~ — DONE: `BranchEvent`, `BranchKind` implemented
- ~~Add `branch: Probe<BranchEvent>` to `CpuProbes`~~ — DONE: wired in CpuProbes
- ~~Add `MmioEvent` to `src/events.rs`~~ — DONE: `MmioEvent` implemented
- ~~Update `src/lib.rs` re-exports~~ — DONE: `BranchEvent`, `MmioEvent`, `MemAccessEvent` exported

### helm-spy (debug/helm-spy) — Phase 2
- Wire `ProbePluginBridge` to connect probe events to `HelmSpy` (currently standalone, not wired)
- ~~Wire `helm-spy` as workspace dep in root `Cargo.toml`~~ — DONE: workspace dependency in root Cargo.toml
- ~~Wire `helm-report` as workspace dep in root `Cargo.toml`~~ — DONE: workspace dependency in root Cargo.toml

### helm-report (debug/helm-report) — Phase 2/3
- Implement `Sink` trait and implementations: `FileSink`, `AsyncFileSink`, `StderrSink`, `TcpSink`, `NullSink`, `BinaryTraceSink<T>`
- Implement `sink_from_uri(uri) -> Box<dyn Sink>` URI dispatcher
- Implement formatters: `TextFormatter`, `JsonFormatter`, `CsvFormatter`, `GemstatsFormatter`
- Implement `Report` struct + `deliver()` + `ReportSchedule`
- Wire `HelmSpySnapshot` from `helm-spy` dep (currently uses local copy)
- Add `helm-report` dep to `helm-python/Cargo.toml`

### helm-plugin removal — Phase 3
- Move `runtime/info.rs` types to `helm-spy::events`: `InsnInfo`, `BranchInfo`, `MemInfo`, `SyscallInfo`, `FaultInfo`, `InsnClass`, `BranchKind`, `ArchContext`, `FaultKind`
- Move `runtime/scoreboard.rs` to `helm-spy::primitives`: `Scoreboard<T>`
- Delete `runtime/registry.rs` (`HelmPluginRegistry` replaced by `HelmSpy::subscribe()`)
- Delete `runtime/callback.rs` (all `Box<dyn Fn(...)>` callback type aliases)
- Delete `api/plugin.rs` (`HelmPlugin` trait and `HelmPluginArgs`)
- Delete entire `builtins/` directory (11 built-in plugins replaced by helm-spy primitives)
- Remove `helm-plugin` dep from `helm-engine/Cargo.toml` (replace with `helm-spy`)
- Remove `helm-plugin` dep from `helm-python/Cargo.toml`
- Remove `helm-plugin` from workspace members after all deps migrated

### helm-engine instrumentation — Phase 3
- Remove `add_plugin()` from `helm-engine` (Python compat shim until helm-spy API ready)
- Remove `InstrumentedMem` or add mem probe alongside (not removed yet)
- Wire `HelmSpy` fully into `helm-engine`

### hw crates — migrate sim_trace imports — Phase 1 — NO MIGRATION NEEDED
- ~~`hw/helm-hw-char/`: migrate `helm_debug::sim_trace` to `helm_diag`~~ — NOT NEEDED: hw crates do not import `helm_debug::sim_trace`; they already use `helm-devices` only
- ~~`hw/helm-hw-timer/`: same migration~~ — NOT NEEDED: same reason
- ~~`hw/helm-hw-rtc/`: same migration~~ — NOT NEEDED: same reason

### helm-arch — execute function probe wiring — Phase 2
- Add `probes: &CpuProbes` parameter to `aarch64_execute()` or use thread-local approach; two options: (1) thread-local `CURRENT_PROBES: RefCell<Option<*mut CpuProbes>>` set by engine before each step (faster to ship), (2) explicit `probes: Option<&mut CpuProbes>` parameter (cleaner, Phase 2 refactor)

### Open instrumentation questions to resolve before Phase 2
- **DashMap vs sharded counters for HeatMap**: `DashMap` (external dep) vs manual `[Mutex<HashMap>; N]`. Resolve before Phase 2 begins.
- **EventStream<InsnInfo> with ArchContext**: `ArchContext::Aarch64` is 256 bytes; 1M events = 256MB. Need a compact form or explicit opt-in — proposal: `EventStream<CompactInsnEvent>` by default.
- **helm-plugin removal timing**: remove immediately (breaking) or keep deprecated until Phase 3? Recommendation: remove immediately once probe wiring replaces call sites.
- **Async delivery thread model**: `AsyncFileSink` background thread — per-sink (simple) or shared global I/O thread pool (fewer threads). Resolve before Phase 3 delivery work.
- **Python lambda callbacks in Trigger/Watchpoint**: Python lambdas acquire the GIL; a trigger firing mid-quantum stalls the engine. Need a buffered approach: fire sets a flag, Python checks at quantum boundaries (same pattern as PyO3 syscall callbacks).

---

## Python API (helm-python)

### SimObject hierarchy — Phase B (device pyclasses) — MOSTLY DONE
- ~~Add `GicV2`, `Pl011` pyclasses~~ — DONE: PyO3 wrappers in helm-python/src/devices.rs
- ~~Add `MemorySpace.add_map()` to replace hardcoded address map~~ — DONE: `add_map(base, device, size, bank=0)` in memory_space.rs
- Add port wiring support: `device.irq = gic.spi(N)` stores `PortRef` resolved at `instantiate()` — PortRef struct exists but resolution logic not yet wired

### Platform in Python — Phase C
- Move `build_arm_virt()` logic to `python/helm/boards/arm_virt.py`
- Rust `build_arm_virt()` becomes internal helper or is removed
- Python fully controls platform composition

### Device introspection — Phase D
- Device properties read live Rust state after `instantiate()`
- CPU register access through `system.cpu.xn(N)` etc.
- GIC/UART state inspection: `system.gic.pending_mask`, `system.uart.tx_count`

### helm-python instrumentation migration — Phase 3
- Remove `add_plugin(name, args)` PyO3 method (breaking change)
- Remove `set_sim_trace(uri)` PyO3 method (breaking change)
- Add `observe() -> HelmSpy` — returns a Python-visible session object
- Add `HelmSpy` pyclass with `track_insns()`, `track_branches()`, `track_memory(l1d_size, ...)`, `report(sink, format)`
- Add `breakpoint(pc, action)`, `watchpoint(addr, size, kind)` Python methods
- Add `HelmSpy` query methods: `.insn_count.value()`, `.insn_mix.table()`, `.hot_pcs.top(n)`, `.cache_l1d.hit_rate()` as PyO3-exposed methods
- Update Python examples in `examples/debug/` to new API (`add_plugin` → `observe().track_*()`)

---

## Engine Architecture (helm-engine)

### Memory unification (medium-term)
- Converge toward one authoritative memory API: `framework/helm-memory` as the single long-term memory surface
- Move FlatMem fast path idea and HelmAddressSpace device dispatch behavior into unified `MemoryMap` API
- Currently three overlapping abstractions exist: `FlatMem` (engine), `HelmAddressSpace` (system_mem.rs), `MemoryMap` (helm-memory)

### Engine responsibility split (medium-term) — DONE
- ~~Extract ISA-specific state into dedicated runtime structs~~ — DONE: `Aarch64Core`, `RiscvCore` in session.rs
- ~~Move FS machine state into dedicated machine/runtime object~~ — DONE: `HelmMachine`, `HelmBoard`, `HelmVcpu`
- ~~Runtime enum with per-ISA variants~~ — DONE: `HelmCore` enum with `Aarch64(Aarch64Core)` / `Riscv(RiscvCore)`
- ~~Multi-core scheduling~~ — DONE: `HelmCluster`, `HelmCoreSet`, `HelmSchedulePolicy`, `HelmAdvancePolicy`

### Platform as real construction boundary (medium-term)
- Evolve `runtime/helm-platform` to produce frozen runtime descriptions: memory regions, device instances/factories, interrupt routes, boot config, attachment slots
- Engine consumes that result; stops containing board construction logic itself

### Two-phase architecture formalization
- Make build/run boundary explicit in both code and docs
- Build phase: object graph creation, parameter validation, platform wiring, naming, schema use
- Run phase: pure simulation on frozen state, numeric handles only, no structural mutation
- Introduce one authoritative "built system" boundary (`SystemBuilder` → `BuiltSystem` or similar)

---

## Devices (helm-devices / hw/)

### GIC-v3 (Phase 1) — DONE
- ~~Implement GicDistributor, GicRedistributor~~ — DONE (commit 441e252): `Gicv3Distributor`, `Gicv3Redistributor` with shared state
- ~~Wire GIC CPU interface registers (ICC_*)~~ — DONE (commit 75fcf67): ICC_SRE, ICC_IAR1, ICC_EOIR1, ICC_PMR, ICC_CTLR, ICC_IGRPEN1 sysregs
- ~~Wire GICv3 into FS boot path~~ — DONE (commit af0efe4): arm-virt platform supports GICv3
- Remaining: GIC ITS (LPI) not yet implemented

### SysRegMap (Phase 1, helm-core) — DONE
- ~~Implement `SysRegMap` in `helm-core` with `Inline` (zero-cost field offset) and `Handler(Box<dyn SysRegHandler>)` entries~~ — DONE: `SysRegMap` with `Inline`/`Handler` entries in `helm-core/src/sysreg.rs`
- Wire `MPIDR_EL1`, `SCTLR_EL1`, `TTBR0_EL1` as `Inline`; `CNTPCT_EL0`, `ICC_IAR1_EL1` as `Handler` — wiring deferred to elaborate() integration
- Map built at `elaborate()`, immutable during RUN — design ready, elaborate() integration pending

### ARM Generic Timer (Phase 1)
- Implement `SysRegHandler` for `CNTPCT_EL0` with clock closure captured at `elaborate()` using `TimerScheduler::current_tick()`
- Timer comparator-to-interrupt path: EventQueue callbacks drain only at instruction boundaries (already decided — verify this is enforced)

### TimerScheduler trait (Phase 0, helm-core) — DONE
- ~~Define `trait TimerScheduler: Send + Sync + 'static` with `schedule_callback(delay_ticks, callback)` and `current_tick() -> u64`~~ — DONE: trait in `helm-core/src/lib.rs` with `schedule_callback`, `current_tick`, `cancel`
- ~~Implement in `helm-event::EventQueue`~~ — DONE: `EventQueue` implements `TimerScheduler`
- Devices store `Arc<dyn TimerScheduler>` populated at `elaborate()` — wiring deferred to Phase 2 device integration

### PowerController trait (Phase 1, helm-core) — DONE
- ~~Define `trait PowerController: Send + Sync` with `cpu_on(target_mpidr, entry_point, context_id)`, `cpu_off(this_mpidr)`, `system_reset()`~~ — DONE: trait with `PowerError` enum in `helm-core/src/lib.rs`
- Implement in `helm-engine`; PSCI SMC/HVC handler receives `Arc<dyn PowerController>` at `elaborate()` — integration pending

### DmaPort trait (Phase 1, helm-core) — DONE
- ~~Define `trait DmaPort: Send + Sync` with `dma_read(addr, buf: &mut [u8])` and `dma_write(addr, buf: &[u8])`~~ — DONE: trait in `helm-core/src/lib.rs`
- Implement by `World` using its `MemoryMap`; devices receive `Arc<dyn DmaPort>` at `elaborate()` — integration pending
- Used by: DMA engines, GIC LPI table reads

### AffinityMap (Phase 1) — DONE
- ~~Implement `AffinityMap`~~ — DONE: `AffinityMap` in `helm-platform/src/affinity.rs` with bidirectional cpu_idx↔mpidr mapping
- Wire `World::affinity_map() -> &AffinityMap`; Python config `register_affinity()` — integration pending
- GIC Distributor stores `Arc<AffinityMap>` at `elaborate()` — integration pending
- Same pattern extends to SMMU stream IDs and PCI requester IDs (Phase 3+)

### register_bank! macro enhancements (Phase 1)
- Add per-register `width` qualifier (default 32): `reg TTBR0 @ 0x08 width 64 { ... }` — generated field is `u64`, hook signature uses `(old: u64, new: u64)`
- Phase 2+: generate compile-time schema hash (`const_fnv1a_hash` of field names+types) stored in checkpoint header for migration detection; `#[serde(default)]` handles field addition

### DLD (Dynamically Loadable Device) features (Phase 2+) — PARTIALLY DONE
- ~~Add `aliases: &'static [&'static str]` to `DeviceDescriptor`~~ — DONE: field in registry.rs
- ~~Add `required_capabilities: &'static [HostCapability]` to `DeviceDescriptor`~~ — DONE: field + `HostCapability` enum in registry.rs
- ~~Replace `python_class: &'static str` with `python_class_extra: Option<&'static str>`~~ — DONE: field in registry.rs
- ~~Add `struct_size: usize` guard to `DeviceDescriptor`~~ — DONE: field + `std::mem::size_of::<DeviceDescriptor>()` in test descriptor
- DLD checkpoint migration: optional `helm_device_migrate_checkpoint` C export — Phase 3
- Phase 3+ (optional): WebAssembly via Wasmtime as optional isolation mode for untrusted DLDs

### PCI / Bus features (Phase 1/3+)
- BAR re-programming: implement `RemapCommand` queue on `PciBus`, drained by `MemoryMap` after `Device::write()` returns; FlatView recomputation lazy
- PCIe hot-plug (Phase 3+): two-phase validated hot-plug — requires relaxing "frozen after startup" rule for bus subsystem only
- PCIe AER (Phase 3+): error injection API `PciEndpoint::inject_error(aer_type)` + hierarchical propagation
- MSI-X shortcut (Phase 3+): `OptimizedMsiRouter` when no SMMU present, selected at `elaborate()` time
- Phase 3+: `ProtocolDevice<P: BusProtocol>` trait for non-register bus protocols (CAN, USB bulk)

### Multi-hart thread safety (Phase 3+)
- Per-device opt-in via ticket-lock at dispatch layer (`DeviceSlot` wrapping `UnsafeCell<Box<dyn Device>>`); devices stay `!Sync`; threading concern invisible to device authors

---

## Debug (helm-debug)

### Watchpoint and Breakpoint engines — Phase 2 — DONE
- ~~Add `src/watchpoint.rs`: `WatchpointEngine`~~ — DONE: 144 lines with add/remove/set_enabled/check, WatchKind, WatchAction, WatchResult + unit tests
- ~~Add `src/breakpoint.rs`: `BreakpointEngine`~~ — DONE: 173 lines with add/remove/set_enabled/check, BreakAction, BreakResult + unit tests
- Wire to `Probe<MemAccessEvent>` and `Probe<CpuStepEvent>` — pending Phase 2 probe integration

### InspectionAPI — Phase 3
- Add `src/inspect.rs`: `InspectionAPI` — dump arch state, memory range, device state on demand from Python

### GDB RSP — Phase 2 — DONE
- ~~Implement real GDB RSP in `src/gdb/rsp.rs` and `src/gdb/target.rs`~~ — DONE: full RSP implementation (260 lines) with g/G/m/M/c/s/z/Z/?/q/Q/D/k packets, `GdbTarget` trait with 10 methods, `StopReason` enum, checksum validation, NoAckMode

### CheckpointManager — Phase 2 — DONE
- ~~Implement serialization in `src/checkpoint.rs`~~ — DONE: `CheckpointHeader` (magic + version + entry_count), `CheckpointManager` with save_values/restore_values, length-prefixed binary format + unit tests

---

## Versioning (API surfaces)

Versioning infrastructure status (most items now implemented):

- **Device Plugin ABI**: ~~split `HELM_DEVICES_ABI_VERSION` into MAJOR/MINOR~~ — DONE: `HELM_DEVICE_ABI_MAJOR` + `HELM_DEVICE_ABI_MINOR` in sdk.rs; ~~`struct_size` guard on `DeviceDescriptor`~~ — DONE: `struct_size: usize` field; ~~ABI check protocol~~ — DONE: `DeviceRegistry::check_abi()`
- **Instrument Plugin ABI**: deferred — `helm-plugin` being replaced by `helm-spy`
- **Python API**: ~~`helm_ng.__version__`~~ — DONE: `env!("CARGO_PKG_VERSION")` in lib.rs; ~~`version_manifest()`~~ — DONE: returns helm_ng, device_sdk, device_abi versions; `DeprecationWarning` at deprecated call sites — DONE
- **SimObject / Object Model**: `ClassDescriptor` with version field — DONE
- **Checkpoint Format**: ~~`CheckpointHeader` with version field~~ — DONE: magic + version + entry_count; schema hash for migration — Phase 3
- **HelmEventBus Event Types**: ~~`#[non_exhaustive]`~~ — DONE; discriminant stabilization — DONE; ~~DLD restriction documented~~ — DONE
- **Debug Protocol (HelmProtocol)**: versioned handshake — Phase 3

---

## Advanced Analysis (Phase 4/5 — helm-spy)

- `SimPoint` BBV computation: `src/analysis/simpoint.rs` — subscribe to branches, emit basic block vectors per interval for SimPoint tool
- `PowerModel`: per-class instruction energy × count estimation: `src/analysis/power.rs`
- `DiffAnalysis`: compare two `HelmSpy` sessions: `src/analysis/diff.rs`
- `CorrelHist2D`: joint distribution primitive `src/primitives/correl.rs` (implemented standalone, not yet wired)
- `BranchPredictor` full implementations: BiModal fully done; GShare — verify completeness
- `GemstatsFormatter` in helm-report: gem5 stats.txt compatible output format
- `Trigger` + `Window` primitives: `at_insn(N)`, `at_pc(addr)`, `pc_range(s,e)`, `counter_reaches(c,n)`; `Window::gate<T>(inner)` — ROI analysis, warmup skip, phase-conditional collection
- `BinaryTraceSink<T>`: drain `TraceRing<T, N>` to typed binary file with `TraceHeader`; Python reads via `mmap` + `struct.unpack_from`
