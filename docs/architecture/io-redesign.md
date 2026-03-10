# HELM I/O Subsystem Redesign

**Status**: Draft
**Date**: 2026-03-10
**Depends on**: `restructuring-plan.md` (Phase 0 types)
**Goal**: Redesign helm-device for runtime-loadable peripherals, hot-plug
attach/detach, interface-based inter-device communication, cooperative
multi-clock scheduling, and LLVM-IR accelerator loading.

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

**Applicable to HELM:**
- Two-phase lifecycle (construct + realize)
- MemoryRegion transaction model (batch mutations, atomic rebuild)
- Factory registry with string-keyed type lookup
- GPIO-style typed IRQ wires

### 1.2  Simics

**Strengths:**
- **Interface system** — devices communicate through named interface vtables
  (`SIM_get_interface()`), not direct pointers.  Replacing a device means
  updating one attribute; the interface pointer is re-resolved automatically.
  This is the single most important pattern.
- Attribute system — all device state is accessible as typed attributes
  (integer, string, object-ref, list).  Checkpoint = read all attributes.
  Restore = write all attributes.
- Port objects — one device exposes the same interface multiple times under
  named ports (4-port switch, multi-channel DMA)
- Connector framework — `hotplug: true/false`, `direction: up/down/any`,
  typed connector matching
- `conf_class_t` lifecycle: `new_instance` → attribute setting →
  `finalize_instance` → `delete_instance`
- DML (Device Modeling Language) — DSL compiling to C, with `connect`,
  `implement`, `register bank` keywords

**Weaknesses:**
- C API, no type safety at interface boundaries
- DML is proprietary, not open-source
- Memory map is a flat attribute list, not a tree with priorities

**Applicable to HELM:**
- Interface-based communication (Rust traits = Simics interfaces)
- Attribute-backed connections with auto-resolve
- Port objects for multi-instance interfaces
- Connector with `hotplug` flag
- `new_instance` / `finalize` / `delete` lifecycle

### 1.3  higan

**Strengths:**
- Cooperative multi-clock scheduling via libco coroutines
- Per-device timestamp with scalar normalization for different clock domains
- Selective synchronization — only sync on shared-resource access
- `step(N)` + `resume(peer)` protocol — simple, composable
- Sequential device code (no state machines for complex processors)

**Weaknesses:**
- Coroutine stacks are hard to serialize for save/restore
- Performance penalty vs flat state machines for simple devices
- libco is C, not Rust-native

**Applicable to HELM:**
- Absolute scheduler with per-device timestamps (already in
  `DeviceScheduler`)
- Selective sync based on bus topology
- Optional async-fn or coroutine-based device execution for complex devices
- `step(N)` protocol for clock advancement

### 1.4  gem5-SALAM

**Strengths:**
- LLVM IR → CDFG → cycle-accurate execution with <1% timing error
- Three-queue scheduler (reservation/compute/memory) with functional unit
  contention modeling
- YAML-driven hardware profile — no recompilation to change FU counts/latencies
- AccCluster pattern: CommInterface + LLVMInterface + SPM + DMA as a unit
- Memory-mapped register interface for host↔accelerator control
- DMA for system↔scratchpad data movement
- Merged into gem5 mainline (2025)

**Weaknesses:**
- Requires specific LLVM IR structure (single inlined function)
- C++ codebase, tight coupling to gem5 SimObject model
- Static CDFG elaboration — no runtime code generation

**Applicable to HELM:**
- LLVM IR loading and CDFG construction (helm-llvm already does this)
- Hardware profile config (TOML/YAML) mapping opcodes → FU latencies
- Three-queue cycle-accurate scheduler
- AccCluster composition: CommInterface device + LLVMInterface engine + SPM
- DMA integration through DeviceBus transactions

---

## 2  Current Limitations

| Problem | Impact |
|---------|--------|
| No `detach` — `DeviceBus::attach_device` is one-way | Can't hot-plug/unplug devices |
| Linear device lookup — `for slot in &mut self.slots` | O(n) per MMIO access |
| Devices hold `Box<dyn CharBackend>` directly | Can't rewire backends at runtime |
| No interface resolution — devices call backend methods directly | Replacing a device requires rebuilding the entire platform |
| `DynamicDeviceLoader` has no `dlopen` — factory only, no shared-lib loading | Can't load external `.so` device models |
| `DeviceScheduler` is disconnected from `DeviceBus` | Scheduler and bus are separate hierarchies |
| No LLVM accelerator hot-load | Accelerator config is hardcoded at build time |
| `Platform` has no `remove_device()` | Platforms are static after construction |
| `MemRegionTree` has no change notification | JIT TLB cache doesn't know when regions change |
| `IrqRouter` has no `remove_route()` | IRQ wiring is permanent |

