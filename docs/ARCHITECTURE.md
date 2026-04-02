# helm-ng Architecture

> Next-generation simulator: Rust core, Python config, multi-ISA, multi-mode, multi-timing.
>
> **Implementation status (2026-03):** AArch64 SE+FS pipeline working end-to-end (GICv2+GICv3, PL011,
> SP804, ELF/kernel Image loader, ~150 syscalls, SMP scheduling, PyO3 bindings, `helm-aarch64` CLI).
> RISC-V RV64GC+Zicsr decode/execute implemented with `LinuxRiscv64SyscallHandler` and `helm-riscv64`
> binary. Pluggable JIT framework with dynasm-rs backend. Typed engine event dispatch. IOMMU models
> (SMMUv3, AMD-Vi, RISC-V). VirtIO device backends (block, console, net, rng).

---

## Design Inspirations (What We Take and What We Reject)

| Simulator | Take | Reject |
|-----------|------|--------|
| **Gem5** | Python-driven config layer, typed param system, SimObject composability, port-based memory interface, three access modes (atomic/timing/functional) | Two incompatible memory subsystems, single-threaded event loop, monolithic Classic cache coherence, no stable API contract |
| **SIMICS** | Attribute/interface separation (state exposure ≠ runtime communication), HAP-style observable event system, first-class checkpointing, determinism by design | DML complexity, commercial-grade scope for a solo project |
| **QEMU** | Block-chaining insight (chain translated blocks for near-native speed), MemoryRegion tree (unified RAM/MMIO/alias model), QOM realize/unrealize lifecycle | BQL as a global lock (model at trait level instead), TCG complexity pre-JIT phase |
| **Higen** | Multi-mode accuracy thinking — not one execution fidelity, but selectable per experiment | Not publicly documented enough to borrow more directly |

---

## The 4-Item Irreducible Core

First principles analysis shows every simulator decomposes to exactly four irreducible abstractions. **Everything else is layered on top.**

```
1. ArchState    — typed register file + PC (all architecturally-visible state)
2. Decoder      — bytes → Instruction  (ISA-specific)
3. Executor     — (ArchState, Instruction, MemInterface) → (ArchState, EffectList)
4. MemInterface — read(addr, size) → bytes  |  write(addr, size, bytes)
```

**Key insight from first principles:** Caches, event queues, timing models, OS interfaces, device models, and config layers are NOT part of the core. They compose on top. Build and validate the 4-item core first.

---

## Execution Modes

Three orthogonal execution modes, cleanly separated from the timing model:

```
FE  — Functional Emulation
      Execute instructions correctly, no timing, no OS interface.
      Used for: ISA validation, fast-forward front-end, correctness testing.
      Speed: 100M–1B instructions/sec.

SE  — Syscall Emulation
      FE + syscall interception. Syscall instruction → dispatch to host OS handler.
      Used for: userspace binaries without booting a kernel.
      Constraint: limited syscall coverage, no scheduling/page-fault modeling.

FS  — Full System (future phase)
      Complete hardware platform simulation. Boot a real kernel.
      Used for: OS research, driver development, full-stack accuracy.
      Requires: interrupt controller, timer, storage device models.
```

**Implementation rule:** The CPU model (HelmEngine) is mode-agnostic. It calls `system.handle_syscall(tc)` when it encounters a syscall instruction. A `SyscallHandler` implementor (SE: dispatches to host; FS: routes to simulated OS interrupt) decides what to do. The kernel never knows which.

---

## Timing Models

Three selectable timing models, composable with any execution mode:

```
Virtual   — Global virtual clock, discrete event queue (BinaryHeap).
                  Every latency = a scheduled future event.
                  No real-time relationship. Fastest for long simulations.
                  Speed: 1–10M simulated instructions/sec (with O3-level detail).

Interval   — Sniper-style interval simulation.
                  Execute intervals functionally, apply timing at miss events
                  (cache misses, branch mispredicts, TLB misses).
                  ~5% IPC error vs cycle-accurate. ~10x faster.
                  Speed: 10–100M instructions/sec.
                  `timing="interval"` / `--timing interval` selects
                  the default two-level L1D/L2 estimator; explicit
                  `interval:...`, `--l1d-*`, and `--l2-*` overrides
                  expose the hierarchy.

Accurate   — Cycle-accurate pipeline simulation.
                  Every pipeline stage per cycle. Maximum fidelity.
                  Used for: microarchitecture research, RTL correlation.
                  Speed: 0.1–1M instructions/sec.
```

---

## Core Rust Architecture

### The Foundational Design Rule

> Monomorphize only timing (proven hot path). Dispatch ISA, mode, and the PyO3 boundary via enum — one match per Python call, zero overhead per simulated instruction.

### Type Hierarchy

