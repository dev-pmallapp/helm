# helm-devices — Design Decisions

> Resolved answers to all 40 design questions in [DESIGN-QUESTIONS.md](./DESIGN-QUESTIONS.md).
> Each decision is derived from the research findings in [RESEARCH.md](./RESEARCH.md).
> Decisions are binding for implementation; phased decisions note the applicable phase.

---

## Domain 1: Device Modeling Infrastructure

**Q1.1 — Interior mutability strategy for `Device::read()`**
**DECISION:** Change `Device::read(&self, ...)` to `Device::read(&mut self, ...)`.
The hot loop is single-threaded (design rule 8); `World::mmio_read` already holds exclusive access during dispatch. Eliminates the entire `Cell`/`RefCell`/`UnsafeCell` debate — device state is directly mutable on read. Matches QEMU and gem5 (`read()` is non-const in both). FIFO draining on read (`rx_fifo.pop_front()`) works without any wrapper. The LLD-device-trait already notes this as the most pragmatic path.

---

**Q1.2 — Banked register sets selected by control register**
**DECISION (Phase 0):** Manual dispatch in `on_read_*`/`on_write_*` hooks; the macro does not handle banking.
**DECISION (Phase 1+):** Overlay banks — multiple `register_bank!` invocations generate separate structs; the device's `Device::read/write` dispatches between them based on runtime bank-select state.
The `banked_by = CTRL.FIELD` macro clause is rejected: banking logic varies too wildly across devices (2-way DLAB, N-way per-CPU, nested) to resist uniform macro treatment.

---

**Q1.3 — `MmioHandler` concrete type vs. `dyn DeviceCallbacks` vs. generic**
**DECISION:** Keep the current design — generated `mmio_write` receives `&mut ConcreteDevice`.
Hot-path rule forbids vtable dispatch on every MMIO write. Register-layout reuse across devices is rare in practice. The generic `<D: RegisterHooks>` adds complexity without benefit because the hook trait is device-specific by construction. Confirmed by svd2rust and QEMU (both generate monomorphic per-device dispatch).

---

**Q1.4 — Device obtaining `EventQueue` without `helm-event` dependency**
**DECISION:** Define `trait TimerScheduler` in `helm-core`; `helm-event::EventQueue` implements it; devices store `Arc<dyn TimerScheduler>` populated at `elaborate()`.

```rust
// In helm-core
pub trait TimerScheduler: Send + Sync + 'static {
    fn schedule_callback(&self, delay_ticks: u64, callback: Box<dyn FnOnce() + Send>);
    fn current_tick(&self) -> u64;
}
```

Preserves the `helm-devices → helm-core only` hard constraint. The vtable overhead is on the cold schedule path, not the MMIO path. Rejects `Any` (runtime panics) and `HelmEventBus` translation (wrong semantic model). Matches Renode's constructor-injection pattern.

---

**Q1.5 — Multi-bank hook borrow conflicts**
**DECISION:** Complex multi-bank devices (GIC, SMMU) bypass `register_bank!` and implement `Device::read/write` manually against a single flat state struct.
`register_bank!` is for simple devices with ≤16 registers and local side effects (UART, GPIO, timer). The GIC has ~100+ registers with cross-bank dependencies that cannot be cleanly expressed in the macro. Manual dispatch with a flat `GicState` struct (as QEMU and gem5 both use) eliminates all borrow conflicts. The macro boundary is: per-bank independence holds → use the macro; cross-bank side effects required → bypass it.

---

**Q1.6 — ParamSchema validation timing**
**DECISION:** Type/range validation at Python attribute assignment time (Python descriptor protocol); cross-field semantic validation at `realize()`.
Assignment-time validation: `DeviceRegistry` registration augments the injected Python class with `TypedParam` descriptors derived from `ParamSchema`. Writing `uart.clock_hz = "bad"` raises `ValueError` immediately. Semantic validation (e.g., baud divisor > 0 when FIFO enabled) stays in `realize()` because it requires all fields to be present. Matches QEMU's two-phase model (property setter = type, realize = semantics).

---

**Q1.7 — Registers wider than 32 bits**
**DECISION:** Add a per-register `width` qualifier to `register_bank!`, defaulting to 32.