---

## 3  Proposed Design

### 3.1  Core Principles

1. **Interface-based communication** (Simics pattern) — devices never hold
   direct references to other devices.  They hold a `Connection<dyn Iface>`
   that resolves an interface trait object on demand.
2. **Two-phase lifecycle** (QEMU pattern) — `Default::default()` is infallible
   construction; `realize(&mut MachineCtx)` is failable bringup.
3. **Attach/detach at any time** — `DeviceBus::attach()` and `detach()` are
   the hot-plug primitives.  Address space is rebuilt transactionally.
4. **Loadable devices** — external `.so` files export a C-ABI entry point.
   Rust-native devices use the factory registry.  Both produce `Box<dyn Device>`.
5. **Multi-clock cooperative scheduling** (higan pattern) — devices with
   their own clocks participate in a timestamp-ordered scheduler.
6. **LLVM-IR accelerator loading** (gem5-SALAM pattern) — LLVM bitcode loaded
   at runtime, CDFG elaborated, driven by a three-queue cycle-accurate engine.

### 3.2  The Device Trait — Revised

```rust
/// Core device trait.  Every peripheral implements this.
///
/// Lifecycle: construct (infallible) → realize (failable) → run → unrealize → drop
pub trait Device: Send + Sync {
    /// Human-readable type name (e.g. "pl011", "virtio-blk").
    fn type_name(&self) -> &str;

    /// Human-readable instance name (e.g. "uart0").
    fn instance_name(&self) -> &str;

    // ── Lifecycle ────────────────────────────────────────────────

    /// Failable initialization.  Called after all properties are set.
    /// Register MMIO regions, resolve interface connections, wire IRQs.
    fn realize(&mut self, ctx: &mut DeviceCtx) -> HelmResult<()> { Ok(()) }

    /// Tear down.  Called before detach.  Deregister regions, drop connections.
    /// Must be idempotent (may be called multiple times).
    fn unrealize(&mut self, ctx: &mut DeviceCtx) -> HelmResult<()> { Ok(()) }

    /// Can this device be detached at runtime?
    fn is_hotpluggable(&self) -> bool { false }

    /// Reset to power-on state.
    fn reset(&mut self) -> HelmResult<()> { Ok(()) }

    // ── MMIO ─────────────────────────────────────────────────────

    /// MMIO regions this device occupies.  Returned during realize().
    fn regions(&self) -> &[MemRegion] { &[] }

    /// Handle a bus transaction (timed path).
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()>;

    /// Fast-path read (FE mode).  Default delegates to transact().
    fn read_fast(&mut self, offset: Addr, size: usize) -> HelmResult<u64> {
        let mut txn = Transaction::read(offset, size);
        txn.offset = offset;
        self.transact(&mut txn)?;
        Ok(txn.data_u64())
    }

    /// Fast-path write (FE mode).  Default delegates to transact().
    fn write_fast(&mut self, offset: Addr, size: usize, val: u64) -> HelmResult<()> {
        let mut txn = Transaction::write(offset, size, val);
        txn.offset = offset;
        self.transact(&mut txn)?;
        Ok(())
    }

    // ── Time ─────────────────────────────────────────────────────

    /// Periodic tick for time-driven devices.
    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        Ok(vec![])
    }

    /// Clock frequency in Hz (0 = passive, only responds to transactions).
    fn clock_hz(&self) -> u64 { 0 }

    // ── Serialization ────────────────────────────────────────────

    fn checkpoint(&self) -> HelmResult<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    fn restore(&mut self, state: &serde_json::Value) -> HelmResult<()> {
        Ok(())
    }
}
```

### 3.3  DeviceCtx — The Realize/Unrealize Context

Passed to `realize()` and `unrealize()`.  Provides access to bus, IRQ, and
connection APIs.  Devices cannot call these outside of realize/unrealize.