```rust
// ── Timing Models: lightweight structs monomorphized into HelmEngine ──
// Generic parameter T is monomorphized into HelmEngine — no vtable.

pub struct VirtualTiming { /* private */ }
pub struct IntervalTiming { /* private */ }
pub struct AccurateTiming;

pub trait TimingModel: Send + 'static {
    #[inline(always)]
    fn on_memory_access(&mut self, addr: u64, cycles: u64);
    #[inline(always)]
    fn on_branch_mispredict(&mut self, penalty_cycles: u64);
}

// ── ISA: enum inside the kernel (decode once per instruction) ─────
#[derive(Debug, Clone, Copy)]
pub enum Isa { RiscV, AArch64, AArch32 }

// ── Execution Mode: cold path (set once at config time) ───────────
#[derive(Debug, Clone, Copy)]
pub enum ExecMode {
    Functional,  // FE — pure interpretation, no OS; fastest correctness check
    Syscall,     // SE — intercept syscalls, dispatch to host OS (user-space binaries)
    System,      // FS — boot real kernel, full privilege model (Phase 3)
    Hardware,    // HAE — KVM/HVMX hardware-assisted; CPU on host HW, devices in Rust
}

// ── The Kernel: generic over timing only ─────────────────────────
pub struct HelmEngine<T: TimingModel> {
    pub isa:     Isa,
    pub mode:    ExecMode,
    pub timing:  T,
    pub arch:    ArchState,
    pub memory:  MemoryMap,
}

impl<T: TimingModel> HelmEngine<T> {
    /// Hot loop — monomorphized, T::on_memory_access inlined, no vtable.
    pub fn run(&mut self, n_insns: u64) {
        for _ in 0..n_insns {
            match self.isa {
                Isa::RiscV   => self.step_riscv(),
                Isa::AArch64 => self.step_aarch64(),
                Isa::AArch32 => self.step_aarch32(),
            }
        }
    }
}

// ── PyO3 Boundary: thin enum wrapping concrete types ─────────────
// One enum dispatch per Python call. Exhaustiveness compiler-checked.
pub enum HelmSim {
    VirtualTiming(HelmEngine<VirtualTiming>),
    IntervalTiming(HelmEngine<IntervalTiming>),
    AccurateTiming(HelmEngine<AccurateTiming>),
    Hardware(HardwareEngine), // HAE — KVM/HVMX, no TimingModel
}

impl HelmSim {
    pub fn run(&mut self, n_insns: u64) {
        match self {
            Self::VirtualTiming(k) => k.run(n_insns),
            Self::IntervalTiming(k) => k.run(n_insns),
            Self::AccurateTiming(k) => k.run(n_insns),
        }
    }
}

// ── Factory: called from PyO3 #[pyfunction] ───────────────────────
pub enum TimingChoice {
    VirtualTiming { ipc: f64 },
    IntervalTiming {
        ipc: f64,
        interval_len: u64,
        mem_model: TimingMemModelConfig,
    },
    AccurateTiming,
}

pub fn build_simulator(
    isa: Isa,
    mode: ExecMode,
    timing: TimingChoice,
    mem_base: u64,
    mem_size: usize,
) -> HelmSim {
    match timing {
        TimingChoice::VirtualTiming { ipc } => HelmSim::VirtualTiming(
            HelmEngine::new(isa, mode, VirtualTiming::new(ipc), mem_base, mem_size)
        ),
        TimingChoice::IntervalTiming {
            ipc,
            interval_len,
            mem_model,
        } => HelmSim::IntervalTiming(
            HelmEngine::new(
                isa,
                mode,
                IntervalTiming::new(ipc, interval_len),
                mem_base,
                mem_size,
            )
            .with_timing_mem_model_config(mem_model)
        ),
        TimingChoice::AccurateTiming => HelmSim::AccurateTiming(
            HelmEngine::new(isa, mode, AccurateTiming::default(), mem_base, mem_size)
        ),
    }
}
```

---

## Memory System

### Unified MemoryRegion Model (inspired by QEMU)

All memory — RAM, MMIO, ROM, aliases — uses one unified type:

```rust
pub enum MemoryRegion {
    Ram   { data: Vec<u8> },
    Mmio  { handler: Box<dyn MmioHandler> },
    Alias { target: Arc<MemoryRegion>, offset: u64, size: u64 },
    Container { subregions: Vec<(u64, MemoryRegion)> },
}

pub trait MmioHandler: Send + Sync {
    fn read(&self, offset: u64, size: usize) -> u64;
    fn write(&mut self, offset: u64, size: usize, value: u64);
}
```

The `MemoryMap` maintains a sorted `FlatView` of non-overlapping ranges. A lookup resolves an address to a `MemoryRegion` in O(log n). MMIO handlers use `Box<dyn MmioHandler>` — justified because MMIO is a cold path (device I/O, not per-instruction).

### Three Access Modes (from Gem5's port model)

```
Timing    — async, event-driven, flow-controlled (timing simulation)
Atomic    — synchronous, returns estimated latency (fast-forward)
Functional — instantaneous, state inspection (debugger, binary load)
```

Constraint: Timing and Atomic cannot coexist on the same memory system simultaneously. Enforce as a runtime invariant — drain in-flight timing transactions before switching.

---

## Component Model

### SimObject Equivalent (Rust)