```
reg TTBR0 @ 0x08 width 64 { field BADDR [47:1]; }
```

Generated struct field is `u64`; hook signature is `on_write_ttbr0(&mut self, old: u64, new: u64)`. Width-32 default preserves backward compatibility. The "always u64 internally" approach is rejected — losing type information in hook signatures forces device authors to mask manually and obscures intent. Matches svd2rust and DML per-register `<size>` declarations.

---

**Q1.8 — Checkpoint format compatibility contract**
**DECISION (Phase 0):** `bincode` serialization with a manual `u32 CKPT_VERSION` as the first field; device author bumps it on breaking changes.
**DECISION (Phase 2+):** The `register_bank!` macro generates a compile-time schema hash (`const_fnv1a_hash` of field names+types). The hash is stored in the checkpoint header. Hash mismatch triggers a migration lookup; `#[serde(default)]` handles field addition transparently.
Bincode stays for performance (compact, fast). The schema hash adds the self-describing detection that bincode lacks, without switching to JSON/postcard.

---

**Q1.9 — Thread safety for multi-hart parallel simulation**
**DECISION (Phase 0–2):** `MemoryMap`-level serialization — the dispatch layer ensures one hart accesses a device at a time. Devices remain `!Sync`. No per-device locking.
**DECISION (Phase 3+):** Per-device opt-in via a ticket-lock at the dispatch layer (`DeviceSlot` wrapping `UnsafeCell<Box<dyn Device>>`). Devices stay `!Sync`; threading concern is invisible to device authors.
`Device` trait requires `Send` (correct). `RefCell` is banned from device state (replaced by `&mut self` from Q1.1 decision). Matches QEMU's BQL model for Phase 0–2, QEMU's BQL-free opt-in direction for Phase 3+.

---

**Q1.10 — Write-1-to-clear hook contract**
**DECISION:** The `on_write_*` hook receives `(old: u32, new: u32)` where `new` is the **post-W1C value** (old & ~written). The raw write value is available via a separately generated `raw_val` parameter or accessor.
Post-W1C is correct for the device author's primary use (acting on the resulting state). The raw write value is available for tracing/logging. Matches gem5's `RegisterBank` W1C handling. Rejects "hook sees raw write" (forces device author to re-implement W1C) and "only post-W1C, no raw" (prevents observability).

---

## Domain 2: Built-in SoC Devices

**Q2.1 — GIC-v3: separate Device impls vs. monolithic**
**DECISION:** Three separate `Device` implementations sharing `Arc<UnsafeCell<GicState>>`:
- `GicDistributor` — implements `Device`, maps GICD (64KB)
- `GicRedistributor { pe_index: usize }` — implements `Device`, maps GICR (128KB per PE)
- `GicIts` — implements `Device`, maps ITS (128KB)
- `GicV3` — implements `SimObject`, owns `GicState`, handles checkpoint

`UnsafeCell` (not `Mutex`) because the simulation is single-threaded during RUN (design rule 8). A parent `GicV3` `SimObject` serializes `GicState` exactly once at checkpoint. Each sub-device registers its own MMIO region independently — the current `region_size() → u64` API is preserved without modification. `Mutex` is rejected (lock contention on every interrupt delivery). Monolith is rejected (requires multi-region `MemoryMap` API extension).

---

**Q2.2 — CPU system registers: `ArchState` fields, Device MMIO, or `SysRegHandler`**
**DECISION:** `SysRegMap` in `helm-core` with two entry kinds:

```rust
enum SysRegEntry {
    Inline { read_offset: usize, write_offset: usize },  // zero-cost field access in ArchState
    Handler(Box<dyn SysRegHandler>),                      // device-injected at elaborate()
}
trait SysRegHandler: Send {
    fn read(&self, ctx: &ExecContext) -> u64;
    fn write(&mut self, ctx: &mut ExecContext, val: u64);
}
```

`MPIDR_EL1`, `SCTLR_EL1`, `TTBR0_EL1` → `Inline` (pointer dereference, zero overhead). `CNTPCT_EL0`, `ICC_IAR1_EL1` → `Handler` (vtable call). Map built at `elaborate()`, immutable during RUN. The MRS/MSR executor in `helm-arch` does one `HashMap::get()` then branches. Validated by QEMU's `ARMCPRegInfo` (which is this exact pattern). Breaks no dependency rules (`SysRegHandler` lives in `helm-core`).

