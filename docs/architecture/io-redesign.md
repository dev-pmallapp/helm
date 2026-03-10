# HELM I/O Subsystem Redesign

**Status**: Implemented
**Date**: 2026-03-10
**Depends on**: `restructuring-plan.md` (Phase 0 types)
**Goal**: Redesign helm-device for runtime-loadable peripherals, hot-plug
attach/detach, interface-based inter-device communication, cooperative
multi-clock scheduling, and LLVM-IR accelerator loading.

---

## Implementation Status

| Component | File(s) | Status |
|-----------|---------|--------|
| `Connection<I>` + `DeviceInterface` | `connection.rs` | Done |
| `AddressMap` (binary search, transactional mutations, listeners) | `address_map.rs` | Done |
| `IrqWire` + `IrqSink` | `irq_wire.rs` | Done |
| `DeviceCtx` (realize/unrealize context) | `device_ctx.rs` | Done |
| Device trait lifecycle (`realize`/`unrealize`/`is_hotpluggable`/`clock_hz`) | `device.rs` | Done |
| `IrqRouter::remove_route()` / `remove_routes_for_device()` | `irq.rs` | Done |
| `DeviceRegistry` introspection (`PropertySpec`, `DeviceConfig`, `list_properties`) | `loader.rs` | Done |
| `CoopScheduler` (higan-style multi-clock) | `coop_scheduler.rs` | Done |
| `PlatformV2` (AddressMap + CoopScheduler + IrqRouter) | `platform_v2.rs` | Done |
| `LlvmAcceleratorDevice` (Device trait) | `helm-llvm/accel_device.rs` | Done |
| 45 new tests across 6 test files | `tests/` | Done |

### Not yet migrated (future PRs)
- Existing devices (Pl011, Sp804, etc.) to use `Connection<I>` instead of `Box<dyn Backend>`
- `FsSession` from `DeviceBus` to `AddressMap`
- `dlopen` device loading (stub only — `load_library()` not implemented)
- Three-queue cycle-accurate CDFG engine inside `LlvmAcceleratorDevice` (currently runs to completion synchronously)

---

## 1  Evaluation of Prior Art

### 1.1  QEMU

**Strengths:**
- QOM type registry with runtime `object_class_by_name()` lookup
- Two-phase lifecycle (`instance_init` infallible, `realize` failable) —
  allows introspection before commitment
- MemoryRegion transaction model — batched add/del with atomic FlatView rebuild
- GPIO model — typed `qemu_irq` wires with fan-out (`SPLIT_IRQ`) and
  interception for teardown
- Hot-plug 3-phase protocol (request → guest ack → unplug) — safe for
  guest-visible devices
- `-device` / `device_add` command-line/QMP for runtime instantiation

**Weaknesses:**
- Devices call each other through direct C function pointers — replacing a
  device requires rewiring every caller
- `unrealize` is rarely implemented — most devices are permanent
- Global BQL (Big QEMU Lock) for device state — no per-device concurrency
- Properties are stringly-typed C macros, not type-safe

**Adopted in HELM:**
- Two-phase lifecycle → `Device::realize()` / `Device::unrealize()`
- MemoryRegion transaction model → `AddressMap` with `commit()`
- Factory registry → `DynamicDeviceLoader` with `PropertySpec`
- GPIO-style typed IRQ wires → `IrqWire` + `IrqSink`

### 1.2  Simics

**Strengths:**
- **Interface system** — devices communicate through named interface vtables
  (`SIM_get_interface()`), not direct pointers.  Replacing a device means
  updating one attribute; the interface pointer is re-resolved automatically.
- Attribute system — all device state is accessible as typed attributes
  (integer, string, object-ref, list).  Checkpoint = read all attributes.
- Port objects — one device exposes the same interface multiple times under
  named ports (4-port switch, multi-channel DMA)
- Connector framework — `hotplug: true/false`, `direction: up/down/any`,
  typed connector matching