```rust
/// Every hardware component implements this trait.
pub trait SimObject: Send {
    fn name(&self) -> &str;
    fn init(&mut self);
    fn elaborate(&mut self, system: &mut System);
    fn startup(&mut self);
    fn reset(&mut self);
    fn checkpoint_save(&self) -> Vec<u8>;
    fn checkpoint_restore(&mut self, data: &[u8]);
}
```

### Lifecycle

```
init()       — allocate component, set defaults
elaborate()  — connect ports, resolve cross-references
startup()    — schedule first events, warm caches
run()        — simulation loop
reset()      — return to post-elaborate state (for repeat runs)
checkpoint_save/restore — checkpoint protocol
```

### Hierarchy and Naming

Components are addressed by path: `system.cpu0.icache`, `system.membus`. The `System` struct maintains a tree. Paths are resolved at elaborate time; no dynamic lookup in the hot path.

---

## Python Configuration Layer (Gem5-inspired)

### Two-Phase Model

```
Phase 1 — Config (Python):
  Import helm Python package (backed by PyO3-generated bindings)
  Instantiate components as Python objects with typed params
  Wire connections declaratively
  Call system.instantiate()

Phase 2 — Simulation (Rust):
  Python config is complete; Rust takes over
  system.run(n_instructions) — pure Rust hot loop
  Python can call back in for: checkpoint, stats dump, ROI markers
```

### Typed Parameter System

```python
# Python-side configuration
class System(SimObject):
    timing: str = "virtual"
    mode: str = "se"
    ipc: float = 4.0

class Cpu(SimObject):
    isa: str = "aarch64"
    model: str = "cortex-a55"
    width: int = 4

# Usage
system = System(
    "virt",
    timing="interval:interval_len=256,l1d_size=64KiB,l2_size=1MiB",
    mode="se",
)
system.cpu = Cpu("cpu0", isa="aarch64", model="cortex-a55")
system.instantiate()
system.run(1_000_000_000)
```

### PyO3 Binding Strategy

- `helm_ng` Rust crate exports a `#[pymodule]`
- Each `SimObject` trait implementor can be wrapped with `#[pyclass]`
- The `HelmSim` factory is a `#[pyfunction]`
- Typed params validated at Python boundary via PyO3 `FromPyObject`
- No pybind11, no SWIG — PyO3 is idiomatic Rust

---

## Debuggability System

Debuggability is a first-class architectural concern, not a retrofit. Four pillars:

### 1. Checkpoint / Restore

Every `SimObject` must implement `checkpoint_save() -> Vec<u8>` and `checkpoint_restore(&mut self, data: &[u8])`. The `System` serializes the full tree. Checkpoints are:
- Differential (only changed state since last checkpoint)
- Architecture-state only (not simulator-internal performance counters)
- Compatible across runs with same ISA/mode configuration

Use cases: fast-forward to ROI entry (save at boot, restore for repeated experiments), fault injection, reverse execution.

### 2. GDB Remote Serial Protocol Stub

Implement the GDB RSP server in the simulator. The CPU exposes:
```rust
pub trait GdbTarget {
    fn read_register(&self, reg: GdbReg) -> u64;
    fn write_register(&mut self, reg: GdbReg, val: u64);
    fn read_memory(&self, addr: u64, len: usize) -> Vec<u8>;
    fn write_memory(&mut self, addr: u64, data: &[u8]);
    fn step(&mut self) -> StopReason;
    fn r#continue(&mut self) -> StopReason;
}
```
Exposes: GDB `target remote :1234` connects, full `break`/`watch`/`step`/`backtrace` works. LLDB also compatible via RSP.

### 3. Event Trace Logging

Structured trace output in a defined format (inspired by SIMICS HAPs):

```rust
#[derive(serde::Serialize)]
pub enum TraceEvent {
    InsnFetch  { pc: u64, bytes: u32 },
    MemRead    { addr: u64, size: u8, value: u64, cycle: u64 },
    MemWrite   { addr: u64, size: u8, value: u64, cycle: u64 },
    Exception  { vector: u32, pc: u64 },
    Syscall    { nr: u64, args: [u64; 6] },
    BranchMiss { pc: u64, target: u64, penalty: u32 },
}
```

Traces write to a ring buffer (zero allocation on hot path), flushed to file periodically. Python can subscribe to trace events via callback.

### 4. Statistics Registration

Inspired by Gem5's Stats system:

```rust
pub struct PerfCounter {
    pub name: String,
    pub desc: String,
    value: AtomicU64,
}

impl PerfCounter {
    pub fn inc(&self) { self.value.fetch_add(1, Ordering::Relaxed); }
    pub fn get(&self) -> u64 { self.value.load(Ordering::Relaxed); }
}

// Registration at SimObject elaborate() time:
// system.stats.register("cpu0.icache.hits", &self.hit_counter);
```

Dump to JSON/CSV at end of simulation or on demand from Python.

---

## Multi-ISA Architecture

### The Two Critical Interfaces