---

**Q2.3 — ARM SMMU: walk in device or delegated to helm-memory**
**DECISION:** Shared walk **primitives** (page table descriptor parsing, granule logic) in a `helm-memory-walk` sub-crate or module; separate walk **engines** for CPU MMU and SMMU.
The SMMU does not reuse the CPU's TLB (different invalidation semantics, VMID vs ASID tagging, different fault paths). Validated by QEMU — `target/arm/ptw.c` and `hw/arm/smmu-common.c` are separate with no shared code. The SMMU device lives in a future `helm-devices-arm` crate and imports the walk primitives. It does not depend on `helm-memory`'s TLB infrastructure. This is Phase 1+ work.

---

**Q2.4 — ARM Generic Timer `CNTPCT_EL0`: VirtualClock direct or Device read**
**DECISION:** `SysRegHandler` with a clock closure captured at `elaborate()` time:

```rust
// During elaborate(), timer device registers:
sysreg_map.register(CNTPCT_KEY, Box::new(TimerCounterHandler {
    clock: Arc::clone(&self.system_clock),
}));
```

Not `ArchState` (would couple ArchState to clock semantics). Not MMIO (wrong architectural model; GICv3 has no memory-mapped CNTPCT). The `TimerScheduler` from Q1.4 provides `current_tick()`; the `TimerCounterHandler` translates ticks based on `CNTFRQ_EL0` to produce the counter value. Different behavior per `TimingModel` is encapsulated in the clock implementation, not in the handler.

---

**Q2.5 — ARM PSCI: Device, engine logic, or `PowerController` trait**
**DECISION:** `trait PowerController` in `helm-core`; the engine implements it; the PSCI SMC/HVC handler receives `Arc<dyn PowerController>` at `elaborate()`.

```rust
// In helm-core
pub trait PowerController: Send + Sync {
    fn cpu_on(&self, target_mpidr: u64, entry_point: u64, context_id: u64) -> PsciResult;
    fn cpu_off(&self, this_mpidr: u64) -> PsciResult;
    fn system_reset(&self);
}
```

Preserves dependency direction (`helm-arch` depends on `helm-core`, not `helm-engine`). The ISA executor dispatches SMC/HVC to the PSCI handler which calls `self.power_controller.cpu_on(...)`. The engine (in `helm-engine`) implements `PowerController` and manages the hart scheduler. Rejected: PSCI-as-Device (wrong direction; device would need engine reference), hardcoded engine logic (cannot be replaced by custom PSCI implementation).

---

**Q2.6 — GIC-v3 LPI tables: `MemInterface`, `MemoryMap`, or direct `FlatMem`**
**DECISION:** DMA port abstraction — `Arc<dyn DmaPort>` stored at `elaborate()` time, with a cached LPI table in the GIC:

```rust
pub trait DmaPort: Send + Sync {
    fn dma_read(&self, phys_addr: u64, buf: &mut [u8]);
}
```

LPI configuration table is read once at `GICR_PROPBASER` write time and cached in the GIC's state. Cache invalidation occurs on TLBI commands. On the interrupt-delivery hot path, LPI priority/enable comes from the cache, not a live DMA read. Full `MemoryMap` path is too slow for interrupt delivery critical path. Direct `FlatMem` reference breaks abstraction and bypasses SMMU. Caching eliminates the per-interrupt DMA cost while retaining architectural correctness.

---

**Q2.7 — GICv3 ICC_* system registers: dispatch without `helm-devices` dependency**
**DECISION:** Same mechanism as Q2.2 — `SysRegMap` injection at `elaborate()`.
The GIC CPU interface (one per PE) calls `sysreg_map.register(ICC_IAR1_KEY, Box::new(GicCpuIfHandler { ... }))` during `elaborate()`. The MRS/MSR executor in `helm-arch` does a single lookup — it has no knowledge of the GIC. `helm-arch` depends only on `helm-core`. The GIC's `SysRegHandler` implementation lives in `helm-devices`. This is the same answer as Q2.2 by design — all device-backed sysregs use the same injection mechanism.