- `conf_class_t` lifecycle: `new_instance` → attribute setting →
  `finalize_instance` → `delete_instance`

**Weaknesses:**
- C API, no type safety at interface boundaries
- DML is proprietary, not open-source
- Memory map is a flat attribute list, not a tree with priorities

**Adopted in HELM:**
- Interface-based communication → `Connection<I: DeviceInterface>`
- Connector with `hotplug` flag → `Connection::hotpluggable()`
- Lifecycle → `Device::realize()` / `Device::unrealize()` with `DeviceCtx`

### 1.3  higan

**Strengths:**
- Cooperative multi-clock scheduling via libco coroutines
- Per-device timestamp with scalar normalization for different clock domains
- Selective synchronization — only sync on shared-resource access
- `step(N)` + `resume(peer)` protocol — simple, composable

**Weaknesses:**
- Coroutine stacks are hard to serialize for save/restore
- Performance penalty vs flat state machines for simple devices
- libco is C, not Rust-native

**Adopted in HELM:**
- Absolute scheduler with per-device timestamps → `CoopScheduler` + `DeviceClock`
- Femtosecond-precision timestamps for correct multi-clock interleaving
- `renormalize()` to prevent timestamp overflow

### 1.4  gem5-SALAM

**Strengths:**
- LLVM IR → CDFG → cycle-accurate execution with <1% timing error
- Three-queue scheduler (reservation/compute/memory) with functional unit
  contention modeling
- YAML-driven hardware profile — no recompilation to change FU counts/latencies
- AccCluster pattern: CommInterface + LLVMInterface + SPM + DMA as a unit
- Memory-mapped register interface for host↔accelerator control

**Weaknesses:**
- Requires specific LLVM IR structure (single inlined function)
- C++ codebase, tight coupling to gem5 SimObject model

**Adopted in HELM:**
- `LlvmAcceleratorDevice: Device` — hot-pluggable, MMIO-based control
- `IrqWire` for completion interrupt
- Leverages existing `helm-llvm` `Accelerator` + `InstructionScheduler`
- Three-queue CDFG engine deferred to future PR

---

## 2  Architecture

### 2.1  Core Principles

1. **Interface-based communication** (Simics pattern) — devices hold a
   `Connection<dyn Iface>` that resolves an interface trait object on demand.
2. **Two-phase lifecycle** (QEMU pattern) — construction is infallible;
   `realize(&mut DeviceCtx)` is failable bringup.
3. **Attach/detach at any time** — `AddressMap::attach()` and `detach()`
   are the hot-plug primitives.  Address space is rebuilt transactionally.
4. **Loadable devices** — `DynamicDeviceLoader` factory registry with
   `PropertySpec` introspection.
5. **Multi-clock cooperative scheduling** (higan pattern) — `CoopScheduler`
   drives devices in timestamp order with femtosecond precision.
6. **LLVM-IR accelerator loading** (gem5-SALAM pattern) —
   `LlvmAcceleratorDevice` wraps `Accelerator` as a hot-pluggable `Device`.

### 2.2  The Device Trait — Extended

```rust
pub trait Device: Send + Sync {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()>;
    fn regions(&self) -> &[MemRegion];
    fn name(&self) -> &str;

    // Lifecycle (default no-ops — existing impls compile unchanged)
    fn realize(&mut self, _ctx: &mut DeviceCtx) -> HelmResult<()> { Ok(()) }
    fn unrealize(&mut self, _ctx: &mut DeviceCtx) -> HelmResult<()> { Ok(()) }
    fn is_hotpluggable(&self) -> bool { false }
    fn clock_hz(&self) -> u64 { 0 }

    // Existing defaults
    fn reset(&mut self) -> HelmResult<()> { Ok(()) }
    fn tick(&mut self, _cycles: u64) -> HelmResult<Vec<DeviceEvent>> { Ok(vec![]) }
    fn checkpoint(&self) -> HelmResult<serde_json::Value> { Ok(Value::Null) }
    fn restore(&mut self, _state: &Value) -> HelmResult<()> { Ok(()) }

    // Fast path (FE mode)
    fn read_fast(&mut self, offset: Addr, size: usize) -> HelmResult<u64> { ... }
    fn write_fast(&mut self, offset: Addr, size: usize, val: u64) -> HelmResult<()> { ... }
}
```