```rust
/// Context available during device realize/unrealize.
///
/// This is the only way to register MMIO regions, wire IRQs, and
/// resolve interface connections.  Holding this borrow prevents
/// concurrent device mutation.
pub struct DeviceCtx<'a> {
    bus: &'a mut AddressMap,
    irq: &'a mut IrqRouter,
    connections: &'a mut ConnectionRegistry,
    device_id: DeviceId,
}

impl<'a> DeviceCtx<'a> {
    /// Map an MMIO region into the address space.
    pub fn map_region(&mut self, region: &MemRegion) -> HelmResult<RegionHandle> { ... }

    /// Unmap a previously mapped region.
    pub fn unmap_region(&mut self, handle: RegionHandle) -> HelmResult<()> { ... }

    /// Connect an output IRQ line to a destination.
    pub fn connect_irq(&mut self, line: u32, dest_irq: u32) -> HelmResult<IrqHandle> { ... }

    /// Disconnect an IRQ.
    pub fn disconnect_irq(&mut self, handle: IrqHandle) -> HelmResult<()> { ... }

    /// Resolve an interface connection by name.
    /// Returns None if the target is not connected.
    pub fn resolve<I: DeviceInterface + ?Sized>(
        &self, name: &str,
    ) -> Option<Arc<Mutex<dyn I>>> { ... }

    /// This device's unique ID.
    pub fn device_id(&self) -> DeviceId { self.device_id }
}
```

### 3.4  Interface-Based Communication (Simics Pattern in Rust)

The key insight: Rust traits ARE Simics interfaces.  A `CharBackend` trait is
exactly a Simics `serial_interface_t`.  The difference is how connections are
managed.

#### DeviceInterface marker trait

```rust
/// Marker trait for interfaces that can be connected between devices.
/// Every connectable interface must implement this.
pub trait DeviceInterface: Send + Sync + 'static {
    /// Interface type name (e.g. "serial", "block", "net").
    fn interface_name() -> &'static str;
}

// Existing backends become DeviceInterfaces:
impl DeviceInterface for dyn CharBackend {
    fn interface_name() -> &'static str { "serial" }
}
impl DeviceInterface for dyn BlockBackend {
    fn interface_name() -> &'static str { "block" }
}
impl DeviceInterface for dyn NetBackend {
    fn interface_name() -> &'static str { "net" }
}
```

#### Connection<I> — The Smart Slot

Replaces `Box<dyn CharBackend>` inside devices.  A `Connection` is an
attribute-backed slot that can be wired/rewired at runtime.

```rust
/// A connectable slot that holds a resolved interface reference.
///
/// Simics equivalent: a `connect` statement in DML.
/// Rust equivalent: an `Option<Arc<Mutex<dyn I>>>` with change notification.
pub struct Connection<I: DeviceInterface + ?Sized> {
    /// Display name (e.g. "serial-out", "block-backend").
    name: String,
    /// Resolved interface.  None = not connected.
    target: Option<Arc<Mutex<dyn I>>>,
    /// Is runtime reconnection allowed?
    hotplug: bool,
}

impl<I: DeviceInterface + ?Sized> Connection<I> {
    pub fn new(name: &str, hotplug: bool) -> Self {
        Self { name: name.to_string(), target: None, hotplug }
    }

    /// Is something connected?
    pub fn is_connected(&self) -> bool { self.target.is_some() }

    /// Get the connected interface.  Panics if not connected.
    pub fn get(&self) -> &Arc<Mutex<dyn I>> {
        self.target.as_ref().expect("connection not wired")
    }

    /// Try to get the connected interface.
    pub fn try_get(&self) -> Option<&Arc<Mutex<dyn I>>> {
        self.target.as_ref()
    }

    /// Wire a target.  Called by DeviceCtx during realize or hot-plug.
    pub fn connect(&mut self, target: Arc<Mutex<dyn I>>) -> HelmResult<()> {
        if self.target.is_some() && !self.hotplug {
            return Err(HelmError::Config("connection is not hot-pluggable".into()));
        }
        self.target = Some(target);
        Ok(())
    }

    /// Disconnect.  Called during unrealize or hot-unplug.
    pub fn disconnect(&mut self) -> HelmResult<()> {
        self.target = None;
        Ok(())
    }
}
```

#### Usage in a Device