---

**Q2.8 — EventQueue callback timing: instruction boundaries?**
**DECISION:** EventQueue callbacks are **only** drained at instruction boundaries.
`drain_until(current_tick)` is called once per quantum in the dispatch loop, never mid-instruction. This guarantees that `InterruptPin::assert()` from an EventQueue callback only modifies interrupt state when `ArchState` is in a consistent post-instruction state. The quantum size is timing-model specific (Virtual: 1000 instructions, Interval: per-IPC block, Accurate: per pipeline drain). Rejected: arbitrary callback points (unsafe interrupt injection into mid-instruction state).

---

**Q2.9 — Watchdog-initiated system reset mechanism**
**DECISION:** `HelmEventBus` with a `HelmEvent::DeviceSignal` variant plus a `DeviceAction` return type on `write()`.

```rust
// Device fires:
self.event_bus.fire(&HelmEvent::DeviceSignal {
    device: self.object_id(),
    signal: "watchdog_reset",
    data: None,
});
```

The engine subscribes to `HelmEvent::DeviceSignal { signal: "watchdog_reset" }` and calls `World::reset_all()` between instruction steps. The device does not need a reference to `World`. Signal names are `&'static str` — no dynamic allocation. Rejected: `DeviceAction::Reset` return from `write()` (changes infallible signature); direct `World` reference in device (wrong direction).

---

**Q2.10 — GIC affinity routing vs. World's flat object namespace**
**DECISION:** `World::affinity_map() → &AffinityMap`; populated from Python config; frozen after `startup()`.

```python
# In platform Python config:
system.register_affinity(cpu0, mpidr=0x00000000)
system.register_affinity(cpu1, mpidr=0x00000001)
```

The `AffinityMap` maps `Mpidr → HelmObjectId`. GIC Distributor stores `Arc<AffinityMap>` at `elaborate()`. `MPIDR_EL1` in each hart's `ArchState` is an `Inline` `SysRegMap` entry pointing to a field set from the affinity config. The map validates that `ArchState` MPIDR fields and `AffinityMap` entries agree during `validate_wiring()`. The same pattern extends to SMMU stream IDs and PCI requester IDs (Phase 3+).

---

## Domain 3: Dynamically Loadable Devices

**Q3.1 — Hot-reload of `.so` plugins at runtime**
**DECISION:** Checkpoint-bracketed swap — no true live hot-reload. Phase 2+, gated behind `--dev-reload`.

Protocol:
1. `HelmEngine::pause()` — quiesce event queue, drain IO
2. `World::checkpoint_save()` — serialize all device state
3. `DeviceRegistry::unload_plugin(name)` — drop `Box<dyn Device>`, remove from `MemoryMap`, drop `Library` (triggers `dlclose`)
4. `DeviceRegistry::load_plugin(new_path)` — `dlopen` new `.so`, ABI version check
5. `DeviceRegistry::create(name, params)` — instantiate new device from new factory
6. `World::wire_and_restore()` — re-map MMIO, re-wire interrupts, call `checkpoint_restore()`
7. `HelmEngine::resume()`

True live hot-reload (without pause) is not pursued. The wiring graph is frozen after `startup()` by design; unfreezing it introduces unbounded complexity. Wasm isolation is rejected for MMIO-frequency devices (10–88% call overhead).

---

**Q3.2 — Minimum stable ABI surface**
**DECISION:** Pure C ABI with `cbindgen` — the existing `LLD-device-registry.md` design is correct and is kept.

Stable surface:
- `HELM_DEVICES_ABI_VERSION: u32` — bumped on any breaking change
- `helm_device_register: extern "C" fn(*mut DeviceRegistry)` — sole entry point
- `DeviceDescriptor` — `#[repr(C)]` struct with `*const c_char` name, `extern "C"` factory function pointer, `extern "C"` vtable function pointers for `Device` methods

No `abi_stable` (ABI changes per `0.y.0` release). No `stabby` (newer, less battle-tested). No Rust trait objects crossing the boundary directly. Survives any Rust toolchain upgrade because the boundary is C types and C function pointers. Also accessible to C/C++ plugin authors.

---