### 2.3  DeviceCtx — Realize/Unrealize Context

```rust
pub struct DeviceCtx<'a> {
    pub device_id: DeviceId,
    pub address_map: &'a mut AddressMap,
    pub irq_router: &'a mut IrqRouter,
}

impl DeviceCtx {
    fn map_region(&mut self, base, size, priority) -> RegionHandle;
    fn unmap_region(&mut self, handle);
    fn connect_irq(&mut self, source_line, dest_ctrl, dest_irq) -> usize;
    fn disconnect_irq(&mut self, route_index);
    fn disconnect_all_irqs(&mut self);
}
```

### 2.4  Connection<I> — Interface-Based Communication

```rust
pub struct Connection<I: ?Sized> {
    name: String,
    backend: Option<Box<I>>,
    hotplug: bool,
}

impl<I: ?Sized> Connection<I> {
    fn new(name) -> Self;
    fn hotpluggable(name) -> Self;
    fn connect(&mut self, backend: Box<I>) -> Result<(), ConnectionError>;
    fn disconnect(&mut self) -> Option<Box<I>>;
    fn try_get(&self) -> Option<&I>;
    fn try_get_mut(&mut self) -> Option<&mut I>;
}
```

### 2.5  AddressMap — O(log n) Transactional Address Space

```rust
pub struct AddressMap {
    devices: Vec<Option<DeviceEntry>>,
    regions: Vec<MappedRegion>,
    flat_view: Vec<FlatViewEntry>,
    pending: Vec<Mutation>,
    listeners: Vec<Box<dyn AddressMapListener>>,
}

impl AddressMap {
    fn attach(&mut self, name, device) -> DeviceId;
    fn detach(&mut self, id) -> Option<Box<dyn Device>>;
    fn map_region(&mut self, id, base, size, priority) -> RegionHandle;
    fn unmap_region(&mut self, handle);
    fn commit(&mut self);  // rebuild flat view, notify listeners
    fn dispatch(&mut self, txn) -> HelmResult<()>;  // O(log n)
    fn read_fast(&mut self, addr, size) -> HelmResult<u64>;
    fn write_fast(&mut self, addr, size, val) -> HelmResult<()>;
    fn device(&self, id) -> Option<&dyn Device>;
    fn device_mut(&mut self, id) -> Option<&mut dyn Device>;
}
```

### 2.6  IrqWire + IrqSink — Typed Interrupt Wires

```rust
pub trait IrqSink: Send + Sync {
    fn set_level(&self, line: u32, level: bool);
}

pub struct IrqWire {
    sink: Option<Arc<dyn IrqSink>>,
    line: u32,
}

impl IrqWire {
    fn new(line) -> Self;
    fn connect(&mut self, sink: Arc<dyn IrqSink>);
    fn disconnect(&mut self);
    fn set_level(&self, level: bool);
}
```

### 2.7  IrqRouter — Extended

```rust
impl IrqRouter {
    fn add_route(&mut self, route) -> usize;
    fn remove_route(&mut self, index) -> Option<IrqRoute>;
    fn remove_routes_for_device(&mut self, device_id);
    // ... existing deliver/has_pending/ack methods
}
```

### 2.8  CoopScheduler — Multi-Clock Cooperative Scheduling

```rust
pub struct DeviceClock {
    pub timestamp: u128,  // femtoseconds
    pub freq_hz: u64,
}

pub struct CoopScheduler {
    entries: Vec<SchedulerEntry>,
}

impl CoopScheduler {
    fn register(&mut self, device_id, freq_hz);
    fn unregister(&mut self, device_id);
    fn step(&mut self, map: &mut AddressMap) -> HelmResult<Vec<DeviceEvent>>;
    fn run_steps(&mut self, steps, map) -> HelmResult<Vec<DeviceEvent>>;
    fn run_until_fs(&mut self, target_fs, map) -> HelmResult<Vec<DeviceEvent>>;
    fn renormalize(&mut self);
}
```