```rust
struct Pl011 {
    regs: Pl011Regs,
    serial_out: Connection<dyn CharBackend>,  // replaces Box<dyn CharBackend>
    irq_handle: Option<IrqHandle>,
    region_handle: Option<RegionHandle>,
}

impl Pl011 {
    fn new(name: &str) -> Self {
        Self {
            regs: Pl011Regs::default(),
            serial_out: Connection::new("serial", true),  // hot-pluggable
            irq_handle: None,
            region_handle: None,
        }
    }
}

impl Device for Pl011 {
    fn realize(&mut self, ctx: &mut DeviceCtx) -> HelmResult<()> {
        // Map MMIO
        self.region_handle = Some(ctx.map_region(&self.regions()[0])?);
        // Wire IRQ
        self.irq_handle = Some(ctx.connect_irq(0, 33)?);
        // Resolve serial backend (if connected)
        if let Some(backend) = ctx.resolve::<dyn CharBackend>("serial") {
            self.serial_out.connect(backend)?;
        }
        Ok(())
    }

    fn unrealize(&mut self, ctx: &mut DeviceCtx) -> HelmResult<()> {
        if let Some(h) = self.region_handle.take() { ctx.unmap_region(h)?; }
        if let Some(h) = self.irq_handle.take() { ctx.disconnect_irq(h)?; }
        self.serial_out.disconnect()?;
        Ok(())
    }

    fn is_hotpluggable(&self) -> bool { true }

    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write && txn.offset == 0x00 {
            // TX data register — write byte to serial backend
            if let Some(backend) = self.serial_out.try_get() {
                let mut b = backend.lock().unwrap();
                b.write(&[txn.data[0]])?;
            }
        }
        // ... register dispatch
        Ok(())
    }
    // ...
}
```

### 3.5  AddressMap — Transactional Address Space

Replaces the current `DeviceBus` linear scan with binary-search dispatch and
transactional mutations.

```rust
/// Manages the flat address map with transactional add/remove.
///
/// Inspired by QEMU's MemoryRegion + FlatView.
pub struct AddressMap {
    /// Sorted, non-overlapping flat entries.  Rebuilt on commit.
    flat_view: Vec<FlatEntry>,
    /// Pending mutations (batched, applied atomically on commit).
    pending: Vec<Mutation>,
    /// Listeners notified after each commit (JIT TLB, DTB generator, etc.).
    listeners: Vec<Box<dyn AddressMapListener>>,
    /// Device storage — indexed by DeviceId.
    devices: Vec<Option<DeviceSlot>>,
}

enum Mutation {
    AddRegion { device_id: DeviceId, region: MemRegion },
    RemoveRegion { handle: RegionHandle },
}

/// Listener for address space changes.
pub trait AddressMapListener: Send {
    fn on_region_add(&mut self, entry: &FlatEntry);
    fn on_region_remove(&mut self, entry: &FlatEntry);
}

impl AddressMap {
    /// Attach a device.  Does NOT map it yet — call realize() which calls
    /// ctx.map_region() to actually place it in the address space.
    pub fn attach(&mut self, device: Box<dyn Device>) -> DeviceId { ... }

    /// Detach a device.  Calls unrealize() first, which unmaps regions
    /// and disconnects IRQs.
    pub fn detach(&mut self, id: DeviceId) -> HelmResult<Box<dyn Device>> {
        let slot = self.devices[id as usize].as_mut()
            .ok_or(HelmError::Config("device not found".into()))?;
        // unrealize cleans up regions and IRQs via DeviceCtx
        slot.device.unrealize(&mut DeviceCtx { ... })?;
        let slot = self.devices[id as usize].take().unwrap();
        self.commit(); // rebuild flat view
        Ok(slot.device)
    }

    /// Commit pending mutations — rebuild flat view, notify listeners.
    pub fn commit(&mut self) { ... }

    /// O(log n) address lookup via binary search on flat_view.
    pub fn lookup(&self, addr: Addr) -> Option<(DeviceId, Addr)> { ... }

    /// Transaction dispatch — find device, adjust offset, call transact().
    pub fn dispatch(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        let (id, offset) = self.lookup(txn.addr)
            .ok_or(HelmError::Memory { addr: txn.addr, reason: "unmapped".into() })?;
        txn.offset = offset;
        self.devices[id as usize].as_mut().unwrap().device.transact(txn)
    }

    /// Fast-path read — O(log n) lookup + direct call.
    pub fn read_fast(&mut self, addr: Addr, size: usize) -> HelmResult<u64> {
        let (id, offset) = self.lookup(addr)
            .ok_or(HelmError::Memory { addr, reason: "unmapped".into() })?;
        self.devices[id as usize].as_mut().unwrap().device.read_fast(offset, size)
    }
}
```