**Q3.3 — Transparent mixing of plugin and built-in devices**
**DECISION:** The existing `LLD-device-registry.md` design is correct and transparent mixing already works.
Both paths produce a `DeviceDescriptor` struct; `DeviceRegistry` is blind to the source. Built-ins use `inventory::submit!` (linker-level registration), plugins use `helm_device_register` (C-ABI call at load time). Python config writes `helm_ng.Uart16550()` without knowing or caring which path produced the descriptor. Checkpoint/restore is identical because both paths produce the same `SimObject` lifecycle. No changes needed.

---

**Q3.4 — Plugin isolation without full process isolation**
**DECISION (Phase 0–2):** Layered defense — Rust type safety as primary defense, no additional sandboxing.
Plugin devices implement `Device: SimObject + Send`. Safe Rust code in a plugin cannot corrupt host memory (no `unsafe` required in a basic device implementation). Panics are wrapped with `catch_unwind` at the MMIO dispatch boundary.
**DECISION (Phase 3+ optional):** WebAssembly via Wasmtime as an optional isolation mode for untrusted device plugins — not on the default path. `seccomp` filtering of plugin syscalls is a lighter alternative. No process isolation (IPC latency on every MMIO call is unacceptable). The trust model for Phase 0–2 is: plugins are trusted code from the same developer.

---

**Q3.5 — Python class `__init__` params vs. `ParamSchema` divergence**
**DECISION:** `ParamSchema` is authoritative. The `python_class: &'static str` field in `DeviceDescriptor` is removed; replaced by `python_class_extra: Option<&'static str>` for additional methods/properties not covered by the schema.
The loader auto-generates the Python class from `ParamSchema` at registration time. This eliminates the divergence problem entirely — there is one source of truth. Device authors who need custom Python-side behavior (custom `__repr__`, helper properties) add it via `python_class_extra`. The generated `__init__` signature exactly matches the `ParamSchema` field list.

---

**Q3.6 — Checkpoint migration when plugin is upgraded**
**DECISION:** Version tag + `#[serde(default)]` for non-breaking changes; `fn migrate_checkpoint(old: u32, data: &[u8]) -> Vec<u8>` export for breaking changes.

```rust
// Plugin exports (optional, checked at load time):
#[no_mangle]
pub extern "C" fn helm_device_migrate_checkpoint(
    name: *const c_char,
    old_version: u32,
    data: *const u8,
    len: usize,
    out_len: *mut usize,
) -> *mut u8;  // caller frees
```

Phase 0: manual `CKPT_VERSION` bump. Phase 2+: schema hash (from Q1.8 decision) detects incompatible checkpoints at restore time and invokes `migrate_checkpoint` if the export exists. `#[serde(default)]` handles field addition without migration code. Removed fields require a migration function.

---

**Q3.7 — Device type aliasing and versioned names**
**DECISION:** Add `aliases: &'static [&'static str]` to `DeviceDescriptor`. No versioned names (no `@N` suffix) for Phase 0–2.
`DeviceRegistry` resolves aliases to the canonical name. Python class injection uses the canonical name; aliases are lookup-only. Checkpoint blobs store the canonical name. When a device is renamed, the old name is added as an alias permanently. Versioned names (`uart16550@1.0`) are deferred until helm-ng has stable device releases.

---

**Q3.8 — Plugin host capability requirements declaration**
**DECISION:** Both declarative and imperative:
- Add `required_capabilities: &'static [HostCapability]` to `DeviceDescriptor` for well-known capabilities (KVM, raw sockets, VFIO, huge pages). `DeviceRegistry::load_plugin()` checks these at load time and fails fast with a clear diagnostic.
- Optional `fn check_requirements() -> Result<(), String>` export for device-specific requirements not covered by the enum.

```rust
pub enum HostCapability { Kvm, RawSocket, Vfio, HugePages }
```

Load-time checking gives the best UX — "device X requires KVM; this host does not support KVM" before any simulation starts.

---

**Q3.9 — HelmEventBus event type identity across `.so` boundaries**
**DECISION:** Plugins use `HelmEvent::Custom { name: &'static str, data: Arc<dyn Any + Send + Sync> }` only.
Core event variants (`Exception`, `CsrWrite`, `MemWrite`, etc.) are defined in `helm-core` and have stable discriminants because they are part of the core ABI. Plugins cannot add new enum variants (Rust enums are not open). The `Custom` variant's identity is the `name` string (a hash of the name string is computed for fast dispatch). No enum discriminant crossing `.so` boundaries. This is already the implicit design in `HelmEvent`; this decision makes it explicit.