### 2.9  PlatformV2 — Integrated Platform

```rust
pub struct PlatformV2 {
    pub name: String,
    pub address_map: AddressMap,
    pub irq_router: IrqRouter,
    pub scheduler: CoopScheduler,
}

impl PlatformV2 {
    fn add_device(&mut self, name, base, device) -> HelmResult<DeviceId>;
    fn remove_device(&mut self, id) -> Option<Box<dyn Device>>;
    fn dispatch(&mut self, txn) -> HelmResult<()>;
    fn read_fast(&mut self, addr, size) -> HelmResult<u64>;
    fn write_fast(&mut self, addr, size, val) -> HelmResult<()>;
    fn tick(&mut self, cycles) -> HelmResult<Vec<DeviceEvent>>;
    fn reset(&mut self) -> HelmResult<()>;
}
```

### 2.10  DeviceRegistry — Extended Loader

```rust
pub struct PropertySpec {
    pub name: String,
    pub ty: PropertyType,  // String | U64 | Bool | Json
    pub description: String,
    pub default: Option<Value>,
    pub required: bool,
}

pub struct DeviceConfig {
    pub type_name: String,
    pub instance_name: String,
    pub properties: HashMap<String, Value>,
}

impl DynamicDeviceLoader {
    fn register_with_properties(&mut self, name, factory, properties);
    fn list_properties(&self, type_name) -> Option<&[PropertySpec]>;
    fn create_from_config(&self, config) -> Result<Box<dyn Device>, DeviceLoadError>;
}
```

### 2.11  LlvmAcceleratorDevice

```rust
pub struct LlvmAcceleratorDevice {
    accel: Option<Accelerator>,
    status: AccelStatus,  // Idle | Running | Complete | Error
    irq: IrqWire,         // completion interrupt
    clock: u64,           // for CoopScheduler
}

impl Device for LlvmAcceleratorDevice {
    fn is_hotpluggable(&self) -> bool { true }
    fn clock_hz(&self) -> u64 { self.clock }
    // transact: MMIO register map (STATUS/CONTROL/CYCLES/LOADS/STORES)
    // tick: placeholder for future CDFG engine
}
```

---

## 3  Migration from Current helm-device

| Current | New | Status |
|---------|-----|--------|
| `DeviceBus` (linear scan) | `AddressMap` (binary search + FlatView) | New type added; DeviceBus still works |
| `DeviceBus::attach_device()` only | `AddressMap::attach()` + `realize()` | Done |
| No `detach` | `AddressMap::detach()` | Done |
| `Box<dyn CharBackend>` in device struct | `Connection<dyn CharBackend>` | Type available; migration per-device |
| `Platform::add_device()` hardcoded | `PlatformV2::add_device()` with registry support | Done |
| `DynamicDeviceLoader` (factory only) | + `PropertySpec`, `DeviceConfig`, introspection | Done |
| `IrqLine` value type | `IrqWire` with `IrqSink` handler | New type added; IrqLine still works |
| `IrqRouter` (no remove) | + `remove_route()`, `remove_routes_for_device()` | Done |
| `MemRegionTree` (no listeners) | `AddressMap` with `AddressMapListener` | Done |
| `DeviceScheduler` (disconnected from bus) | `CoopScheduler` integrated with `AddressMap` | Done |
| `helm-llvm::AcceleratorDevice` (MemoryMappedDevice) | `LlvmAcceleratorDevice` (Device trait) | Done |

### Backward Compatibility

- All 30+ existing `impl Device` types compile unchanged (default no-op lifecycle methods)
- `LegacyWrapper` continues to adapt old `MemoryMappedDevice` impls to `Device`
- `DeviceBus` and `Platform` remain working alongside `AddressMap` and `PlatformV2`
- `DeviceScheduler` remains working alongside `CoopScheduler`