```
Interface 1: CPU → ISA  (two traits, separated by path temperature)

  ExecContext  — HOT PATH — used inside ISA execute() per instruction
    read_int_reg / write_int_reg
    read_float_reg / write_float_reg
    read_csr / write_csr
    read_pc / write_pc
    read_mem / write_mem
    raise_exception
    Dispatch: static (concrete type passed to execute()) — zero overhead

  ThreadContext — COLD PATH — used by GDB stub, SyscallHandler, Python
    All ExecContext methods +
    get_hart_id / get_isa / get_exec_mode
    read_all_regs / write_all_regs (for checkpoint/GDB register dump)
    pause / resume
    Dispatch: &mut dyn ThreadContext — dynamic, fine on cold paths

Interface 2: CPU → Memory (MemInterface)
  Three access modes: timing, atomic, functional.
  All CPU models use the same interface.
  Memory system is CPU-model-agnostic.
```

### Hart — The Hardware Thread Abstraction

`Hart` is helm-ng's ISA-neutral term for a hardware execution thread.
Equivalent to: RISC-V "hart", ARM "PE" (Processing Element), SIMICS "processor object".

```rust
pub trait Hart: SimObject {
    fn step(&mut self, mem: &mut dyn MemInterface) -> Result<(), HartException>;
    fn get_pc(&self) -> u64;
    fn get_int_reg(&self, idx: usize) -> u64;
    fn set_int_reg(&mut self, idx: usize, val: u64);
    fn isa(&self) -> Isa;
    fn exec_mode(&self) -> ExecMode;
    fn thread_context(&mut self) -> &mut dyn ThreadContext;
}
```

In a multi-core simulation, `System` owns N `Hart` instances. Each Hart drives one `HelmEngine<T>` internally.

### AArch32 + AArch64 in One ISA Object