### 3.6  IRQ Model — Typed Wires

Replace the current `IrqLine` value type with a wire-based model inspired
by QEMU's `qemu_irq`.

```rust
/// An IRQ output pin on a device.  Connected to an input on another device.
pub struct IrqWire {
    handler: Option<Arc<dyn IrqSink>>,
}

/// Trait for IRQ input receivers.
pub trait IrqSink: Send + Sync {
    fn set_level(&self, line: u32, level: bool);
}

impl IrqWire {
    pub fn new() -> Self { Self { handler: None } }

    pub fn connect(&mut self, sink: Arc<dyn IrqSink>) {
        self.handler = Some(sink);
    }

    pub fn disconnect(&mut self) {
        self.handler = None;
    }

    /// Assert/deassert the IRQ line.
    pub fn set_level(&self, line: u32, level: bool) {
        if let Some(h) = &self.handler {
            h.set_level(line, level);
        }
    }
}
```

`InterruptController` (GIC, PLIC, PIC) implements `IrqSink`.  Machine code
connects device output wires to controller inputs during realize.

### 3.7  Device Factory and Dynamic Loading

```rust
/// Registry of device types.  Supports both built-in and dynamically loaded devices.
pub struct DeviceRegistry {
    factories: HashMap<String, DeviceFactoryEntry>,
}

struct DeviceFactoryEntry {
    type_name: String,
    version: String,
    /// Properties this device type accepts (for introspection / CLI help).
    properties: Vec<PropertySpec>,
    create: Box<dyn Fn(&DeviceConfig) -> HelmResult<Box<dyn Device>> + Send + Sync>,
}

/// Device configuration — parsed from CLI, Python, or TOML.
pub struct DeviceConfig {
    pub type_name: String,
    pub instance_name: String,
    pub properties: HashMap<String, serde_json::Value>,
}

impl DeviceRegistry {
    /// Register a built-in device type.
    pub fn register<F>(&mut self, type_name: &str, props: Vec<PropertySpec>, factory: F)
    where F: Fn(&DeviceConfig) -> HelmResult<Box<dyn Device>> + Send + Sync + 'static
    { ... }

    /// Load a device from a shared library (.so / .dylib).
    ///
    /// The library must export: `extern "C" fn helm_device_entry() -> DeviceVTable`
    pub fn load_library(&mut self, path: &Path) -> HelmResult<()> { ... }

    /// Create a device instance.
    pub fn create(&self, config: &DeviceConfig) -> HelmResult<Box<dyn Device>> {
        let factory = self.factories.get(&config.type_name)
            .ok_or(HelmError::Config(format!("unknown device type: {}", config.type_name)))?;
        (factory.create)(config)
    }

    /// List all registered device types.
    pub fn list_types(&self) -> Vec<&str> { ... }

    /// List properties for a device type (for CLI help / introspection).
    pub fn list_properties(&self, type_name: &str) -> Option<&[PropertySpec]> { ... }
}

/// C-ABI entry point for loadable device libraries.
#[repr(C)]
pub struct DeviceVTable {
    pub api_version: u32,
    pub type_name: *const c_char,
    pub create: unsafe extern "C" fn(config_json: *const c_char) -> *mut c_void,
    pub destroy: unsafe extern "C" fn(ptr: *mut c_void),
}
```

### 3.8  Multi-Clock Cooperative Scheduler (higan-inspired)

Extends the existing `DeviceScheduler` with higan's absolute timestamp
model and selective synchronization.