---

**Q3.10 — Plugin-registered custom bus trait implementations**
**DECISION (Phase 0–2):** Plugins implement only `Device` (MMIO). No custom `Bus` trait implementations.
**DECISION (Phase 3+):** Define `trait ProtocolDevice` with an associated `Transaction` type as the extension point for non-register bus protocols (CAN, USB bulk, custom message protocols). Plugins can implement `ProtocolDevice<CAN>` where `CAN: BusProtocol`. The `BusDevice` register-based interface covers I2C/SPI/AMBA. The `ProtocolDevice` trait is a Phase 3 design exercise.

---

## Domain 4: Bus and Protocol Modeling

**Q4.1 — PCIe ECAM config space: PciBus internal decode vs. per-function MemoryMap regions**
**DECISION:** PciBus internal decode — the existing `LLD-bus-framework.md` design is correct and is kept.
`PciBus` is a single `Device` mapped as one ECAM region in `MemoryMap`. `PciBus::read/write` call `decode_ecam()` to extract BDF and dispatch to `HashMap<(u8,u8,u8), Box<dyn PciEndpoint>>`. Per-function 4KB regions in `MemoryMap` are rejected (up to 65536 regions; destroys FlatView performance). Matches QEMU, gem5, and SIMICS unanimously. Config space is a cold path; HashMap lookup overhead is negligible.

---

**Q4.2 — MSI-X: MemoryMap MMIO dispatch vs. dedicated MSI routing shortcut**
**DECISION (Phase 0–2):** Full `MemoryMap` address-space path. PCIe device calls `self.msi_pin.send(addr, data)` → `MemoryMap::write(addr, 4, data)` → FlatView dispatch → GIC ITS `Device::write()`.
**DECISION (Phase 3+):** `OptimizedMsiRouter` shortcut when no SMMU is present, selected at `elaborate()` time.
The address-space path is architecturally correct and enables SMMU MSI interception. MSI delivery is not on the per-instruction hot path. Matches QEMU's `address_space_stl_le()` approach. The gem5 shortcut is rejected for Phase 0–2 (breaks SMMU correctness, breaks memory trace observability).

---

**Q4.3 — I2C multi-master arbitration**
**DECISION:** Single-master I2C — the existing `LLD-bus-framework.md` design is correct. Multi-master deferred indefinitely.
QEMU, gem5, and Renode all implement single-master I2C. Multi-master I2C is rare in modeled embedded systems. The limitation is documented in `LLD-bus-framework.md`. If needed in future, the transaction-level arbitration creative approach (deterministic tie-breaking by object ID) is the preferred implementation path — not bit-level SDA simulation.

---

**Q4.4 — AMBA AXI backpressure: model READY/VALID or collapse to zero-latency**
**DECISION (Virtual mode, Phase 0–1):** Zero-latency — all bus transactions complete in the same simulation tick.
**DECISION (Interval mode, Phase 1+):** Estimated latency — `Device::write()` returns immediately, but the timing model accounts for bus latency in the IPC estimate. No `BusStall` return type.
**DECISION (Phase 3+ Accurate mode):** AT-level per-channel modeling with `BusStall { ready_in_cycles }` return type, only if latency-accurate AMBA modeling is a stated goal.
The `Device::write() → ()` infallible signature is preserved for Phase 0–2. No backpressure API is added before the Accurate timing model is implemented.

---

**Q4.5 — DMA engine memory access: `MemoryMap`, `BusMaster` trait, or direct `FlatMem`**
**DECISION:** DMA goes through `MemoryMap` via a `DmaContext` stored at `elaborate()`.

```rust
pub trait DmaPort: Send + Sync {
    fn dma_read(&self, addr: u64, buf: &mut [u8]);
    fn dma_write(&self, addr: u64, buf: &[u8]);
}
```