Following Gem5's ARM implementation:
- One `ArmState` struct with unified system register array (covering both sub-architectures)
- Mode-dependent register banking via `flatten_reg_idx(idx, current_mode) → physical_idx`
- `PCState` carries current execution sub-architecture (Thumb bit, ITSTATE for predication)
- Exception level transitions (EL0–EL3) tracked via PSTATE
- Start with AArch64 only; add AArch32 later (it's a separate decode tree)

### ISA Priority Order for Development

```
1. RISC-V RV64GC    — simpler encoding, reference implementations abundant, validate core first
2. ARM AArch64       — add once RISC-V is solid, use deku crate for complex encoding
3. ARM AArch32/Thumb — defer; adds register banking complexity
```

---

## Dynamic Device Loading

Devices can be shipped as external `.so` plugins, each bundling:
1. A Rust `SimObject + MmioHandler` implementation (compiled to cdylib)
2. An embedded Python class definition (for the config layer param schema)

### Device Model: Offset-Only, IRQ-Capable

**A device has no knowledge of its base address.** The address mapping is owned by the `MemoryMap` / Python config. The device only sees byte offsets within its mapped region. This matches how real hardware works — the device is location-agnostic; the system integrator places it.

```rust
/// Core device interface — receives reads/writes at offsets, raises IRQs.
pub trait Device: SimObject {
    /// Called when a read hits this device's mapped region.
    fn read(&self, offset: u64, size: usize) -> u64;

    /// Called when a write hits this device's mapped region.
    fn write(&mut self, offset: u64, size: usize, val: u64);

    /// Device region size in bytes (used by MemoryMap for bounds checking).
    fn region_size(&self) -> u64;

    /// Receive a named signal (reset, clock-enable, DMA-ack, etc.)
    fn signal(&mut self, name: &str, val: u64);
}
```

**Devices raise interrupts via an output pin — they have no knowledge of IRQ numbers, controllers, or routing.** The device just asserts or deasserts its interrupt output. How that signal is routed — to which controller, on which line — is a platform/SoC concern defined in Python config.

```rust
/// A device's interrupt output pin. The device holds this; it knows nothing
/// about where the signal goes. Routing is wired by the platform at elaborate().
pub struct InterruptPin {
    wire: Option<Arc<InterruptWire>>,
    state: AtomicBool,
}

impl InterruptPin {
    pub fn assert(&self);    // raise interrupt — propagates to whatever is wired
    pub fn deassert(&self);  // lower interrupt
    pub fn is_asserted(&self) -> bool;
}

/// The other end of a wire — receives the signal level change.
/// Implemented by interrupt controllers (PLIC, GIC, PIC).
pub trait InterruptSink: Send + Sync {
    fn on_assert(&self, wire_id: WireId);
    fn on_deassert(&self, wire_id: WireId);
}
```

A typical device declares its interrupt output as a field:

```rust
pub struct Uart16550 {
    clock_hz: u32,
    pub irq_out: InterruptPin,  // device owns the pin, knows nothing else
    // ...
}

impl Device for Uart16550 {
    fn write(&mut self, offset: u64, size: usize, val: u64) {
        // ... handle register write ...
        if self.rx_fifo_full() {
            self.irq_out.assert();   // raise — don't know or care where it goes
        }
    }
}
```

**Wiring is a platform/SoC definition — done in Python config, not in the device:**

```python
uart = helm_ng.Uart16550(clock_hz=1_843_200)  # device params only
plic = helm_ng.Plic(num_sources=64)

system.map_device(uart, base=0x09000000)

# Platform wires UART's interrupt pin to PLIC input 33
# UART doesn't know about PLIC or the number 33
system.wire_interrupt(uart.irq_out, plic.input(33))
```

This mirrors a real SoC: the UART IP block has an `irq` output port; the chip designer connects it to the interrupt controller input in the netlist. The UART RTL has no `#define IRQ_NUM 33`.

**For a full platform (e.g. a RISC-V board), interrupt routing is defined once in a platform file:**

```python
# platforms/virt_riscv.py
def build(system):
    plic  = helm_ng.Plic(num_sources=64, num_contexts=2)
    clint = helm_ng.Clint()
    uart  = helm_ng.Uart16550(clock_hz=1_843_200)
    disk  = helm_ng.VirtIoDisk(image="disk.img")

    system.map_device(plic,  base=0x0c000000)
    system.map_device(clint, base=0x02000000)
    system.map_device(uart,  base=0x10000000)
    system.map_device(disk,  base=0x10001000)

    system.wire_interrupt(uart.irq_out,  plic.input(10))
    system.wire_interrupt(disk.irq_out,  plic.input(8))
    system.wire_interrupt(plic.irq_out,  cpu.external_irq)
    system.wire_interrupt(clint.sw_out,  cpu.software_irq)
    system.wire_interrupt(clint.tim_out, cpu.timer_irq)
```

### helm-devices Structure

```
helm-devices/
└── src/
    ├── lib.rs               # re-exports, DeviceRegistry, .so loader
    ├── device.rs            # Device trait, IrqLine, IrqController trait
    ├── params.rs            # ParamSchema, ParamField, ParamType, DeviceParams
    ├── registry.rs          # DeviceRegistry, PluginError, DeviceDescriptor
    └── bus/
        ├── mod.rs
        ├── pci/             # PCI/PCIe bus infrastructure (config space, BARs, MSI)
        └── amba/            # AMBA/AHB/APB bus (ARM system buses)
```

### Device Parameters

Devices declare only their own internal config — not address or IRQ (those are system-level):

```rust
pub struct ParamSchema { fields: Vec<ParamField> }
pub struct ParamField  { pub name: &'static str, pub kind: ParamType, pub default: ParamValue }

pub enum ParamType {
    Int, Bool, MemorySize, String,
    Enum(&'static [&'static str]),
}

pub struct DeviceParams { values: HashMap<String, ParamValue> }
impl DeviceParams {
    pub fn get_int(&self, name: &str)  -> i64  { ... }
    pub fn get_bool(&self, name: &str) -> bool { ... }
    pub fn get_str(&self, name: &str)  -> &str { ... }
}
```

### Plugin Contract — `.so` Entry Point

```rust
// Every device plugin exports this C-ABI symbol
#[no_mangle]
pub extern "C" fn helm_device_register(registry: *mut DeviceRegistry) {
    let r = unsafe { &mut *registry };
    r.register(DeviceDescriptor {
        name:         "uart16550",
        version:      "1.0.0",
        description:  "16550-compatible UART",
        factory:      |params| Box::new(Uart16550::from_params(params)),
        param_schema: || ParamSchema::new()
                            .field("clock_hz", ParamType::Int, 1_843_200),
    });
}

// Also embedded: Python class string (no base_addr/irq — those are system-level)
pub static PYTHON_CLASS: &str = r#"
class Uart16550(Device):
    clock_hz: Param.Int = 1_843_200
"#;
```

### Plugin Loading

```rust
impl DeviceRegistry {
    pub fn load_plugin(&mut self, path: &Path) -> Result<(), PluginError>;
    pub fn create(&self, name: &str, params: DeviceParams) -> Result<Box<dyn Device>, PluginError>;
    pub fn list(&self) -> &[DeviceDescriptor];
    pub fn param_schema(&self, name: &str) -> Option<&ParamSchema>;
}
```

```python
helm_ng.load_plugin("./libhelm_uart16550.so")  # registers Uart16550 class

uart = helm_ng.Uart16550(clock_hz=3_686_400)   # only device-internal params
system.map_device(uart, base=0x09000000)
system.connect_irq(uart, controller=plic, irq=33)
```

---

## HelmEventBus — Observability Event System

Inspired by SIMICS HAPs (Hardware Action Points). A named, typed pub-sub bus where any component fires events and any tool (debugger, tracer, Python script) subscribes — with no coupling between source and subscriber.

**Two event systems in helm-ng — distinct purposes:**

| System | Purpose | Timing |
|--------|---------|--------|
| `EventQueue` (`helm-event`) | Schedule future simulation callbacks at tick T | Asynchronous — deferred |
| `HelmEventBus` (`helm-devices/src/bus/event_bus`) | Observable named events fired by components | Synchronous — subscribers run inline |

```rust
// helm-devices/bus

pub enum HelmEvent {
    Exception     { cpu: &'static str, vector: u32, pc: u64, tval: u64 },
    MemWrite      { addr: u64, size: usize, val: u64, cycle: u64 },
    CsrWrite      { csr: u16, old: u64, new: u64 },
    MagicInsn     { pc: u64, value: u64 },       // debug marker in target code
    SyscallEnter  { nr: u64, args: [u64; 6] },
    SyscallReturn { nr: u64, ret: u64 },
    ModeChange    { from: ExecMode, to: ExecMode },
    Custom        { name: &'static str, data: Vec<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelmEventKind {
    Exception, MemWrite, CsrWrite, MagicInsn,
    SyscallEnter, SyscallReturn, ModeChange, Custom,
}

pub struct HelmEventBus { ... }

impl HelmEventBus {
    pub fn subscribe<F>(&mut self, kind: HelmEventKind, f: F)
    where F: Fn(&HelmEvent) + Send + 'static;

    pub fn fire(&self, event: HelmEvent);   // synchronous: all subscribers called inline
    pub fn unsubscribe(&mut self, id: SubscriberId);
}
```

**Who owns it:** `System` owns one `HelmEventBus`. `HelmEngine` holds a shared reference (`Arc<HelmEventBus>`). All devices and the engine fire events; all tools subscribe.

**`TraceLogger` is a `HelmEventBus` subscriber** — not a separate system. It subscribes to all event kinds and writes to its ring buffer.

**Python integration:**
```python
def on_exception(event):
    print(f"Exception vec={event.vector:#x} pc={event.pc:#x}")
    sim.pause()

sim.event_bus.subscribe("Exception", on_exception)
sim.event_bus.subscribe("MagicInsn", lambda e: print("magic hit"))
```

---

## Directory Structure

```
helm-ng/
├── Cargo.toml                    # Workspace root — members = ["framework/*","runtime/*","hw/*","debug/*"]
├── framework/
│   ├── helm-core/                # ArchState, ExecContext, ThreadContext, MemInterface, HartException
│   ├── helm-memory/              # MemoryRegion, MemoryMap, FlatView, FlatMem, HelmAddressSpace
│   ├── helm-timing/              # VirtualTiming, IntervalTiming, AccurateTiming, TimingModel trait
│   ├── helm-event/               # EventQueue — time-ordered discrete events
│   ├── helm-devices/             # Device trait, InterruptPin, DeviceRegistry, HelmEventBus
│   │   └── src/
│   │       ├── framework/        # Device trait, Transaction, ParamSchema, backend
│   │       └── bus/              # Bus/BusDevice traits, HelmEventBus, AMBA/I2C/SPI
│   ├── helm-stats/               # PerfCounter, PerfHistogram, StatsRegistry
│   ├── helm-plugin/              # HelmPluginRegistry, HelmPlugin trait
│   ├── helm-probe/               # Zero-cost typed probe points (CpuProbes, GicProbes)
│   ├── helm-diag/                # Structured diagnostic channel (DiagMonitor, DiagLevel)
│   ├── helm-jit/                 # Pluggable JIT backend framework (JitBackend trait, dynasm backend)
│   └── helm-decode/              # .decode file parser + code generator
├── runtime/
│   ├── helm-arch/                # All ISA implementations
│   │   └── src/
│   │       ├── riscv/            # RV64I+Zicsr done; M/A/F/D in progress
│   │       ├── aarch64/          # ~2100-line executor; SE+FS working
│   │       └── aarch32/          # future
│   ├── helm-engine/              # HelmEngine<T>, HelmSim, ExecMode, Isa, typed event dispatch
│   │   └── src/
│   │       ├── se/               # LinuxAarch64SyscallHandler, LinuxRiscv64SyscallHandler
│   │       ├── loader/           # ELF64 loader, arm64_image loader
│   │       ├── session.rs        # HelmMachine, HelmBoard, HelmCore, HelmVcpu, HelmCluster
│   │       ├── fs.rs             # FS-mode step loop (AArch64)
│   │       └── machine.rs        # Multi-core coordination + scheduling policies
│   ├── helm-platform/            # ArmVirtPlatform, machine wiring
│   ├── helm-debug/               # GdbServer (RSP), TraceLogger, CheckpointManager
│   ├── helm-python/              # PyO3 bindings → _helm_ng module; python/helm/ package
│   └── helm-cli/                 # helm-aarch64, helm-system-aarch64, helm-riscv64 binaries
├── hw/
│   ├── helm-hw-char/             # PL011 UART
│   ├── helm-hw-timer/            # SP804 dual timer
│   ├── helm-hw-rtc/              # PL031 RTC
│   ├── helm-hw-dma/              # DMA engine
│   ├── helm-hw-intc/             # GICv2 + GICv3 (distributor, redistributor, ICC_* sysregs)
│   ├── helm-hw-iommu/            # IOMMU models: SMMUv3, AMD-Vi, RISC-V IOMMU
│   ├── helm-hw-pci/              # PCI ECAM host bridge
│   └── helm-hw-virtio/           # VirtIO MMIO transport + device backends (blk, console, net, rng)
├── debug/
│   ├── helm-spy/                 # HelmSpy, analysis models
│   └── helm-report/              # Output sinks (JSON, CSV)
├── assets/
│   ├── aarch64/alpine/           # Alpine Linux kernel, DTB, rootfs, APKs
│   └── riscv/bin/                # busybox, static-sh (statically linked RV64 binaries)
├── examples/
│   └── fs/boot_rpi_full.py       # AArch64 FS boot (working)
├── python/helm/                  # helm Python package (helm.aarch64.*, etc.)
└── docs/
    ├── ARCHITECTURE.md           # this file
    ├── object-model.md
    ├── traits.md
    ├── api.md
    ├── testing.md
    ├── plans/                    # active implementation plans
    └── design/                   # 60+ per-crate design documents
```

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Python Config Layer                         │
│   helm.System("virt", timing="interval:l1d_size=64KiB")            │
│   helm.Cpu("cpu0", isa="aarch64", model="cortex-a55")              │
│   system.instantiate() → system.run(1_000_000_000)                 │
└───────────────────────────┬─────────────────────────────────────────┘
                            │ PyO3 / build_simulator()
                            ▼
┌─────────────────────────────────────────────────────────────────────┐
│                    HelmSim (enum wrapper)                      │
│  VirtualTiming(HelmEngine<VirtualTiming>)                      │
│  IntervalTiming(HelmEngine<IntervalTiming>)                    │
│  AccurateTiming(HelmEngine<AccurateTiming>)                    │
└───────────────────────────┬─────────────────────────────────────────┘
                            │ one enum dispatch per Python call
                            ▼
┌─────────────────────────────────────────────────────────────────────┐
│             HelmEngine<T: TimingModel>  (hot loop)                   │
│                                                                     │
│  loop {                                                             │
│    match isa { RiscV => step_riscv(), AArch64 => step_aarch64() }  │
│    timing.on_memory_access(...)   ← inlined, zero vtable           │
│  }                                                                  │
│                                                                     │
│  ExecMode (enum): FE / SE / FS                                      │
│  ISA (enum):      RiscV / AArch64 / AArch32                        │
└──────────┬──────────────────────┬───────────────────────────────────┘
           │                      │
           ▼                      ▼
┌─────────────────┐   ┌────────────────────────────────────────┐
│  ArchState      │   │         MemoryMap                      │
│  registers[]    │   │  FlatView (sorted non-overlapping)     │
│  pc             │   │  ┌──────────────────────────────────┐  │
│  csrs           │   │  │ 0x0000  RAM (Vec<u8>)            │  │
└─────────────────┘   │  │ 0x1000  MMIO UART                │  │
                      │  │ 0x2000  ROM (alias → flash)      │  │
                      │  └──────────────────────────────────┘  │
                      └────────────────────────────────────────┘
           │
           ▼
┌─────────────────────────────────────────────────────────────────────┐
│                     Debuggability Layer                             │
│  GDB RSP Stub ── TraceLogger ── Checkpoint ── Stats Registry        │
└─────────────────────────────────────────────────────────────────────┘
           │
           ▼
┌─────────────────────────────────────────────────────────────────────┐
│                  Event Queue (VirtualTiming only)              │
│  BinaryHeap<Reverse<TimedEvent>>                                    │
│  Components schedule future events via: queue.schedule(t, callback)│
└─────────────────────────────────────────────────────────────────────┘
```

---

## Phased Build Plan

### Phase 0 — MVP: Correct RISC-V SE Simulator (4–6 weeks)

**Goal:** Execute real RISC-V Linux binaries (statically linked) with correct output. No timing.

Deliverables:
1. `helm-core`: `ArchState` (RV64GC register file + CSRs), `MemInterface` trait, flat memory (`Vec<u8>`)
2. `helm-isa-riscv`: Full RV64IMACFD decode + execute (match + bit ops, no DSL yet)
3. `helm-engine`: `HelmEngine<VirtualTiming>` with `ExecMode::Syscall`, no timing
4. `helm-engine/se`: ~50 Linux syscalls (read, write, open, mmap, brk, exit, clone, ...)
5. Validation: RISC-V official test suite + riscv-tests + run `hello_world`, `ls`, `bash`

**Does NOT include:** Caches, event queue, timing, ARM, Python config, GDB stub.

---

### Phase 1 — Timing + Event System + GDB (6–10 weeks)

**Goal:** Timing-accurate memory simulation with GDB debugging support.

Deliverables:
1. `helm-event`: Discrete event queue (`BinaryHeap<Reverse<TimedEvent>>`)
2. `helm-memory`: `MemoryRegion` tree, `FlatView`, `MmioHandler` trait, three access modes
3. `helm-timing`: `VirtualTiming` with event-driven L1/L2 cache (Classic model, fixed protocol)
4. `helm-debug`: GDB RSP stub (read/write regs, read/write mem, step, continue, breakpoints)
5. `helm-stats`: PerfCounter, PerfHistogram, formula, JSON dump
6. `helm-engine`: `IntervalTiming` timing model (Sniper-style interval simulation)
7. Validation: Cache miss rate accuracy vs. Cachegrind on known benchmarks

---

### Phase 2 — Python Config + ARM AArch64 (8–12 weeks)

**Goal:** Gem5-style Python config layer + ARM support.

Deliverables:
1. `helm-python`: PyO3 bindings for `HelmSim`, `build_simulator()`, typed params
2. `helm_ng` Python package: `SimObject`, `Cpu`, `Cache`, `Memory`, `Param.*` types
3. `helm-isa-arm`: AArch64 decode (using `deku` crate) + execute, CSR/system register file
4. `HelmSim` enum wrapping all three timing models
5. `helm-debug`: Trace logging (ring buffer, serde-serialized `TraceEvent`)
6. Checkpoint / restore via `checkpoint_save()` / `checkpoint_restore()` on all SimObjects
7. Validation: AArch64 RISC-V tests + ARM official ISA validation suite

---

### Phase 3 — Full System + Accurate Timing (future)

**Goal:** Boot Linux. Cycle-accurate pipeline model.

Deliverables:
1. `helm-devices`: VirtIO disk, VirtIO network, UART (16550), PLIC, CLINT
2. `helm-engine`: `ExecMode::System` — interrupt delivery, page table walker, MMU
3. `helm-timing`: `AccurateTiming` cycle-accurate model (5-stage in-order first, OoO later)
4. ARM AArch32 / Thumb support
5. ARM AArch64 FS boot: Linux kernel on VirtIO disk

---

## Open Design Questions

1. **AArch32 on day one or defer?** — Recommend defer until AArch64 is solid.
2. **Coherence protocol for multi-core?** — SE mode can avoid this; FS mode needs it. Start single-core.
3. **JIT / binary translation?** — Not in scope for Phase 0–2. Add as an optional `TimingModel` variant in Phase 3+ once functional correctness is established.
4. **Device Modeling Language (DML)?** — SIMICS uses it; not recommended for a solo project. Rust + `MmioHandler` trait is sufficient.
5. **ISA DSL for instruction decode?** — Gem5 uses a custom parser. For helm-ng: Rust procedural macros or a simple Python code-gen script is sufficient. Evaluate after implementing RISC-V by hand.
6. **Multi-core / SMP?** — Phase 3+. Requires: shared memory model, inter-processor interrupt (IPI), cache coherence protocol. Do not add until single-core SE is solid.
7. **Interval simulation accuracy target?** — Sniper achieves ~5% IPC error. Aim for <10% on SPEC CPU benchmarks as a Phase 2 milestone.

---

## Key Design Decisions (Recorded)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Core language | Rust | Memory safety, PyO3 for Python, modern tooling |
| Python FFI | PyO3 | Idiomatic Rust, no pybind11/SWIG |
| Timing dispatch | Generic `T: TimingModel` | Monomorphized, inlined on hot path |
| Mode dispatch | `ExecMode` enum | Closed set, exhaustive, cold path |
| PyO3 boundary | `HelmSim` enum | One match per Python call, zero per instruction |
| ISA dispatch | `Isa` enum inside kernel | Decode is once-per-instruction, not per memory access |
| Memory model | Unified `MemoryRegion` tree | RAM/MMIO/ROM/alias via one type (QEMU-inspired) |
| Memory access modes | Timing / Atomic / Functional | Gem5 port model — necessary for mode switching |
| Coherence | Deferred | Start single-core; add when multi-core is needed |
| ISA start | RISC-V RV64GC | Simpler encoding, abundant references, validate first |
| ARM encoding | `deku` crate | Declarative bit-field parsing matching ISA spec tables |
| Event queue | `BinaryHeap<Reverse<TimedEvent>>` | Stdlib, no deps, min-heap for earliest-first |
| Debuggability | GDB RSP + TraceLogger + Checkpoint | Built in from Phase 0 (GDB) and Phase 1 (trace/checkpoint) |
| Observability | HelmEventBus (pub-sub) | Named typed events; TraceLogger is a subscriber, not separate |
| ExecContext split | ExecContext (hot) + ThreadContext (cold) | Hot path inlined; cold path dyn dispatch for GDB/Python/SE |
| ISA layout | helm-arch/src/{riscv,aarch64,aarch32} | Single crate, all ISAs co-located with their tests |
| Device interface | Device trait: read/write offset + signal + InterruptPin | No base addr, no IRQ number on device |
| IRQ model | InterruptPin → wire → InterruptSink (PLIC/GIC/PIC) | Device asserts pin; routing is platform config |
| Device plugins | .so with C-ABI entry + embedded Python class | Runtime load, zero recompile to add new device |
| Device params | ParamSchema + DeviceParams (no addr/IRQ) | Internal config only; addr+IRQ are system-level |
| Bus subsystems | helm-devices/src/bus/{pci,amba} | PCI config space + AMBA/AHB/APB infrastructure |
| Build system | Cargo workspaces | Rust native, no SCons/CMake complexity |