```rust
/// Per-device clock state.
pub struct DeviceClock {
    /// Absolute timestamp in normalized tick space.
    pub timestamp: u64,
    /// Scalar multiplier: ticks_per_native_cycle in the common time base.
    pub scalar: u64,
    /// Native clock frequency in Hz.
    pub freq_hz: u64,
}

impl DeviceClock {
    /// Advance by N native cycles.
    pub fn step(&mut self, native_cycles: u64) {
        self.timestamp += native_cycles * self.scalar;
    }
}

/// Cooperative scheduler for multi-clock devices.
///
/// Devices that declare `clock_hz() > 0` are registered with the scheduler.
/// The scheduler picks the device with the smallest timestamp and ticks it.
pub struct CoopScheduler {
    entries: Vec<SchedulerEntry>,
    /// Normalization base — LCM of all frequencies or a sufficient scalar.
    base_scalar: u64,
}

struct SchedulerEntry {
    device_id: DeviceId,
    clock: DeviceClock,
}

impl CoopScheduler {
    /// Register a clocked device.
    pub fn register(&mut self, id: DeviceId, freq_hz: u64) { ... }

    /// Unregister (on detach).
    pub fn unregister(&mut self, id: DeviceId) { ... }

    /// Advance the earliest device by one tick.  Returns events.
    pub fn step(&mut self, map: &mut AddressMap) -> HelmResult<Vec<DeviceEvent>> {
        let entry = self.earliest_mut()?;
        let id = entry.device_id;
        entry.clock.step(1);
        let device = map.device_mut(id)?;
        device.tick(1)
    }

    /// Run until global time reaches target.
    pub fn run_until(
        &mut self, target_ns: u64, map: &mut AddressMap,
    ) -> HelmResult<Vec<DeviceEvent>> { ... }

    /// Prevent timestamp overflow: subtract min from all timestamps.
    pub fn renormalize(&mut self) { ... }
}
```

### 3.9  LLVM-IR Accelerator Loading (gem5-SALAM-inspired)

Builds on the existing `helm-llvm` crate.  The accelerator becomes a
loadable `Device` that can be hot-plugged into any bus.

#### AcceleratorConfig — TOML-driven hardware profile

```toml
[accelerator]
name = "matmul"
ir_file = "matmul.bc"       # LLVM bitcode
mmr_base = 0x10020000       # memory-mapped register base
mmr_size = 64
irq_num = 68

[scratchpad]
base = 0x2f100000
size = 98304                 # 96 KB
latency_ns = 2
ports = 4
ready_mode = true

[dma]
base = 0x10020040
irq_num = 95
max_req_size = 64
buffer_size = 4096

[functional_units]
int_adder    = { count = 4, latency = 1 }
int_mul      = { count = 2, latency = 3 }
fp_adder     = { count = 2, latency = 4 }
fp_mul       = { count = 2, latency = 5 }
fp_div       = { count = 1, latency = 15 }
memory       = { count = 4, latency = 2 }    # matches SPM ports
```

#### LlvmAccelerator Device