---

## 4  Testing

| Test | What it verifies | Status |
|------|-----------------|--------|
| AddressMap attach/map/dispatch | Basic lifecycle | Pass |
| AddressMap binary search (100 devices) | O(log n) correctness | Pass |
| AddressMap detach removes regions | Clean teardown | Pass |
| AddressMap transactional batch | Batch 3 ops, single commit | Pass |
| AddressMap listener notifications | on_region_add/remove callbacks | Pass |
| AddressMap fast path parity | read_fast matches dispatch | Pass |
| Connection connect and use | Write through connected backend | Pass |
| Connection disconnect returns None | Disconnected → try_get is None | Pass |
| Connection hotplug swap | Disconnect + reconnect different backend | Pass |
| Connection non-hotplug rejects reconnect | Second connect fails | Pass |
| IrqWire connect/assert/deassert | Wire to mock sink, verify callbacks | Pass |
| IrqWire disconnect silences | No callbacks after disconnect | Pass |
| IrqWire reconnect | New sink receives events | Pass |
| IrqRouter add_route returns index | Route indexing | Pass |
| IrqRouter remove_route | Remove by index | Pass |
| IrqRouter remove_routes_for_device | Bulk removal | Pass |
| DeviceCtx realize maps region | Region appears after map_region | Pass |
| DeviceCtx unrealize unmaps region | Region disappears after unmap | Pass |
| DeviceCtx IRQ lifecycle | connect_irq + disconnect_irq | Pass |
| DeviceCtx disconnect_all_irqs | Bulk IRQ cleanup | Pass |
| CoopScheduler register and step | Single device ticked | Pass |
| CoopScheduler multi-clock ordering | Fast device ticked more often | Pass |
| CoopScheduler unregister mid-run | Removed device stops ticking | Pass |
| CoopScheduler renormalize | Timestamps reset to 0 | Pass |
| DeviceClock step | Femtosecond precision | Pass |
| PlatformV2 add/remove lifecycle | 3 devices, remove 1, remaining work | Pass |
| PlatformV2 dispatch to device | Read/write through platform | Pass |
| PlatformV2 hot-plug mid-simulation | Add device after 100 ticks | Pass |
| DeviceRegistry list_properties | Introspect registered properties | Pass |
| DeviceRegistry create_from_config | Config-based device creation | Pass |

**Total: 560 tests pass, 1 pre-existing GIC failure.**

---

## 5  Comparison Summary

| Feature | QEMU | Simics | higan | gem5-SALAM | HELM |
|---------|------|--------|-------|------------|------|
| Type registry | QOM hash table | `SIM_register_class` | N/A | gem5 SimObject | `DynamicDeviceLoader` |
| Device lifecycle | init → realize → unrealize | new → finalize → delete | N/A | SimObject init | construct → realize → unrealize |
| Inter-device comms | Direct fn ptrs | Interface vtables | Direct calls | gem5 ports | `Connection<dyn Trait>` |
| Hot-plug | 3-phase | `hotplug` connector flag | N/A | N/A | `is_hotpluggable()` + `unrealize()` |
| Address space | MemoryRegion + FlatView | `map` attribute list | N/A | gem5 AddrRange | `AddressMap` + FlatView + listeners |
| IRQ model | `qemu_irq` typed wire | `signal` interface | Direct calls | gem5 IntSink | `IrqWire` + `IrqSink` |
| Multi-clock | N/A | N/A | Cooperative threads | gem5 events | `CoopScheduler` |
| Accelerator | N/A | N/A | N/A | LLVM IR → CDFG | `LlvmAcceleratorDevice` |
| Checkpoint | VMState descriptors | Attribute get/set | Coroutine serial | gem5 serialize | `serde` + `checkpoint()`/`restore()` |
| Loadable devices | `-device` / `device_add` | `SIM_create_object` | N/A | Python config | `DeviceRegistry` + factory |