`DmaPort` is implemented by `World` (using its `MemoryMap`). Devices receive `Arc<dyn DmaPort>` at `elaborate()`. The borrow conflict (`&mut Device` + `&mut MemoryMap` simultaneously) is resolved: the device calls `self.dma_port.dma_write(...)` which goes through `Arc` into the `World` implementation — no simultaneous borrows. DMA writes are visible to `HelmEventBus` and SMMU can intercept them. Direct `FlatMem` reference is rejected (bypasses SMMU, breaks observability).

---

**Q4.6 — PCI BAR re-programming from inside a config space write handler**
**DECISION:** Post-write `RemapCommand` queue on `PciBus`, drained by `MemoryMap` after `Device::write()` returns.

```rust
// PciBus::write() pushes:
self.pending_remaps.push(RemapCommand { old_region, new_base });
// World drains after write() returns:
pci_bus.drain_remaps(&mut memory_map);
```

The `Device::write() → ()` signature is preserved (no return type change). The borrow conflict is resolved: `Device::write()` ends its borrow, then the caller (World's dispatch loop) drains pending remaps and calls `MemoryMap::remap()`. FlatView recomputation is lazy (deferred to next `mmio_read/write` on affected region). Matches the existing `LLD-bus-framework.md`'s design intent and solves the borrow problem cleanly.

---

**Q4.7 — SPI flash XIP: dual-view modeling**
**DECISION:** Two separate `Device` implementations owned by the QSPI controller device, registered as separate MemoryRegions.
- Region 1 (command): maps the QSPI register bank (e.g., 64 bytes) — always active — `Device` implementing `register_bank!`
- Region 2 (XIP window): maps flash contents as read-only — active only in XIP mode — `Device` returning flash data from its buffer

The QSPI controller device toggles Region 2's presence in `MemoryMap` by calling `memory_map.map/unmap` via the post-write command queue (same mechanism as Q4.6). The flash is both a `BusDevice` attached to an `SpiBus` (command mode) and a `Device` in `MemoryMap` (XIP mode). Both views share the flash content buffer via `Arc<FlashContents>`.

---

**Q4.8 — PCIe AER error propagation through bus hierarchy**
**DECISION (Phase 0–2):** No AER modeling. PCI config space includes the AER Extended Capability header with static/zeroed registers (prevents Linux `lspci` complaints). No error injection, no error propagation.
**DECISION (Phase 3+):** Error injection API — `PciEndpoint::inject_error(aer_type: AerErrorType)` that sets AER status registers and fires an `InterruptPin` for the correctable/uncorrectable error interrupt. Hierarchical propagation (endpoint → bridge → root complex) deferred to when Linux AER driver compatibility is a goal.

---

**Q4.9 — `Bus::attach()` and PCIe hot-plug**
**DECISION (Phase 0–2):** No hot-plug. The wiring graph is frozen after `startup()` (existing design rule). `Bus::attach()` is callable only during `elaborate()`.
**DECISION (Phase 3+):** Two-phase validated hot-plug:
1. Validate: check BAR space available, MSI routing possible, power budget
2. Commit: call `Bus::attach()` post-startup, update `MemoryMap`, re-wire MSI, fire `HelmEvent::PciHotPlugInsert`

Hot-plug requires relaxing the "frozen after startup" rule for the bus subsystem only, with explicit tracking of which `MemoryMap` regions are hot-plug-managed. This intersects with Q4.6 (BAR programming) and Q4.9 (interrupt re-wiring). Deferred to Phase 3.

---

**Q4.10 — USB xHCI ring polling: EventQueue timers, `advance()`, or doorbell**
**DECISION (Phase 0–2):** Pure doorbell-driven — no timer polling. `Device::write()` on the doorbell register synchronously processes the command/transfer ring.

```rust
// xhci.rs Device::write():
if offset == DOORBELL_REGISTER {
    self.process_ring(ring_index);  // synchronous, completes immediately
}
```

Matching QEMU's `xhci.c` doorbell handler. No `EventQueue` timers for ring polling. No `Device::step()` (adds overhead to every CPU instruction for devices not being used).
**DECISION (Phase 3+):** Doorbell triggers ring processing; completion events are deferred via `EventQueue` with simulated DMA latency for Interval/Accurate timing modes.
The doorbell assumption is valid for Linux xHCI guests — the xHCI driver rings the doorbell immediately after posting transfer descriptors. Timer-based polling is rejected (arbitrary interval, no clear correct value).

---

## Summary Table

| Q | Decision (short) | Phase |
|---|-----------------|-------|
| Q1.1 | `&mut self` on `Device::read()` | Phase 0 |
| Q1.2 | Manual hook dispatch; overlay banks later | Phase 0 / Phase 1+ |
| Q1.3 | Concrete `&mut ConcreteDevice` in generated code | Phase 0 |
| Q1.4 | `TimerScheduler` trait in `helm-core` | Phase 0 |
| Q1.5 | Complex devices bypass macro, use flat state struct | Phase 0 |
| Q1.6 | Type/range at assignment; semantic at `realize()` | Phase 0 |
| Q1.7 | Per-register `width` qualifier, default 32 | Phase 1 |
| Q1.8 | bincode + manual version; schema hash | Phase 0 / Phase 2+ |
| Q1.9 | MemoryMap-level serialization; per-device opt-in | Phase 0–2 / Phase 3+ |
| Q1.10 | Hook sees post-W1C `new`; raw write via accessor | Phase 0 |
| Q2.1 | 3 separate `Device` impls, `Arc<UnsafeCell<GicState>>` | Phase 1 |
| Q2.2 | `SysRegMap` with `Inline`/`Handler` in `helm-core` | Phase 1 |
| Q2.3 | Shared walk primitives; separate walk engines | Phase 1 |
| Q2.4 | `SysRegHandler` with clock closure | Phase 1 |
| Q2.5 | `PowerController` trait in `helm-core` | Phase 1 |
| Q2.6 | `DmaPort` trait + cached LPI table in GIC | Phase 1 |
| Q2.7 | `SysRegMap` injection (same as Q2.2) | Phase 1 |
| Q2.8 | EventQueue drains only at instruction boundaries | Phase 0 |
| Q2.9 | `HelmEvent::DeviceSignal`, engine subscribes | Phase 1 |
| Q2.10 | `World::affinity_map()`, configured from Python | Phase 1 |
| Q3.1 | Checkpoint-bracketed swap, `--dev-reload` flag | Phase 2+ |
| Q3.2 | Pure C ABI (existing design), no `abi_stable` | Phase 0 |
| Q3.3 | Existing design correct, transparent mixing works | Phase 0 |
| Q3.4 | Layered defense (Rust safety + catch_unwind) | Phase 0 / Phase 3+ optional Wasm |
| Q3.5 | `ParamSchema` authoritative, auto-generate Python class | Phase 1 |
| Q3.6 | Version tag + `#[serde(default)]` + `migrate_checkpoint` export | Phase 0 / Phase 2+ |
| Q3.7 | `aliases` field on `DeviceDescriptor`; no versioned names | Phase 1 |
| Q3.8 | `required_capabilities` enum + `check_requirements()` export | Phase 1 |
| Q3.9 | `Custom { name, data }` only from plugins; typed variants for core | Phase 0 |
| Q3.10 | MMIO `Device` only; `ProtocolDevice` trait in Phase 3+ | Phase 0 / Phase 3+ |
| Q4.1 | PciBus internal decode (existing design correct) | Phase 1 |
| Q4.2 | Full MemoryMap address-space path; shortcut Phase 3+ | Phase 1 / Phase 3+ |
| Q4.3 | Single-master I2C (existing design); multi-master deferred | Phase 1 |
| Q4.4 | Zero-latency (Virtual); estimated (Interval); AXI-AT (Phase 3+) | Phase 1+ |
| Q4.5 | `DmaPort` trait via `MemoryMap`, stored at `elaborate()` | Phase 1 |
| Q4.6 | Post-write `RemapCommand` queue; lazy FlatView recompute | Phase 1 |
| Q4.7 | Two separate MemoryRegions (command regs + XIP window) | Phase 1 |
| Q4.8 | No AER Phase 0–2; error injection API Phase 3+ | Phase 3+ |
| Q4.9 | No hotplug Phase 0–2; two-phase hotplug Phase 3+ | Phase 3+ |
| Q4.10 | Doorbell-driven; EventQueue completion (Phase 3+) | Phase 1 / Phase 3+ |