```rust
/// A loadable LLVM-IR accelerator device.
///
/// Implements `Device` — can be attached to any DeviceBus.
/// Internally runs a three-queue cycle-accurate CDFG engine
/// (gem5-SALAM architecture).
pub struct LlvmAccelerator {
    name: String,
    config: AcceleratorConfig,

    // ── CDFG engine ──────────────────────
    cdfg: Option<Cdfg>,                // built from LLVM IR at realize()
    reservation_queue: VecDeque<CdfgNode>,
    compute_queue: Vec<ComputeSlot>,
    memory_queue: Vec<MemorySlot>,
    fu_pool: FunctionalUnitPool,

    // ── System interface ─────────────────
    mmr: [u64; 8],                      // memory-mapped registers
    status: AccelStatus,                // Idle / Running / Done
    irq_wire: IrqWire,                  // completion interrupt
    irq_handle: Option<IrqHandle>,
    region_handle: Option<RegionHandle>,

    // ── Scratchpad ───────────────────────
    spm: Vec<u8>,
    spm_region: Option<RegionHandle>,

    // ── Connection to system bus ─────────
    system_bus: Connection<dyn MemoryAccess>,  // for DMA reads/writes
}

impl Device for LlvmAccelerator {
    fn type_name(&self) -> &str { "llvm-accelerator" }
    fn instance_name(&self) -> &str { &self.name }

    fn realize(&mut self, ctx: &mut DeviceCtx) -> HelmResult<()> {
        // 1. Parse LLVM IR bitcode → CDFG
        self.cdfg = Some(Cdfg::from_bitcode(&self.config.ir_file)?);
        // 2. Map MMR region
        self.region_handle = Some(ctx.map_region(&MemRegion {
            name: format!("{}-mmr", self.name),
            base: self.config.mmr_base,
            size: self.config.mmr_size,
            kind: RegionKind::Io,
            priority: 0,
        })?);
        // 3. Map scratchpad region
        self.spm_region = Some(ctx.map_region(&MemRegion {
            name: format!("{}-spm", self.name),
            base: self.config.spm_base,
            size: self.config.spm_size,
            kind: RegionKind::Ram { backing: ... },
            priority: 0,
        })?);
        // 4. Wire completion IRQ
        self.irq_handle = Some(ctx.connect_irq(0, self.config.irq_num)?);
        // 5. Build FU pool from config
        self.fu_pool = FunctionalUnitPool::from_config(&self.config.functional_units);
        Ok(())
    }

    fn unrealize(&mut self, ctx: &mut DeviceCtx) -> HelmResult<()> {
        // Tear down in reverse
        if let Some(h) = self.irq_handle.take() { ctx.disconnect_irq(h)?; }
        if let Some(h) = self.spm_region.take() { ctx.unmap_region(h)?; }
        if let Some(h) = self.region_handle.take() { ctx.unmap_region(h)?; }
        self.cdfg = None;
        Ok(())
    }

    fn is_hotpluggable(&self) -> bool { true }

    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        // MMR dispatch: start/status/args
        if !txn.is_write {
            txn.set_data_u64(self.mmr[txn.offset as usize / 8]);
        } else {
            let reg = txn.offset as usize / 8;
            self.mmr[reg] = txn.data_u64();
            if reg == 0 {
                // Start register written — kick off execution
                self.start_execution();
            }
        }
        Ok(())
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        if self.status != AccelStatus::Running {
            return Ok(vec![]);
        }
        // Drive the three-queue engine for `cycles` steps
        for _ in 0..cycles {
            self.advance_compute_queue();
            self.advance_memory_queue();
            self.dispatch_from_reservation_queue();
            if self.cdfg_complete() {
                self.status = AccelStatus::Done;
                return Ok(vec![DeviceEvent::Irq { line: 0, assert: true }]);
            }
        }
        Ok(vec![])
    }

    fn clock_hz(&self) -> u64 { 200_000_000 }  // 200 MHz default
}
```

### 3.10  Platform Builder — Updated

```rust
impl Platform {
    /// Attach a device, realize it, and register with the scheduler.
    pub fn add_device(
        &mut self,
        config: DeviceConfig,
        registry: &DeviceRegistry,
    ) -> HelmResult<DeviceId> {
        let mut device = registry.create(&config)?;
        let id = self.address_map.attach(device);
        let mut ctx = DeviceCtx::new(&mut self.address_map, &mut self.irq_router, id);
        self.address_map.device_mut(id)?.realize(&mut ctx)?;
        // Register with scheduler if clocked
        let hz = self.address_map.device(id)?.clock_hz();
        if hz > 0 {
            self.scheduler.register(id, hz);
        }
        self.address_map.commit();
        Ok(id)
    }

    /// Detach a device — unrealize, unregister from scheduler, remove.
    pub fn remove_device(&mut self, id: DeviceId) -> HelmResult<Box<dyn Device>> {
        self.scheduler.unregister(id);
        self.address_map.detach(id)
    }

    /// Hot-swap a device backend (e.g., reconnect a UART to a different serial backend).
    pub fn rewire<I: DeviceInterface + ?Sized>(
        &mut self,
        device_id: DeviceId,
        connection_name: &str,
        new_target: Arc<Mutex<dyn I>>,
    ) -> HelmResult<()> { ... }
}
```

---

## 4  Migration from Current helm-device

| Current | New | Change |
|---------|-----|--------|
| `DeviceBus` (linear scan) | `AddressMap` (binary search + FlatView) | Replace |
| `DeviceBus::attach_device()` | `AddressMap::attach()` + `realize()` | Split into two phases |
| No `detach` | `AddressMap::detach()` calls `unrealize()` | Add |
| `Box<dyn CharBackend>` in device struct | `Connection<dyn CharBackend>` | Replace |
| `Platform::add_device()` hardcoded | `Platform::add_device(config, registry)` | Generalize |
| `DynamicDeviceLoader` (factory only) | `DeviceRegistry` (factory + dlopen) | Extend |
| `IrqLine` value type | `IrqWire` with `IrqSink` handler | Replace |
| `IrqRouter` (no remove) | `IrqRouter` with `remove_route()` | Extend |
| `MemRegionTree` (no listeners) | `AddressMap` with `AddressMapListener` | Replace |
| `DeviceScheduler` (disconnected) | `CoopScheduler` integrated with `AddressMap` | Replace |
| `helm-llvm::Accelerator` (standalone) | `LlvmAccelerator: Device` (attachable) | Adapt |

