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
- Wire `helm-spy` as workspace dep in root `Cargo.toml`
- Wire `helm-report` as workspace dep in root `Cargo.toml`

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

### hw crates — migrate sim_trace imports — Phase 1
- `hw/helm-hw-char/`: change all `use helm_debug::sim_trace::*` to `use helm_diag::*` (~5–10 call sites); add `helm-diag` dep, remove `helm-debug` dep if only used for sim_trace
- `hw/helm-hw-timer/`: same migration
- `hw/helm-hw-rtc/`: same migration

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

### SimObject hierarchy — Phase B (device pyclasses) — PARTIALLY DONE
- ~~Add `GicV2`, `Pl011` pyclasses~~ — DONE: PyO3 wrappers in helm-python/src/devices.rs
- Add `MemorySpace.add_map()` to replace hardcoded address map
- Add port wiring support: `device.irq = gic.spi(N)` stores `PortRef` resolved at `instantiate()`

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

### SysRegMap (Phase 1, helm-core)
- Implement `SysRegMap` in `helm-core` with `Inline` (zero-cost field offset) and `Handler(Box<dyn SysRegHandler>)` entries
- Wire `MPIDR_EL1`, `SCTLR_EL1`, `TTBR0_EL1` as `Inline`; `CNTPCT_EL0`, `ICC_IAR1_EL1` as `Handler`
- Map built at `elaborate()`, immutable during RUN

### ARM Generic Timer (Phase 1)
- Implement `SysRegHandler` for `CNTPCT_EL0` with clock closure captured at `elaborate()` using `TimerScheduler::current_tick()`
- Timer comparator-to-interrupt path: EventQueue callbacks drain only at instruction boundaries (already decided — verify this is enforced)

### TimerScheduler trait (Phase 0, helm-core)
- Define `trait TimerScheduler: Send + Sync + 'static` with `schedule_callback(delay_ticks, callback)` and `current_tick() -> u64`
- Implement in `helm-event::EventQueue`
- Devices store `Arc<dyn TimerScheduler>` populated at `elaborate()`

### PowerController trait (Phase 1, helm-core)
- Define `trait PowerController: Send + Sync` with `cpu_on(target_mpidr, entry_point, context_id)`, `cpu_off(this_mpidr)`, `system_reset()`
- Implement in `helm-engine`; PSCI SMC/HVC handler receives `Arc<dyn PowerController>` at `elaborate()`

### DmaPort trait (Phase 1, helm-core)
- Define `trait DmaPort: Send + Sync` with `dma_read(addr, buf: &mut [u8])` and `dma_write(addr, buf: &[u8])`
- Implement by `World` using its `MemoryMap`; devices receive `Arc<dyn DmaPort>` at `elaborate()`
- Used by: DMA engines, GIC LPI table reads

### AffinityMap (Phase 1)
- Implement `World::affinity_map() -> &AffinityMap`; populated from Python config `system.register_affinity(cpu0, mpidr=0x00000000)`
- GIC Distributor stores `Arc<AffinityMap>` at `elaborate()`
- Same pattern extends to SMMU stream IDs and PCI requester IDs (Phase 3+)

### register_bank! macro enhancements (Phase 1)
- Add per-register `width` qualifier (default 32): `reg TTBR0 @ 0x08 width 64 { ... }` — generated field is `u64`, hook signature uses `(old: u64, new: u64)`
- Phase 2+: generate compile-time schema hash (`const_fnv1a_hash` of field names+types) stored in checkpoint header for migration detection; `#[serde(default)]` handles field addition

### DLD (Dynamically Loadable Device) features (Phase 2+)
- Add `aliases: &'static [&'static str]` to `DeviceDescriptor` for device rename support
- Add `required_capabilities: &'static [HostCapability]` + optional `fn check_requirements() -> Result<(), String>` export to `DeviceDescriptor` (fast-fail at load time)
- Remove `python_class: &'static str` field from `DeviceDescriptor`; replace with `python_class_extra: Option<&'static str>`; auto-generate Python class from `ParamSchema`
- DLD checkpoint migration: optional `helm_device_migrate_checkpoint(name, old_version, data, len, out_len)` C export; invoke at restore time when schema hash mismatches
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

### Watchpoint and Breakpoint engines — Phase 2
- Add `src/watchpoint.rs`: `WatchpointEngine` — subscribes to `Probe<MemAccessEvent>`; fires action when watched address range is accessed
- Add `src/breakpoint.rs`: `BreakpointEngine` — subscribes to `Probe<CpuStepEvent>` (pre_step); fires action when PC matches

### InspectionAPI — Phase 3
- Add `src/inspect.rs`: `InspectionAPI` — dump arch state, memory range, device state on demand from Python

### GDB RSP stub — Phase 2
- Implement real GDB RSP in `src/gdb/rsp.rs` and `src/gdb/target.rs` (currently a stub)

### CheckpointManager — Phase 2
- Implement CBOR serialization in `src/checkpoint.rs` (currently a stub)

---

## Versioning (API surfaces)

The following API surfaces need versioning infrastructure implemented (none currently implemented):

- **Device Plugin ABI**: split `HELM_DEVICES_ABI_VERSION: u32` into `HELM_DEVICE_ABI_MAJOR + HELM_DEVICE_ABI_MINOR`; add `struct_size` guard to `DeviceDescriptorC`; implement check protocol at `dlopen` time
- **Instrument Plugin ABI**: implement `PluginRegistryC` with `struct_size` guard + `HELM_PLUGIN_ABI_MAJOR/MINOR` symbols (note: `helm-plugin` is being replaced by `helm-spy` — coordinate with plugin removal)
- **Python API**: add `helm_ng.__version__` semver string; add `version_manifest()` returning per-surface versions; add `DeprecationWarning` at deprecated call sites
- **SimObject / Object Model**: implement `u32` version in `ClassDescriptor`; check at `ClassRegistry::global()` init
- **Checkpoint Format**: implement `CheckpointHeader` with `u32` version field; `#[serde(default)]` for field addition; schema hash for breaking change detection; `helm_device_migrate_checkpoint` export protocol
- **HelmEventBus Event Types**: mark all variants `#[non_exhaustive]`; stabilize discriminants; document DLD restriction to `Custom { name, data }` only
- **Debug Protocol (HelmProtocol)**: implement versioned handshake with `u32 major + u32 minor` at connection time

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