### Backward Compatibility

- `LegacyWrapper` continues to adapt old `MemoryMappedDevice` impls to `Device`
- `DeviceBus` can be kept temporarily as a thin wrapper around `AddressMap`
- Existing ARM devices (pl011, sp804, gic, etc.) gain `realize`/`unrealize`
  incrementally — default impls in `Device` keep them working unchanged

---

## 5  Testing

| Test | What it verifies | Location |
|------|-----------------|----------|
| Attach/detach lifecycle | Device realized on attach, unrealized on detach, regions cleaned up | `device/src/tests/lifecycle.rs` |
| Hot-plug mid-simulation | Attach UART, run 1M insns, detach, run 1M more — no crash | Same |
| AddressMap binary search | O(log n) lookup correctness for 1, 10, 100, 1000 devices | `device/src/tests/address_map.rs` |
| AddressMap transaction | Batch add 3 regions, commit once, verify FlatView | Same |
| AddressMap listener | JIT TLB listener receives on_region_add/remove callbacks | Same |
| Connection hot-swap | UART connected to BufferCharBackend, rewired to StdioCharBackend mid-run | `device/src/tests/connection.rs` |
| Connection disconnected read | Device reads from disconnected connection → graceful None | Same |
| IrqWire connect/disconnect | Wire → assert → verify sink called; disconnect → assert → no call | `device/src/tests/irq_wire.rs` |
| DeviceRegistry builtin | Register pl011, create via config, verify type_name | `device/src/tests/registry.rs` |
| DeviceRegistry introspect | List properties for a type, verify matches expected | Same |
| CoopScheduler multi-clock | 2 devices at different frequencies, verify timestamp ordering | `device/src/tests/scheduler.rs` |
| CoopScheduler detach | Remove a clocked device mid-run, verify scheduler continues | Same |
| LlvmAccelerator load | Load matmul.bc, realize, write start MMR, tick until done | `llvm/src/tests/accelerator_device.rs` |
| LlvmAccelerator hot-plug | Attach accelerator, run, detach, verify clean teardown | Same |
| LlvmAccelerator FU contention | 1 FP multiplier, 4 concurrent fmul — verify stalls | Same |
| Platform add_device/remove_device | Full platform lifecycle with 5 devices | `device/src/tests/platform.rs` |
| Checkpoint/restore with hot-plug | Attach 2 devices, checkpoint, detach 1, restore, verify both present | `device/src/tests/checkpoint.rs` |

---

## 6  Comparison Summary

| Feature | QEMU | Simics | higan | gem5-SALAM | HELM (proposed) |
|---------|------|--------|-------|------------|-----------------|
| Type registry | QOM hash table | `SIM_register_class` | N/A | gem5 SimObject | `DeviceRegistry` |
| Device lifecycle | init → realize → unrealize | new → finalize → delete | N/A | SimObject init | construct → realize → unrealize |
| Inter-device comms | Direct fn ptrs | Interface vtables | Direct calls | gem5 ports | `Connection<dyn Trait>` |
| Hot-plug | 3-phase (request/ack/unplug) | `hotplug` connector flag | N/A | N/A | `is_hotpluggable()` + `unrealize()` |
| Address space | MemoryRegion tree + FlatView | `map` attribute list | N/A | gem5 AddrRange | `AddressMap` + FlatView + listeners |
| IRQ model | `qemu_irq` typed wire | `signal` interface | Direct calls | gem5 IntSink | `IrqWire` + `IrqSink` trait |
| Multi-clock | N/A (single-threaded) | N/A (event-driven) | Cooperative threads | gem5 events | `CoopScheduler` (higan-style) |
| Accelerator loading | N/A | N/A | N/A | LLVM IR → CDFG | `LlvmAccelerator: Device` |
| Checkpoint | VMState descriptors | Attribute get/set | Coroutine serialization | gem5 serialize | `serde` + `checkpoint()`/`restore()` |
| Loadable devices | `-device` / `device_add` | `SIM_create_object` | N/A | Python config | `DeviceRegistry` + `dlopen` |
