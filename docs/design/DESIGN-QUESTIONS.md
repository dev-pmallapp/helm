# Helm-ng Design Questions

> Questions derived from research that every design document must answer.
> Each question includes: context, options & trade-offs table, answer, rationale, and impact.
> Research sources: SIMICS API, Gem5, QEMU QOM, Sniper, PTLsim, RISC-V spec, ARM ARM.

---

## Architecture Diagrams

# Architectural Diagrams

This section provides a visual reference for the major structural and behavioral patterns in helm-ng. Each diagram is rendered in plain ASCII so it is legible in any text viewer, terminal, or Markdown renderer without special tooling.

---

## 1. Full Crate Architecture

The following diagram shows all ten crates, the dependency arrows between them, and the primary types that live in each. Arrows point from dependent to dependency (i.e., an arrow from `helm-arch` to `helm-core` means `helm-arch` depends on `helm-core`).

```
  ┌──────────────────────────────────────────────────────────────────────────────┐
  │                              helm-python                                     │
  │   PyO3 bindings · HelmSim enum · Python helm_ng package · config API        │
  └────────────────────────────────┬─────────────────────────────────────────────┘
                                   │
          ┌────────────────────────┼────────────────────────┐
          │                        │                        │
  ┌───────▼──────────┐   ┌─────────▼────────┐   ┌──────────▼──────────┐
  │   helm-debug     │   │   helm-engine     │   │    helm-stats       │
  │                  │   │                  │   │                     │
  │  GdbServer       │   │  World           │   │  PerfCounter        │
  │  TraceLogger     │   │  HelmEngine<T>   │   │  PerfHistogram      │
  │  Checkpoint      │   │  HelmSim         │   │  PerfFormula        │
  │  Manager         │   │  Scheduler       │   │  StatsRegistry      │
  └───────┬──────────┘   │  ExecMode        │   └──────────┬──────────┘
          │              │  LinuxSyscall    │              │
          │              │  Handler         │              │
          │              │  FdTable         │              │
          │              └────────┬─────────┘              │
          │                       │                        │
          └───────────────────────┼────────────────────────┘
                                  │ depends on
          ┌───────────────────────▼────────────────────────┐
          │                   helm-timing                  │
          │                                                │
          │  TimingModel trait   Virtual / Interval /      │
          │  MicroarchProfile    Accurate implementations  │
          └───────────────────────┬────────────────────────┘
                                  │
          ┌───────────────────────▼────────────────────────┐
          │                  helm-devices                  │
          │                                                │
          │  Device trait        InterruptPin/Wire/Sink    │
          │  ClassDescriptor     InterfaceRegistry         │
          │  AttrStore           register_bank! macro      │
          │  DeviceRegistry      HelmEventBus              │
          │  bus/{pci,amba,event_bus}                      │
          └───────┬──────────────────────┬─────────────────┘
                  │                      │
   ┌──────────────┘        ┌─────────────┘
   │                       │
   │    ┌──────────────────┼──────────────────────┐
   │    │                  │                      │
   │  ┌─▼────────────┐   ┌─▼──────────────┐   ┌──▼───────────────┐
   │  │  helm-arch   │   │  helm-memory   │   │   helm-event     │
   │  │              │   │                │   │                  │
   │  │  RISC-V      │   │  MemoryRegion  │   │  EventQueue      │
   │  │  AArch64     │   │  FlatView      │   │  EventClass      │
   │  │  AArch32     │   │  CacheModel    │   │  (discrete-time  │
   │  │  decode +    │   │  TlbModel      │   │   scheduler)     │
   │  │  execute     │   │  MemFault      │   │                  │
   │  │  SyscallAbi  │   │                │   │                  │
   │  └──────┬───────┘   └───────┬────────┘   └───────┬──────────┘
   │         │                   │                    │
   │         └───────────────────┼────────────────────┘
   │                             │
   │                  ┌──────────▼───────────┐
   └──────────────────►       helm-core      │
                      │                     │
                      │  ArchState trait     │
                      │  ExecContext (hot)   │
                      │  ThreadContext       │
                      │  SyscallHandler      │
                      │  AttrValue/Kind      │
                      │  HelmObjectId        │
                      │  PendingObject       │
                      └─────────────────────┘
```

**What this shows.** `helm-core` is the universal foundation — every other crate depends on it, directly or transitively. The three ISA/memory/event crates (`helm-arch`, `helm-memory`, `helm-event`) all sit at the same layer and share no mutual dependency, which means new ISAs or memory models can be added without touching each other. `helm-devices` is one layer above them and introduces the device model together with the event bus. `helm-timing` sits above `helm-devices` because timing models need to interact with the bus. The engine, debug, and stats crates form the top execution layer, and `helm-python` is the final façade that composes them all into a usable package.

**Key design decisions visible here.**
- The diamond shape at `helm-core` enforces that shared abstractions (`ArchState`, `ExecContext`, `HelmObjectId`) are defined once and depended on from above, preventing duplication.
- `helm-event` is a peer of `helm-arch` and `helm-memory`, not a dependency of them. Components fire events upward into `helm-engine`'s `EventQueue`; they never call back downward.
- `helm-timing` is deliberately between `helm-devices` and `helm-engine` so that timing models can react to device bus traffic without creating a cycle.

---

## 2. Simulation Execution Flow

This sequence diagram traces a simulation run from Python configuration all the way through instruction execution and event dispatch back to observable side effects.

```
  Python                World            HelmEngine<T>        ISA (helm-arch)
  (user script)         (helm-engine)    (helm-engine)        decode/execute
  ─────────────         ─────────────    ─────────────        ───────────────
       │                     │                 │                     │
       │  sim = HelmSim(...)  │                 │                     │
       │─────────────────────►                 │                     │
       │                     │                 │                     │
       │  sim.elaborate()     │                 │                     │
       │─────────────────────►                 │                     │
       │                     │ instantiate()   │                     │
       │                     │ (two-phase,     │                     │
       │                     │  see §5)        │                     │
       │                     │─────────────────►                     │
       │                     │ register engine  │                     │
       │                     │◄─────────────────                     │
       │                     │                 │                     │
       │  sim.run(n_insns)    │                 │                     │
       │─────────────────────────────────────── ►                    │
       │                     │    Scheduler::run() kicks HelmEngine  │
       │                     │                 │                     │
       │                     │                 │  step() [hot loop]  │
       │                     │                 │─────────────────────►
       │                     │                 │  fetch + decode      │
       │                     │                 │◄─────────────────────
       │                     │                 │  execute(ExecCtx)   │
       │                     │                 │─────────────────────►
       │                     │                 │  mem read/write      │
       │                     │                 │ (→ helm-memory)      │
       │                     │                 │◄─────────────────────
       │                     │                 │                     │
       │              TimingModel::on_insn()   │                     │
       │                     │◄────────────────                      │
       │                     │                 │                     │
       │            EventQueue::push(event)    │                     │
       │                     │◄────────────────                      │
       │                     │                 │                     │
       │   EventQueue drains scheduled events  │                     │
       │                     │                 │                     │
       │                     │  device_cb()    │  HelmEventBus       │
       │                     │─────────────────────────────────────►│
       │                     │                 │  TraceLogger        │
       │                     │                 │ ─────────────────►  │
       │                     │                 │  GdbServer check    │
       │                     │                 │ ─────────────────►  │
       │                     │                 │  Python callback    │
       │                     │                 │ ─────────────────►  │
       │                     │                 │                     │
       │  ◄────────── stats / return value ────                      │
```

**What this shows.** The Python script is entirely in the configuration phase until `sim.run()` is called. Once execution begins, the hot loop lives inside `HelmEngine::step()` and never returns to Python unless interrupted. The timing model is called synchronously on every instruction, and queued events are drained after each batch of instructions (or at a configurable granularity). The `HelmEventBus` is the single fan-out point for all observable side effects — tracing, debugging, and Python hooks all subscribe once at startup and receive events without polling.

**Key design decisions visible here.**
- The `Scheduler` layer means the engine does not call itself; it is called by a scheduler that can also manage multiple engines for multicore setups.
- `TimingModel::on_insn()` is on the hot path, which is why `HelmEngine<T>` is generic over `T: TimingModel` — the call is monomorphized away and costs no virtual dispatch.
- Event dispatch through `HelmEventBus` is synchronous and in-order relative to simulation time, so subscribers always see a consistent world state.

---

## 3. Component Interaction During a Memory Access

This diagram shows what happens at each layer of the memory subsystem when a CPU issues an instruction fetch or a data load.

```
  HelmEngine (step)
        │
        │  ExecContext::load(vaddr, size)
        ▼
  ┌─────────────────────────────────────────────────────────────┐
  │                         TlbModel                           │
  │                                                            │
  │   lookup(vaddr) ──► hit? ──► return paddr                  │
  │                      │                                     │
  │                      └─ miss ──► walk page table           │
  │                                  (via MemoryRegion)        │
  │                                  insert entry              │
  │                                  return paddr              │
  └─────────────────────────────┬───────────────────────────────┘
                                │  paddr
                                ▼
  ┌─────────────────────────────────────────────────────────────┐
  │                        CacheModel                          │
  │                                                            │
  │   lookup(paddr) ──► hit? ──► return data  ────────────────►│── to CPU
  │                      │                                     │
  │                      └─ miss                               │
  │                          │                                 │
  │                          │  record miss stats              │
  │                          │  (helm-stats PerfCounter)       │
  └──────────────────────────┼──────────────────────────────────┘
                             │  cache miss, need fill
                             ▼
  ┌─────────────────────────────────────────────────────────────┐
  │                        FlatView                            │
  │  (flattened projection of the MemoryRegion tree)           │
  │                                                            │
  │   resolve(paddr) ──► find region ──► Device or RAM?        │
  │                                         │         │        │
  │                                      Device      RAM       │
  │                                         │         │        │
  │                              Device::read()  memcpy        │
  │                                         │         │        │
  │                                         └────┬────┘        │
  └──────────────────────────────────────────────┼─────────────┘
                                                 │  data bytes
                                                 ▼
  ┌─────────────────────────────────────────────────────────────┐
  │                      TimingModel                           │
  │                                                            │
  │  on_mem_access(paddr, latency_cycles)                      │
  │    └─► adjusts virtual time / stall counter               │
  │    └─► may push EventClass::MemEvent to EventQueue         │
  └──────────────────────────────────────────────┬─────────────┘
                                                 │
                                                 ▼
                                         HelmEventBus
                                   (TraceLogger records access,
                                    GdbServer watchpoint check)
```

**What this shows.** A single logical memory access fans out through four distinct layers: address translation (TLB), caching, physical address resolution (FlatView), and timing annotation. Each layer is independently modeled and can be swapped or disabled. The TLB and cache are in `helm-memory`; the FlatView dispatches to device or RAM via the `MemoryRegion` tree; the timing model in `helm-timing` records the effective latency.

**Key design decisions visible here.**
- Devices and RAM are peers in the `FlatView`. A MMIO register read goes through the exact same path as a DRAM load — device authors do not need special cache-bypass logic.
- `MemFault` propagates upward cleanly if any layer raises it (unmapped address, permission violation), and the ISA layer converts it to the appropriate CPU exception.
- Stats recording (`PerfCounter`) is a non-intrusive side effect at the cache layer, not a return value; it does not perturb the data path.

---

## 4. Interrupt Routing

This diagram traces the path from a peripheral asserting an interrupt line to the CPU taking the exception.

```
  Peripheral Device
  (e.g., UART, timer)
        │
        │  self.irq_pin.assert()
        ▼
  ┌─────────────────┐
  │  InterruptPin   │  (helm-devices)
  │                 │  level-sensitive or edge
  └────────┬────────┘
           │  connected via
           ▼
  ┌─────────────────┐
  │  InterruptWire  │  (helm-devices)
  │                 │  carries signal between devices
  └────────┬────────┘
           │  drives
           ▼
  ┌─────────────────────────────────────────────────────┐
  │               InterruptSink (PLIC / GIC)            │
  │                                                     │
  │   on_assert(irq_num)                                │
  │     ├─► set pending bit in PLIC registers           │
  │     ├─► compute priority / threshold                │
  │     └─► if CPU should take IRQ:                    │
  │           push HelmEvent::ExternalIrq               │
  │           to EventQueue                             │
  └───────────────────────────┬─────────────────────────┘
                              │
                              ▼
  ┌─────────────────────────────────────────────────────┐
  │                     EventQueue                      │
  │  (ordered by virtual time)                          │
  └───────────────────────────┬─────────────────────────┘
                              │  dequeued by Scheduler
                              ▼
  ┌─────────────────────────────────────────────────────┐
  │               HelmEngine::check_pending_irq()       │
  │                                                     │
  │   reads PLIC claim register                         │
  │   if IRQ pending and CPU interrupts enabled:        │
  │     ├─► save PC, set cause register                 │
  │     ├─► redirect fetch to exception vector          │
  │     └─► HelmEventBus fire: IrqTaken{irq, cpu}      │
  └─────────────────────────────────────────────────────┘
                              │
              ┌───────────────┴──────────────────┐
              ▼                                  ▼
       TraceLogger                         Python callback
       (records irq entry)              (user hook, optional)
```

**What this shows.** Interrupt routing is a layered model: hardware signal → wire → sink (interrupt controller) → event queue → engine check → CPU exception entry. Each layer is independently modeled and each can be substituted. A GIC model can replace a PLIC model by implementing the same `InterruptSink` trait.

**Key design decisions visible here.**
- The `InterruptPin`/`Wire`/`Sink` chain mirrors real hardware: pins are local to a device, wires connect them to controllers, and the controller (sink) arbitrates priority. This keeps device models small and focused.
- Interrupts enter the engine via the `EventQueue` rather than being checked on every instruction. This allows the timing model to model interrupt latency realistically — the event timestamp determines when the CPU actually sees the interrupt.
- `HelmEventBus` is fired *after* the CPU state is updated, so a TraceLogger or GdbServer observer always sees the machine in the post-exception state.

---

## 5. Object Lifecycle

This diagram shows the two-phase configuration protocol and the full lifetime of a simulated object from Python creation to shutdown.

```
  Python (config phase)              World (helm-engine)
  ─────────────────────              ───────────────────

  obj = MyDevice(...)
        │
        │  returns PendingObject
        ▼
  ┌─────────────────────────────────────────────────────────────┐
  │  PHASE 1 — ALLOCATION                                       │
  │                                                             │
  │  PendingObject holds:                                       │
  │    class_name, attrs, parent_id                             │
  │  No Rust Device is constructed yet.                         │
  │  No side effects. Safe to serialize / clone.                │
  └─────────────────────────────────────────────────────────────┘
        │
        │  sim.elaborate()  (or sim.run())
        ▼
  ┌─────────────────────────────────────────────────────────────┐
  │  PHASE 2 — World::instantiate()                             │
  │                                                             │
  │  Step A — alloc                                             │
  │    DeviceRegistry::create(class_name)                       │
  │    → Box<dyn Device>  inserted into World                   │
  │    → assigned HelmObjectId                                  │
  │                                                             │
  │  Step B — set_attrs                                         │
  │    for each (key, AttrValue) in PendingObject.attrs:        │
  │      Device::set_attr(key, value)                           │
  │    Memory regions / IRQ wires resolved here                 │
  │                                                             │
  │  Step C — finalize                                          │
  │    Device::finalize()                                       │
  │    Device may allocate internal state,                      │
  │    subscribe to HelmEventBus, register mmio                 │
  │                                                             │
  │  Step D — all_finalized  (called after ALL objects done)    │
  │    Device::all_finalized()                                  │
  │    Cross-device wiring, PLIC claim-enable setup, etc.       │
  └─────────────────────────────────────────────────────────────┘
        │
        │  sim.run()
        ▼
  ┌─────────────────────────────────────────────────────────────┐
  │  SIMULATION RUNNING                                         │
  │                                                             │
  │  Device::step() called by Scheduler                         │
  │  Device::read() / write() called on MMIO access             │
  │  Device::on_event() called via HelmEventBus                 │
  └──────────────────────────┬──────────────────────────────────┘
                             │
              ┌──────────────┴──────────────┐
              ▼                             ▼
  CheckpointManager::save()     CheckpointManager::restore()
        │                                   │
        │  Device::checkpoint()             │  Device::restore()
        │  (serialize state to blob)        │  (deserialize blob)
        │                                   │
        └──────────────┬────────────────────┘
                       │
                       ▼
              simulation continues
                  (possibly with different TimingModel)
                       │
                       ▼
  ┌─────────────────────────────────────────────────────────────┐
  │  SHUTDOWN                                                   │
  │                                                             │
  │  World::shutdown()                                          │
  │    Device::shutdown() (flush buffers, close files)         │
  │    StatsRegistry::emit() (dump final counters)             │
  │    World dropped → all Device boxes freed                  │
  └─────────────────────────────────────────────────────────────┘
```

**What this shows.** Every simulated object goes through a strict four-step instantiation sequence before any simulation clock ticks. The `PendingObject` is the serializable snapshot of user intent; `World::instantiate()` converts it into a live Rust object. Checkpointing interrupts the running phase and can resume into a different timing mode.

**Key design decisions visible here.**
- Separating `finalize` from `all_finalized` is essential for cross-device dependencies: a device can wire itself to another during `all_finalized` knowing every peer is already allocated and attributed.
- `PendingObject` is intentionally free of Rust lifetimes or pointers. This means Python can build an entire machine description as a plain data structure and pass it to Rust in a single `elaborate()` call, making scripted configurations reproducible and diffable.
- Checkpoint/restore is on the `Device` trait, so every device is responsible for its own state. This keeps `World` out of the serialization business.

---

## 6. Two-Phase Python Configuration Model

This diagram makes the hard boundary between the pure Python config phase and the Rust simulation phase explicit.

```
  ┌──────────────────────────────────────────────────────────────────────────┐
  │  PYTHON CONFIG PHASE                (pure data, no simulation side       │
  │  ─────────────────                  effects, safe to re-run/replay)      │
  │                                                                          │
  │   from helm_ng import HelmSim, RiscVCpu, Uart, Plic, Ram               │
  │                                                                          │
  │   sim = HelmSim(timing="virtual")   ──► PendingObject (HelmEngine)      │
  │   cpu = RiscVCpu(hartid=0)          ──► PendingObject (RiscVCpu)        │
  │   ram = Ram(base=0x8000_0000,       ──► PendingObject (Ram)             │
  │             size=0x1000_0000)                                            │
  │   uart = Uart(base=0x1000_0000)     ──► PendingObject (Uart)            │
  │   plic = Plic(base=0x0C00_0000)     ──► PendingObject (Plic)            │
  │                                                                          │
  │   uart.irq >> plic.source[1]        ──► InterruptWire descriptor        │
  │   sim.load_elf("firmware.elf")      ──► recorded in PendingObject attrs  │
  │                                                                          │
  │   # At this point: zero Rust Device objects exist.                       │
  │   # All state is in PendingObjects (serializable Python dicts).          │
  │                                                                          │
  └──────────────────────────────────┬───────────────────────────────────────┘
                                     │
                                     │  sim.elaborate()
                                     │  (or implicitly at sim.run())
                                     ▼
  ╔══════════════════════════════════════════════════════════════════════════╗
  ║                         elaborate() boundary                            ║
  ║  World::instantiate() runs all four phases (alloc, set_attrs,           ║
  ║  finalize, all_finalized) for every PendingObject in dependency order.  ║
  ╚══════════════════════════════════════════════════════════════════════════╝
                                     │
                                     ▼
  ┌──────────────────────────────────────────────────────────────────────────┐
  │  RUST SIMULATION PHASE              (stateful, clock ticks, no Python   │
  │  ─────────────────────               config changes allowed)            │
  │                                                                          │
  │   World owns all live Device boxes                                       │
  │   HelmEngine<Virtual> drives the hot loop                                │
  │   Scheduler::run(n_insns) blocks until done                              │
  │                                                                          │
  │   Python may still:                                                      │
  │     sim.stats()           ──► read StatsRegistry (safe)                  │
  │     sim.checkpoint()      ──► pause + serialize (safe)                   │
  │     sim.restore(blob)     ──► replace running state (safe)               │
  │     sim.subscribe(cb)     ──► add HelmEventBus listener (safe)           │
  │                                                                          │
  │   Python must NOT:                                                        │
  │     modify attrs of live objects   (raises HelmError)                    │
  │     call elaborate() again         (raises HelmError)                    │
  └──────────────────────────────────────────────────────────────────────────┘
```

**What this shows.** The `elaborate()` call is a one-way gate. Before it, the entire machine description is a pure data structure with no Rust side effects. After it, the machine is live and configuration is frozen. The Python API still provides runtime observability (stats, checkpoints, event callbacks) but cannot mutate the structural topology.

**Key design decisions visible here.**
- Making the config phase side-effect-free means user scripts are reproducible: running the same Python file twice produces an identical simulation, regardless of OS state or prior runs.
- The hard boundary enables fast iteration: a CI system can elaborate, run a fixed instruction count, collect stats, tear down, and repeat without any persistent process state.
- Serializing `PendingObject` to JSON before `elaborate()` gives a machine-readable snapshot of a configuration that can be diffed, stored, and loaded, enabling reproducible benchmarks.

---

## 7. Timing Mode Switch

This diagram shows how a checkpoint is used to transition a running simulation between timing fidelity modes, which is the primary mechanism for region-of-interest (ROI) profiling.

```
  ┌───────────────────────────────────────────────────────────────────┐
  │  VIRTUAL MODE  (HelmEngine<Virtual>)                              │
  │                                                                   │
  │  - Instruction count is the only notion of time                   │
  │  - Memory access latency = 0 cycles                               │
  │  - No pipeline stall modeling                                     │
  │  - ~10x faster than Accurate                                      │
  │  - Suitable for booting OS, running startup code                  │
  │                                                                   │
  │  Scheduler::run(until: InsnCount(5_000_000))                      │
  └──────────────────────────┬────────────────────────────────────────┘
                             │  sim.checkpoint()
                             ▼
  ╔═══════════════════════════════════════════════════════════════════╗
  ║  CHECKPOINT BLOB                                                  ║
  ║  (serialized World state: all Device states, CPU registers,       ║
  ║   memory contents, EventQueue, virtual clock)                     ║
  ╚═══════════════════════════════════════════════════════════════════╝
                             │  sim.restore(blob, timing="interval")
                             ▼
  ┌───────────────────────────────────────────────────────────────────┐
  │  INTERVAL MODE  (HelmEngine<Interval>)                            │
  │                                                                   │
  │  - MicroarchProfile drives IPC estimation per instruction class   │
  │  - Memory latency modeled with fixed-latency cache hierarchy       │
  │  - Moderate accuracy, moderate speed                              │
  │  - Suitable for workload warm-up, cache warm-up                   │
  │                                                                   │
  │  Scheduler::run(until: InsnCount(500_000))                        │
  └──────────────────────────┬────────────────────────────────────────┘
                             │  sim.checkpoint()
                             ▼
  ╔═══════════════════════════════════════════════════════════════════╗
  ║  CHECKPOINT BLOB  (same format, timing model is not stored —      ║
  ║  the new mode is injected at restore time)                        ║
  ╚═══════════════════════════════════════════════════════════════════╝
                             │  sim.restore(blob, timing="accurate")
                             ▼
  ┌───────────────────────────────────────────────────────────────────┐
  │  ACCURATE MODE  (HelmEngine<Accurate>)                            │
  │                                                                   │
  │  - Cycle-level pipeline model per MicroarchProfile                │
  │  - Out-of-order / in-order as configured                          │
  │  - Full cache + TLB timing                                        │
  │  - Slowest; used only for the ROI of interest                     │
  │                                                                   │
  │  Scheduler::run(until: InsnCount(10_000))                         │
  │  sim.stats()  ──►  collect IPC, cache miss rate, etc.             │
  └───────────────────────────────────────────────────────────────────┘

  Speed vs. Fidelity tradeoff:
  Virtual ──────────────────────────────────────────────► Accurate
  (fastest, least detail)                         (slowest, full detail)
```

**What this shows.** The checkpoint blob is timing-model-agnostic: it captures architectural state (registers, memory, device registers, event queue) but not microarchitectural state. When `restore()` is called with a new timing parameter, a fresh `HelmEngine<NewMode>` is constructed and attached to the restored world. The CPU and device states are preserved; only the timing substrate changes.

**Key design decisions visible here.**
- Because `HelmEngine<T>` is generic and the checkpoint does not include `T`'s internal state, switching modes costs only the `restore()` call. There is no runtime polymorphism on the hot path after the switch.
- The three-phase fast-forward → warm-up → ROI pattern is a standard methodology from academic full-system simulation. helm-ng makes it a first-class workflow rather than an afterthought.
- `MicroarchProfile` is the shared configuration object that parameterizes both `Interval` and `Accurate` modes, so a user can calibrate a profile once and use it at both fidelity levels.

---

## 8. HelmEventBus Subscription Model

This diagram shows how components subscribe to the event bus at finalization time and how events are dispatched synchronously during simulation.

```
  ─── elaborate() ───────────────────────────────────────────────────────────
  Subscriber registration (one-time, during finalize / all_finalized):

  TraceLogger ──► bus.subscribe(EventClass::InsnExecuted, handler)
  GdbServer   ──► bus.subscribe(EventClass::InsnExecuted, handler)
  GdbServer   ──► bus.subscribe(EventClass::MemAccess,    handler)
  GdbServer   ──► bus.subscribe(EventClass::IrqTaken,     handler)
  Python cb   ──► bus.subscribe(EventClass::IrqTaken,     handler)
  Python cb   ──► bus.subscribe(EventClass::Custom(42),   handler)
  UserDevice  ──► bus.subscribe(EventClass::MemAccess,    handler)

  ─── sim.run() ─────────────────────────────────────────────────────────────
  Event firing (per instruction / per device action):

  HelmEngine                  HelmEventBus
  ──────────                  ────────────
       │                           │
       │  fire(InsnExecuted{pc,    │
       │        insn, cycles})     │
       │──────────────────────────►│
       │                           │  dispatch to all InsnExecuted subscribers
       │                           │──────────────────────────────────────────►  TraceLogger::on_event()
       │                           │──────────────────────────────────────────►  GdbServer::on_event()
       │                           │                                              (check breakpoints)
       │                           │◄──────────────────────────────────────────
       │◄──────────────────────────│  all handlers returned (synchronous)
       │                           │
       │  fire(MemAccess{paddr,    │
       │        rw, size})         │
       │──────────────────────────►│
       │                           │──────────────────────────────────────────►  GdbServer::on_event()
       │                           │                                              (check watchpoints)
       │                           │──────────────────────────────────────────►  UserDevice::on_event()
       │◄──────────────────────────│
       │                           │
       │  fire(IrqTaken{irq, cpu}) │
       │──────────────────────────►│
       │                           │──────────────────────────────────────────►  TraceLogger::on_event()
       │                           │──────────────────────────────────────────►  Python callback
       │◄──────────────────────────│

  ─── Custom events ─────────────────────────────────────────────────────────
  Device fires Custom(42) event ──► only subscribers of Custom(42) notified
  Others are unaffected.
```

**What this shows.** The `HelmEventBus` is a synchronous, in-process pub/sub dispatcher. Subscriptions are registered once at elaborate time; during simulation, `fire()` immediately calls all registered handlers in subscription order before returning to the caller. There is no queue, no thread crossing, and no copy of the event beyond the current stack frame.

**Key design decisions visible here.**
- Synchronous dispatch means handlers see the exact machine state at the moment of the event. A GdbServer breakpoint handler can halt execution immediately; a Python callback can read registers and they will reflect the instruction that just executed.
- Handlers are registered by `EventClass`, not by source. The bus does not know or care which component fired the event — this decouples device authors from observer authors.
- Custom event classes (numeric IDs) allow user-defined device models to define their own event types and let Python scripts subscribe to them, without modifying the core `EventClass` enum.

---

## 9. Multi-ISA Dispatch Path

This diagram shows how a single `HelmEngine::step()` call fans out to the correct ISA decode/execute implementation.

```
  HelmEngine<T>::step()
        │
        │  reads self.isa (ISA enum set at construction)
        ▼
  ┌─────────────────────────────────────────────────────────────┐
  │  match self.isa                                             │
  │                                                             │
  │  ├─ ISA::RiscV32  ──►  step_riscv(ctx, XLEN::X32)          │
  │  ├─ ISA::RiscV64  ──►  step_riscv(ctx, XLEN::X64)          │
  │  ├─ ISA::AArch64  ──►  step_aarch64(ctx)                   │
  │  └─ ISA::AArch32  ──►  step_aarch32(ctx)                   │
  └──────────┬──────────────────────────────────────────────────┘
             │  (example: RiscV64 path)
             ▼
  ┌────────────────────────────────────────────────────────────────────┐
  │  helm-arch / src/riscv/                                            │
  │                                                                    │
  │  step_riscv(ctx: &mut ExecContext, xlen: XLEN)                     │
  │    │                                                               │
  │    │  1. fetch: ctx.fetch_insn(ctx.pc())  ──► raw u32/u16         │
  │    │                                                               │
  │    │  2. decode: decode_riscv(raw)                                 │
  │    │       ├─ compressed (16-bit C extension) ──► expand           │
  │    │       └─ standard  (32-bit)              ──► DecodedInsn     │
  │    │                                                               │
  │    │  3. execute: match DecodedInsn                                │
  │    │       ├─ ADD  ──► exec_add(ctx)                               │
  │    │       ├─ LOAD ──► exec_load(ctx)  ──► ctx.load(vaddr, size)  │
  │    │       ├─ JAL  ──► exec_jal(ctx)   ──► ctx.set_pc(target)     │
  │    │       ├─ ECALL──► ctx.syscall(SyscallAbi::RiscV)             │
  │    │       └─ ...                                                  │
  │    │                                                               │
  │    │  4. advance: ctx.set_pc(ctx.pc() + insn_len)                 │
  └────┴───────────────────────────────────────────────────────────────┘
             │
             │  ExecContext methods (defined in helm-core)
             ▼
  ┌────────────────────────────────────────────────────────────────────┐
  │  ExecContext (hot, per-instruction)                                │
  │                                                                    │
  │  fetch_insn(vaddr)  ──► TlbModel ──► CacheModel ──► data          │
  │  load(vaddr, size)  ──► TlbModel ──► CacheModel ──► data          │
  │  store(vaddr, data) ──► TlbModel ──► CacheModel                   │
  │  reg(n)             ──► ArchState register file                    │
  │  set_reg(n, v)      ──► ArchState register file                    │
  │  set_pc(v)          ──► updates PC in ArchState                    │
  │  syscall(abi)       ──► SyscallHandler dispatch                    │
  └────────────────────────────────────────────────────────────────────┘
             │
             │  SyscallHandler (helm-core trait, impl in helm-engine/se/)
             ▼
  ┌────────────────────────────────────────────────────────────────────┐
  │  LinuxSyscallHandler (helm-engine/se/)                             │
  │                                                                    │
  │  dispatch(SyscallAbi::RiscV, ctx)                                  │
  │    ├─ read syscall number from a0                                  │
  │    ├─ look up in FdTable / host OS                                 │
  │    └─ write return value to a0                                     │
  └────────────────────────────────────────────────────────────────────┘
```

**What this shows.** ISA selection is a one-time match at the top of `step()`, after which execution follows a purely linear path through fetch → decode → execute with no further ISA branching. The `ExecContext` interface is ISA-neutral, so the memory subsystem, register file, and syscall handler are all shared across ISAs.

**Key design decisions visible here.**
- The ISA match is over a simple enum, not a vtable. The compiler can inline each branch independently. In a build targeting a single ISA, the other branches are eliminated by dead-code removal.
- `ExecContext` is the narrow waist of the system: ISA execute functions call only its methods, and those methods are defined in `helm-core`. This means `helm-arch` has no direct dependency on `helm-memory`, `helm-timing`, or `helm-devices` — only on `helm-core`.
- `SyscallAbi` carries the ISA-specific calling convention information so that `LinuxSyscallHandler` can extract arguments without needing to know the ISA itself.

---

## 10. Plugin Device Loading Sequence

This diagram shows how an externally compiled device shared library is loaded, registered, and made available to Python configuration scripts.

```
  User (Python config or CLI)
        │
        │  sim.load_plugin("libmydevice.so")
        ▼
  ┌─────────────────────────────────────────────────────────────┐
  │  HelmSim::load_plugin()  (helm-python, PyO3 boundary)       │
  │                                                             │
  │  dlopen("libmydevice.so")  ──► OS loads shared library      │
  │  dlsym("helm_device_register")  ──► fn pointer              │
  └──────────────────────────────┬──────────────────────────────┘
                                 │  call helm_device_register(registry)
                                 ▼
  ┌─────────────────────────────────────────────────────────────┐
  │  libmydevice.so  (external Rust or C crate)                 │
  │                                                             │
  │  #[no_mangle]                                               │
  │  pub extern "C" fn helm_device_register(r: *mut DevReg) {  │
  │    r.register(ClassDescriptor {                             │
  │      name: "MyDevice",                                      │
  │      attrs: &[("base_addr", AttrKind::U64),                 │
  │               ("irq",       AttrKind::U32)],                │
  │      factory: || Box::new(MyDevice::default()),             │
  │    });                                                      │
  │  }                                                          │
  └──────────────────────────────┬──────────────────────────────┘
                                 │
                                 ▼
  ┌─────────────────────────────────────────────────────────────┐
  │  DeviceRegistry  (helm-devices)                             │
  │                                                             │
  │  Stores ClassDescriptor under name "MyDevice"               │
  │  factory fn is retained for World::instantiate()            │
  └──────────────────────────────┬──────────────────────────────┘
                                 │
                                 │  InterfaceRegistry injects Python class
                                 ▼
  ┌─────────────────────────────────────────────────────────────┐
  │  InterfaceRegistry  (helm-devices)                          │
  │                                                             │
  │  Generates Python wrapper class "MyDevice" from attrs list  │
  │  Injects into helm_ng Python module namespace               │
  │  Python can now: from helm_ng import MyDevice               │
  └──────────────────────────────┬──────────────────────────────┘
                                 │
                                 │  User Python config
                                 ▼
  ┌─────────────────────────────────────────────────────────────┐
  │  Python config phase                                        │
  │                                                             │
  │  dev = MyDevice(base_addr=0x4000_0000, irq=5)               │
  │    ──►  PendingObject("MyDevice", attrs={...})              │
  └──────────────────────────────┬──────────────────────────────┘
                                 │
                                 │  sim.elaborate()
                                 ▼
  ┌─────────────────────────────────────────────────────────────┐
  │  World::instantiate()                                       │
  │                                                             │
  │  Looks up "MyDevice" in DeviceRegistry                      │
  │  Calls factory() ──► Box<dyn Device>                        │
  │  Calls set_attr("base_addr", 0x4000_0000)                   │
  │  Calls set_attr("irq", 5)                                   │
  │  Calls finalize() / all_finalized()                         │
  └─────────────────────────────────────────────────────────────┘
```

**What this shows.** Plugin loading follows a strict sequence: dynamic linking → self-registration → Python class injection → user config → elaboration. At no point does the simulator have privileged knowledge of the device — it is treated identically to any built-in device once registered.

**Key design decisions visible here.**
- The `helm_device_register` entry point is a stable C ABI function. This means device libraries do not need to be compiled against the same Rust crate version as the simulator; they only need to match the C-ABI `DeviceRegistry` struct layout, which is independently versioned.
- Python class injection via `InterfaceRegistry` means plugin devices are first-class Python citizens. Users do not need a separate Python wrapper file; the attribute schema declared in `ClassDescriptor` is sufficient to generate the wrapper at load time.
- Because the factory is a closure stored in `ClassDescriptor`, the `DeviceRegistry` is the single source of truth for what devices exist. `World::instantiate()` never imports from any device module directly — it only calls into the registry.

---

# helm-core Design Questions — Enriched

## Architecture (Q1–Q6)

---

### Q1: Should `ArchState` be ISA-generic (one struct for all ISAs) or ISA-specific (separate `RiscvArchState`, `Aarch64ArchState`)?

**Answer:** ISA-generic trait with ISA-specific implementations.

**Context**

`ArchState` is the root of everything the CPU model touches per thread: registers, PC, flags, privilege level. If this type is monolithic and ISA-coupled (like QEMU's `CPUArchState`), adding a new ISA means forking the entire execution pipeline. A trait-based design lets each ISA own its state layout while the engine, debugger, and stats subsystems program against the trait — enabling true multi-ISA support without conditional compilation sprawl. This choice propagates upward to `ExecContext`, `ThreadContext`, and the fetch/decode/execute pipeline: every generic bound in those layers traces back to `ArchState`.

**Options & Trade-offs**

| Option | Pros | Cons | Used By |
|---|---|---|---|
| ISA-generic trait (`trait ArchState`) | Multi-ISA support; engine stays ISA-agnostic; testable per ISA in isolation | Trait object or monomorphization cost; more boilerplate per ISA | gem5 (`ThreadContext` trait), SIMICS (`conf_object_t` + interface registration) |
| ISA-specific flat structs, no trait | Zero abstraction overhead; dead-simple layout; mirrors hardware ABI exactly | One engine implementation per ISA; no shared infrastructure; combinatorial explosion with new ISAs | QEMU (`CPUArchState` — `CPUX86State`, `CPURISCVState` are unrelated C structs) |
| Union/enum wrapping all ISAs | Single type; no generics in the engine | Enum match on every access; wasted memory; adding an ISA requires touching core union | Rare; some embedded simulators use tagged unions for 2–3 ISAs |

**Rationale**

Gem5 demonstrates that a trait/interface approach (`ThreadContext`, `ExecContext`) cleanly decouples ISA-specific state from the CPU model pipeline without sacrificing performance — static dispatch via template parameter eliminates vtable cost. QEMU's flat-struct approach achieves raw speed but at the cost of being architecturally impossible to extend without forking the TCG backend per ISA.

**Impact**

`helm-core` (ExecContext, ThreadContext bounds), `helm-engine` (generic dispatch), `helm-debug` (register read/write via trait), `helm-stats` (PC sampling), `helm-python` (ISA introspection API).

---

### Q2: How are floating-point registers stored — as `[f64; 32]` (typed) or `[u64; 32]` (bit-cast on use)?

**Answer:** `[u64; 32]` with bit-cast on use, following IEEE-754 and the RISC-V NaN-boxing rules.

**Context**

The RISC-V ISA specification stores all floating-point registers as 64-bit values regardless of the operation width. A 32-bit float written to an FP register must NaN-box the upper 32 bits (fill with 1s); reading a 32-bit float requires validating that the upper bits are all 1s or the value is treated as the canonical NaN. Storing as `[f64; 32]` destroys this bit-level fidelity because Rust's `f64` type does not preserve NaN payload bits across operations. AArch64 has 128-bit SIMD/FP registers (Q registers) viewed as D (64-bit), S (32-bit), H (16-bit), or B (8-bit) — again requiring raw bit storage.

**Options & Trade-offs**

| Option | Pros | Cons | Used By |
|---|---|---|---|
| `[u64; 32]` (raw bits, cast on use) | Spec-correct NaN-boxing; portable across Rust toolchains; handles SIMD views cleanly | Requires explicit cast helpers; slightly less readable instruction code | gem5 (`RegVal` as `uint64_t`), QEMU (`fpregs` as `FPReg` union), RISC-V spec |
| `[f64; 32]` (typed float) | Readable; no manual casting; direct use in arithmetic | Rust may canonicalize NaN payloads; breaks NaN-boxing invariant; cannot represent SIMD views | None among major simulators for RISC-V or AArch64 |
| `[u128; 32]` (full SIMD width) | Correct for AArch64 Q registers natively; future-proof for vector extensions | Wasteful for RV32/RV64 base ISA; wider than necessary for most ops | Some AArch64-only simulators, SIMICS internal AArch64 model |

**Rationale**

The RISC-V Unprivileged ISA spec (§11.2) mandates NaN-boxing for scalar FP values written into wider registers — this is a correctness requirement, not an optimization. Storing as `u64` and casting at instruction boundaries is the only representation that correctly round-trips all bit patterns including signaling NaNs and NaN payloads that software may inspect.

**Impact**

`helm-core` (register file layout, FP read/write helpers), ISA decode crates (every FP instruction), `helm-debug` (register display and GDB `p/f` format), `helm-stats` (FP operation profiling).

---

### Q3: CSRs (RISC-V) and system registers (AArch64) have fundamentally different access patterns. Should `CsrFile` be a flat array with index dispatch, or a sparse `HashMap<u16, u64>`?

**Answer:** ISA-specific implementation — not abstracted at the architecture level.

**Context**

RISC-V has a 12-bit CSR address space (4096 possible indices) but most implementations populate fewer than 100 registers; the spec mandates that accessing an unimplemented CSR raises an illegal-instruction exception. AArch64 system registers use a 5-field encoding (`op0:op1:CRn:CRm:op2`) yielding ~700 architecturally defined registers, plus implementation-defined ones, with per-EL banking (the same register name may resolve to different physical storage depending on the current exception level). These constraints make a shared `CsrFile` abstraction misleading — the access semantics, address encoding, and banking rules are fundamentally different per ISA.

**Options & Trade-offs**

| Option | Pros | Cons | Used By |
|---|---|---|---|
| Flat array `[u64; 4096]` (RISC-V) | O(1) access; cache-friendly; simple bounds check | Wastes ~32 KB for sparse implementations; no hook point for side-effects | gem5 RISC-V (`isaReg` array with 4096 slots) |
| Sparse `HashMap<u16, u64>` | Memory-efficient for sparse sets; easy to model unimplemented = absent | Hash overhead on every CSR access (CSR instructions are not rare in OS workloads); HashMap is not cache-friendly | SIMICS AArch64 system register model |
| Match-dispatch (Rust `match` on CSR index) | Zero allocation; inlineable; side-effect hooks per register are natural | Verbose; every new register requires a code change; not data-driven | Many lightweight RISC-V emulators (rv32emu, riscv-emu-rust) |
| ISA-specific struct (no shared abstraction) | Each ISA optimizes independently; correct semantics without compromise | No shared interface means debugger/tools must know ISA | **Selected approach** — parallels gem5's per-ISA ISA object design |

**Rationale**

Gem5 uses a flat array for RISC-V CSRs (fast, bounded) and a completely separate 650+ entry flat array for AArch64 system registers with banking logic layered on top. SIMICS uses a sparse map for AArch64 because its register discovery model is dynamic. There is no single best structure — the ISA's access pattern, banking semantics, and side-effect requirements determine the right data structure, validating the decision to keep this ISA-specific.

**Impact**

`helm-core` (excluded by design — no shared `CsrFile` trait), RISC-V ISA crate (owns `RiscvCsrFile`), AArch64 ISA crate (owns `Aarch64SysRegFile`), `helm-debug` (must query CSRs via ISA-specific path), privilege/trap handling in each ISA crate.

---

### Q4: Should `ExecContext` (hot path) be a trait or a concrete struct?

**Answer:** Trait — decoupled architecture; dispatched via static (generic) dispatch, not `dyn`.

**Context**

`ExecContext` is invoked for every instruction executed — in a performant simulator running at hundreds of MIPS, even nanosecond-level overhead per call accumulates to seconds of wall time per benchmark. The choice between trait and concrete struct determines whether the instruction implementations can be compiled as a single monomorphized unit (zero dispatch overhead) or must pay a vtable lookup per call. Beyond performance, making `ExecContext` a trait is what enables the CPU model to be swapped — functional, timing, and trace-replay modes can each implement the trait differently without changing a single instruction decode function.

**Options & Trade-offs**

| Option | Pros | Cons | Used By |
|---|---|---|---|
| Trait + static dispatch (generic `<C: ExecContext>`) | Zero runtime overhead after monomorphization; full inlining across instruction/context boundary; mode-swappable | Longer compile times; larger binary (one copy per instantiation) | gem5 (`ExecContext` as template parameter — `execute(ExecContext *xc)` instantiated per CPU model) |
| Trait + dynamic dispatch (`dyn ExecContext`) | Single compiled copy; easy to store in collections | Vtable call per method — measurable overhead at billions of calls/sec; kills inlining | Not used in any major performance simulator for the hot path |
| Concrete struct (no trait) | Simplest; maximum inlining; smallest binary | Cannot swap CPU models or simulation modes without conditional compilation; untestable in isolation | QEMU (monolithic `cpu_exec` function, mode via global state) |

**Rationale**

Gem5's design is the clearest existence proof: `ExecContext` is a C++ abstract class used only as a template parameter, never as a virtual base in the hot path. This gives the decode/execute logic full inlining while the CPU model (SimpleThread, O3CPU, MinorCPU) provides the concrete implementation. Rust's monomorphization achieves the same result with `impl<C: ExecContext>` — the trait is the API contract; the generic parameter is the zero-cost dispatch mechanism.

**Impact**

`helm-engine` (generic bound on execute loop), all ISA decode crates (every instruction `fn execute<C: ExecContext>`), `helm-timing` (TimingExecContext implements trait), `helm-debug` (tracing wrapper implements trait), test infrastructure (mock `ExecContext` for unit tests).

---

### Q5: How does `ExecContext::read_mem` return a `MemFault`? As `Result<u64, MemFault>` or by calling `raise_exception()` directly?

**Answer:** Return `Result<u64, MemFault>` — the instruction handler performs ISA-specific exception mapping.

**Context**

Memory faults (page faults, access faults, misaligned access, PMP violations in RISC-V, translation faults in AArch64) must be converted to ISA-specific exceptions with ISA-specific syndrome information. If `read_mem` calls `raise_exception()` directly, `ExecContext` must contain ISA-specific exception encoding logic — a layering violation that couples the memory interface to every ISA's exception model. Returning `Result` keeps `read_mem` ISA-agnostic: the fault propagates to the instruction handler which knows how to map `MemFault::PageFault` to a RISC-V `LoadPageFault` with the correct `tval`, or to an AArch64 `DataAbort` with the correct ESR syndrome word.

**Options & Trade-offs**

| Option | Pros | Cons | Used By |
|---|---|---|---|
| `Result<T, MemFault>` — caller maps to exception | Clean separation; memory system stays ISA-agnostic; easy to test fault injection; functional and timing modes handled uniformly | Every instruction that touches memory must propagate `?` or match; slightly more boilerplate | gem5 (fault returned as `Fault` object from `readMem`; CPU model calls `fault->invoke()`) |
| `raise_exception()` direct call inside `read_mem` | Instruction code is simpler (no `?`); fault is always handled | `ExecContext` must know ISA exception encoding; memory and exception systems entangled; hard to unit-test | QEMU (longjmp-based `cpu_loop_exit_restore` — effectively a non-local exit, not a return value) |
| Callback/closure on fault | Flexible; caller provides fault handler inline | Complex API; closure capture adds overhead on every call; unusual in systems code | Not common in major simulators |

**Rationale**

Gem5's approach — returning a `Fault` object that the CPU model's execute loop then invokes — is the cleanest separation seen in production simulators. The RISC-V spec's fault taxonomy (instruction/load/store × misaligned/access/page) maps naturally to a `MemFault` enum whose variants carry the faulting address. Returning `Result` is idiomatic Rust and enables `?` propagation through instruction handlers without hidden control flow.

**Impact**

`helm-memory` (`MemInterface::read`/`write` return type), `helm-core` (`ExecContext` trait signature), all ISA decode crates (fault → exception mapping per instruction class), `helm-debug` (fault injection testing), `helm-timing` (timing model must still return fault before committing side effects).

---

### Q6: What is the exact split between `ExecContext` (hot) and `ThreadContext` (cold)?

**Answer:** Separate traits with no inheritance; `RiscvHart`/`Aarch64Hart` implement both; syscall emulation receives only `&mut dyn ThreadContext`. Exact method allocation is now fully specified below.

**Context**

The distinction matters because everything in `ExecContext` is called in the instruction execute loop — millions to billions of times per simulated second. Anything that touches that interface pays the monomorphization cost and constrains inlining. `ThreadContext` covers operations that happen at thread-management granularity: context switches, debugger register reads, checkpoint save/restore, OS syscall emulation hooks. Conflating the two forces the execution engine to carry cold-path state in its hot generic parameter, bloating instruction-level code with methods it never calls in steady state.

**Options & Trade-offs**

| Option | Pros | Cons | Used By |
|---|---|---|---|
| Separate traits, no inheritance (`ExecContext` + `ThreadContext`) | Clean boundary; hot path carries only what it needs; cold path accessible via separate handle | Two trait bounds in code that needs both (e.g., syscall emulation during execution) | gem5 (separate `ExecContext` and `ThreadContext` — accessed via `xc->tcBase()` pointer, NOT inheritance) |
| `ThreadContext: ExecContext` (supertrait) | Single handle for all operations; no pointer indirection | Every `ThreadContext` implementor must implement the full hot-path API; blurs the performance boundary | Some simpler simulators; the originally proposed approach |
| Single unified struct (no trait split) | Simplest implementation; no dispatch | Cannot swap hot-path implementations independently; timing vs. functional modes differ only in hot-path behavior | QEMU (monolithic `CPUState`) |

**Rationale**

Gem5's source confirms (from `exec_context.hh` and `thread_context.hh`) that the two interfaces are separate and that `tcBase()` on an `ExecContext` gives a pointer to its `ThreadContext` when the cold path is needed. Critically, gem5's syscall emulation (`syscall_emul.hh`) receives only a `ThreadContext *tc` parameter — it never uses `ExecContext`. The ECALL/SVC instruction handler raises a fault to exit the execute loop; the engine dispatches to the syscall handler with a `ThreadContext` reference. This pattern eliminates the "dual-access problem" — `ExecContext` and `ThreadContext` are never needed simultaneously in the same function.

**Exact Method Allocation for helm-ng**

`ExecContext` — hot path, monomorphized generic `H: ExecContext`. Every method called per instruction cycle:

```rust
pub trait ExecContext {
    // Integer registers — called on nearly every instruction
    fn read_ireg(&self, reg: IReg) -> u64;
    fn write_ireg(&mut self, reg: IReg, val: u64);

    // Floating-point registers — called on FP instructions
    fn read_freg(&self, reg: FReg) -> u64;          // raw u64 bits (NaN-box invariant)
    fn write_freg(&mut self, reg: FReg, val: u64);

    // PC — called every instruction
    fn read_pc(&self) -> u64;
    fn write_next_pc(&mut self, val: u64);           // sets PC for NEXT cycle

    // CSR (RISC-V) / system registers (AArch64) — called on CSR instructions only
    fn read_csr(&self, csr: u16) -> Result<u64, CsrFault>;
    fn write_csr(&mut self, csr: u16, val: u64) -> Result<(), CsrFault>;

    // Privilege level — needed for address translation mode selection
    fn privilege_level(&self) -> PrivilegeLevel;

    // Exception entry — called on traps, faults, ECALL; unwinds execute loop
    fn raise_exception(&mut self, cause: ExceptionCause) -> !;

    // SC failure counter — RISC-V atomics
    fn read_sc_failures(&self) -> u32;
    fn write_sc_failures(&mut self, n: u32);
}
```

`ThreadContext` — cold path, always `&mut dyn ThreadContext`. Never called per instruction:

```rust
pub trait ThreadContext {
    // Identity
    fn hart_id(&self) -> u32;
    fn isa(&self) -> Isa;

    // Full register file — for GDB read/write, checkpoint, context switch
    fn read_ireg_raw(&self, idx: usize) -> u64;         // idx 0..32
    fn write_ireg_raw(&mut self, idx: usize, val: u64);
    fn read_freg_raw(&self, idx: usize) -> u64;
    fn write_freg_raw(&mut self, idx: usize, val: u64);

    // PC — direct set (GDB, checkpoint, context switch)
    fn read_pc(&self) -> u64;
    fn set_pc(&mut self, val: u64);

    // Privilege level — for GDB, checkpoint, OS context switch
    fn privilege_level(&self) -> PrivilegeLevel;
    fn set_privilege_level(&mut self, pl: PrivilegeLevel);

    // CSR/system-register raw access — no side effects, for checkpoint and GDB
    fn read_csr_raw(&self, csr: u16) -> u64;
    fn write_csr_raw(&mut self, csr: u16, val: u64);

    // Syscall ABI convenience — SE mode only; reads args, writes return value
    fn syscall_args(&self) -> SyscallArgs;              // reads a0-a7 / x0-x7 per ISA ABI
    fn set_syscall_return(&mut self, val: i64);         // writes a0 / x0

    // Lifecycle — activate/suspend for multi-hart scheduling
    fn status(&self) -> HartStatus;
    fn activate(&mut self);
    fn suspend(&mut self);
    fn halt(&mut self);

    // Checkpoint hooks — called by CheckpointManager
    fn save_attrs(&self, store: &mut AttrStore);
    fn restore_attrs(&mut self, store: &AttrStore);
}
```

**Dual-access resolution for SE syscall emulation**

The ECALL instruction execute arm calls `ctx.raise_exception(ExceptionCause::Ecall)` — this is a diverging call that unwinds back to the engine loop via `StopReason::Syscall`. The engine then calls `syscall_handler.handle(&mut hart.as_thread_context(), &mut mem_map)`. The syscall handler **only ever sees `&mut dyn ThreadContext`** — it reads `ctx.syscall_args()`, dispatches, and calls `ctx.set_syscall_return(val)`. `ExecContext` is never needed inside the syscall handler. This matches gem5's pattern exactly: `syscall_emul.hh` handlers receive `ThreadContext *tc`, never an `ExecContext *`.

**Implementing both on the same struct**

`RiscvHart` and `Aarch64Hart` implement both traits on the same struct. The engine holds:
- `hart: H` where `H: ExecContext` — monomorphized, used in the hot execute loop
- `hart.as_thread_context() -> &mut dyn ThreadContext` — a method that returns `self` as a trait object for cold-path callers

This is the same pattern as gem5's `SimpleThread`, which implements `ThreadContext` on the concrete hart struct. The `as_thread_context()` method is `#[inline(never)]` to keep it off the hot path.

**Impact**

`helm-core` (both trait definitions), `helm-engine` (execute loop bound `H: ExecContext`; scheduler/syscall dispatch calls `as_thread_context()`), `helm-arch` (both traits implemented on `RiscvHart` and `Aarch64Hart`), `helm-debug` (uses `&mut dyn ThreadContext` exclusively — no `ExecContext`), `helm-engine/se` (syscall handler receives `&mut dyn ThreadContext`).

---

## Memory Interface (Q7–Q9)

---

### Q7: Does `MemInterface` live in `helm-core` or `helm-memory`?

**Answer:** `helm-memory` owns `MemInterface`; `helm-core` depends on it via trait import.

**Context**

`MemInterface` is the contract that `ExecContext::read_mem` and `write_mem` delegate to. Where it lives determines the dependency graph: if it lives in `helm-core`, then `helm-memory` must depend on `helm-core` (or they form a cycle). If it lives in `helm-memory`, then `helm-core` takes a dependency on `helm-memory` — but the trait is defined where the implementations live, which is the conventional Rust pattern (trait defined in the same crate as its primary implementor). A third option — a thin `helm-mem-interface` crate containing only the trait — avoids cycles entirely at the cost of another crate boundary.

**Options & Trade-offs**

| Option | Pros | Cons | Used By |
|---|---|---|---|
| `MemInterface` in `helm-memory` | Trait and implementations co-located; standard Rust convention; `helm-core` depends on `helm-memory` | `helm-core` now has a non-trivial dependency; harder to build `helm-core` standalone | Most Rust simulator crates (trait lives with implementations) |
| `MemInterface` in `helm-core` | `helm-core` standalone; memory crate depends on core | `helm-memory` must depend on `helm-core`; `helm-core` contains memory concepts it doesn't own conceptually | gem5 (memory port interfaces defined in `mem/` which `cpu/` depends on) |
| Thin `helm-mem-interface` crate (trait only) | Zero cycles; each crate depends only on the interface crate | Extra crate; more `Cargo.toml` churn; common pattern only in large workspaces | Some large Rust workspaces (tokio's `io-util` split, `bytes` crate separation) |

**Rationale**

The cleaner separation is for `helm-memory` to own `MemInterface` — this keeps the memory subsystem's contract, implementations (functional store, cache hierarchy, timing model), and trait in one place. `helm-core`'s `ExecContext` takes a generic `M: MemInterface` parameter from `helm-memory`, which is a clean dependency direction (execution depends on memory, not the reverse).

**Impact**

`Cargo.toml` dependency graph for all crates, `helm-core` (`ExecContext` generic bound `M: MemInterface`), `helm-engine` (wires concrete memory implementation to `ExecContext`), `helm-timing` (provides timing `MemInterface` implementation), test harnesses (mock `MemInterface` must be importable without pulling in full `helm-memory`).

---

### Q8: For timing mode, who owns the "in-flight request" state — CPU or memory system?

**Answer:** Split by pipeline stage — fetch/decode state lives in the CPU pipeline; cache/memory in-flight request state lives in the memory system.

**Context**

In a timing-accurate simulation, a memory request is not instantaneous: it occupies pipeline registers (the CPU knows it is "waiting for memory"), cache MSHR entries (the cache tracks the outstanding miss), and bus/interconnect slots (the memory system knows the request is in flight). Ownership of each piece of state must follow the component that can make progress decisions based on it. The CPU pipeline stalls based on whether its load/store unit has a pending result — that is CPU state. The cache hierarchy tracks MSHR (Miss Status Holding Register) entries and arbitrates DRAM banks — that is memory system state.

**Options & Trade-offs**

| Option | Pros | Cons | Used By |
|---|---|---|---|
| CPU owns all in-flight state | Simple: one place to look for request status; no coordination protocol | CPU must model MSHR, bus queues, cache state — couples CPU model to memory microarchitecture | Some simple in-order simulators |
| Memory system owns all in-flight state | Correct layering; cache/DRAM model is self-contained; CPU only sees "pending" vs. "complete" | CPU must poll or receive callbacks; event/callback protocol required between CPU and memory | gem5 (Ports + PacketQueue; CPU sends packet, memory system calls `recvTimingResp` callback) |
| Split ownership (selected) | Each component owns state it acts on; natural fit for event-driven simulation | Requires clear interface for "request submitted" / "response ready" events | gem5 (effectively this — CPU pipeline state in `LSQ`, MSHR in `Cache`); SIMICS timing model |

**Rationale**

Gem5's port-based design demonstrates the split: the CPU's Load-Store Queue (`LSQ`) tracks which instructions are waiting for memory responses (CPU-side state), while the cache's MSHR tracks outstanding misses and the DRAM controller tracks row/bank state (memory-side state). The boundary is the port interface — a clean callback protocol. Helm-ng's `helm-timing` and `helm-engine` should mirror this split, with `helm-memory` owning MSHR-equivalent state and the CPU pipeline owning instruction-level pending state.

**Impact**

`helm-timing` (CPU pipeline state machine), `helm-memory` (MSHR / in-flight request tracking), `helm-event` (event queue for request/response callbacks), `helm-engine` (coordinates timing between CPU and memory event loops), `helm-stats` (latency measurement crosses this boundary).

---

### Q9: How does functional mode guarantee no side effects (cache state updates)?

**Answer:** Functional accesses bypass the cache hierarchy entirely, going directly to the backing physical memory store. Cache state is never modified.

**Context**

Functional mode exists for initialization (loading ELF segments), debugger memory reads/writes, OS syscall emulation, and checkpoint restore — all cases where the caller wants the correct value at an address without perturbing the simulated cache state. If a functional read accidentally triggers a cache fill, it corrupts the timing simulation: cache hit/miss rates become wrong, MSHR state may be modified, and prefetcher training data is polluted. Guaranteeing no side effects requires that the functional access path never touches cache data arrays or replacement metadata.

**Options & Trade-offs**

| Option | Pros | Cons | Used By |
|---|---|---|---|
| Bypass cache — go directly to backing store | Cache state never touched; simple and correct; trivially testable | Does not reflect what the CPU would see if the cache is dirty (stale data risk if cache has dirty lines not yet flushed) | gem5 (`sendFunctional(pkt)` — packet traverses cache hierarchy but `isFunctional()` check causes cache to forward without updating state; ultimately hits backing store) |
| Flush cache before functional access | Always coherent — cache and memory agree | Enormous overhead; destroys simulation state; unacceptable for debugger use | Not used in production simulators |
| Peek into cache, fall back to memory | Returns what the CPU would actually see (respects dirty cache lines) | Complex coherence logic; cache must be consulted in read path; write path still problematic | gem5 functional actually does this — cache `recvFunctional` checks if it holds the line and responds if so (snooping path) |

**Rationale**

Gem5's `sendFunctional` mechanism is the clearest documented solution: functional packets travel the same physical path as timing packets but every component checks `pkt->isFunctional()` and responds without updating replacement policy, prefetcher state, or MSHR entries. The cache will snoop its own data arrays to return dirty data if present, but will not count the access as a hit or trigger a fill. This means functional reads are coherent (they see dirty cache lines) but non-invasive (they leave no trace). Helm-ng should adopt the same packet-type-flag approach in `helm-memory`.

**Impact**

`helm-memory` (functional vs. timing access path split; cache snoop logic for dirty-line handling), `helm-debug` (all debugger memory access goes through functional path), OS/ELF loader (`helm-engine` initialization uses functional writes), checkpoint save/restore (reads architectural memory state without disturbing simulation), `helm-stats` (must not count functional accesses in cache statistics).

---

# Design Questions: Engine & Architecture (Q10–Q24)

> Enriched design questions for `helm-engine` and `helm-arch` subsystems.
> Each question includes context, an options/trade-offs table, an answer, a rationale, and an impact line.

---

## Q10 — `HelmEngine<T: TimingModel>`: ownership of `ArchState` and `MemoryMap`

**Context**

`HelmEngine<T>` is the central per-hart execution engine. It must hold or reference both the architectural register state (`ArchState`) and the address-space map (`MemoryMap`). The choice of ownership vs. borrowing has cascading effects: checkpoint scope (can you serialize the engine alone?), Python API ergonomics (can the Python layer read register state without going through the engine?), and multi-hart sharing (two harts cannot both own the same `MemoryMap`). Gem5 models each CPU as owning its own `ArchState`; QEMU's `CPUArchState` is embedded in the `CPUState` struct (owned). Both share the memory map via a reference/port model rather than ownership — QEMU passes `AddressSpace *` pointers; Gem5 connects via ports. SIMICS similarly keeps per-vCPU register state owned and the address space referenced.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Engine owns both `ArchState` and `MemoryMap` | Single checkpoint scope, simple borrow checker story | Two harts cannot share one `MemoryMap`; duplication | — |
| Engine owns `ArchState`, borrows `MemoryMap` | Harts share the map; checkpoint ArchState only | Lifetime parameters (`'mem`) on engine struct; complicates `Arc` wrapping | Gem5, QEMU, SIMICS |
| Both externally owned; engine holds `Arc<Mutex<...>>` | Full multi-hart sharing, no lifetimes | Lock contention on hot path; checkpoint must coordinate externally | QEMU MTTCG (partial) |
| Both externally owned; engine holds raw references | Zero-cost access | Unsafe; lifetime unsoundness risk | — |

**Answer:** `HelmEngine<T>` owns `ArchState` and borrows (or holds `Arc` of) `MemoryMap`.

**Rationale:** Register state is intrinsically per-hart and must be checkpointed with the hart. The memory map is intrinsically shared in a multi-hart system — owning it per-engine would require full duplication or a reference-counted wrapper anyway. Using `Arc<MemoryMap>` (or `Arc<RwLock<MemoryMap>>`) avoids lifetime parameters on `HelmEngine` while preserving sharing semantics. `ArchState` is small enough (< 4 KB for RV64 or AArch64) that ownership is not a burden.

**Impact:** `HelmEngine<T>` struct holds `ArchState` by value (owned) and `Arc<MemoryMap>` (shared). Checkpoint serialization targets only `ArchState` + engine-local metadata. Python inspection of register state goes through `HelmEngine::arch_state() -> &dyn ArchState`.

---

## Q11 — Does `HelmEngine` implement `SimObject`?

**Context**

`SimObject` is the component-lifecycle trait: it provides `name()`, `reset()`, `serialize()`/`deserialize()`, and hooks into the `World` component tree. If `HelmEngine` implements `SimObject`, it participates in the global checkpoint system automatically, can be looked up by name, and receives `reset()` signals from `World::reset_all()`. If it does not, checkpointing must be wired manually and the engine is invisible to the component tree. Gem5's `BaseCPU` inherits from `SimObject`; every CPU is registered and checkpointed through the same mechanism. SIMICS treats each processor as a `conf_object_t`, which is SIMICS's equivalent of `SimObject`.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `HelmEngine: SimObject + Hart + Execute` | Automatic checkpoint/reset; component tree visible; uniform lifecycle | Trait bloat; forces `dyn SimObject` boxes for heterogeneous hart lists | Gem5 (`BaseCPU`), SIMICS |
| `HelmEngine: Hart + Execute` only; `HelmSim` wraps and implements `SimObject` | `HelmEngine` stays focused; `HelmSim` is the API boundary already | Checkpoint delegation is manual; two-layer indirection | — |
| `HelmEngine` implements no traits; all via `HelmSim` | Minimal trait surface | Engine not reusable outside `HelmSim` context | — |

**Answer:** `HelmEngine` implements `Hart` and `Execute`. `HelmSim` (the outer enum/wrapper) implements `SimObject`.

**Rationale:** `HelmEngine` is a generic struct parameterized on `T: TimingModel` — making it implement `SimObject` directly would require `SimObject` to be object-safe or tie the component tree to the timing model. Delegating `SimObject` to `HelmSim` keeps the component boundary at the right layer, the same layer where the Python API lives. `HelmSim` already erases `T` for external consumers; it is the natural `SimObject` boundary.

**Impact:** `World` stores `Vec<Box<dyn SimObject>>` where each entry may be a `HelmSim`. Checkpoint calls `HelmSim::serialize()` which delegates to `HelmEngine::checkpoint_arch_state()`. `HelmEngine` can be unit-tested without a `World`.

---

## Q12 — How does `HelmSim` expose `ArchState` inspection without knowing `T`?

**Context**

`HelmSim` is the type-erasing enum (or `dyn` wrapper) that Python and the rest of the system see. It hides the concrete `HelmEngine<T>` type. But Python needs to read and write registers (`sim.read_reg("x1")`), inspect PC, and query privilege level. This requires access to `ArchState` from a context where `T` is unknown. The question is how to surface that access without leaking `T` or requiring `unsafe` downcasting. Gem5 exposes register state through the `ThreadContext` interface, which is virtual and ISA-independent at the numeric-index level. QEMU exposes registers through `cpu_get_phys_page_debug` and `gdbstub` hooks — always through an interface layer, never directly into `CPUArchState`.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `HelmSim::arch_state() -> &dyn ArchState` | Clean, object-safe; Python calls one method | `ArchState` trait must be object-safe (no generics in methods) | Gem5 (`ThreadContext`) |
| `HelmSim` stores a pre-erased `Arc<dyn ArchState>` alongside the engine | No dynamic dispatch in hot path | Arc overhead; state shared between engine and `HelmSim` | — |
| `HelmSim` exposes named-register access methods directly (`read_ireg`, `write_ireg`) | No trait needed | Combinatorial method explosion; ISA-specific methods leak into `HelmSim` | QEMU gdbstub (partial) |
| `HelmSim` exposes a `PyO3`-specific `inspect()` method returning a Python dict | Simple for Python | Not usable from Rust callers; duplicated logic | — |

**Answer:** `HelmSim` exposes `&dyn ThreadContext` via a method, where `ThreadContext` is the object-safe supertrait of `ArchState` inspection.

**Rationale:** `ThreadContext` is already the planned cold-path inspection trait (per Q6 in the core design questions). Making it object-safe (no generic methods) and having `HelmSim` return `&dyn ThreadContext` gives Python and debuggers a uniform interface. The PyO3 layer calls `sim.thread_context()` and then dispatches named reads/writes through `ThreadContext`. This avoids duplicating inspection logic.

**Impact:** `ThreadContext` trait must be object-safe — no `fn read_reg<R: Register>()` style; instead `fn read_ireg(idx: u8) -> u64`, `fn read_freg(idx: u8) -> u64`, `fn read_pc() -> u64`. `HelmSim` adds `fn thread_context(&self) -> &dyn ThreadContext` with a match arm per variant.

---

## Q13 — Is `build_simulator()` the only way to create a simulator?

**Context**

`build_simulator(isa: Isa, mode: ExecMode, timing: TimingConfig) -> HelmSim` is the proposed factory. It centralizes all construction logic, validates the combination of ISA + mode + timing, and is the single point for Python to call. The alternative is exposing `HelmEngine<Virtual>`, `HelmEngine<Interval>` etc. as directly constructible from Python via PyO3 `#[pyclass]`. The tradeoff is ergonomics vs. flexibility. Gem5 uses a Python-driven configuration system where objects are constructed by class name via reflection — flexible but complex. SIMICS uses a factory (`SIM_create_object`) as the canonical path; direct construction is unsupported from scripts.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `build_simulator()` factory only | One validation path; easy to version; PyO3 binding surface is minimal | Less flexible; can't construct partial engines for testing | SIMICS (`SIM_create_object`) |
| Direct PyO3 `#[pyclass]` on each `HelmEngine<T>` variant | Maximum flexibility; no factory needed | Combinatorial bindings; invalid combos constructable; type explosion in PyO3 | — |
| Factory + internal `unsafe` builder for test code | Clean public API; tests bypass validation | Unsafe in test code; factory diverges from test construction path | — |
| Factory + a `HelmEngineBuilder` fluent API in Rust | Ergonomic for Rust callers; Python uses factory | Two construction paths to maintain | Gem5 (SimObject params) |

**Answer:** `build_simulator()` is the primary (and Python-only) construction path. Rust unit tests may construct `HelmEngine<T>` directly via `pub(crate)` constructors.

**Rationale:** Exposing every `HelmEngine<T>` variant to PyO3 is not practical — it creates N × M bindings (ISAs × timing modes) and makes invalid combinations (e.g., `HelmEngine<DetailedCache>` for an ISA not yet supported in timing mode) constructable from Python without validation. The factory is the single place to enforce invariants. Rust tests do not need PyO3 ergonomics and can use crate-internal constructors.

**Impact:** `build_simulator` is a `#[pyfunction]` in the PyO3 module init. `HelmEngine<T>` constructors are `pub(crate)` or `pub` only within `helm-engine`. The factory is the sole source of truth for which (ISA, mode, timing) triples are supported.

---

## Q14 — Who owns the `Scheduler`?

**Context**

The `Scheduler` manages quantum boundaries — it decides when each hart runs, for how long, and when to synchronize. Ownership determines the API shape: if `World` owns the `Scheduler`, then `sim.run()` is `World::run()` and the user never sees the scheduler directly. If the `Scheduler` is a standalone struct, the user creates it, adds harts, and calls `scheduler.run()`. Gem5's `EventQueue` is owned by the simulation (`simulate()` function drives it). SIMICS's scheduler is internal to the kernel — users call `SIM_run_alone()` or `SIM_continue()`, never touching the scheduler directly. QEMU's main loop owns the timer and vCPU scheduling.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `World` owns `Scheduler`; `sim.run()` delegates to it | Single entry point; World coordinates reset and checkpoint with run | Less testable; scheduler not reusable outside World | Gem5, SIMICS, QEMU |
| `Scheduler` is standalone; user composes | Maximum flexibility; testable without World | Two APIs to maintain (`scheduler.run()` vs `world.run()`); sync with World state | — |
| `World` owns `Scheduler` but exposes it as `&mut Scheduler` | Scheduler configurable (quantum size) while World owns lifecycle | Borrow checker: can't hold `&mut Scheduler` and call `world.add_hart()` simultaneously | — |
| `Scheduler` is an `Arc` field of `World`, shared with harts | Harts can self-schedule | Complex; circular Arc references likely | — |

**Answer:** `World` owns the `Scheduler`. Configuration (quantum size, hart registration) happens before `world.run()` is called; the scheduler is not exposed directly after construction.

**Rationale:** The World-owns-Scheduler model is universal in production simulators for good reason: it is the only design where checkpoint, reset, and synchronization are guaranteed to be consistent. A standalone scheduler creates a coordination problem — who calls `reset()` on the scheduler when `World::reset()` fires? World ownership eliminates that question.

**Impact:** `World::add_hart(hart: Box<dyn Hart>)` registers a hart with the internal scheduler. `World::set_quantum(n: u64)` sets the quantum before running. `World::run(until: Option<u64>)` drives the scheduler loop. The `Scheduler` type is not `pub` outside `helm-engine`.

---

## Q15 — Can the scheduler be paused mid-quantum?

**Context**

Breakpoints require the simulator to stop execution at a specific instruction, which may fall in the middle of a quantum. This is a fundamental tension in temporal decoupling: the quantum is the unit of synchronization, but the breakpoint fires at instruction granularity. Three approaches exist in production simulators: QEMU uses `setjmp`/`longjmp` to exit the TCG translated block early; Gem5 schedules a zero-tick event that fires before the next instruction when a breakpoint is set; SIMICS sets a `stop_flag` checked at the quantum boundary — meaning breakpoints can be delayed by up to one quantum. The `HelmEventBus::fire(Exception)` mechanism in helm-ng must participate in whichever model is chosen.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Stop flag checked per instruction in `Execute::run()` | Simple; no `unsafe`; low overhead when no breakpoint | One extra branch per instruction always | SIMICS (quantum-boundary variant) |
| `HelmEventBus::fire(Exception)` sets a Rust `AtomicBool`; `Execute::run()` polls it | Thread-safe; no `unsafe`; works for single and multi-hart | Same per-instruction poll overhead | — |
| Early exit via Rust `?` on each instruction (`Result<(), BreakHit>`) | Idiomatic Rust; no global state | All execute methods must return `Result`; cascading change | — |
| Separate "breakpoint quantum" of size 1 when breakpoint is near | Zero overhead in normal run; exact stop | Requires lookahead or binary search for breakpoint PC | Gem5 (approximate) |

**Answer:** `Execute::run()` returns `Result<u64, StopReason>` where `StopReason` includes `Breakpoint`, `Exception`, and `QuantumEnd`. Each instruction step returns `?` to propagate early exits.

**Rationale:** The `Result`-based approach is idiomatic Rust and integrates naturally with the error-propagation model already used for memory faults. It avoids global `AtomicBool` state and makes the control flow explicit and auditable. The cost (one `?` per instruction) is negligible compared to instruction execution itself and eliminates the need for `unsafe` longjmp-style exits. `HelmEventBus::fire(Exception)` translates the exception into a `StopReason` returned from the current instruction.

**Impact:** `Execute::step() -> Result<(), StopReason>` and `Execute::run(n: u64) -> Result<u64, StopReason>`. The scheduler loop matches on `StopReason` to decide whether to advance the hart or pause simulation. `World::run()` surfaces `StopReason::Breakpoint` to the Python caller.

---

## Q16 — In multi-hart mode, do harts share `MemoryMap` or have separate views?

**Context**

In a real multi-core system, all cores share the physical address space. They may have different TLBs (virtual → physical translation is per-core), but the physical memory map is unified. A simulator that gives each hart a separate `MemoryMap` must either duplicate all MMIO region registrations or add a forwarding layer — neither is done in practice. QEMU shares a single `AddressSpace` between all vCPUs; per-vCPU translation is handled in the TLB, not the address space. Gem5 shares the memory system via a crossbar/bus; each CPU port connects to the same bus. Separate views would require full physical memory duplication or a complex alias system.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Single `Arc<MemoryMap>` shared by all harts | Correct model; no duplication; MMIO registered once | Concurrent reads need `RwLock` or lock-free design | QEMU, Gem5 |
| Per-hart `MemoryMap` with shared `Arc<Ram>` for RAM | Harts can have different MMIO views (useful for asymmetric SoCs) | Complex; device registration per-hart; coherence harder | Rare; embedded SoC simulators |
| Per-hart `MemoryMap` fully independent | Maximum isolation for testing | Physically incorrect for shared memory; duplication of all regions | Testing only |

**Answer:** All harts share a single `Arc<RwLock<MemoryMap>>`. TLB state is per-hart (owned by `ArchState`); the physical address map is global.

**Rationale:** Sharing the `MemoryMap` is architecturally correct and is what every production multi-hart simulator does. The cost of `RwLock` is acceptable because: (a) in functional mode, harts run sequentially (no contention); (b) in timing mode with temporal decoupling, harts run in separate quanta and only synchronize at boundaries, so the map is effectively read-only during a quantum. If write-heavy MMIO arises, `DashMap` or region-level locking can be introduced later.

**Impact:** `HelmEngine<T>` holds `Arc<RwLock<MemoryMap>>`. `World::add_hart()` clones the `Arc` for each new hart. TLB shootdowns (e.g., from `satp` write) invalidate only the local hart's TLB, not the map. Multi-hart tests can verify shared MMIO by writing from one hart and reading from another.

---

## Q17 — Default quantum size and per-hart vs. global configuration

**Context**

The quantum size determines the accuracy/performance tradeoff for temporal decoupling. A small quantum (e.g., 100 instructions) means harts synchronize frequently — more accurate but slower. A large quantum (e.g., 1M instructions, as SIMICS uses) is fast but can cause harts to appear far apart in simulated time. Gem5's `simQuantum` defaults to 1M ticks (not instructions). SIMICS documentation recommends 500K–1M instructions as a default for server workloads. The question of per-hart vs. global configuration matters for asymmetric systems (e.g., a big core and a small core running at different effective IPCs).

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Global quantum, configurable at `World` level | Simple; no per-hart state | Cannot model asymmetric hart speeds | Gem5 (`simQuantum`), SIMICS |
| Per-hart quantum, set on each `HelmEngine` | Models asymmetric throughput | More complex scheduler; harts desynchronize at different rates | — |
| Global default with per-hart override | Best of both | Slightly more complex API | — |

**Answer:** Default quantum is 10,000 instructions globally, configurable via `World::set_quantum(n: u64)`. Per-hart override is not supported in Phase 0 but the scheduler API reserves it.

**Rationale:** 10,000 instructions is a reasonable default: large enough to amortize synchronization overhead, small enough to keep debuggability acceptable (a breakpoint fires within 10K instructions of its target in the worst case). SIMICS's 500K–1M defaults are tuned for throughput benchmarks on server silicon; helm-ng's primary early use case is correctness testing where 10K is more appropriate. Per-hart quanta are deferred because the scheduler complexity increase is not justified before there is an asymmetric workload to test against.

**Impact:** `Scheduler::quantum` is a `u64` field defaulting to `10_000`. `World::set_quantum(n)` sets it. The scheduler runs each hart for `quantum` instructions per turn. Future: `World::set_hart_quantum(hart_id, n)` can be added without breaking the existing API.

---

## Q18 — Single `Isa` enum vs. separate `RiscvHart` / `Aarch64Hart` structs

**Context**

`helm-arch` must expose one or more concrete hart types that `HelmEngine<T>` instantiates. Two structural options: (1) a single `Isa` enum with variants that dispatch to ISA-specific logic, or (2) separate structs (`RiscvHart`, `Aarch64Hart`) that both implement the `Hart` trait. The enum approach means callers hold one type but pay for runtime dispatch inside the enum methods. The separate-struct approach means the caller must be generic over the hart type or use `Box<dyn Hart>`, but each struct has a flat, non-dispatching execute loop. Gem5 uses separate CPUModel classes per ISA (each a different C++ template instantiation). QEMU uses a single `CPUState` struct with an ISA-specific `CPUArchState` embedded via a union — closer to the enum approach.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Single `Isa` enum dispatching to ISA-specific state | One type in `HelmEngine`; no generics on hart | `match isa { ... }` in every hot method; inlining limited; new ISA = touch every match | QEMU (union approach) |
| Separate `RiscvHart<T>`, `Aarch64Hart<T>` implementing `Hart` | Clean per-ISA state; full inlining; no dispatch overhead | `HelmEngine` must be generic over hart type or use `Box<dyn Hart>` | Gem5 (class hierarchy) |
| `Box<dyn Hart>` in engine; separate structs | No generics on engine; harts are objects | Vtable per method call in hot path; no inlining | — |

**Answer:** Separate `RiscvHart` and `Aarch64Hart` structs, both implementing `Hart`. `HelmEngine<T>` is additionally generic over `H: Hart` (or ISA is erased at the `HelmSim` level).

**Rationale:** The enum approach forces every hot-path method in the engine to contain an ISA dispatch, even when the ISA is fixed at construction time. Separate structs allow the compiler to monomorphize and inline the entire execute path for each ISA. The ergonomic cost (an extra generic parameter or `Box<dyn Hart>`) is paid at the `HelmSim` boundary, not in the inner loop. This matches Gem5's approach and is the only viable path to near-native simulation speed.

**Impact:** `HelmEngine<T, H: Hart>` or `HelmEngine<T>` with `H` erased at `HelmSim`. Each ISA crate (`helm-arch-riscv`, `helm-arch-aarch64`) exports its hart struct. Adding a new ISA does not require touching existing hart implementations.

---

## Q19 — RISC-V decode: `Instruction` enum vs. `Box<dyn Executable>`

**Context**

The RISC-V instruction decode step maps a `u32` word to a representation that can be executed. Two approaches: (1) a pure `decode(u32) -> Instruction` function returning a Rust enum with one variant per instruction mnemonic (or per instruction group), which is then matched in the execute step; (2) a `decode(u32) -> Box<dyn Executable>` returning a trait object that carries both the decoded fields and an `execute` method. Enum is heap-free, inlineable, and fast. Trait object enables plug-in custom instructions but allocates on every decode. Gem5 uses a decoded `StaticInst` object (heap-allocated, cached in the decode cache). QEMU avoids this by generating native code via TCG — decode and execute are fused.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `decode(u32) -> Instruction` (enum) | Zero allocation; full inlining; matches exhaustively at compile time | Enum grows with each ISA extension; decode and execute coupled by match arm | — |
| `decode(u32) -> Box<dyn Executable>` | Plugin-friendly; custom instructions via trait impl | Heap alloc per instruction; vtable dispatch; no inlining of execute | Gem5 (`StaticInst`) |
| `decode(u32) -> Instruction` + decode cache | Amortize re-decode of the same word | Cache management; invalidation on SMC; complexity | Gem5 decode cache |
| `decode(u32) -> &'static dyn Executable` (static dispatch table) | No allocation; still trait-object flexible | `'static` lifetime; no per-instruction state (fields must be separate) | — |

**Answer:** `decode(u32) -> Instruction` where `Instruction` is an enum. A decode cache keyed on the raw `u32` is added in Phase 1 if profiling shows decode as a bottleneck.

**Rationale:** For a functional simulator focused on correctness, allocation-free decode with a `match` execute loop is simpler, faster, and easier to audit. Trait objects would be appropriate if helm-ng needed runtime-pluggable custom instructions (e.g., vendor extensions loaded as `.so` plugins) — that is not a Phase 0 requirement. The enum approach also gives exhaustive match checking at compile time, which is valuable for ISA compliance testing.

**Impact:** `helm-arch-riscv` defines `enum Instruction { Add { rd, rs1, rs2 }, Addi { rd, rs1, imm }, ... }`. `RiscvHart::execute(inst: Instruction, ctx: &mut ExecContext)` is a `match`. The enum is `#[non_exhaustive]` to allow extension without breaking downstream code.

---

## Q20 — RISC-V CSR side effects: `ExecContext::write_csr()` vs. ISA execute loop

**Context**

Certain RISC-V CSR writes have architectural side effects beyond storing a value. Writing `satp` changes the address translation mode and must flush the TLB. Writing `mstatus` changes the privilege level and may update cached MPIE/MIE bits. Writing `mtvec` changes the trap vector base. In Gem5, CSR side effects are triggered in the ISA's execute loop — `BaseCPU::setMiscReg()` has a per-CSR switch statement that calls `scheduleTlbShootdown()` or similar. `ExecContext::setMiscReg()` in Gem5 is a thin write that calls into the CPU model, which then calls the ISA's `setMiscRegNoEffect()` + post-write hook. SIMICS handles CSR side effects via attribute setters on the processor object.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `ExecContext::write_csr()` triggers side effects inline | Side effects guaranteed to run; single call site | `ExecContext` must know about TLB, privilege model — couples it to the CPU | — |
| ISA execute loop calls `write_csr()` then explicit side-effect handlers | Clear separation: write is just a write; effects are explicit | Each CSR instruction's execute arm must remember to call the handler | Gem5 |
| `write_csr()` on `RiscvHart` (not `ExecContext`) dispatches per-CSR | Side effects localized in hart; `ExecContext` stays thin | `ExecContext` can't call `write_csr` directly; must delegate | — |
| Observer/event pattern: `write_csr` fires a `CsrWritten` event; listeners react | Maximum decoupling | Overhead; event system needed even for trivial CSRs | — |

**Answer:** The ISA execute loop (each CSR instruction's execute arm) calls `ctx.write_csr(csr, val)` and then explicitly calls the per-CSR side-effect handler on `RiscvHart`. `ExecContext::write_csr()` is a pure storage write.

**Rationale:** Keeping `ExecContext::write_csr()` as a pure storage write preserves the design principle that `ExecContext` is a thin hot-path interface. Side effects are architectural and belong in the ISA layer. Making them explicit in the execute arm (as Gem5 does) means side effects are visible in the code without tracing through virtual calls. A `match csr_index { SATP => hart.flush_tlb(), MSTATUS => hart.update_priv(), ... }` in the CSR-write execute arm is readable and performant.

**Impact:** `RiscvHart` exposes `flush_tlb()`, `update_privilege_cache()`, `set_trap_vector(base)` etc. as methods called explicitly after `ctx.write_csr()`. `ExecContext` does not reference TLB or privilege types. Adding a new CSR side effect requires modifying only the CSR instruction's execute arm.

---

## Q21 — AArch64 decode: `deku` crate vs. hand-written bit-field parsing

**Context**

AArch64 instruction encoding uses irregular bit-field layouts: the same bit positions mean different things in different instruction classes. The `deku` crate provides proc-macro derive attributes (`#[deku(bits = "5", endian = "big")]`) that generate parsing code from struct field annotations, reducing manual bit-extraction boilerplate. Hand-written decode uses explicit bit masking (`(word >> 21) & 0x7FF`) and is typically 20–30% faster due to better compiler optimization of the generated code. The AArch64 ISA has ~1000 encodings across dozens of encoding classes; hand-written decode of all of them is a significant maintenance burden with high error risk.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `deku` derive macros | Declarative; self-documenting; lower error rate on irregular fields | ~20–30% decode overhead; `deku` is an external dependency; generated code harder to debug | — |
| Hand-written bit masking | Fastest; no dependencies; full control | Error-prone on 1000+ encodings; hard to review against ARM spec | Many ISA simulators |
| `deku` for parsing, hand-written for the hot-path subset | Best coverage vs. performance tradeoff | Two decode paths to maintain; coherence risk | — |
| Code-generated from ARM's machine-readable ISA XML | Spec-accurate; maintainable | Complex tooling; XML schema changes break codegen | ARM Fast Models |

**Answer:** Use `deku` for the initial implementation of all encoding classes. Profile after integration; if decode shows >5% of runtime, hand-optimize the top 20 most-executed instruction classes.

**Rationale:** Correctness on 1000+ irregular encodings is the dominant risk in early development. `deku` significantly reduces the probability of a bit-mask error (which would manifest as a silent wrong-register operand, hard to catch in testing). The 20–30% decode overhead translates to perhaps 2–5% total simulation slowdown for a functional simulator where decode is not the bottleneck. The profile-then-optimize approach follows the SIMICS and Gem5 precedent of starting with a correct but not maximally fast decode and optimizing only what profiling identifies.

**Impact:** `helm-arch-aarch64/src/decode/` uses `#[derive(DekuRead)]` structs per encoding class. The `deku` crate is added as a dependency to `helm-arch-aarch64` only. A `DECODE_PROFILE` feature flag gates instruction-level timing collection for profiling.

---

## Q22 — AArch64 encoding organization in the LLD

**Context**

With ~1000 instruction encodings, the LLD for AArch64 decode must impose an organizational structure that makes it possible to implement, review, and maintain encodings without losing track of coverage. ARM's ARM ISA reference organizes encodings by encoding class (the top-level `op0`/`op1` bits in the 32-bit instruction word determine the class). Functional category (load/store, arithmetic, branch, SIMD/FP) is the other natural axis. Alphabetical ordering is used by some reference manuals but is hostile to implementation (semantically related encodings are scattered).

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| By encoding group (ARM ISA tree structure) | Directly matches the ARM ISA reference decode tree; easy to cross-reference spec | Groups don't align with functional testing (all load/store tests span groups) | ARM Fast Models |
| By functional category (data proc, memory, branch, SIMD/FP) | Matches test structure; easier to implement one subsystem at a time | Category boundaries are fuzzy; some instructions span categories | QEMU |
| Alphabetical | Easy to look up a specific mnemonic | Zero relationship to encoding proximity; no structural benefit | Some reference docs |
| Hybrid: top-level by encoding group, file-level by functional category | Spec-accurate structure with readable files | Two axes of organization; requires an index | Gem5 |

**Answer:** Top-level modules in `helm-arch-aarch64/src/decode/` are organized by ARM encoding class (matching the ISA reference decode tree). Within each module, instruction variants are grouped by functional category with inline comments linking to the ARM ISA reference section.

**Rationale:** Organizing by encoding class is the most defensible choice for a simulator that must eventually achieve spec compliance: every encoding in the ARM ISA reference tree has a direct home. Cross-referencing during implementation (`// ARM ISA A64 C3.2.1`) is essential for correctness review. Functional grouping within a file is secondary but aids readability during testing.

**Impact:** `decode/data_processing_immediate.rs`, `decode/data_processing_register.rs`, `decode/loads_stores.rs`, `decode/branches.rs`, `decode/simd_fp.rs` etc. Each file maps to one ARM ISA encoding class. Coverage tracking is done by noting which ARM ISA sections have corresponding `deku` structs.

---

## Q23 — AArch32 stubbing for SE mode

**Context**

AArch64 EL1 can run AArch32 code at EL0 (the `nTnE` bit in `SPSR_EL1`). In SE (syscall-emulation) mode, the simulator runs userspace binaries; the OS is not simulated. A binary compiled for AArch32 (Thumb or ARM32) may be launched under an AArch64 EL1 stub. Without AArch32 support, such binaries cannot run at all. Full AArch32 implementation is a large engineering effort (separate register file with aliases `r0`–`r15`, CPSR, Thumb decode, etc.). Stubbing means: trap on any EL0 AArch32 instruction, return a meaningful error, and allow the test harness to skip AArch32 tests.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Full AArch32 + Thumb decode | Correct; runs real AArch32 EL0 binaries | ~6 months additional work; doubles ISA surface | QEMU, Gem5 |
| Stub: detect AArch32 EL0 entry and raise `UndefinedInstruction` | Minimal work; AArch64 tests unaffected | AArch32 binaries fail at first instruction | — |
| Stub: implement the 20 most common AArch32 instructions | Runs simple AArch32 userspace (libc startup, basic arithmetic) | Partial implementation is hard to test correctly; spec gaps | — |
| Stub: refuse to set `nTnE` in `SPSR_EL1`; return `SIGILL` on syscall | Prevents AArch32 EL0 entry entirely | Incorrect architecture behavior; may confuse test harness | — |

**Answer:** Stub via detection: on any EL0 instruction fetch where the hart is in AArch32 state (PSTATE.nRW = 1), raise an `UndefinedInstruction` exception and log a `STUB: AArch32 not implemented` message. `SPSR_EL1.nRW` is allowed to be set (no architectural prevention) but AArch32 execution halts at the first instruction.

**Rationale:** The goal is that AArch64 binaries run correctly and AArch32 binaries fail clearly and early rather than silently executing wrong code. Preventing `nRW` from being set is incorrect and would break EL1 kernel stubs that set it during context switch. Raising `UndefinedInstruction` on first AArch32 fetch is the most architecturally honest stub and gives the test harness a clear signal to skip.

**Impact:** `RiscvHart::fetch()` — analogously `Aarch64Hart::fetch()` checks `pstate.nRW` and returns `Err(StopReason::Unimplemented("AArch32"))` before decode. The `World::run()` loop converts this to a Python `NotImplementedError`. AArch32 work is tracked as a Phase 2 milestone.

---

## Q24 — RISC-V C extension: pre-expansion vs. separate instruction variants

**Context**

The RISC-V C (Compressed) extension encodes 16-bit instructions that are defined as aliases of specific 32-bit base instructions with restricted register fields. For example, `C.ADD` is `ADD rd, rd, rs2` with `rd` and `rs2` in the compressed register encoding. Two decode strategies: (1) pre-expansion — detect the 16-bit encoding, expand to the canonical 32-bit word, then feed into the standard 32-bit decoder; (2) separate variants — add `CAdd`, `CLw`, etc. to the `Instruction` enum and handle them in the execute match. Gem5 pre-expands in the decode step (the expanded instruction is a `MacroOp` that spawns the equivalent micro-ops). QEMU maintains a separate C decode table and dispatches to separate TCG helpers — effectively separate variants.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Pre-expansion (16→32 before decode) | Simpler execute loop; no C-specific cases in execute; single instruction path | Extra expansion step per C instruction; expansion logic must handle all C encodings; slightly more decode work | Gem5 |
| Separate `Instruction` variants for C encodings | No pre-expansion step; C instructions visible as distinct types in traces | Execute match grows by ~50 arms; C-specific logic duplicated or forwarded to base instruction | QEMU |
| Pre-expansion with expansion cache | Amortize expansion for loops | Cache management; SMC invalidation | — |
| Decode C as base instruction with compressed operand encoding tag | One variant per base instruction; tag carries register remapping | Complex match arm logic; tag must be checked in every C-capable execute | — |

**Answer:** Pre-expansion. The fetch step detects 16-bit instructions (by checking the two LSBs ≠ `11`), expands to the canonical 32-bit encoding using the RISC-V C expansion table, and feeds the result into the standard 32-bit decoder.

**Rationale:** Pre-expansion keeps the `Instruction` enum clean — it models the architectural meaning ("`ADD rd, rd, rs2`") not the encoding accident ("`C.ADD` in compressed form"). The execute loop has no awareness of whether an instruction came from a 16-bit or 32-bit encoding, which simplifies correctness review and matches how the RISC-V spec defines C instructions (as aliases). The expansion table is a fixed, testable lookup of ~48 cases that can be unit-tested against the spec in isolation. Gem5's choice of pre-expansion in a mature, widely-used simulator is strong evidence for this approach.

**Impact:** `RiscvHart::fetch()` returns a `(u32, PcIncrement)` where `PcIncrement` is 2 or 4 depending on whether compression was applied. The `decode(u32) -> Instruction` function is unaware of compression. Traces and disassembly reconstruct the original encoding from the `PcIncrement` field for display purposes.

---

*Generated for helm-ng design review. Last updated: 2026-03-14.*

---

# Design Questions: helm-memory (Q25–Q37), helm-timing (Q38–Q50), helm-event (Q51–Q54)

> Enriched design question reference. Each question includes context, an options and trade-offs table,
> the project answer, a rationale for that answer, and an impact statement.

---

## helm-memory

---

### Q25 — Overlapping subregion priority: last added wins, first added wins, or explicit priority field?

**Context**

When a `Container` region has two or more children whose address ranges overlap, the memory system must have a deterministic rule for which child "wins" at the overlapping addresses. This question is fundamental to address space layout: it determines how peripheral apertures, ROM shadows, and aliased regions compose. QEMU's `MemoryRegion` tree resolves overlaps at `FlatView` generation time using a priority field (defaulting to 0), where higher-priority regions shadow lower ones. When two regions share the same priority, the last one added wins, which effectively makes priority-tie-breaking order-dependent. Gem5 takes a different approach: address ranges must be non-overlapping at construction time and an overlap is an assertion failure. The SIMICS approach allows overlapping memory spaces but requires the user to assign explicit priority values per space.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Last added wins** | Simple API — no extra field. Natural for additive configuration (add a ROM that shadows part of RAM). Matches QEMU's same-priority tie-break. | Order-dependent; configuration order matters, which can be surprising when build order changes. Hard to override without removing and re-adding. | QEMU (default, priority-tie-break) |
| **First added wins** | Stable across additions; base mappings are never displaced. | Counterintuitive for overlay use-cases (e.g., adding a boot ROM shadow over RAM). Less common in practice. | — |
| **Explicit priority field** | Explicit, order-independent. Matches full QEMU semantics (`priority` arg on `memory_region_add_subregion_overlap`). Easy to document and reason about. | Requires every caller to supply a priority. More verbose API. Must pick default (0) and document what it means. | QEMU (full API), SIMICS memory spaces |

**Answer:** Last added wins (matching QEMU's same-priority default behavior). Helm-ng does not expose a priority field in Phase 0. If two subregions overlap and have the same implicit priority, the one added later takes precedence at the overlapping addresses.

**Rationale:** The vast majority of address space configurations are non-overlapping by design; the overlap rule is only exercised when adding ROM shadows, PCIe BAR overlays, or boot-time remapping. Last-added-wins is the simplest implementation (the `FlatView` recompute just iterates subregions in reverse insertion order) and matches the behavior users familiar with QEMU will expect. An explicit priority field can be added in Phase 1 if needed without breaking the API.

**Impact:** `FlatView` recomputation iterates `Container::children` in reverse order; earlier children are shadowed by later ones at overlapping addresses. Documentation must warn that `add_region` ordering is significant when overlaps are intentional.

---

### Q26 — FlatView recomputed eagerly or lazily?

**Context**

`FlatView` is the resolved, sorted list of non-overlapping `FlatRange` entries that the memory system actually uses for address lookup. It is derived from the `MemoryRegion` tree. Every time the tree changes (region added, removed, alias target changed), `FlatView` must be regenerated. The question is whether regeneration happens immediately on the mutation (eager) or is deferred until the next read access (lazy). QEMU uses a lazy approach with an `ioeventfd` / `MemoryListener` notification pipeline: `memory_region_transaction_commit()` triggers a FlatView rebuild and fires `MemoryListener::region_add/del` callbacks. The rebuild is deferred across a transaction to batch multiple mutations. Gem5 does not have a `FlatView` equivalent; its port-based routing rebuilds at construction only.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Eager (on every add_region)** | FlatView is always consistent; no staleness possible. Simpler correctness invariant. | Expensive if many regions are added in a loop during configuration. Each add pays the full rebuild cost. | — |
| **Lazy (dirty flag, rebuild on next lookup)** | Free during batch configuration; pay rebuild cost once on first access. Natural batching: Python config script adds 10 regions, then calls elaborate() — one rebuild. | Must track dirty flag. Lookup must check the flag before binary search. Requires care in multi-threaded context. | QEMU (transactional), Sniper |
| **Transactional (begin/commit API)** | Explicit batch: `mm.begin_update(); ...; mm.commit()` triggers rebuild. Zero ambiguity. | More API surface. Forgetting to commit is a bug (may need a guard type). | QEMU (explicit transaction) |

**Answer:** Lazy recomputation with a `dirty: bool` flag. The `FlatView` is rebuilt on the next lookup call when `dirty` is set. `MemoryListener` callbacks are fired after recomputation to invalidate cache tags that mapped into remapped regions.

**Rationale:** In practice, all regions are added during configuration/elaboration, before any simulation access. Lazy evaluation means the rebuild cost is paid exactly once — on the first memory access — regardless of how many `add_region` calls preceded it. This is both simpler and faster than eager rebuilding. The dirty-flag check adds one branch on the hot-path lookup (predictable: almost always false during simulation). If a transactional API is later desired, it can be implemented as a convenience wrapper that sets dirty and calls `ensure_flat()`.

**Impact:** `MemoryMap::lookup()` must call `ensure_flat_view()` before the binary search. `add_region()` and `remove_region()` set `dirty = true` and fire no callbacks immediately. `MemoryListener::region_add` / `region_del` are fired synchronously inside `ensure_flat_view()`.

---

### Q27 — Does MemoryMap own the Device handler for MMIO, or store HelmObjectId?

**Context**

When a physical address maps to an MMIO region, the memory system must dispatch the access to a device handler. Two ownership strategies exist. Direct ownership: `MemoryMap` stores a `Box<dyn Device>` (or `Arc<dyn Device>`) inside the `MemoryRegion::Mmio` variant. Indirect ownership: `MemoryMap` stores a `HelmObjectId` and looks up the device at access time by querying a `World` registry. The choice affects plugin lifecycle, dependency direction, and checkpoint complexity.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **MemoryMap owns Box\<dyn Device\>** | No registry lookup on hot path. Simple lifetime: device lives as long as the map. Easy to checkpoint — device is co-located with its address mapping. | `helm-memory` acquires a dependency on the `Device` trait. Circular dependency risk if `Device` needs `MemoryMap`. Plugin removal requires removing from both map and registry. | QEMU (MemoryRegion owns the MMIO ops) |
| **Store HelmObjectId, look up in World** | `helm-memory` stays independent of `helm-devices`. Device lifecycle managed by `World`. Natural for device hot-plug: swap the device object without touching `MemoryMap`. | Indirection cost on every MMIO access (registry hash lookup). Requires `World` or a shared registry to be accessible from `MemoryMap`. | Gem5 (objects referenced by name/id) |
| **Arc\<dyn Device\> (shared ownership)** | No look-up overhead. Device can be referenced from both `MemoryMap` and `World`. | `Arc` adds reference counting overhead. Shared mutation requires `Arc<Mutex<dyn Device>>`. | SIMICS (shared pointer model) |

**Answer:** `MemoryMap` owns `Box<dyn Device>` (or `Arc<Mutex<dyn Device>>` for devices shared across multiple regions) directly inside the `MemoryRegion::Mmio` variant. The `Device` trait is defined in `helm-core` to avoid a circular dependency.

**Rationale:** MMIO dispatch is on the access critical path. Eliminating a registry hash-lookup per access is the primary performance justification. Placing the `Device` trait in `helm-core` — which `helm-memory` already depends on — avoids introducing a new dependency edge. Device plug/unplug can be accommodated by swapping the `Box` inside the `MemoryRegion` and invalidating the `FlatView`. For Phase 0 (no hotplug), the simpler `Box<dyn Device>` suffices.

**Impact:** `helm-core` must define the `Device` trait (or a trimmed `MmioHandler` trait) so that `helm-memory` can reference it without depending on `helm-devices`. `World` may hold a separate reference (`Arc`) to each device for Python inspection and lifecycle management.

---

### Q28 — How is MemoryRegion::Alias implemented for subranges?

**Context**

An alias region makes a subrange of the address space behave as a transparent window into another region, potentially at a different base address and/or covering only part of the target. The classic example is a boot ROM shadow: physical addresses 0x0000_0000–0x0000_FFFF are aliased to ROM at 0x1FC0_0000–0x1FCF_FFFF. The alias adds an `offset` field so that accesses to the alias base are re-based to the target. This is the same model used by QEMU's `memory_region_init_alias`.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Offset field on Alias variant** | Simple arithmetic: `target_read(alias.offset + offset_into_alias)`. Composable: alias of an alias is resolved recursively at FlatView time. | Must bound-check the alias range against the target size at construction. | QEMU (`MemoryRegion::alias`, `alias_offset`) |
| **Copy the bytes at construction** | No indirection at access time. | Defeats the purpose: a live alias (e.g., PCIe ROM window) must reflect writes to the target in real time. Stale copy is wrong. | — |
| **Redirect via FlatRange at FlatView time** | The alias is resolved to the target's backing `FlatRange` during `FlatView` recomputation. At access time, no alias indirection exists — it's just a range pointing at the target's backing store. | FlatView recomputation becomes more complex (must chase alias chains). | QEMU (FlatView flattens aliases) |

**Answer:** Alias resolution is performed during `FlatView` recomputation. The `MemoryRegion::Alias { target, offset, size }` variant is chased at FlatView-build time until a non-alias target is found. The resulting `FlatRange` points directly at the concrete backing region (Ram, Rom, Mmio) with an adjusted offset. At access time there is no alias indirection: the access hits the flattened range with the correct backing.

The access address arithmetic is:
```
read(alias_base + x)  →  target.read(alias.offset + x)
```
where `alias.offset` is the byte offset into the target region's backing store at which the alias starts.

**Rationale:** Flattening aliases at `FlatView` recomputation time eliminates runtime alias indirection on the hot path. This is exactly what QEMU does. Alias chains (alias of alias) are handled by iterating during flatten rather than nesting at access time. Construction-time validation (`alias.offset + alias.size <= target.size`) catches misconfigurations early.

**Impact:** `FlatView::rebuild()` must implement alias-chasing. `FlatRange` must carry a `base_offset: u64` field so that `read_at(flat_range, local_offset)` can compute `backing_store_offset = flat_range.base_offset + local_offset` without re-examining the alias chain. Alias-of-alias depth should be bounded (e.g., max 8 levels) to detect configuration cycles.

---

### Q29 — Should MemoryMap support dynamic region add/remove?

**Context**

Static address maps are fully configured at elaborate time and never change during simulation. Dynamic maps change at runtime: PCIe configuration space writes remap BARs, BIOS/UEFI remaps ROM after POST, hotplug adds or removes devices. Supporting dynamic remapping requires `add_region` / `remove_region` to be callable during simulation, which implies invalidating the `FlatView` and, critically, handling in-flight `Timing` requests that were issued under the old mapping.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Static only (no runtime changes)** | Zero complexity. FlatView never needs invalidation after elaboration. Safe for in-flight requests. | Cannot model PCIe BAR remapping, hotplug, ROM shadowing switch. Limits system-level fidelity. | Gem5 (port ranges fixed post-construction) |
| **Dynamic with drain requirement** | Full fidelity for PCIe, hotplug. Same model as real hardware (driver must quiesce DMA before unmap). | Caller must drain in-flight Timing requests before mutation. Requires documented protocol and runtime check. | QEMU, SIMICS |
| **Dynamic with copy-on-write FlatView** | Existing requests hold a reference to the old FlatView; they complete safely. New requests use the new FlatView. | More complex: FlatView must be `Arc`-wrapped and versioned. Higher memory use if many snapshots coexist. | Conceptual (not widely implemented in simulators) |

**Answer:** Dynamic `add_region` / `remove_region` is supported. Callers are responsible for draining all in-flight `Timing` requests before mutating the map. This is enforced in debug builds by a runtime check (panic if `pending_timing_count > 0`); in release builds it returns `Err(MemFault::ModeMismatch)`.

**Rationale:** PCIe BAR remapping is a core requirement for any system-level simulation that boots a real OS. The drain-before-mutate protocol mirrors real hardware behavior (a driver must quiesce DMA before remapping BARs), so users familiar with hardware will find it natural. The validation is lightweight (an atomic counter) and catches real bugs. Phase 0 only needs this for ROM shadow switches; PCIe BAR remapping is Phase 1.

**Impact:** `MemoryMap` must maintain a `pending_timing_count: AtomicU32` that `Timing` requests increment/decrement. `add_region` and `remove_region` check this counter. `FlatView` is invalidated (`dirty = true`) after every structural mutation. `MemoryListener::region_del` is fired to allow cache invalidation of lines in the remapped range.

---

### Q30 — Cache replacement policy: true LRU or pseudo-LRU (PLRU)?

**Context**

When a cache set is full and a new line must be brought in, a victim way must be chosen. True LRU (Least Recently Used) tracks the exact access order of all ways in a set, which requires O(log₂(ways)) bits per set and O(ways) work to update the ordering on every access. Pseudo-LRU (PLRU) approximates LRU using a binary tree of `ways-1` bits per set, updating in O(1) by flipping one bit per level of the tree. Real hardware (Intel Haswell, SiFive U74) universally uses PLRU or a similar approximation for caches with more than 2 ways because true LRU hardware is prohibitively expensive. Gem5 implements both: true LRU for configurations with 8 or fewer ways, PLRU for larger.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **True LRU** | Exact replacement order. Best possible cache miss rate for LRU-friendly workloads. Useful for small-associativity validation. | O(ways) update per access. Requires full permutation state (e.g., 3 bits for 2-way, but 6+ bits for 4-way to encode full order). Not what real hardware does. | Gem5 (≤ 8 ways), academic cache simulators |
| **Pseudo-LRU (PLRU, binary tree)** | O(1) update, O(1) victim selection. `ways-1` bits per set. Accurate approximation: within ~5% of true LRU miss rate on standard benchmarks. Matches real hardware. | Slight inaccuracy vs. true LRU. Behavior differs for degenerate access patterns (e.g., sequential scan width = assoc+1). | Gem5 (> 8 ways), Intel, ARM, SiFive hardware |
| **Random replacement** | O(1), minimal state, easiest to implement. | Worst miss rate. Not representative of any real hardware. Unusable for performance modeling. | Some academic simulators |
| **NRU (Not Recently Used)** | Single bit per line. Very simple. Approximation of LRU. | Coarser than PLRU; one wrong bit can cause a cascade of poor replacements. | Some embedded cache controllers |

**Answer:** Pseudo-LRU (PLRU) via a binary tournament tree. For an N-way set, `N-1` bits are stored as a `u64` bitmask. Each access calls `touch(way)` which flips bits along the path from root to the accessed way's leaf, always pointing the tree away from the accessed way. Victim selection follows the bits from root to leaf, choosing the subtree pointed to at each node.

**Rationale:** PLRU is the correct choice for a microarchitecture simulator that aims to match real hardware behavior. No shipping RISC-V or AArch64 SoC uses true LRU for L1 caches due to hardware cost. PLRU's O(1) update keeps `CacheModel::read` / `write` fast on the hot path. The `u64` bitmask fits in a single register, avoiding heap allocation per set. For correctness validation, the PLRU implementation can be compared against a reference true-LRU on small test cases.

**Impact:** `CacheSet` stores `plru_bits: u64` alongside `ways: Vec<CacheLine>`. `touch(way)` and `plru_victim()` are the two PLRU methods. Maximum supported associativity is 64 ways (limited by the `u64` bitmask; 32 ways = 31 tree bits, well within range). The PLRU helpers are also reused by `TlbModel` to avoid code duplication.

---

### Q31 — CacheModel write-back or write-through?

**Context**

Write policy determines what happens to cached data when the processor writes to a cached address. Write-back (WB): the write goes only to the cache line (line is marked dirty). The dirty line is written to the next level only on eviction. Write-through (WT): every write is immediately propagated to the next cache level and to memory. The choice affects dirty bit tracking, eviction traffic, and the accuracy of memory bandwidth modeling. All modern high-performance processors (Cortex-A, SiFive U74, Apple M-series) use write-back L1/L2/L3. Write-through is found in small embedded controllers and some L1 instruction caches. Gem5 defaults to write-back.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Write-back** | Low write traffic: writes are absorbed in the cache. Accurate for modern CPUs. Dirty bit enables accurate writeback-on-eviction modeling. | Dirty bit per line adds 1 bit of state. Eviction handling is more complex (must writeback if dirty). | Gem5 (default), Sniper, all modern CPUs |
| **Write-through** | No dirty bit needed. Simpler eviction (line is always clean). Easy to model for simple embedded cores. | Generates write traffic on every store hit — unrealistic for high-performance cores. Over-estimates memory bandwidth for stores. | Simple embedded cores (Cortex-M), some L1I caches |
| **Configurable per-cache-level** | Flexible: L1D WB, L1I WT (typical for real hardware). | Adds configuration complexity. Must validate combinations. | Gem5 (config flag) |

**Answer:** Write-back by default, with `CacheConfig::write_back: bool` allowing per-level configuration (defaulting to `true`). The `CacheLine::dirty` bit is set on write hits. On eviction, if `dirty == true`, the evicted line's data is written to the next cache level (or to backing RAM if LLC). Write-allocate policy is assumed for write-back caches: a write miss allocates a new line (fills from next level, then writes in place).

**Rationale:** Write-back accurately models the behavior of the target microarchitectures (SiFive U74, Cortex-A72). It produces realistic memory bandwidth figures because only evicted dirty lines generate downstream traffic. Write-through would overestimate store-driven bandwidth by an order of magnitude for typical workloads. The `write_back` config flag allows a future write-through L1I model without changing the core struct.

**Impact:** `CacheModel::write()` must mark the hit line dirty. `CacheModel::fill_line()` must return the evicted `(addr, data)` pair when the evicted line is dirty, so the caller can propagate the writeback to the next level. `CacheStats::writebacks` tracks writeback events for performance counter export.

---

### Q32 — In Interval timing, does cache state persist between intervals or reset?

**Context**

The Interval (Sniper-style) timing model divides execution into fixed-length intervals of ~10,000 committed instructions, computing a CPI estimate per interval. Between intervals, the question is whether the simulated cache's tag/dirty state carries forward (warmup effects modeled) or is reset to cold/empty (each interval is independent). In Sniper's original design, the cache model (the Graphite cache model) persists across intervals — this is essential for accurately capturing warmup, working-set effects, and temporal locality patterns. Resetting between intervals would make every interval look like a cold-start, dramatically overstating miss rates for workloads with warm working sets.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Persist (state carries forward)** | Accurate warmup modeling. LLC state reflects actual access history. Temporal locality is captured. Miss rates correctly decrease as the working set fits in cache. | Cannot easily decompose per-interval contribution of cache effects (state is entangled across intervals). | Sniper (Graphite model), gem5 |
| **Reset between intervals** | Each interval is statistically independent. Useful for steady-state throughput analysis where warmup is not the point of interest. | Wildly inaccurate for any workload with a warm steady-state cache (virtually all real workloads). Overstates MPKI. | — |
| **Configurable (persist or reset)** | User can choose: warmup analysis vs. steady-state analysis. | More complexity. Warmup mode should be the default. | Conceptual |

**Answer:** Cache state persists between intervals. The `Arc<CacheModel>` shared between `IntervalTimed` and `helm-memory` is the same object across the full simulation run. No flush or reset is performed at interval boundaries.

**Rationale:** Resetting cache state at interval boundaries would produce MPKI figures that are only valid for a cold-start scenario, making the model useless for workloads that spend more than a few percent of time in warmup. Persistence is not just more accurate — it is the architecturally correct behavior. The shared `Arc<CacheModel>` design naturally achieves this with no extra mechanism.

**Impact:** The `Arc<CacheModel>` must be correctly shared: `helm-memory` holds one `Arc` (for functional/atomic accesses), `IntervalTimed` holds another clone of the same `Arc`. Both see the same `CacheSet` state. The mutex protecting the LLC (`Arc<Mutex<CacheModel>>`) must be acquired correctly by both paths.

---

### Q33 — How does the LLC model inter-hart coherence?

**Context**

In a multi-hart simulation, all harts share the Last-Level Cache (LLC). When hart 0 writes a cache line, hart 1's L1 cache may hold a stale copy of that line. Real hardware resolves this with a coherence protocol (MESI, MOESI, directory-based). Implementing a full coherence protocol in Phase 0 is a multi-month project (Gem5's Ruby subsystem is ~100K lines). The question is how to handle this for Phase 0 while preserving the ability to add coherence later.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Shared LLC struct (Arc<Mutex<CacheModel>>) — no MESI** | Simple. Correct for single-threaded workloads. LLC state is consistent. Zero coherence protocol overhead. | Incorrect for multi-threaded workloads that rely on cache-to-cache transfer detection. Over-counts LLC misses (each L1 miss goes to LLC regardless of whether another hart's L1 has it). | Gem5 Classic (no Ruby), Sniper (software coherence approximation) |
| **MESI state machine per cache line** | Accurate coherence modeling. Captures cache-to-cache transfer, invalidation, upgrade misses. | Enormous complexity. Each cache line needs 2 state bits and a directory. Full protocol validation required. Out of scope for Phase 0. | Gem5 Ruby, SIMICS |
| **Snoopy bus model (broadcast invalidate)** | Simpler than full MESI. Correct invalidation semantics. | Doesn't scale to many harts. Bus serialization is unrealistic for modern ring/mesh interconnects. | Older simulators |
| **Per-hart private LLC (no sharing model)** | Trivially simple. | Completely wrong: misses the LLC as a shared resource. LLC contention invisible. | Functional-only simulators |

**Answer:** Shared `Arc<Mutex<CacheModel>>` for the LLC. All harts access the same `CacheModel` object under the mutex. No MESI state is tracked. Inter-hart L1 coherence is approximated by assuming that L1 caches are effectively transparent for correctness purposes (SE mode runs single-threaded workloads in Phase 0; full-system multi-hart coherence is Phase 3).

**Rationale:** For Phase 0 single-hart and Phase 1 multi-hart functional simulation, a shared LLC under a mutex is sufficient. The LLC model's primary job in Phase 0/1 is hit-rate and bandwidth accounting, not protocol correctness. The `Arc<Mutex<CacheModel>>` abstraction does not prevent a future Ruby-style per-bank state machine from being slotted in behind the same interface. MESI is explicitly deferred to Phase 3 in the HLD.

**Impact:** `CacheModel::read()` and `write()` must be called while holding the mutex for LLC accesses. Per-hart L1 caches are unshared structs (no lock needed). The mutex is acquired for every LLC access, which will be a contention point at high hart counts — acceptable in Phase 0 where harts run temporally decoupled. In Phase 3, the LLC can be sharded (per-bank mutex) if contention is measured to be significant.

---

### Q34 — Are MSHRs modeled per-cache-level with capacity enforcement?

**Context**

MSHRs (Miss Status Holding Registers) track in-flight cache misses that have been sent to the next memory level but have not yet been filled. Real hardware limits the number of outstanding misses per cache level: typical values are 8–32 for L1D, 16–64 for L2, and 32–64 for LLC. When the MSHR file is full, new misses stall the pipeline. Modeling MSHR capacity is the primary mechanism by which memory-level parallelism (MLP) is accurately captured. Without MSHR capacity enforcement, the model assumes infinite MLP, which overstates performance for bandwidth-bound workloads. Gem5 models MSHRs as a configurable parameter (`mshrs` in `BaseCache`) and enforces the limit.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Per-level MSHR with capacity enforcement** | Accurate MLP modeling. Correctly stalls the pipeline when outstanding misses exceed hardware limits. Exposes MSHR fill rate as a stat. | Must track which cache-line-aligned addresses are outstanding. Adds `MshrFile` struct per `CacheModel`. | Gem5, Sniper |
| **Global MSHR (one file for all levels)** | Simpler. No need to track per-level outstanding. | Incorrect: L1 and L2 have different MSHR budgets. Misses at different levels would compete for the same slots. | — |
| **Infinite MSHRs (no modeling)** | No state overhead. | Assumes perfect MLP. Understates miss penalties for bandwidth-limited workloads by 2–5×. | Purely functional simulators |

**Answer:** MSHRs are modeled per cache level. Each `CacheModel` owns an `MshrFile` with a configurable capacity (default: 8 for L1D, 16 for L2, 32 for LLC, matching typical SiFive U74 and Cortex-A72 configurations). A miss that finds the MSHR file full returns `CacheLookupResult::MshrFull`, which signals the pipeline to stall until an MSHR is freed. MSHR merging (a second miss to the same cache line while an MSHR is already allocated for it) is handled by `MshrFile::is_pending()`.

**Rationale:** MSHR capacity is the dominant factor in memory-level parallelism modeling. Unlimited MSHRs would produce systematically optimistic IPC figures for memory-bound workloads (matrix multiplication, streaming benchmarks). The `MshrFile` is a `HashSet<u64>` of in-flight line-aligned addresses — O(1) lookup, O(1) insert, O(1) remove. The overhead is negligible on the hot path. MSHR merging is also important: a second L1 miss to the same cache line should not allocate a second MSHR; it should wait for the first. The `is_pending()` check handles this correctly.

**Impact:** `CacheConfig::mshrs: u32` is a required field (no default at the type level; the Python param system provides defaults per cache level). `CacheLookupResult::MshrFull { addr }` must be handled by the pipeline stall mechanism in `helm-timing`. Stats: `MshrFile::outstanding()` is exported as a per-level performance counter.

---

### Q35 — Does the TLB model ASID isolation? Which SFENCE.VMA variants in Phase 0?

**Context**

ASID (Address Space Identifier) allows the TLB to hold entries for multiple address spaces simultaneously without requiring a full flush on every context switch. RISC-V's `SFENCE.VMA` instruction has four semantically distinct variants based on whether `rs1` and `rs2` are `x0` (zero register) or a general-purpose register:

1. `SFENCE.VMA x0, x0` — flush all TLB entries for all ASIDs (global flush).
2. `SFENCE.VMA x0, rs2` — flush all TLB entries for the ASID in `rs2`.
3. `SFENCE.VMA rs1, x0` — flush all TLB entries covering the VA in `rs1`, all ASIDs.
4. `SFENCE.VMA rs1, rs2` — flush TLB entries for the VA in `rs1` matching the ASID in `rs2`.

Global entries (PTE G-bit set) are never flushed by ASID-selective variants. A minimal Phase 0 implementation that only flushes all on any `SFENCE.VMA` is functionally correct but degrades performance modeling (over-counts TLB misses after context switches that use ASID-selective flushes).

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **All four variants, all ASIDs** | Full architectural compliance. Accurate TLB miss rate modeling for OS context switches. Supports ASID-based OS optimizations (Linux uses ASID-selective flushes). | More implementation work. Must track `asid: u16` on every `TlbEntry`. Global-entry handling adds a branch. | RISC-V ISA specification requirement |
| **Global flush only (Phase 0 minimal)** | Simplest implementation: ignore `rs1`/`rs2`, always flush all. Architecturally safe (over-flushing is legal). | Over-estimates TLB miss rate after context switches. Makes ASID-aware OS optimizations invisible to the model. | Spike (RISC-V reference), qemu -machine virt Phase 0 |
| **ASID-selective only (no VA-selective)** | Handles the common Linux case (ASID flush on switch_mm). VA-selective is rarely used in isolation. | Incomplete: VA-selective flush (`SFENCE.VMA rs1, x0`) is used by JIT compilers for instruction cache coherence. | — |

**Answer:** All four SFENCE.VMA variants are implemented from Phase 0:
- `flush_all()` — rs1=x0, rs2=x0.
- `flush_asid(asid)` — rs1=x0, rs2=asid.
- `flush_va(va)` — rs1=va, rs2=x0.
- `flush_asid_va(asid, va)` — rs1=va, rs2=asid.

Global entries (G-bit set) are preserved by ASID-selective and VA+ASID flushes. The HLD originally indicated Phase 1 for full ASID-selective support, but the LLD implementation delivers all four variants in Phase 0.

**Rationale:** Implementing all four variants at the `TlbModel` level adds negligible complexity beyond implementing one (the flush logic is a filter over `sets` entries). Deferring ASID-selective flush to Phase 1 would mean that Linux's `switch_mm` path — which uses ASID-selective flushing on ASID-capable hardware — would be incorrectly modeled as a full TLB flush, dramatically over-counting TLB misses in multi-process workloads. The `TlbEntry::asid` and `TlbEntry::global` fields already exist; the four flush methods are straightforward set-filter operations.

**Impact:** RISC-V ISA decoder must decode `SFENCE.VMA` and extract `rs1`/`rs2` register values before calling the appropriate `TlbModel::flush_*` method. The ASID width (`satp.ASID` field) is 16 bits in Sv39. AArch64 equivalent (`TLBI` instructions) will follow the same four-variant pattern in the AArch64 TLB model.

---

### Q36 — Page table walker: function on TLB miss or hardware walker component?

**Context**

When the TLB misses, the physical address must be obtained by walking the page table in memory. Two implementation models are common. A software function: `sv39_walk(satp_ppn, va, asid, access, mem)` is called synchronously, reads PTEs via `FunctionalMem`, and returns a `TlbEntry`. A hardware walker component (SimObject in Gem5 terms): a separate struct connected to the memory system via a port, issuing timing-accurate memory requests for each PTE fetch (each level costs one cache lookup + potential cache miss). Gem5's hardware PTW is a `PageTableWalker` SimObject with a `MemPort` connection; PTE fetches experience real cache latency.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Function calling FunctionalMem** | Simple. Zero infrastructure overhead. Page table walk is transparent from the timing model's perspective (the miss penalty is accounted for by a fixed page-walk latency from `MicroarchProfile`). | Inaccurate: PTE fetches do not experience cache latency. A 3-level Sv39 walk could be 3 L1 misses, 3 LLC misses, or some mix — function model ignores this. | Spike, QEMU (functional mode), fast simulators |
| **Hardware PTW SimObject with MemPort** | Accurate: each PTE fetch costs a real cache access. Captures page-walk cache (PWC) effects. Used by Gem5 in timing mode. | Much more complex: PTW must be a `SimObject` with its own event queue interactions. Page-walk cache (PWC) modeling needed for accuracy. Multi-level walk creates 3 sequential memory accesses. Phase 3 scope. | Gem5, SIMICS |
| **Function with CacheModel-instrumented PTE reads** | Middle ground: PTW is a function but PTE reads go through the L2/LLC `CacheModel` (not L1). Captures LLC PTE hit/miss at low complexity. | L1D PTE caching is ignored. Slightly more complex than pure functional. | Sniper (approximation) |

**Answer:** Page table walker is a function (`sv39_walk`, `aarch64_4k_walk`) that uses `FunctionalMem` (bypassing cache and TLB side effects) to read PTEs. A fixed page-walk penalty from `MicroarchProfile.page_walk_penalty_cycles` is charged on each TLB miss in the timing models. Hardware PTW as a SimObject is deferred to Phase 3.

**Rationale:** For Phase 0 and Phase 1, the functional walker is correct for the primary goal: ensuring virtual-to-physical translation is architecturally accurate (right PTE interpretation, right huge-page detection, right fault conditions). Timing accuracy of PTE fetches is a second-order effect for single-hart SE mode workloads where page table walks are infrequent relative to instruction execution. The function approach keeps `helm-memory` free of event-queue dependencies. The fixed penalty from `MicroarchProfile` is a reasonable approximation that can be tuned against hardware measurements.

**Impact:** `TlbModel::translate()` returns `Err(MemFault::PageFault)` on miss. The caller (ISA execute loop or pipeline stage) calls `sv39_walk()` / `aarch64_4k_walk()` with a `&dyn FunctionalMem` reference. On success, it calls `TlbModel::insert(entry)` and retries the translation. The timing model charges `MicroarchProfile.page_walk_penalty_cycles * walk_levels` on each TLB miss. Page-walk cache (PWC) modeling is a Phase 3 enhancement.

---

### Q37 — How are huge pages (Sv39 gigapages/megapages) handled in the TLB?

**Context**

RISC-V Sv39 supports three page sizes: 4KB (standard), 2MB (megapage, leaf at level 1 of the 3-level page table), and 1GB (gigapage, leaf at level 2). A TLB entry for a huge page maps a much larger virtual-to-physical range in a single entry. The TLB implementation must: (a) detect huge pages during the page walk (leaf PTE at non-zero level), (b) store the page size with the TLB entry, (c) compute the physical address using the correct page-offset mask for the page size, and (d) correctly invalidate huge-page entries on `SFENCE.VMA`. The challenge is that a huge-page VPN is shorter (30 bits for gigapage vs. 39 bits for 4KB) and set indexing must use the correct VPN granularity.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **PageSize enum per TlbEntry, multi-granule check at translate()** | Architecturally correct. Single TlbModel handles all page sizes. Translate() iterates from largest to smallest page size. | `translate()` must check each page size separately, adding 2 extra iterations in the common case (4KB only). | QEMU, gem5, this design |
| **Separate TLB per page size (L-TLB for huge pages)** | Real hardware often has a separate huge-page TLB (STLB for large pages). Avoids multi-granule iteration in the common path. | Two TLB data structures. Flush operations must target both. More API surface. | ARM Cortex-A (separate L-TLB) |
| **Expand huge pages to 4KB entries at insert time** | TlbEntry always maps 4KB. Simpler translate(). | Enormous state blow-up: a 1GB gigapage becomes 262,144 TlbEntry objects. Completely impractical. | — |

**Answer:** `TlbEntry` carries a `size: PageSize` field (`Page4K`, `Page2M`, `Page1G`). `TlbModel::translate()` iterates over configured page sizes (largest first) and checks the VPN at the appropriate granularity for each. Physical address is computed as:
```
pa = (entry.ppn << page_size.trailing_zeros()) | (va & page_size.offset_mask())
```
Huge pages are detected in the page table walker when a leaf PTE is found at a non-terminal level (level 2 for gigapage, level 1 for megapage). The walker validates that the lower PPN bits are zero (misaligned superpage check per RISC-V spec). `flush_va()` and `flush_asid_va()` iterate over all page sizes to invalidate huge-page entries that cover the target VA.

**Rationale:** The multi-granule iteration in `translate()` adds at most 2 extra comparisons in the 4KB-only case (check 1GB: miss, check 2MB: miss, check 4KB: hit). With the configured `page_sizes` list sorted largest-first, the iteration short-circuits on the first hit. This is negligible compared to the branch predictor mispredict that caused the TLB lookup in the first place. The `PageSize` enum on `TlbEntry` is the minimal extra state needed and is architecture-agnostic (AArch64 also supports 2MB and 1GB blocks).

**Impact:** `TlbConfig::page_sizes: Vec<PageSize>` must list the sizes the hardware supports (all three for Sv39; AArch64 supports 4KB/64KB granule options). VPN computation in the walker (`sv39_walk`) uses `level`-dependent shift amounts: level 2 → 30-bit shift, level 1 → 21-bit shift, level 0 → 12-bit shift. The `TlbEntry::vpn` field stores the VPN at the entry's own granularity (not the 4KB VPN), which is necessary for correct set-indexing and flush-VA matching.

---

## helm-timing

---

### Q38 — In Virtual mode, what does tick represent: instruction count or estimated cycle count?

**Context**

`Virtual` is the fastest timing model in Helm-ng: it does not simulate a pipeline or cache hierarchy but still needs to advance a simulated clock so that device timers (UART baud-rate timers, interrupt controllers, watchdogs) fire at approximately the right simulated time. If `tick` = instruction count (1 tick per instruction), device timers are calibrated in instructions, which is ISA-dependent and non-portable. If `tick` = estimated cycle count, device timers are calibrated in cycles, which matches how real hardware timers work (they count clock edges, not retired instructions). Gem5's atomic mode uses an estimated cycles-per-instruction model. Spike has no cycle count.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Tick = instruction count** | Trivial: no IPC estimate needed. Deterministic and reproducible. | Device timers configured in Hz/cycles would need conversion. UART baud rate timers would be wrong by the IPC factor. | Spike (no cycle count), functional simulators |
| **Tick = estimated cycles (IPC model)** | Device timers work correctly without conversion: `fire_at_cycle = current_cycle + timer_interval_cycles`. Configurable IPC matches intended use (SE mode fast-forward with approximate timing). | Requires an IPC estimate. Accuracy is entirely dependent on `MicroarchProfile.virtual_ipc`. | Gem5 atomic mode (fixed IPC), QEMU (no cycle model) |
| **Tick = wall-clock time (real time)** | Trivially reproducible across runs on the same host. | Non-deterministic (depends on host speed). Useless for performance studies. | — |

**Answer:** In `Virtual` mode, `tick` represents estimated cycle count. `Virtual::on_insn()` advances the cycle counter by `ceil(1.0 / ipc)` where `ipc` is `MicroarchProfile.virtual_ipc` (default: 1.0, meaning 1 cycle per instruction). This produces a deterministic, reproducible pseudo-clock that device timers consume via `EventQueue::drain_until(current_cycles)`.

**Rationale:** Device timers in real hardware fire based on clock cycles, not instruction counts. A UART baud-rate divisor of 16 at 50 MHz gives a 3.125 MHz baud clock — expressed in cycles, not instructions. To model this correctly in simulation, the EventQueue must be driven by a cycle counter. The IPC=1.0 default is a reasonable approximation for in-order integer workloads; users targeting FP-heavy or memory-bound workloads can adjust `virtual_ipc` in the profile. The formula `ceil(1.0 / ipc)` ensures the cycle counter is always a non-negative integer.

**Impact:** `MicroarchProfile` must expose a `virtual_ipc: f64` field (range: 0.1–10.0, default 1.0). `Virtual::current_cycles()` returns the accumulated integer cycle count. Python users can configure `sim.timing_model.virtual_ipc = 0.5` to model a 2-CPI machine. The cycle counter must be a `u64` (not `f64`) to avoid floating-point accumulation error over billions of instructions.

---

### Q39 — Does Virtual mode drive the EventQueue, or bypass it?

**Context**

The `EventQueue` is the mechanism by which device timers fire at the correct simulated time. If `Virtual` mode bypasses the queue entirely, device timer events never fire — a UART would never generate a baud-rate interrupt, a platform timer would never expire. If `Virtual` mode drives the queue, it must call `EventQueue::drain_until(current_cycles)` at regular intervals. The question is where and how often the drain call happens. QEMU's TCG fast-path calls timer callbacks at basic-block granularity. Gem5's atomic mode calls `EventQueue::serviceEvents()` at a configurable frequency.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Drive EventQueue via on_interval_boundary()** | Device timers fire. Full-system simulation is possible. Minimal overhead: drain is called at interval boundaries (every 10K instructions), not per-instruction. | Must implement `on_interval_boundary`. EventQueue drain cost is O(k log n) where k = number of events firing. | Gem5 atomic, Sniper Virtual |
| **Drive EventQueue per instruction** | Highest timer resolution. Device events never fire more than 1 cycle late. | Enormous overhead: `drain_until` is called 10^9 times for a 1-second workload. EventQueue overhead dominates. | — |
| **Bypass EventQueue entirely** | Zero overhead. | Device timers never fire. Full-system simulation impossible. SE-mode-only. | Spike, functional simulators |

**Answer:** `Virtual` mode drives the `EventQueue`. `Virtual::on_interval_boundary()` is called every `interval_length` instructions (default 10,000) and calls `EventQueue::drain_until(current_cycles)`. This fires all device timer events whose `fire_at` cycle ≤ `current_cycles`. Without this drain, UART baud-rate timers and interrupt controllers would never fire in Virtual mode.

**Rationale:** The interval-based drain is the correct compromise: it does not add per-instruction overhead, yet device timers fire with a maximum latency of `interval_length / virtual_ipc` cycles (= 10,000 cycles at IPC=1.0), which is within 10µs at 1 GHz — acceptable for timer interrupt modeling. SE mode workloads that don't use the EventQueue at all pay no overhead (the drain on an empty queue is O(1)). Full-system simulation (OS boot) requires device timers to fire.

**Impact:** `Virtual::on_insn()` must track an instruction counter and call `on_interval_boundary()` when the counter reaches `interval_length`. The `EventQueue` reference must be accessible to `Virtual` — either passed to `on_interval_boundary()` as a `&mut EventQueue` parameter (the `TimingModel` trait signature includes this) or stored as a field. The HLD shows `on_interval_boundary(&mut self, eq: &mut EventQueue)` in the trait signature, which is the correct design.

---

### Q40 — How does OoOWindow model inter-instruction dependencies (RAW hazards)?

**Context**

The `OoOWindow` is the core of the Interval timing model. It approximates out-of-order execution by tracking a sliding window of instructions (128–512 entries, configurable) and computing the critical path through that window. The critical path length determines the interval's CPI. To compute the critical path, the model must track RAW (Read-After-Write) hazards: instruction B depends on a result from instruction A; B cannot issue until A completes. The Sniper simulator tracks this via a `reg_ready` table: for each register, the simulated cycle at which the instruction writing it completes. A dependent instruction's issue cycle is `max(all_src_reg_ready_cycles)`.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **reg_ready[64] table (Sniper approach)** | O(1) per instruction: read `reg_ready[src1]`, `reg_ready[src2]`, compute issue cycle, write `reg_ready[dst]`. Simple, fast, correct for the dominant RAW hazard case. | Ignores WAW (write-after-write) and WAR (write-after-read) — these are handled by register renaming in real OoO hardware, so ignoring them is architecturally correct for rename-capable cores. | Sniper |
| **Full dependency graph (DAG of instructions)** | Can compute the true critical path including memory dependencies. More accurate for complex dependency chains. | O(window_size²) in the worst case for chain-following. Significant memory allocation per interval. Complex to implement. | Academic critical-path analysis tools |
| **Single-issue in-order model (no OoO)** | O(1), simplest possible. Correct for 5-stage in-order cores. | Ignores ILP. Understates IPC for OoO cores by 2–4× on ILP-rich workloads. Misrepresents OoO cores. | Virtual mode |

**Answer:** `OoOWindow` maintains a `reg_ready: [Cycles; 64]` table (64 integer registers; separate tables for FP and vector registers if needed). For each instruction: `issue_cycle = max(reg_ready[src0], reg_ready[src1], current_front_of_window_cycle)`. The instruction completes at `issue_cycle + fu_latency`. `reg_ready[dst] = issue_cycle + fu_latency`. Memory RAW hazards (load-to-use) are approximated using the cache model's hit/miss result: a load that hits L1 contributes `L1_hit_latency` to the consumer's readiness; a load that misses goes to the LLC miss penalty.

**Rationale:** The `reg_ready` table approach is the Sniper design, validated against real hardware across dozens of published papers. It captures the dominant source of CPI variance (dependency chain length on critical path) at O(1) per instruction. The simplification of ignoring WAW/WAR is architecturally justified: modern OoO cores with physical register files eliminate WAW/WAR via renaming. Memory RAW through the cache model is the key extension that makes Interval mode more accurate than simple IPC=constant models.

**Impact:** `OoOWindow` must decode each instruction's source and destination registers. This requires a thin decode layer in `IntervalTimed` that extracts `(src_regs, dst_reg, fu_class)` from the `InsnInfo` struct provided by `TimingModel::on_insn()`. `InsnInfo` must include `src_regs: &[RegId]`, `dst_reg: Option<RegId>`, and `fu_class: FuClass` for the OoO model to function correctly. The window size is `MicroarchProfile.ooo_window_size` (default: 128).

---

### Q41 — Interval boundary trigger: fixed instruction count or miss event?

**Context**

An "interval" in the Sniper sense is a segment of execution between two boundary events. At each boundary, the `OoOWindow` computes the CPI for the preceding interval and resets. The choice of boundary trigger has a significant impact on the accuracy of the model. A fixed instruction count (e.g., every 10K instructions) is simple and predictable. A miss event (boundary fires on every cache miss) is more accurate because cache misses are the dominant source of CPI variance — the interval model best approximates the pipeline behavior within a stretch of instructions that either all hit or all miss.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Fixed instruction count** | Simple. Deterministic interval lengths. Easy to reason about overhead (fixed number of `on_interval_boundary` calls per billion instructions). | May split a cache miss event in the middle of an interval, creating inaccurate CPI estimates for that interval. | Sniper (secondary trigger) |
| **Miss event (every cache miss)** | Most accurate: each interval represents a coherent execution phase (hit-dominated or miss-dominated). The critical path within the interval is dominated by either compute or memory, not mixed. | Variable interval length. A miss-heavy workload fires many boundaries, increasing overhead. Very short intervals (1 instruction between misses) degenerate the model. | Sniper (primary trigger) |
| **Both (first to fire)** | Captures both temporal coherence (miss boundary) and overhead bound (instruction count limit). Best of both worlds. | Slightly more implementation complexity: must check miss event in `on_mem_access` and count in `on_insn`. | Sniper (actual design: 10K instruction cap + miss event) |

**Answer:** Both triggers are used: a cache miss fires an immediate interval boundary, and a fixed count of 10,000 committed instructions fires a boundary if no miss occurred first. Both triggers call `on_interval_boundary()`. The miss trigger is implemented in `on_mem_access()`: if `CacheLookupResult::Miss` is returned, `on_interval_boundary()` is called before returning from `on_mem_access()`. The instruction count is tracked in `on_insn()`.

**Rationale:** This is the Sniper design, chosen because it matches the analytical underpinning of the interval model: intervals should be bounded both by execution phases (miss events separate hit-dominated from miss-dominated phases) and by a safety maximum (prevent very long intervals where the approximation diverges from reality). The 10,000-instruction cap is a configurable parameter in `MicroarchProfile.interval_length` to allow tuning for different workload characteristics.

**Impact:** `IntervalTimed` must track `insn_since_boundary: u64` in `on_insn()` and call `on_interval_boundary(eq)` when it reaches `interval_length`. `on_mem_access()` must check if the result is a miss and call `on_interval_boundary(eq)` immediately. Both paths reset `insn_since_boundary = 0`. Overhead analysis: a workload with 1% miss rate and 10K interval length fires ~100K miss-triggered boundaries per billion instructions — well within acceptable overhead.

---

### Q42 — How does IntervalTimed handle branch misprediction penalty?

**Context**

Branch misprediction flushes the instruction fetch pipeline and re-fetches from the correct target address. In a real OoO processor, the penalty is approximately the number of pipeline stages from fetch to execute (typically 10–20 cycles for modern deep pipelines). In the Interval model, there is no branch predictor and no pipeline flush mechanism. The model must approximate misprediction penalty without cycle-accurate pipeline state. The dominant approaches are: (1) charge a fixed penalty per misprediction from the microarchitecture profile, or (2) use a bimodal/2-bit saturating counter predictor to estimate misprediction frequency and charge dynamically.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Fixed penalty from MicroarchProfile** | Simple. Deterministic. The penalty value can be calibrated against hardware measurements. No predictor state to maintain. | Ignores branch type (unconditional vs. indirect vs. conditional). Does not capture predictor warm-up. | Sniper (interval model simplification), this design |
| **Bimodal predictor + fixed penalty** | More realistic misprediction rate. Captures loop-predictable vs. irregular branch behavior. | Adds 1 bit per branch PC (or 2-bit saturating counter) of predictor state. ~32KB for a 16K-entry predictor. Still uses fixed penalty. | Sniper (more detailed mode) |
| **Full tournament predictor (TAGE/ITTAGE)** | State-of-the-art prediction accuracy. Matches measured misprediction rates for realistic workloads. | Enormous implementation complexity. Overkill for an interval model whose primary accuracy lever is the cache model, not the branch predictor. | Gem5 `DecoupledBPredUnit` |

**Answer:** Fixed penalty from `MicroarchProfile.branch_mispredict_penalty_cycles` (default: 15 cycles). `TimingModel::on_branch_outcome(taken, predicted)` is called by the ISA execute loop. When `predicted != taken` (misprediction), `IntervalTimed` adds `branch_mispredict_penalty_cycles` to the current CPI accumulator for the interval. No branch predictor state is maintained.

**Rationale:** The Interval model's primary accuracy driver is the cache miss model, not the branch predictor. Branch misprediction contributes ~5–15% of total CPI for typical integer workloads — significant but secondary to LLC miss penalties (which can contribute 30–60%). A fixed penalty charged on measured mispredictions (from the ISA execute loop, which knows the actual branch outcome) gives a reasonable first-order approximation. A bimodal predictor can be added as a `MicroarchProfile` option in Phase 2 without changing the `TimingModel` trait.

**Impact:** `InsnInfo` (passed to `on_insn`) must carry `is_branch: bool`. The ISA execute loop must call `on_branch_outcome(taken, predicted)` after resolving each branch. For `Virtual` mode, `on_branch_outcome` is a no-op. The `IntervalTimed` implementation accumulates branch penalty in `cpi_stack.branch_penalty: Cycles` per interval and includes it in the CPI computation at interval boundary.

---

### Q43 — Does Interval mode maintain a software cache model or use helm-memory CacheModel?

**Context**

The Interval model requires cache miss/hit information to: (a) trigger interval boundaries on misses, (b) determine the cache miss penalty for the CPI stack computation, and (c) track miss rates for performance statistics. Two approaches: maintain a separate software cache model inside `IntervalTimed` (like Sniper's Graphite cache model, which is independent of the hardware simulation), or share the `helm-memory` `CacheModel` directly. Sharing requires `IntervalTimed` to hold an `Arc<CacheModel>` reference.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Separate software cache model in IntervalTimed** | IntervalTimed is self-contained. Can be used without `helm-memory`. Sniper uses this approach (Graphite model). | Two cache models exist simultaneously: one in `helm-memory` for actual memory dispatch, one in `IntervalTimed` for CPI computation. Their states can diverge. | Sniper (Graphite model) |
| **Share helm-memory CacheModel via Arc\<CacheModel\>** | Single source of truth for cache state. No divergence. Cache state persists correctly across intervals (Q32). Stats are unified. | `helm-timing` gains a compile-time dependency on `helm-memory`. Tighter coupling. | This design |
| **Query via callback/trait object** | Decoupled: `IntervalTimed` calls a `CacheLookup` trait object that `helm-memory` implements. | Extra indirection per access. | Conceptual |

**Answer:** `IntervalTimed` holds an `Arc<CacheModel>` (or `Arc<Mutex<CacheModel>>` for LLC) shared with `helm-memory`. Miss/hit outcomes come from the real cache model. This is why Q32 (cache state persistence across intervals) is answered by the `helm-memory` design rather than the timing model.

**Rationale:** A separate software cache model would require maintaining two independent cache state machines in sync, which is error-prone and wasteful. The shared `Arc<CacheModel>` means `on_mem_access()` in `IntervalTimed` calls `cache.read(addr)` or `cache.write(addr)` — the same call that `helm-memory`'s atomic/timing paths make. The result (hit/miss/MSHR full) is used for both the CPI stack computation and the actual memory access dispatch. This is simpler and more correct than Sniper's dual-model approach.

**Impact:** `helm-timing` crate must have `helm-memory` as a dependency (the HLD DAG shows this as an allowed edge). `IntervalTimed::new()` takes `Arc<CacheModel>` arguments for each cache level (L1D, L2, LLC). The `TimingModel::on_mem_access()` implementation in `IntervalTimed` calls the cache model, checks the result, and fires an interval boundary on miss.

---

### Q44 — Accurate pipeline depth: 5-stage in-order or full OoO from day one?

**Context**

`AccuratePipeline` is the cycle-accurate timing model in Helm-ng. The question is whether Phase 0 implements a simple 5-stage in-order pipeline (IF→ID→EX→MEM→WB) or jumps directly to a full out-of-order microarchitecture (ROB, issue queue, reservation stations, LSQ, physical register file). A 5-stage in-order pipeline models cores like the SiFive E31 (RV32IMC, in-order). Full OoO models cores like the SiFive U74, Cortex-A72, Apple M-series. Most modern application-class RISC-V and AArch64 cores are OoO.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **5-stage in-order (Phase 0)** | Achievable in Phase 0 scope. Correct for simple cores (embedded RISC-V, Cortex-M). Establishes pipeline framework that OoO can be layered onto. | Significantly understates IPC for OoO application cores. Not useful for Cortex-A72 / SiFive U74 accuracy targets. | Gem5 MinorCPU, Sniper (simple core) |
| **Full OoO from day one** | Immediately models the target hardware accurately. No intermediate inaccurate phase. | Multi-month implementation. ROB, RS, physical register file, LSQ, store buffer, forwarding paths — each is a significant design effort. Phase 0 will miss its timeline. | Gem5 O3CPU, SIMICS |
| **OoO window approximation in Accurate mode** | A wider OoO window (like Interval mode) but at cycle granularity. Faster to implement than full OoO, more accurate than 5-stage. | Still an approximation. Structural hazards require careful modeling. | Sniper (hybrid) |

**Answer:** 5-stage in-order pipeline (IF→ID→EX→MEM→WB) in Phase 0. `AccuratePipeline` implements stall and forwarding logic for data hazards. Full OoO (ROB, RS, LSQ) is deferred to Phase 3 as a new struct `AccurateOoO` that implements the same `TimingModel` trait.

**Rationale:** The 5-stage pipeline establishes the correct architectural foundation: pipeline register types, stall/flush mechanisms, forwarding paths. These concepts are reused in the OoO implementation. Attempting full OoO in Phase 0 would consume the entire Phase 0 timeline on pipeline infrastructure, with no time for the ISA, memory system, or device infrastructure. The `TimingModel` trait abstraction means `AccuratePipeline` can be replaced by `AccurateOoO` at compile time without changing any other component.

**Impact:** `AccuratePipeline` stalls on load-use hazards (2-cycle stall in a 5-stage pipeline with 1 MEM stage). It stalls on structural hazards using the functional unit latency table from `MicroarchProfile`. Branch taken causes a 2-cycle flush (ID and IF stages are squashed). These conservative stall estimates model the SiFive E31 accurately and will be refined when `AccurateOoO` is implemented for the U74.

---

### Q45 — Structural hazards: exact (track every FU cycle) or approximate (latency table)?

**Context**

A structural hazard occurs when two instructions need the same hardware resource at the same cycle. In a 5-stage pipeline, the primary structural hazard is functional unit (FU) contention: a divide unit may be non-pipelined (takes 10–20 cycles, blocking a second divide from issuing). An exact model tracks the exact cycle at which each FU becomes free, stalling new instructions if the required FU is busy. An approximate model charges a fixed latency from a table (`MicroarchProfile.fu_latencies`) and inserts pipeline bubbles for the required number of cycles.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Exact FU occupancy tracking** | Correct structural hazard modeling. Captures non-pipelined FUs (divider, FP sqrt). Necessary for accurate throughput on FP-intensive workloads. | Must maintain a `FuBusy: HashMap<FuClass, Cycles>` table tracking when each unit becomes free. Slightly more complex. | Gem5 O3CPU |
| **Latency table (approximate)** | Simple: `stall_cycles = fu_latency[insn.fu_class] - 1`. No FU occupancy state needed. | Assumes only one instruction uses each FU class per window. Incorrect for back-to-back divides (would not stall the second). Overstates throughput for non-pipelined FUs. | Sniper (interval model), Gem5 MinorCPU (simplified) |
| **Per-FU occupancy with pipelined/non-pipelined flag** | Correct for both pipelined (MUL: 3-cycle but next MUL can issue every cycle) and non-pipelined (DIV: 20 cycles, blocks). | One extra bool per FU class in the profile. Slightly more state. | Gem5 FUPool |

**Answer:** Per-FU occupancy tracking with a `FuBusy: [Cycles; FuClass::COUNT]` table in `AccuratePipeline`. Each element stores the simulated cycle at which that functional unit becomes free. A new instruction stalls in the EX stage if `FuBusy[insn.fu_class] > current_cycle`. On issue, `FuBusy[insn.fu_class] = current_cycle + fu_latency`. `MicroarchProfile.fu_latencies` and `MicroarchProfile.fu_pipelined` provide the per-class configuration.

**Rationale:** The latency-table-only approach is a known source of IPC overestimation for FP-intensive and divide-heavy workloads. The occupancy table is trivially cheap: a `[u64; 8]` array (one per FU class), checked and updated once per instruction in the EX stage. This gives correct structural hazard modeling without the full complexity of Gem5's FU pool. It is essential for accurate modeling of the SiFive U74's integer divide unit (non-pipelined, 34-cycle latency for 64-bit divide).

**Impact:** `FuClass` enum must cover: `Int`, `Branch`, `Mul`, `Div`, `FpAdd`, `FpMul`, `FpDiv`, `Load`, `Store`. `MicroarchProfile` must have `fu_latencies: HashMap<FuClass, u8>` and `fu_pipelined: HashMap<FuClass, bool>`. For pipelined FUs, `FuBusy` is not advanced on issue (throughput = 1/cycle). For non-pipelined, `FuBusy` is advanced by the full latency.

---

### Q46 — Does AccuratePipeline reuse helm-memory CacheModel or have its own?

**Context**

The pipeline's MEM stage must access the cache to compute load/store latency. Two options: reuse the `CacheModel` from `helm-memory` (same `Arc<CacheModel>` shared with the `IntervalTimed` model and the atomic/functional paths), or maintain a pipeline-internal cache model. The helm-memory HLD Q46 answer notes: "Pipeline won't know about cache internals; its job is to do task at each stage; fetch should use a MemFetcher which internally uses cache+mmu+walk+mem access."

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Reuse helm-memory CacheModel via Arc** | Consistent cache state across all timing modes. Mode-switching preserves cache state. Unified stats. Cache is always the authoritative source. | `helm-timing` depends on `helm-memory`. Pipeline uses `Arc` lock for LLC. | This design |
| **Pipeline-internal cache model** | Pipeline controls all timing decisions including cache. No lock contention from shared state. | Two cache models diverge in state. Cannot switch timing modes mid-simulation and preserve cache state. Stats are split. | Gem5 (each CPU has its own caches, not shared with memory subsystem directly) |
| **Abstract via MemFetcher trait** | Pipeline calls `MemFetcher::fetch_insn()` / `MemFetcher::load(addr)` returning `Cycles`. Cache internals hidden behind the trait. | The trait implementation still wraps `CacheModel`; this is an API question, not a sharing question. | This design (layering approach) |

**Answer:** `AccuratePipeline` accesses the cache via a `MemFetcher` abstraction (a thin trait over `CacheModel` + `TlbModel` + page table walker). The concrete implementation of `MemFetcher` holds an `Arc<CacheModel>` shared with `helm-memory`. The pipeline does not directly access `CacheModel` internals; it calls `mem_fetcher.load(va, width) -> (data, latency_cycles)` and `mem_fetcher.store(va, width, data) -> latency_cycles`. Cache state is shared and consistent.

**Rationale:** The `MemFetcher` abstraction respects the "pipeline won't know cache internals" constraint from the design note. The pipeline stage knows it stalls for `latency_cycles` returned by `mem_fetcher.load()` — whether that latency comes from an L1 hit (4 cycles) or an LLC miss (100+ cycles) is encapsulated. Sharing the `Arc<CacheModel>` ensures that a future mode-switch from `AccuratePipeline` to `IntervalTimed` mid-simulation (e.g., fast-forward past OS boot in accurate mode, then warm-start interval mode) preserves the cache state.

**Impact:** A `MemFetcher` trait must be defined (likely in `helm-memory` or `helm-core`) with methods `load()`, `store()`, `fetch_insn()`, and `flush_tlb()`. `AccuratePipeline::new()` takes a `Box<dyn MemFetcher>`. The concrete `HelmMemFetcher` struct in `helm-memory` holds `Arc<CacheModel>` + `TlbModel` + a `FunctionalMem` reference for page table walks.

---

### Q47 — How is load-store reordering modeled? RISC-V RVWMO vs AArch64 weak model?

**Context**

Memory ordering defines the order in which load and store operations become visible to other harts. RISC-V uses RVWMO (RISC-V Weak Memory Ordering), a variant of TSO (Total Store Order) with certain relaxations. AArch64 uses a similar weak model. Both are weaker than x86's TSO. Enforcing memory ordering in simulation requires a Load-Store Queue (LSQ): loads can bypass older stores to different addresses, but must observe stores to the same address. This is a significant implementation component. In a 5-stage in-order pipeline (Phase 0), all loads and stores are serialized by the in-order issue policy — RVWMO/weak ordering is trivially satisfied because there is no reordering.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Trivially serialized (in-order Phase 0)** | No LSQ needed. Correct by design: in-order execution serializes all memory ops. No RVWMO violations possible. | Not representative of OoO cores where load-store reordering is intentional. LSQ needed for Phase 3. | Gem5 MinorCPU (in-order) |
| **Full LSQ (Phase 3)** | Models RVWMO/AArch64 memory model accurately. Captures load-to-store forwarding. Required for correctness with multi-threaded workloads. | Major implementation effort. Phase 3 scope with OoO pipeline. Requires memory fence (`FENCE`, `DMB`) modeling. | Gem5 O3CPU, SIMICS |
| **FENCE instruction honored, relaxed otherwise** | `FENCE` instructions cause the pipeline to drain. All other memory ops are reordered freely within the window. Partially correct. | Complex to get right without a full LSQ. May produce incorrect ordering for programs that don't use explicit fences. | — |

**Answer:** Phase 0 in-order pipeline serializes all loads and stores. Memory ordering is trivially correct: a store always completes before the next instruction issues; a load always completes before dependent instructions issue. `FENCE` instructions (RISC-V) and `DMB`/`DSB` instructions (AArch64) stall the pipeline until all in-flight memory operations complete (i.e., they are no-ops in Phase 0 beyond a 1-cycle pipeline stall). LSQ and RVWMO enforcement are deferred to Phase 3 with the OoO implementation.

**Rationale:** The Phase 0 in-order pipeline cannot have memory ordering violations by construction. Every load precedes the instruction that uses its result; every store precedes the instruction that follows it. This is architecturally correct for in-order cores and is a correct simplification for SE mode, which runs a single process on a single hart (no inter-hart ordering concerns). RVWMO/weak model enforcement only matters for multi-threaded programs using lock-free synchronization — Phase 3 scope.

**Impact:** The ISA execute loop must detect `FENCE` / `DMB` / `DSB` instruction classes and call `pipeline.drain_memory()` (a no-op in Phase 0, but will be implemented in Phase 3 to drain the LSQ). `AccuratePipeline` must track load/store completion in the MEM stage but does not need a reorder buffer for Phase 0.

---

### Q48 — Is MicroarchProfile immutable after construction?

**Context**

`MicroarchProfile` holds the parameters that all three timing models use: IPC, functional unit latencies, cache geometry, branch penalty, OoO window size, and pipeline depth. If it is mutable at runtime, Python scripts can change parameters mid-simulation (e.g., hot-swap the branch penalty). If it is immutable, a new `HelmEngine` must be constructed for each parameter configuration. Immutability is simpler to reason about (no race conditions, no need for locking) but less flexible for parameter sweeps.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Immutable after construction** | Thread-safe (no locks needed on reads). Simpler mental model: profile is a constant during simulation. Parameter sweeps require re-construction (acceptable: construction is fast). | Python cannot tweak parameters mid-simulation. Changing one field requires JSON reload or a new profile object. | This design |
| **Mutable with Mutex** | Python can adjust parameters dynamically. Enables adaptive timing (e.g., scale IPC based on sampled workload). | Mutex overhead on every profile field access (hot path). Race conditions if timing models cache field values. | — |
| **Copy-on-write with versioning** | Python creates a modified clone; new version takes effect at the next interval boundary. | Complex: timing models must check for profile updates at interval boundaries. | — |

**Answer:** `MicroarchProfile` is immutable after construction. Fields are private with `pub fn` getter methods. Python can read values via the getters but cannot write them after the profile is built. A new profile requires constructing a new `HelmEngine<T>`. The profile is loaded from JSON at construction time via `MicroarchProfile::from_json(path)` or `MicroarchProfile::from_str(json_str)`.

**Rationale:** The timing model's hot path calls `profile.fu_latencies[fu_class]` and `profile.branch_mispredict_penalty_cycles` on every instruction. If these were behind a `Mutex`, the lock/unlock cost would be paid on every instruction — 10^9 times per simulated second. Immutability eliminates this overhead entirely. Parameter sweeps — the primary use case for mutability — are best expressed as a loop in Python that creates a new `HelmEngine` with a modified profile for each run. `HelmEngine` construction is `O(1)` (just allocating and zeroing cache arrays), so this is fast.

**Impact:** `MicroarchProfile` must derive `Clone` so that Python can create a modified copy: `new_profile = profile.with_branch_penalty(20)` returns a new instance. The `with_*` builder methods are the preferred Python-facing mutation API. `HelmEngine::new(isa, timing, profile)` takes ownership of the profile; subsequent `engine.profile()` returns a `&MicroarchProfile` reference.

---

### Q49 — Which profiles ship with Helm-ng v1?

**Context**

`MicroarchProfile` instances are loaded from JSON files. Helm-ng must ship a set of built-in profiles that users can use without collecting hardware measurements. The minimum viable set covers a generic in-order core and a generic OoO core. Stretch targets are real silicon profiles for the SiFive U74 (used in HiFive Unmatched and StarFive boards) and the Cortex-A72 (used in Raspberry Pi 4 and many cloud instances).

| Profile | Target Hardware | Accuracy Target | Priority |
|---------|----------------|----------------|----------|
| `generic-inorder.json` | Hypothetical simple 5-stage core | Structural placeholder | Required (Phase 0) |
| `generic-ooo.json` | Hypothetical 4-wide OoO core | Structural placeholder | Required (Phase 0) |
| `sifive-u74.json` | SiFive U74 (HiFive Unmatched) | ±15% IPC on SPEC CPU | Phase 1 |
| `cortex-a72.json` | Cortex-A72 (RPi 4) | ±15% IPC on SPEC CPU | Phase 1 |

**Answer:** Four profiles ship with Helm-ng v1: `generic-inorder.json`, `generic-ooo.json`, `sifive-u74.json`, and `cortex-a72.json`. All four are embedded via `include_str!` in the `profiles/` module so they are available without file system access. They are also installed as JSON files alongside the binary for user inspection and modification.

**Rationale:** Embedding the profiles in the binary ensures that `helm validate` and the test suite work in any environment without requiring the profiles directory to be present. Four profiles cover the two primary use cases: generic development/testing (in-order and OoO) and real hardware validation targets (U74 and A72). Both real silicon profiles have publicly documented microarchitecture guides (SiFive U74 Manual, ARM Cortex-A72 TRM) and publicly available SPEC CPU benchmark results for calibration.

**Impact:** A `profiles/` directory in `helm-timing/src/` holds the four JSON files. The `MicroarchProfile::builtin(name: &str)` method returns a pre-parsed profile by name, enabling `sim.timing_model = MicroarchProfile::builtin("sifive-u74")` from Python without specifying a file path. Profile JSON schema must be documented (or validated via a JSON Schema file) so users know how to create custom profiles.

---

### Q50 — How does helm validate compare against real hardware?

**Context**

`helm validate` is the tool that compares simulated performance counter values against measured hardware values. Validation is essential for establishing the accuracy of the timing model. Two approaches: pre-collected counters (hardware is measured offline, results stored as JSON, validator replays the workload in simulation and compares), or live counters (validator reads hardware counters in real time via `perf_event_open` or similar). Pre-collected is portable (no hardware required to run validation) and reproducible. Live counters are more automated but require the target hardware to be present.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Pre-collected JSON counter dumps** | Hardware-independent: validation runs anywhere. Reproducible: same counters every run. Easy to include in CI. Can collect from remote hardware. | Must collect hardware data separately. Data becomes stale if hardware firmware changes. | Most academic simulators |
| **Live perf_event_open counters** | Always up-to-date. Automates the measure-compare loop. Can run on the validation machine itself. | Hardware-dependent: must run on the specific target. Requires `perf_event_open` permissions. Not portable. | gem5's `validateModel` scripts (manual process) |
| **Comparison against reference simulator (Spike)** | Always available. Validates functional correctness, not timing accuracy. | Cannot validate IPC or cache miss rates — only instruction count and register state. | Spike regression tests |

**Answer:** `helm validate` uses pre-collected performance counter JSON dumps. The workflow is: `helm validate --profile sifive-u74.json --counters collected.json --workload dhrystone`. Helm replays `dhrystone` in simulation with the `sifive-u74` profile, then compares simulated counter values (IPC, L1D miss rate, LLC miss rate, branch misprediction rate) against the values in `collected.json`. A tolerance of ±15% is the pass threshold for Phase 1.

**Rationale:** Pre-collected counters are the right approach for a tool that needs to run in CI without requiring specific hardware. The `collected.json` files can be committed to the repository alongside the profiles, enabling any developer to run `helm validate` and confirm that profile changes have not degraded accuracy. For users who have target hardware, a companion tool `helm collect --profile sifive-u74.json --workload dhrystone` can generate fresh `collected.json` files using `perf stat`. The ±15% tolerance is consistent with published Sniper accuracy benchmarks.

**Impact:** A `collected/` directory in `helm-timing/` holds reference JSON files (one per `profile + workload` combination). The JSON schema: `{ "workload": "dhrystone", "hardware": "sifive-u74", "ipc": 1.23, "l1d_miss_rate": 0.021, "llc_miss_rate": 0.003, "branch_mispredict_rate": 0.012 }`. The `helm validate` subcommand is implemented in `helm-ng/src/bin/helm.rs` and returns a non-zero exit code if any counter exceeds the tolerance — suitable for CI assertion.

---

## helm-event

---

### Q51 — Max pending events without performance degradation?

**Context**

The `EventQueue` is a min-heap (`BinaryHeap`) keyed on `fire_at: Cycles`. Rust's `std::collections::BinaryHeap` provides O(log n) insert, O(1) peek (min element), and O(log n) pop. The question is: how many simultaneously pending events are typical for a simulated system, and what initial capacity should the heap have to avoid reallocations during simulation warmup? A typical device configuration includes: UART baud-rate timer (1 event), platform timer (1–4 events per hart), PLIC/GIC pending interrupt events (1–8), disk DMA completion timers (1–4), and scheduler quanta (1 per hart). This totals well under 100 events for a typical system. At thousands of events, a calendar queue (O(1) average) or ladder queue would be faster.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **BinaryHeap, initial capacity 1024** | Zero reallocation for typical device workloads (<100 events). `std::collections::BinaryHeap` is well-tested, no external dep. O(log 1024) ≈ 10 operations per insert/pop. | At 100K+ pending events (e.g., network simulation with many timers), O(log n) starts to show in profiling. | This design, SIMICS |
| **Calendar queue (O(1) average)** | Constant-time insert and pop for uniform time distributions. Better for large event counts. | Complex to implement correctly (bucket sizing, advance logic). Rarely needed for embedded system simulation. | Conservative cache simulator workloads |
| **Lazy sorted Vec (batch sort at drain time)** | Simple insert (O(1) append). Good if drain intervals are long (batch sort amortizes). | O(n log n) sort at each drain. Bad if events arrive and drain frequently. | — |

**Answer:** `EventQueue` uses Rust's `std::collections::BinaryHeap` with an initial capacity of 1024. The capacity can be overridden with `EventQueue::with_capacity(n)` for workloads that are known to have more events. At the expected device count for Helm-ng v1 (< 20 devices, each posting at most a few events), peak pending events will be well under 200 — far below the threshold where O(log n) is a concern. If profiling shows the heap as a bottleneck at > 10K events, a calendar queue can be substituted behind the same `EventQueue` API.

**Rationale:** 1024 is the smallest power of two that comfortably exceeds expected peak event counts with room for bursts (e.g., 64-interrupt-line PLIC with all lines asserted simultaneously). Avoiding `realloc` during simulation warmup is the primary benefit. The `BinaryHeap` is a known quantity with well-understood performance characteristics. Profiling on the actual simulated workloads should drive any future change to a calendar queue.

**Impact:** `EventQueue::new()` calls `BinaryHeap::with_capacity(1024)`. The cancelled-event `HashSet<EventId>` also needs an initial capacity; 64 is reasonable (most cancellations are one-shot: post a timer, cancel it if the device is reset before it fires). Capacity tuning is a `MicroarchProfile`-adjacent configuration option (or a direct parameter to `HelmSim::build()`).

---

### Q52 — Recurring events: re-post in callback or post_recurring() API?

**Context**

Recurring events fire at a fixed interval (e.g., a UART baud-rate timer at 16× the baud clock, a platform timer at 1 MHz). Two design approaches: (1) the device's event callback calls `eq.post_cycles(interval, class, owner, data)` to schedule the next firing before returning; (2) a `post_recurring(interval, class, owner, data)` API schedules an event that automatically re-fires at the given interval without requiring the callback to re-post. The first approach is explicit and flexible (interval can change); the second is implicit and simpler for fixed-rate timers.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Re-post in callback** | Allows dynamic interval adjustment (e.g., reprogrammable timer divider). Simple event system (no recurring-event state). Callback always has the context to decide whether to re-post. | Device code must not forget to re-post. A bug where the callback returns without re-posting silently stops the timer. | SIMICS, this design |
| **post_recurring() API** | Cannot forget to re-post: the event queue handles it automatically. Reduces callback boilerplate for simple fixed-rate timers. | Harder to cancel (must distinguish recurring from one-shot). Cannot adjust interval without cancel + re-post-recurring. Special case in event queue. | Gem5 `schedule(recurring=True)` |
| **Both APIs** | Covers simple (recurring) and complex (re-post) use cases. | More API surface. Risk of confusion between the two modes. | — |

**Answer:** Devices re-post in their callback. There is no `post_recurring()` API. Inside the event callback, the device calls `eq.post_cycles(interval, class, owner, data)` to schedule the next firing. This allows devices to read their current divider register value and post the new interval dynamically, supporting reprogrammable timers without special-casing in the event queue.

**Rationale:** A `post_recurring()` API would require the event queue to store the interval alongside the event and re-post after firing — adding state and complexity to the queue for a minor convenience. The re-post-in-callback pattern is universal in SIMICS-style simulators and is well-understood by device authors. The downside (forgetting to re-post) is detectable in testing (the timer stops firing, which causes observable simulation misbehavior). The flexibility of the re-post pattern (dynamic interval adjustment) outweighs the minor boilerplate cost.

**Impact:** Device callback signatures must receive a `&mut EventQueue` reference so they can call `post_cycles`. The `EventClass::callback` field must be `fn(owner: HelmObjectId, data: &dyn EventData, eq: &mut EventQueue)`. The `EventQueue` reference passed to the callback is the same queue that the callback is draining from — this means the callback is adding to the queue while it is being drained. The `drain_until` implementation must handle events posted during drain (they will naturally be inserted in the heap and will fire in future drain calls if `fire_at > until_cycle`).

---

### Q53 — Event cancellation: by ID, predicate, or (class + object) pair?

**Context**

A device may need to cancel a pending event before it fires (e.g., a UART baud-rate timer cancelled when the device is reset; a disk DMA timer cancelled when the host aborts the command). Cancellation approaches: by `EventId` (exact: cancel the specific event with this ID), by predicate (cancel all events matching a function), or by `(class, owner)` pair (cancel all events of a given class from a given device object). SIMICS uses `SIM_cancel_event` which takes `(class, object, data)`. Gem5 uses `deschedule(Event*)` (pointer-based exact cancel). The tradeoff is between API flexibility and implementation cost.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **By EventId (exact, opaque u64)** | Simple. No scan of the heap needed — mark the ID as cancelled in a HashSet. O(1) cancel, O(log n) heap pop (skips cancelled events at drain time). IDs are never reused. | Caller must retain the `EventId` returned by `post_cycles`. If the ID is lost, the event cannot be cancelled. | Gem5 (pointer-based, equivalent), this design |
| **By predicate** | Maximum flexibility. Can cancel all events matching any condition. | O(n) scan of the heap for every cancel call. Heap must be traversed because `BinaryHeap` does not support random removal efficiently. | — |
| **By (class + owner) pair** | Natural for device reset: cancel all timer events for this device. No need to track IDs. | Also requires O(n) heap scan unless a secondary index `class+owner → [EventId]` is maintained. | SIMICS |
| **By (class + owner) with ID index** | `cancel_all(class, owner)` is O(k) where k = events for that owner. Maintains an `owner_events: HashMap<(class, owner), Vec<EventId>>` index. | Extra index memory. Insert/delete in index adds O(1) per post/cancel. | Hybrid approach |

**Answer:** Cancellation is by `EventId` (exact, opaque `u64`). `cancel(id: EventId) -> bool` marks the event as cancelled in a `HashSet<EventId>`; the event remains in the heap but is skipped when drained. IDs are monotonically increasing and never reused within a simulation run. This avoids O(n) heap rebuilding. For the common "cancel all events for this device on reset" pattern, devices maintain their own list of `EventId`s and call `cancel(id)` for each.

**Rationale:** The ID-based cancel with a cancelled-event `HashSet` is O(1) for cancel and has minimal overhead at drain time (one `HashSet::contains` check per popped event). O(n) predicate scanning would be unacceptable for an event queue that must be drained at every interval boundary. The requirement to retain IDs is a minor burden for device authors, but it is the standard pattern (devices always retain timer IDs for this purpose). A helper `cancel_all(ids: &[EventId])` convenience method can be added without changing the core mechanism.

**Impact:** `EventId` is a `#[derive(Copy, Clone, Eq, Hash)] pub struct EventId(u64)`. The `EventQueue` maintains `cancelled: HashSet<EventId>` and `next_id: u64`. `drain_until` calls `cancelled.remove(&event.id)` when a cancelled event is popped — this prevents the set from growing unboundedly. `post_cycles` / `post_at` return `EventId` which the caller must store if cancellation may be needed.

---

### Q54 — Events per-clock (per-hart) or global (one queue for all harts)?

**Context**

In a multi-hart simulation, two event queue topologies are possible. Per-hart queues: each hart owns its own `EventQueue`; device events are posted to the hart's queue; harts run asynchronously (temporal decoupling). Global queue: one `EventQueue` is shared by all harts and all devices; all events are totally ordered; harts cannot advance past each other. SIMICS uses per-clock queues (each processor is a "clock domain" with its own queue). Gem5 uses a single global `EventQueue` with per-core `EventManagers`. Most commercial simulators use per-clock queues for scalability.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| **Per-hart queue + one global queue for shared devices** | Temporal decoupling: each hart can run its quantum independently without acquiring a shared lock. Natural fit for multi-threaded simulation. Shared devices (timer, PLIC) use a separate global queue drained by the scheduler. | Two queue types to manage. Devices must know which queue to post to (per-hart vs. global). Scheduler must drain global queue between harts. | SIMICS (per-clock), this design |
| **Single global queue** | Simpler: one drain call handles all events. Total temporal ordering is naturally enforced. | All harts must synchronize at every event post/drain — kills multi-threaded scalability. At 4 harts × 10K instruction interval, the global queue is the synchronization bottleneck. | Gem5 (single global event queue with per-event scheduling) |
| **Per-hart queue, no global queue** | Maximum decoupling. | Shared devices (e.g., a platform timer that asserts an interrupt to all harts) cannot post to a single queue. Each device must have a separate instance per hart — impractical. | — |

**Answer:** Per-hart `EventQueue` + one global `EventQueue` for shared devices. Each hart holds its own `EventQueue`. The timing model on each hart drains its own queue at interval boundaries. Shared devices (PLIC, GIC, platform timer) post to the global `EventQueue` owned by `World`. The scheduler drains the global queue between hart quanta. The split implements SIMICS-style temporal decoupling.

**Rationale:** Per-hart queues are the correct design for scalable multi-hart simulation. With per-hart queues, harts run entirely independently within a quantum — no shared locking on the hot path. The global queue for shared devices is small (typically < 10 events at any given simulated time) and is drained infrequently (once per scheduler round). The design matches SIMICS's proven architecture and avoids the Gem5 global-queue bottleneck that requires careful synchronization at scale.

**Impact:** `HelmEngine<T>` owns a per-hart `EventQueue`. `World` owns a global `EventQueue` and the `Scheduler`. The `Scheduler::run_quantum(hart_idx)` method: (1) drains the global queue up to the hart's current cycle, (2) runs the hart's quantum, (3) calls `hart.timing_model.on_interval_boundary(&mut hart.event_queue)` at each interval. Device constructors receive a `&World` reference and use `world.global_event_queue()` to post events. Per-hart device events (e.g., a per-hart performance counter overflow) use the hart's own `EventQueue` via a reference passed from the hart's timing model.

---

*End of dq-memory-timing.md — Questions Q25–Q54.*

---

# Design Questions: helm-devices/bus, helm-devices, helm-engine/se, helm-debug (Q55–Q89)

> Enriched design questions covering HelmEventBus, Device trait, register_bank! macro,
> plugin loading, InterruptPin, SE-mode syscalls, GDB stub, trace logger, and checkpoint system.

---

## helm-devices/bus — HelmEventBus (Q55–Q59)

---

### Q55: Should HelmEventBus support synchronous-only callbacks, or also async fn callbacks?

**Context**

`HelmEventBus` is the simulation-internal publish/subscribe mechanism for events such as
`Exception`, `BreakpointHit`, `HartReset`, and device-level interrupts. The choice of
synchronous-only vs. async affects both correctness and the Python integration surface.
SIMICS HAP callbacks are strictly synchronous — device models must never block, and the
simulation loop does not yield during a HAP. QEMU's notifier chains are similarly synchronous.
The motivation for `async fn` support is primarily Python: PyO3's `pyo3-asyncio` crate allows
awaiting Rust futures from Python async code, but this requires the Rust side to produce a
`Future`. Mixing `async` into the callback chain creates executor dependency (Tokio vs. asyncio)
and requires the subscriber to hold its own waker — significantly complicating the core hot path.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Synchronous-only (`fn(&Event)`) | Zero overhead; no executor dependency; simpler panic handling | Python async code cannot `await` inside callback | SIMICS HAPs, QEMU notifiers |
| Async callbacks (`async fn(&Event)`) | Python asyncio integration; non-blocking Python callbacks | Executor required; waker complexity; not fire-and-forget | None in known simulators |
| Sync Rust + async bridge at Python boundary | Rust core stays sync; Python receives events via queue polled by asyncio | Slightly more Python glue; queue adds latency | gem5 Python event bridge (conceptually) |

**Answer:** Synchronous-only callbacks. Device models and Python subscribers must not block.

**Rationale:** The simulation loop is a tight single-thread (or per-hart thread) hot path.
Introducing `async` machinery into `HelmEventBus::fire()` would require an executor on every
simulation thread, impose `Pin<Box<dyn Future>>` heap allocations per callback invocation,
and couple `helm-devices` to a specific async runtime. SIMICS and QEMU have operated
successfully with synchronous-only callbacks for decades. Python async integration is
handled at the boundary: a Python subscriber receives a `crossbeam` channel item and
polls it from its own asyncio event loop, keeping the Rust core clean.

**Impact:** `HelmEventBus` callback signature is `Box<dyn Fn(&HelmEvent) + Send>`. No
async-related dependencies in `helm-devices`. Python async use requires a wrapper queue.

---

### Q56: How are HelmEventBus subscribers protected from each other — catch_unwind per subscriber?

**Context**

When `HelmEventBus::fire()` calls each subscriber in turn, a panic in subscriber N must
not abort subscribers N+1..N+k or — worse — unwind through the Rust simulation stack and
corrupt simulation state. This is the same robustness concern as SIMICS HAP callbacks:
a buggy Tcl or Python HAP handler should not crash the simulator. The `std::panic::catch_unwind`
API allows Rust code to catch panics and convert them to `Result::Err`, provided the closure
is `UnwindSafe`. Device model closures capturing `Arc`s may not be `UnwindSafe` by default,
requiring `AssertUnwindSafe` wrappers.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `catch_unwind` per subscriber | Isolates panics; simulation continues after bad subscriber | Performance cost (though tiny for rare events); `AssertUnwindSafe` boilerplate | SIMICS (C `setjmp/longjmp` equivalent) |
| No isolation — panic propagates | Simpler; catches real bugs immediately | One bad subscriber kills simulation | Most toy simulators |
| Thread-per-subscriber | Full isolation; can kill individual threads | Massive overhead; synchronization for shared state | Not used in simulators |
| Panic hook + log | Log and abort cleanly | Still aborts simulation | Not viable for production use |

**Answer:** `catch_unwind` per subscriber with `AssertUnwindSafe` wrapping. Log the panic
info (downcast `Box<dyn Any>` to `&str`) and continue with remaining subscribers.

**Rationale:** SIMICS uses `setjmp/longjmp` guards around HAP callbacks for exactly this
reason. In a long-running simulation session with many plugins and Python callbacks, one
misbehaving subscriber should not terminate the run. `catch_unwind` overhead is negligible
for event-bus calls (which are not on the per-instruction hot path). The `AssertUnwindSafe`
wrapper is the accepted Rust idiom when the author can reason that partial state is acceptable
after the panic.

**Impact:** All subscriber invocations in `fire()` are wrapped in `catch_unwind`. Panics are
logged to `TraceLogger` at `ERROR` level. A `SubscriberPanicCount` stat counter is incremented.

---

### Q57: Can Python subscribe to HelmEventBus events? GIL strategy?

**Context**

Python is Helm-ng's primary scripting layer. Researchers write analysis callbacks in Python
(e.g., `on_exception(evt)` to log every page fault). These callbacks are `PyObject` callables
held by PyO3. The challenge: `HelmEventBus::fire()` is called from the Rust simulation thread,
which does not hold the Python GIL. Calling a Python callable without the GIL causes a
segfault. Conversely, holding the GIL for the entire simulation run prevents Python from
doing anything else (including receiving ctrl-C). The PyO3 crate provides `Python::with_gil()`
which acquires the GIL, calls the callable, and releases it — but acquisition is expensive
(~1–5 µs) and must be done for every event.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `Python::with_gil()` per callback | Simple; correct; Python sees event immediately | GIL acquisition cost per event; blocks simulation briefly | PyO3 standard pattern |
| Channel to Python thread (GIL stays on Python side) | Simulation thread never touches GIL; Python thread processes at its pace | Events are async; Python cannot pause simulation in callback | gem5 Python hooks (conceptually) |
| Batch events, acquire GIL once per quantum | Single GIL acquisition amortized over many events | Python callback is delayed; cannot react immediately | Performance-optimized hybrid |
| Require Python to release GIL before simulation runs | Cleanest — simulation thread always safe to call Python | Python code must be disciplined; hard to enforce | Some embedding frameworks |

**Answer:** `Python::with_gil()` per callback for correctness in Phase 0. Document that
Python event subscribers impose GIL acquisition overhead, and provide a channel-based
alternative for high-frequency events.

**Rationale:** Correctness first. The GIL-per-callback model is straightforward and
standard in PyO3. For low-frequency events (exceptions, breakpoints, halts) the overhead
is irrelevant. For high-frequency events (every instruction retired), the channel-based
approach is exposed as an opt-in `subscribe_async(queue)` API that enqueues events without
acquiring the GIL, and the Python side drains the queue on its own schedule.

**Impact:** `HelmEventBus` holds `Vec<Box<dyn Fn(&HelmEvent) + Send>>` for Rust subscribers
and `Vec<Py<PyAny>>` for Python callables. `fire()` calls Rust subscribers first (sync),
then acquires GIL once and iterates Python callables.

---

### Q58: Should HelmEventBus support filtering beyond HelmEventKind (object-scoped)?

**Context**

SIMICS `SIM_hap_add_callback_obj` allows subscribing to HAPs on a *specific object* —
for example, "only `Core_Exception` from `cpu0`", not from `cpu1`. Without object-scoped
filtering, a Python callback for page-fault analysis must check `evt.source == "cpu0"`
manually on every event. For simulations with 64 harts each firing exceptions, this is
significant wasted dispatch overhead. The SIMICS model stores (hap_type, object_id) as
the subscription key. QEMU has no equivalent — all notifiers fire globally.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Kind-only filter (current) | Simple; O(subscribers) dispatch | All subscribers pay inspection cost; no object scoping | QEMU notifiers |
| Kind + object-id filter | Matches SIMICS; eliminates irrelevant dispatch | More complex subscription registry; `HashMap<(Kind, ObjId), Vec<Sub>>` | SIMICS `SIM_hap_add_callback_obj` |
| Predicate closure filter | Maximum flexibility; any field | Each subscriber pays O(1) predicate call; no static optimization | Custom event buses |
| Hierarchical topic (MQTT-style) | Expressive; wildcard subscriptions | Complex routing; foreign to simulator design | Message brokers |

**Answer:** Support Kind + optional `HelmObjectId` filter. Subscription API:
`subscribe(kind, source: Option<HelmObjectId>, callback)`. When `source` is `Some`, the bus
skips the callback for events from other objects at dispatch time.

**Rationale:** The SIMICS object-scoped filter is the right model for simulators with
many cores. A 64-hart simulation where every hart fires `ClockTick` events should not
dispatch to a subscriber registered only for `cpu0`. The implementation is a
`HashMap<HelmEventKind, Vec<(Option<HelmObjectId>, Box<dyn Fn>)>>` — minimal complexity
for meaningful dispatch savings. Predicate closures are not added because they prevent
static analysis of subscription topology.

**Impact:** `subscribe()` gains an `Option<HelmObjectId>` parameter. Dispatch loop checks
`source` before invoking callback. Python API: `bus.subscribe(HelmEventKind.Exception, cpu=sim.cpu0, fn=my_cb)`.

---

### Q59: Behavior when a subscriber calls HelmEventBus::fire() recursively?

**Context**

Recursive HAP firing occurs when a subscriber itself triggers a condition that fires
another (or the same) event. SIMICS explicitly allows recursive HAP firing but maintains
a depth counter and logs a warning above depth 10. Without a depth limit, recursive firing
can produce unbounded stack growth or infinite loops (e.g., an exception handler that
triggers another exception). The call stack in a recursive scenario goes:
`fire(A) → subscriber_1 → fire(B) → subscriber_2 → fire(A) → ...`.
This is distinct from re-entrant locking: `HelmEventBus` holds no mutex during dispatch
(using a read-copy-update or snapshot approach), so a `RefCell`-like borrow panic is the
risk if the subscriber list is mutated during iteration.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Allow with depth counter + warning | Matches SIMICS; flexible | Risk of stack overflow if depth unbounded | SIMICS (depth counter, warn at 10) |
| Allow, no limit | Simplest implementation | Stack overflow on infinite recursion | Not recommended |
| Disallow — panic on re-entry | Catches bugs immediately | Breaks legitimate multi-event chains | Too restrictive |
| Queue recursive events for after current dispatch | No stack growth | Events deferred; order becomes non-deterministic | Some event frameworks |
| Allow up to depth N, then queue remainder | Bounded stack; preserves most ordering | More complex implementation | Best practice |

**Answer:** Allow recursive firing up to depth 8 (thread-local depth counter). At depth 8,
log an `ERROR` and queue remaining events to fire after the outermost `fire()` returns.

**Rationale:** SIMICS's depth-counter approach is the right model. Completely disallowing
recursion breaks legitimate patterns (an exception fires a `BreakpointHit` which fires a
`SimulationPaused`). A depth-8 limit prevents stack overflow while allowing realistic
multi-level event chains. Queuing events above the limit preserves eventual delivery
without unbounded recursion. The thread-local counter avoids synchronization cost.

**Impact:** `fire()` increments a `thread_local! { static DEPTH: Cell<u8> }`. Above limit,
events are appended to a `thread_local` deferred queue drained after `fire()` returns at
depth 0. A `stat::event_bus_max_depth` counter tracks observed peak depth.

---

## helm-devices (Q60–Q71)

---

### Q60: Does Device: SimObject or are they orthogonal?

**Context**

`SimObject` is Helm-ng's component lifecycle trait: it provides `elaborate()`, `reset()`,
`attribute_get/set`, and checkpoint participation. If `Device: SimObject`, every device
automatically joins the component tree, gets a `HelmObjectId`, and can be checkpointed
via the attribute system. If they are orthogonal, a device could exist without a full
`SimObject` lifecycle (useful for headless test harnesses or embedded sub-devices that
don't need independent checkpoint identities). The tradeoff is interface weight vs. flexibility.
gem5's `SimObject` is mandatory for all memory system components. SIMICS requires all
devices to be configuration objects (`conf_object_t`).

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `Device: SimObject` (required) | Uniform lifecycle; automatic checkpoint; component tree navigation | All devices must implement full lifecycle; heavier trait | gem5, SIMICS |
| Orthogonal — Device does not require SimObject | Lightweight Device for embedded/test use; optional tree participation | Checkpoint not automatic; must manually handle in World | QEMU (not all `MemoryRegion` owners are `DeviceState`) |
| `Device` requires subset of SimObject (`elaborate` + `reset` only) | Lighter; still ensures lifecycle hooks | Checkpoint still manual | Custom hybrid |

**Answer:** `Device: SimObject`. Every device that can exist in a `World` must participate
in the component lifecycle, including checkpoint. Headless test harnesses use a minimal
`MockSimObject` impl that satisfies the trait boundary without real behavior.

**Rationale:** The SIMICS model is correct for a production simulator. If devices are
orthogonal to the component tree, checkpoint becomes fragmented — some state is in
`HelmAttr` attributes, some is not, leading to partial checkpoint bugs. Requiring `SimObject`
means the checkpoint system can walk the component tree and collect all state uniformly.
The implementation cost (three extra methods) is low; a `#[derive(SimObject)]` proc-macro
provides a default impl for common cases.

**Impact:** `trait Device: SimObject + Send`. Plugin `.so` files must implement `SimObject`.
`register_bank!` generates the `checkpoint_save`/`checkpoint_restore` delegation automatically.

---

### Q61: Device::region_size() — fixed at construction or dynamic?

**Context**

`Device::region_size()` tells `MemoryMap` how many bytes of address space the device
occupies. For most devices (UART, timer, interrupt controller) this is a fixed value
(e.g., 4 KiB). For PCIe devices, BAR size is fixed by the device specification — the
BAR *address* changes when the OS writes to the config space, but the size stays constant.
QEMU's `memory_region_init_io()` takes a fixed size at creation time; PCIe BARs are
fixed-size regions remapped by the PCIe subsystem. Dynamic region_size would require
`MemoryMap` to re-flatten on every call, invalidating the `FlatView` cache.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Fixed at construction | Simple; `FlatView` cache never invalidated by size change | Cannot model devices that resize (rare) | QEMU (fixed size), SIMICS BAR model |
| Dynamic (re-query before each map add) | Handles exotic devices | Must invalidate FlatView on change; callback needed | Not common |
| Fixed, but resettable via `resize_region()` explicit call | Explicit contract; clear invalidation point | More complex `MemoryMap` API | Custom |

**Answer:** Fixed at construction. `region_size()` returns a `u64` set in the constructor
and never changes. If a device conceptually needs different sizes in different operating
modes, it registers multiple fixed-size regions.

**Rationale:** PCIe BARs, the only realistic case for "dynamic size", have their size
fixed by hardware spec and PCI configuration space encoding. The OS remaps the BAR to a
different base address but does not change its size. QEMU's model (fixed size at
`memory_region_init_io`) is proven correct for the full PCIe device class. Making size
dynamic would require `MemoryMap` to subscribe to device change events and re-flatten
on every such notification — an unjustified complexity.

**Impact:** `fn region_size(&self) -> u64` is a `const`-like method. `MemoryMap::add_device()`
reads it once at map time. No invalidation logic needed for size changes.

---

### Q62: How does a device receive its InterruptPin connections?

**Context**

A UART device needs to assert an IRQ line to an interrupt controller (e.g., PLIC or GIC).
The connection between UART's output `InterruptPin` and PLIC's input must be established
somewhere. Three models exist: constructor injection (pin passed at `new()`), attribute
setting (Python sets `uart.irq = plic.irq_in[3]`), or `finalize()` wiring (World connects
everything after all devices are constructed). SIMICS uses attribute-based wiring:
`SIM_set_attribute(uart, "irq_dev", plic)`. QEMU uses `qdev_connect_gpio_out()` called
during board setup code.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Constructor argument | Clear dependency; immutable after construction | Must create pins before devices; circular dep risk | Small simulators |
| `HelmAttr` attribute setting (Python-side) | Pythonic; matches SIMICS model; flexible per-board config | Pins are `Option<InterruptPin>` until set; risk of forgetting | SIMICS |
| `World::finalize()` auto-wiring from config | Single wiring phase; validated once | Requires config schema to express connections | gem5 (params) |
| `qdev`-style `connect_gpio_out(idx, pin)` | Explicit; index-based for multi-pin devices | Board code must call for each connection; ordering matters | QEMU |

**Answer:** `HelmAttr`-based attribute setting, consistent with the `SimObject` attribute
system. A device declares interrupt output pins as named attributes
(`HelmAttr<Option<InterruptPin>>`). Python wires them: `uart.irq_out = plic.irq_in[3]`.
`World::elaborate()` validates all required pins are connected (not `None`).

**Rationale:** The SIMICS model is proven for complex board configurations with dozens of
interrupt connections. Attribute-based wiring is introspectable (Python can print all
connections), undoable (re-assign before elaboration), and consistent with how all other
device parameters are set. The `elaborate()` validation step catches missing connections
early.

**Impact:** `InterruptPin` must be transferable via `HelmAttr`. `World::elaborate()` calls
`device.validate_connections()` (generated by `register_bank!` or implemented manually).
Missing required connections are `HelmConfigError`.

---

### Q63: What is the exact register_bank! proc-macro API? How are side-effect methods attached?

**Context**

`register_bank!` must generate MMIO read/write dispatch for a set of named registers at
fixed offsets. The key design question is how the macro user attaches side-effect functions
(e.g., writing to `THR` should transmit a byte; reading `LSR` should reflect current
FIFO status). Two approaches: the macro generates a trait with `on_write_THR(&mut self, val)`
methods that the user impl-s, or the user provides function paths inline in the macro
invocation. Inline macro syntax is more compact but less IDE-friendly; trait methods
are verbose but discoverable.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Trait with `on_read_X` / `on_write_X` default no-ops | IDE auto-complete; clear override points; no macro magic | Verbose trait impl for every register | DML (Simics) — each register has read/write methods |
| Inline `on_write = device_method` in macro | Compact; all config in one place | IDE support weaker; macro syntax complex | Some Rust embedded HAL macros |
| Closure fields in struct | Maximum flexibility; closures can capture | Not Send unless `Arc`; no serialization possible | Toy simulators |
| Macro generates dispatch, user writes match arms | Explicit; readable generated code | Repetitive; no field binding | Custom HAL crates |

**Answer:** The macro generates a trait `<BankName>Callbacks` with `on_read_<REG>` and
`on_write_<REG>` default no-ops. The device struct implements `<BankName>Callbacks`
overriding only registers with side effects. Side-effect-free registers use the generated
default (read/write the backing field).

```rust
register_bank! {
    Uart16550 {
        RHR @ 0x00 RO: u8,
        THR @ 0x00 WO: u8,
        LSR @ 0x05 RO: u8,
        IER @ 0x01 RW: u8,
    }
}

impl Uart16550Callbacks for MyUart {
    fn on_write_THR(&mut self, val: u8) { self.tx_fifo.push(val); }
    fn on_read_LSR(&self) -> u8 { self.lsr_flags() }
}
```

**Rationale:** The trait-based approach produces generated code that is inspectable,
testable, and IDE-navigable. The user does not need to understand macro internals to add
a side effect — they implement a trait method. Default no-op implementations ensure that
adding a new register to the macro does not break existing code.

**Impact:** `register_bank!` emits a trait, a struct with backing fields, and a `dispatch_read/write(offset, val)` method. The device struct must implement the callbacks trait. `#[derive(SimObject)]` covers checkpoint delegation.

---

### Q64: Does register_bank! generate serde derive automatically for checkpoint?

**Context**

Checkpoint requires serializing all register state. If `register_bank!` generates
`#[derive(serde::Serialize, serde::Deserialize)]` on the backing struct, checkpoint is
automatic. However, serde requires all field types to implement `Serialize`, which may
not hold for `InterruptPin` or other non-trivial fields embedded in the struct. An alternative
is to generate `HelmAttr`-based checkpoint methods instead of serde, consistent with the
`SimObject` attribute system.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Auto `#[derive(serde)]` | Simple; standard Rust ecosystem | All field types must be `Serialize`; couples to serde | Some embedded register crates |
| Generate `HelmAttr` checkpoint methods | Consistent with SimObject attribute system; no serde dep on devices | More generated code; custom format | SIMICS (attribute-based checkpoint) |
| Manual — device author writes checkpoint | Full control | Tedious; error-prone; easy to miss a register | Not recommended |
| Both — serde for registers, HelmAttr for pins/connections | Best of both | Two serialization paths to maintain | Complex |

**Answer:** `register_bank!` generates `HelmAttr`-based checkpoint methods (`save_attrs()`,
`restore_attrs()`), not serde derive. Serde is not introduced as a device-level dependency.
The `HelmAttr` system handles format (CBOR/JSON) at the checkpoint layer.

**Rationale:** Serde would require every field type (including `InterruptPin`, FIFO buffers,
state machines) to implement `Serialize`, creating a transitive dependency that is hard to
maintain in plugin `.so` files. The `HelmAttr` approach is consistent with the attribute
system already required by `SimObject`, and the checkpoint format is controlled at one
level (the checkpoint subsystem), not scattered across every device.

**Impact:** `register_bank!` emits `impl <Name>Checkpoint { fn save(&self) -> AttrMap; fn restore(&mut self, AttrMap); }`. The format (CBOR/JSON) is decided by Q88 at the checkpoint layer.

---

### Q65: How does register_bank! handle registers with different R/W semantics at same offset?

**Context**

Classic UART example: offset 0x00 is THR (write, transmit holding register) when written
and RHR (read, receive holding register) when read. This same-offset different-register
pattern appears in many legacy peripherals (16550 UART, 8250, Z80 SIO). The macro must
model this without having two fields at the same offset collide in a struct.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Separate `RO` / `WO` register entries at same offset | Explicit; models hardware accurately | Macro must track offset-to-register as `(offset, direction)` pair | DML SIMICS model |
| Single field with `rw_split` annotation | Compact syntax | Obscures that read and write are physically distinct | Some HAL crates |
| Dispatch manually for split offsets, macro for normal regs | Full control where needed | Inconsistent style; split regs fall outside macro | Custom |
| Union-like tagged type | Type-safe | Unusual Rust idiom; confusing to contributors | Rare |

**Answer:** The macro supports explicit `RO`, `WO`, `RW` qualifiers per register entry.
Two entries at the same offset with complementary directions are valid: one `RO` entry
provides the read callback, the other `WO` entry provides the write callback. They may
back different struct fields (e.g., `thr_val: u8` and `rhr_val: u8`).

**Rationale:** This directly mirrors DML's register model and the hardware reality. The
macro's dispatch table is keyed by `(offset, direction)` so both entries coexist without
conflict. The generated `dispatch_read(offset)` only matches `RO`/`RW` entries;
`dispatch_write(offset, val)` only matches `WO`/`RW` entries. Writing to a read-only
offset logs a warning and no-ops (or calls `on_write_reserved()` if overridden).

**Impact:** Macro dispatch table type changes from `HashMap<u64, RegEntry>` to
`HashMap<(u64, Direction), RegEntry>`. Unmatched accesses call `on_access_reserved(&mut self, offset, direction)` default handler.

---

### Q66: Can register_bank! generate Python introspection data?

**Context**

A Python debug script should be able to query a device's register map:
`uart.registers()` → `[{name: "THR", offset: 0, width: 8, access: "WO"}, ...]`. This
enables tools like register viewers, fuzzing harnesses, and auto-generated GDB pretty-printers.
The question is whether this introspection data is generated at macro expansion time (compile-time)
or assembled at runtime. Compile-time generation is zero-cost; runtime requires a static table.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Macro generates static `REGISTER_MAP` const array | Zero runtime cost; Python reads via PyO3 attribute | Requires PyO3 bindings on every device (adds dep) | Custom |
| Macro generates `fn register_map() -> &'static [RegInfo]` | Clean API; Python calls via `device.register_map()` | Adds method to Device trait or a new trait | Preferred |
| Separate JSON/TOML register description file | Language-agnostic; tool-friendly | Separate from code; can drift out of sync | SVD files in embedded |
| No introspection — document manually | Zero implementation cost | Not useful for automation | Toy simulators |

**Answer:** `register_bank!` generates a `fn register_map() -> &'static [RegInfo]` method
(where `RegInfo` = `{name, offset, width_bits, access}`). A `RegisterIntrospect` trait
in `helm-devices` provides this method. Python accesses it via `device.register_map()`.
No PyO3 dependency in `helm-devices` itself — the PyO3 binding layer wraps `register_map()`.

**Rationale:** Static `&'static [RegInfo]` is zero-cost at runtime and easily exposed to
Python. Keeping `RegisterIntrospect` as a pure Rust trait with no PyO3 dependency keeps
`helm-devices` free of Python coupling. The binding layer in `helm-python` converts
`&[RegInfo]` to a Python list of dicts.

**Impact:** New trait `RegisterIntrospect` in `helm-devices`. `register_bank!` emits `impl RegisterIntrospect for <Device>`. Python API: `dev.register_map()` returns list of `{name, offset, width, access}` dicts.

---

### Q67: At .so plugin load, what happens if embedded Python class conflicts with existing name?

**Context**

A plugin `.so` may embed a Python class `Uart16550` that it registers in the `helm_ng`
Python module. If another already-loaded plugin or the simulator core has already registered
`Uart16550`, a name collision occurs. QEMU handles this implicitly — QOM type names must be
globally unique and `type_register_static` aborts on duplicate. The design answer for
Helm-ng (from DESIGN-QUESTIONS.md Q67) is that the simulation will not start on conflict,
since plugin access is via a pre-defined interface.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Refuse to load (simulation does not start) | Deterministic; no silent shadowing | Strict; any name collision blocks all plugins | QEMU (type_register aborts) |
| Namespace by plugin filename (e.g., `myplugin.Uart16550`) | No collision possible | Python code must use namespaced names; less ergonomic | Some plugin systems |
| Log warning, last-loaded wins | Lenient; allows override plugins | Silent breakage if unintended | Not recommended |
| Require unique UUIDs instead of names | Globally unique | Ergonomically terrible | Not used |

**Answer:** Refuse to start simulation. At `plugin_load()` time, check the device class
registry for name collisions. If a collision is detected, emit a `HelmConfigError` listing
both plugins and the conflicting class name. Simulation `elaborate()` fails.

**Rationale:** This matches the DESIGN-QUESTIONS.md answer (Q67) and the QEMU model.
Plugin class names are part of the simulation configuration ABI. Silent shadowing would
make debugging extremely difficult — the researcher would not know which UART implementation
was actually running. Failing early is the correct choice.

**Impact:** `DeviceRegistry::register(name, factory)` returns `Result<(), RegistryError::DuplicateName(name, existing_plugin, new_plugin)>`. `World::load_plugin()` propagates this error.

---

### Q68: How are plugin .so files versioned to prevent ABI mismatches?

**Context**

If `helm-devices` changes a trait (adds a method, changes a signature), a plugin compiled
against the old version will have an incompatible vtable and will crash or silently
misbehave at load time. QEMU has no plugin versioning beyond the QEMU binary version —
plugins must be recompiled against the exact QEMU they will run with. The DESIGN-QUESTIONS.md
answer (Q68) is that a simple version check is sufficient.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Exported `HELM_ABI_VERSION` symbol in plugin | Fast check at load; simple | Plugin author must remember to update | Common shared library pattern |
| Hash of public trait signatures embedded in .so | Detects any ABI change automatically | Requires build tooling to compute hash | Some plugin frameworks |
| Semantic version in manifest sidecar JSON | Human-readable; can express compatibility ranges | Extra file; can be wrong if forgotten | Plugin systems with manifests |
| No versioning — recompile required | Zero implementation cost | Crashes instead of clear errors | QEMU |

**Answer:** Each plugin `.so` exports an `extern "C" fn helm_abi_version() -> u32` symbol.
The simulator checks this against `HELM_ABI_VERSION` constant at load time. Mismatch → refuse
to load with a clear error: `plugin 'foo.so' requires ABI v3, simulator is ABI v4`.

**Rationale:** This is the simplest approach that catches the common case (incompatible API
version). The `HELM_ABI_VERSION` constant in `helm-devices` is a `u32` incremented on
any breaking trait change. A plugin template in the Helm-ng SDK includes the boilerplate
`helm_abi_version()` export so authors cannot forget it. This matches the DESIGN-QUESTIONS.md
answer ("simple check should be fine").

**Impact:** `helm-devices` exports `pub const HELM_ABI_VERSION: u32`. Plugin template exports `fn helm_abi_version() -> u32 { helm_devices::HELM_ABI_VERSION }`. `World::load_plugin()` checks before any other registration.

---

### Q69: Can a plugin define multiple device classes in one .so?

**Context**

A plugin might want to bundle a UART and a companion DMA controller in one `.so` because
they share internal implementation. The DESIGN-QUESTIONS.md answer (Q69) is No — one device
class per `.so`. This simplifies versioning, loading, and error attribution.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| One device per .so (enforced) | Clear ownership; simple registry; easy to replace one device | More `.so` files to manage; link overhead per device | Preferred model |
| Multiple devices per .so allowed | Fewer files; shared code easy | Name collision more likely; loading one bad class blocks all | QEMU (many devices per file) |
| Multiple allowed but each has separate registration endpoint | Flexible; clear registration | Complex plugin API | Some frameworks |

**Answer:** One device class per `.so`, enforced. `helm_register_device()` (the required
export) registers exactly one device. If a plugin tries to register a second class, the
second registration fails and the simulation does not start.

**Rationale:** The DESIGN-QUESTIONS.md answer is No, and the rationale holds: one-to-one
`.so`-to-class mapping makes versioning, hot-reload, and error attribution unambiguous.
Shared code between related devices should be in a static library linked into both `.so`
files, not combined into one plugin.

**Impact:** Plugin API requires exactly one `extern "C" fn helm_register_device(registry: *mut DeviceRegistry)` export. Registry tracks source `.so` path per class.

---

### Q70: Is InterruptPin clone-able (multiple subscribers) or one-to-one?

**Context**

Some interrupt controllers (GIC, APIC) accept multiple sources feeding the same interrupt
line (wired-OR). If `InterruptPin` is `Clone`, a device can fan-out to multiple controllers.
SIMICS `signal_interface_t` is one-to-one by convention — DML's `connect` declares one outgoing
connection. QEMU `qdev_connect_gpio_out` is one-to-one per GPIO index, but a device can have
multiple GPIO outputs. For Helm-ng, the question is whether the pin itself is multi-subscriber
or whether multi-subscriber is modeled by a separate "interrupt mux" device.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `InterruptPin: Clone` (fan-out built in) | Convenient for wired-OR; no extra device needed | Shared `Arc` state; thread-safety complexity | Custom |
| One-to-one (SIMICS model) | Simple; clear ownership; matches hardware convention | Wired-OR requires explicit mux device | SIMICS, QEMU (per GPIO index) |
| `InterruptPin` holds `Vec<Arc<dyn IrqTarget>>` | Fan-out at pin level; any number of targets | Pin is heavier; iteration cost | Custom |

**Answer:** One-to-one, matching the SIMICS convention. `InterruptPin` wraps
`Option<Arc<dyn IrqTarget>>`. Fan-out (wired-OR) is modeled by an `IrqMux` device that
accepts multiple inputs and drives one output. `InterruptPin` is not `Clone`.

**Rationale:** The SIMICS DML `connect` model is one-to-one and has served thousands of
device models correctly. Wired-OR interrupts are rare in modern SoC designs (GIC uses
separate lines per peripheral). When needed, an explicit `IrqMux` device is a cleaner
model than making every pin a fan-out hub. One-to-one also makes connection validation
trivial — `elaborate()` checks that each required pin has exactly one target.

**Impact:** `InterruptPin` is `pub struct InterruptPin(Option<Arc<dyn IrqTarget>>)`. Not `Clone`. Fan-out requires explicit `IrqMux` device in the component tree.

---

### Q71: How does InterruptPin::assert() behave if not connected?

**Context**

If a device asserts its IRQ line but no interrupt controller input is wired (e.g., during
unit testing, or a misconfigured board), the behavior must be defined. Options range from
silent no-op (swallows the event) to panic (catches misconfiguration immediately) to
log-warning (debuggable but continues). The correct behavior depends on whether unconnected
pins are a programming error or a valid configuration.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Silent no-op | Easy for unit tests; no crashes | Hides misconfiguration; interrupts lost silently | Some embedded HAL mocks |
| Log warning | Visible in trace; simulation continues | Warning flood if device asserts frequently | Pragmatic choice |
| Panic | Catches misconfiguration immediately | Breaks unit tests; too strict | Test-only mode |
| Configurable (no-op/warn/panic policy) | Flexible | Policy needs to be set; more API surface | Some simulators |

**Answer:** Log a `WARN`-level trace event on first `assert()` to an unconnected pin
(once per pin instance, suppressed after first occurrence). No-op for subsequent calls.
In `--strict` mode (a simulator flag), promote to `HelmConfigError` at `elaborate()` time
for pins marked `#[required]`.

**Rationale:** Silent no-op is too dangerous for production simulations — lost interrupts
cause guest OS hangs that are extremely hard to diagnose. A one-time warning surfaces the
issue without flooding the trace log. The `--strict` mode + `#[required]` annotation gives
board authors a way to make unconnected pins a hard error for boards where all IRQ lines
must be wired. Unit test harnesses use default (non-strict) mode.

**Impact:** `InterruptPin::assert()` checks `self.0.is_none()` and calls `tracing::warn_once!`. Device macro supports `#[required]` attribute on pin fields for strict-mode validation.

---

## helm-engine/se (Q72–Q77)

---

### Q72: Which Linux syscalls are in scope for Phase 0 MVP?

**Context**

SE (syscall-emulation) mode runs statically-linked Linux ELF binaries without a kernel.
The syscall surface required grows rapidly: `hello_world` needs only `write` + `exit_group`,
but `ls` needs `openat`, `getdents64`, `fstat`, `read`, `close`, and `ioctl`. A full shell
requires `fork`, `execve`, `wait4`, and signal handling. The MVP must be scoped precisely
to avoid unbounded Phase 0 work. gem5 SE mode implements ~60 syscalls for its MVP; QEMU
user mode implements ~300.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Minimal: `write`, `exit_group` only | Fastest to implement; proves pipeline | Cannot run any real program | Proof-of-concept only |
| hello_world tier: +`read`, `brk`, `mmap` (anon) | Runs most simple static binaries | Still cannot do file I/O | Reasonable Phase 0 |
| ls tier: +`openat`, `getdents64`, `fstat`, `close`, `ioctl` | Runs ls and similar tools | ~15 syscalls; moderate effort | Good MVP scope |
| bash tier: +`fork`, `execve`, `wait4`, `sigaction` | Runs a shell | fork/exec complexity is large; months of work | Phase 1+ |

**Answer:** Phase 0 MVP implements the hello_world + ls tier (~15 syscalls):
`write`, `read`, `exit`, `exit_group`, `brk`, `mmap` (anonymous only), `munmap`,
`openat`, `close`, `fstat`, `newfstatat`, `getdents64`, `ioctl` (minimal), `uname`,
`writev`. `fork`/`execve`/`sigaction` are Phase 1.

**Rationale:** The ls-tier scope is achievable in Phase 0 and produces a visible, testable
artifact (running `ls` on a static binary). It validates the syscall dispatch framework,
the memory allocator, and the ELF loader without requiring process forking (which requires
modeling multiple address spaces). Unimplemented syscalls return `ENOSYS` and log a `WARN`.

**Impact:** `helm-engine/se/src/syscall/` contains one file per syscall group
(`file.rs`, `mem.rs`, `proc.rs`). Unimplemented syscalls map to `SyscallResult::Enosys`. A `--se-strict` flag promotes `ENOSYS` to simulation abort for debugging.

---

### Q73: How are mmap/munmap handled — host memory directly or virtual address space simulation?

**Context**

`mmap(MAP_ANONYMOUS)` is required for `malloc` (via `brk`/`mmap`). Two implementation
strategies: map to the host process's virtual address space (easy but potentially unsafe
and address-space-limited), or simulate a guest virtual address space allocator that tracks
`[guest_va, guest_va+len)` mappings and backs them with a host `Vec<u8>` (safe, portable).
gem5 SE mode uses a simulated address space: `mmap` allocates from a virtual allocator
tracking the guest's `brk` pointer and heap range. QEMU user mode maps to host address
space directly using `mmap(2)`.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Host `mmap` directly | Simple; zero-copy; OS manages memory | Guest addresses conflict with host; not portable; security concerns | QEMU user mode |
| Simulated guest virtual address space | Safe; portable; correct guest VA isolation | More implementation work; `Vec<u8>` backing has copy overhead | gem5 SE mode |
| Hybrid: `brk` simulated, `mmap` to host | Simpler for heap, realistic for file maps | Inconsistent model; file-backed mmap on host is complex | Some research simulators |

**Answer:** Simulated guest virtual address space, matching the gem5 model. A `GuestAddressSpace`
struct maintains a `BTreeMap<GuestVA, Mapping>` tracking all mapped regions. Backing storage
is allocated from a `Vec<u8>` pool managed by `helm-memory`. Guest `mmap` calls allocate a
guest VA range and a host backing buffer; `MemInterface` translates guest VA to the backing buffer.

**Rationale:** Host `mmap` is unsafe for a simulator (guest addresses may collide with
simulator's own address space, especially for 64-bit guests where addresses are large but
ASLR is different). The simulated approach is what gem5 uses and is the architecturally
correct model: the guest has its own address space that the simulator controls. This also
makes future full-system mode easier — the memory model is consistent between SE and FS modes.

**Impact:** `helm-engine/se/src/mm.rs` implements `GuestAddressSpace`. `mmap` syscall handler calls `GuestAddressSpace::alloc(len, prot, flags)` → `GuestVA`. `MemInterface::read/write` for SE mode uses the `GuestAddressSpace` to resolve guest VA to host slice.

---

### Q74: How are signal delivery handled in SE mode?

**Context**

Linux signals (`kill`, `sigaction`, `sigreturn`) require delivering an asynchronous event
to a running guest process. In SE mode without a real kernel, signals must be simulated:
pending signal state, `sigaction` table, and delivery at safe points (syscall return or
instruction boundaries). This is complex — QEMU user mode has ~2000 lines of signal
delivery code. For Phase 0, deferral is acceptable.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Defer to Phase 1 entirely | Zero Phase 0 cost | Programs using `signal()` crash with `ENOSYS` | Phase 0 scope |
| Minimal: `SIGTERM`/`SIGINT` terminate simulation | Handles Ctrl-C cleanly | No user-space signal handlers | Pragmatic Phase 0 subset |
| Full signal delivery (sigaction, sigreturn stack frame) | Runs programs using signals | Very complex; months of work | QEMU user mode (Phase 1+) |

**Answer:** Defer full signal delivery to Phase 1. Phase 0 implements: `sigaction` returns
success but stores nothing (no-op registration); `kill` targeting self with `SIGTERM` terminates
simulation cleanly; `SIGINT` from the host (Ctrl-C) pauses via `HelmEventBus`. User-space
signal frames are not pushed.

**Rationale:** Signal delivery requires pushing a `sigcontext` frame onto the guest stack,
executing the handler, then `sigreturn` restoring state. This is architecture-specific
(RISC-V vs. AArch64 have different frame layouts) and consumes significant Phase 0 time.
The programs in scope for Phase 0 (`hello_world`, `ls`) do not use signals. Deferral is safe.

**Impact:** `sigaction` syscall handler returns `Ok(0)` and logs `WARN("signal delivery not implemented")`. Phase 1 work item created. `SIGTERM`-self handled as `exit_group(130)`.

---

### Q75: How does the syscall handler access the guest register file?

**Context**

System call arguments are in registers (a0–a6 for RISC-V, x0–x6 for AArch64). The syscall
handler must read these without knowing the concrete ISA type at compile time. The handler
needs a `&mut dyn ThreadContext` (or equivalent) to call `read_reg(RegId::A0)` and
`write_reg(RegId::A0, result)`. The `ThreadContext` trait must expose ISA-agnostic register
access by ABI role.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `ThreadContext::read_reg(AbiReg::Arg0)` — ABI-role enum | ISA-agnostic; handler knows nothing about RISC-V vs AArch64 | Requires `AbiReg` enum with all roles; indirection | Best design |
| Direct `ThreadContext::gpr(index: u32)` | Simpler; handler uses ISA-specific index mapping | Handler must know ABI register numbers per ISA | Mixed concerns |
| Pass `SyscallArgs { nr, a0..a6, sp }` struct | Handler is pure function; no context mutation | Must re-extract from context; return value write still needs context | Clean functional style |
| Separate `SyscallContext` trait | Explicit surface for syscall ABI access | Another trait; more indirection | Some simulators |

**Answer:** The SE syscall entry point extracts arguments into a `SyscallArgs { nr, a: [u64; 6], sp, pc }` struct before calling the handler. After the handler returns `SyscallResult`, the entry point writes the return value back to the context via `ctx.set_syscall_return(val)`. The handler receives `&mut SyscallArgs` and a `&mut GuestAddressSpace`; it does not touch the register file directly.

**Rationale:** Pure-function handlers are testable without a full `ThreadContext`. The
`SyscallArgs` extraction is ISA-specific (done in the SE engine's ISA dispatch layer), but
the handlers themselves are ISA-agnostic. `ThreadContext` does not need an `AbiReg` abstraction
for the common case — only the extraction and return-value injection are ISA-specific.

**Impact:** `helm-engine/se/src/entry.rs` has ISA-specific `extract_syscall_args(ctx: &dyn ThreadContext) -> SyscallArgs` and `write_syscall_return(ctx: &mut dyn ThreadContext, val: i64)`. All handlers in `syscall/` are `fn handle_*(args: &SyscallArgs, ...) -> SyscallResult`.

---

### Q76: How is ISA-specific ABI mapping (RISC-V vs AArch64) expressed in SyscallHandler?

**Context**

RISC-V Linux ABI: syscall number in `a7` (x17), arguments in `a0`–`a5`, return in `a0`.
AArch64 Linux ABI: syscall number in `x8`, arguments in `x0`–`x5`, return in `x0`.
The syscall numbers themselves also differ between ISAs (e.g., `write` is syscall 64 on
RISC-V, 64 on AArch64 — same in this case, but `openat` differs). The dispatch table
must be ISA-specific at the number level, but handlers are shared.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| ISA-specific `SyscallTable: HashMap<u64, fn>` | Correct per-ISA numbers; shared handler fns | Two tables to maintain | gem5 (per-ISA syscall tables) |
| Single table with ISA-tagged entries | One file; explicit | Harder to see "what does RISC-V support" at a glance | Some simulators |
| `SyscallAbi` trait with `nr()`, `arg(n)`, `set_ret()` methods | Clean abstraction | More indirection; one trait impl per ISA | Cleanest OO design |

**Answer:** Two ISA-specific `syscall_table` modules (`riscv/syscall_table.rs`,
`aarch64/syscall_table.rs`) each defining `fn dispatch(nr: u64, args: SyscallArgs, ...) -> SyscallResult`.
Both dispatch to shared handler functions in `syscall/`. Argument extraction is done before
dispatch (per Q75) — the tables only map number → handler function pointer.

**Rationale:** Per-ISA tables are the most maintainable: adding a syscall to RISC-V only
requires editing `riscv/syscall_table.rs`. The tables are simple `match nr { 64 => write::handle(args), ... }` expressions — no HashMap overhead, branch predictor-friendly.
Shared handler functions avoid duplication of the actual syscall logic.

**Impact:** `helm-engine/se/src/riscv/syscall_table.rs` and `aarch64/syscall_table.rs` each implement `fn dispatch(...)`. Shared handlers live in `helm-engine/se/src/syscall/`.

---

### Q77: Virtual filesystem or host filesystem directly?

**Context**

SE mode must handle `openat("/etc/hostname", ...)` from the guest. Options: pass the path
directly to the host OS (simplest, but leaks host filesystem into the simulation), or
intercept paths and reroot them under a guest sysroot directory (`/opt/helm-sysroot/`), or
implement a virtual filesystem in memory. SIMICS SE mode is not applicable (SIMICS does
full-system only). QEMU user mode uses host filesystem directly with optional path remapping
(`-L sysroot`).

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Host filesystem directly | Simplest; zero FS implementation work | Guest can read/write host files; security concern; path `/proc` exposes host | QEMU user mode (default) |
| Sysroot remapping (`/` → `/path/to/sysroot`) | Isolated; common pattern for cross-compilation | Sysroot must be populated; host `/proc` still tricky | QEMU `-L`, gem5 with m5term |
| Virtual filesystem (in-memory) | Full isolation; reproducible | Large implementation effort; filesystem semantics complex | Research simulators |
| Sysroot + host passthrough for `/proc`, `/dev` | Practical balance | Policy complexity; which paths are virtual vs. host | Hybrid approach |

**Answer:** Sysroot remapping for Phase 0. `helm-engine/se` accepts a `--sysroot <dir>`
argument. All guest `openat` paths are resolved relative to the sysroot. `/proc` and `/dev`
return stubbed responses (e.g., `/proc/self/maps` returns a minimal map). Host filesystem
is not accessible outside the sysroot.

**Rationale:** Pure host passthrough creates reproducibility problems (simulation behavior
depends on host state) and security concerns (guest writes modify host files). A virtual
filesystem is too complex for Phase 0. Sysroot remapping is the QEMU `-L` model, is
well-understood, and can be populated by a cross-compilation toolchain's sysroot. The
`/proc` stub handles the most common accesses (`/proc/self/maps` for ASAN, `/proc/cpuinfo`
for runtime detection) without a full virtual filesystem.

**Impact:** `SysrootFs` struct in `helm-engine/se/src/fs.rs` wraps all host file operations. `openat` handler calls `SysrootFs::resolve(guest_path) -> host_path`. `/proc` paths handled by a match table returning static stub data.

---

## helm-debug (Q78–Q89)

---

### Q78: Which GDB RSP packet types are required for minimum viable integration?

**Context**

The GDB Remote Serial Protocol (RSP) is the wire protocol used by GDB and LLDB to communicate
with a debug stub. A minimum viable stub must handle enough packets to allow setting a
breakpoint, running to it, reading registers, and stepping. The research context confirms:
`?`, `g`, `G`, `m`, `M`, `c`, `s`, `z0`, `Z0`, `k`, `D` are the minimum for a "hello world"
debug session. `vCont` is required for multi-thread. `qXfer:features:read` is required for LLDB.

| Packet Group | Packets | Required for | Effort |
|---|---|---|---|
| Halt reason | `?` | Any session | Trivial |
| Register read/write | `g`, `G`, `p`, `P` | Inspect/modify regs | Low |
| Memory read/write | `m`, `M`, `x`, `X` | Inspect/modify mem | Low |
| Execution control | `c`, `s`, `C`, `S` | Run/step | Low |
| Breakpoints | `z0`, `Z0` | Software breakpoints | Low |
| Detach/kill | `D`, `k` | Clean exit | Trivial |
| Multi-thread | `H`, `T`, `vCont`, `qC` | Multi-hart | Medium |
| LLDB/GDB enhanced | `qXfer:features:read`, `qSupported` | Register XML, LLDB | Medium |

**Answer:** Phase 0 required: `?`, `g`, `G`, `p`, `P`, `m`, `M`, `c`, `s`, `z0`, `Z0`,
`k`, `D`, `qSupported`, `qAttached`. Phase 1 adds: `vCont`, `H`, `T`, `qC`, `qXfer:features:read`.
Unknown packets respond with empty reply (`$#00`), which is the RSP "not supported" convention.

**Rationale:** The Phase 0 set covers single-hart debugging: connect, set breakpoint, run,
halt, inspect registers and memory, step, disconnect. This is sufficient to debug SE-mode
binaries and validate instruction correctness. `vCont` and multi-thread support are deferred
to Phase 1 along with multi-hart scheduler support. The empty-reply convention for unknown
packets means GDB degrades gracefully on unimplemented extensions.

**Impact:** `helm-debug/src/gdb/packets.rs` implements each packet as a match arm. A `PacketHandler` trait dispatches to per-packet handlers. RSP framing (`$..#xx`) is in `helm-debug/src/gdb/framing.rs`.

---

### Q79: How does GDB stub interact with the simulation loop — pause Scheduler or separate thread?

**Context**

When GDB sends a `?` or breakpoint is hit, the simulation must halt and the stub must
accept further GDB commands. Two architectural choices: the stub runs in the simulation
thread (blocking it when paused), or the stub runs in a separate thread and uses a
synchronization primitive to pause the scheduler. QEMU's GDB stub runs in a separate
thread; it sets `cpu->stop = 1` (an atomic flag) and the main loop detects it. SIMICS
uses a different mechanism: `SIM_break_simulation()` posts a "break" event to the event
queue, which the scheduler processes at the next quantum boundary.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Separate GDB thread, atomic pause flag | Non-blocking for other I/O; QEMU-style | Thread synchronization; shared state access requires locking | QEMU |
| Simulation-thread-driven (stub runs in sim thread) | No thread sync needed; safe shared access | Simulation thread blocks during debug session; no parallelism | Simple stubs |
| Event-queue break event (SIMICS-style) | Clean integration with scheduler; quantum-boundary halt | Event processing latency; halt not instantaneous | SIMICS |
| Tokio async task | Modern; composable with future Python async | Requires async runtime in helm-debug | Overkill for MVP |

**Answer:** Separate GDB listener thread with an `AtomicBool` halt flag, matching the QEMU
model. The GDB thread accepts TCP connections and parses RSP. When it receives `c`/`s` it
sets `halt_flag = false` and unblocks the simulator (via `Condvar`). On breakpoint hit, the
simulation thread sets `halt_flag = true` and blocks on the `Condvar`. The GDB thread then
drives register reads/writes via a `SimAccess` mutex-guarded channel.

**Rationale:** The separate-thread model is essential for a usable debug experience: the
GDB thread must remain responsive (accepting TCP keepalives, processing commands) while the
simulation is running. If the GDB thread is in the simulation thread, Ctrl-C and GDB
interrupts (`^C` = RSP interrupt byte `0x03`) cannot be processed during execution.
The `Condvar`-based pause matches QEMU's approach and is straightforward to implement.

**Impact:** `helm-debug/src/gdb/server.rs` spawns a thread. `helm-debug/src/gdb/sim_access.rs` provides `SimAccess { halt: AtomicBool, condvar: Condvar, reg_req: Mutex<Channel> }`. The scheduler checks `halt_flag` at quantum boundaries.

---

### Q80: Multi-hart GDB support — vCont packets?

**Context**

GDB multi-thread support requires `vCont` packet handling: `vCont;c:1;s:2` means "continue
thread 1, step thread 2". GDB represents harts as threads with `Hg<tid>` thread selection.
`qC` returns the current thread. For multi-hart simulation, each hart must be independently
steppable and resumable. QEMU implements full `vCont` for SMP guests.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `vCont` full implementation | Multi-hart debug; GDB thread view | Complex scheduler integration; Phase 0 scope risk | QEMU, gem5 |
| `vCont` stub (always continue all) | Passes GDB's vCont negotiation | Not actually per-hart | Interim workaround |
| Defer to Phase 1 entirely | Zero Phase 0 cost | GDB falls back to `c`/`s` which still works for single-hart | Phase 0 correct choice |

**Answer:** Defer `vCont` to Phase 1. Phase 0 GDB stub advertises `vCont` support in
`qSupported` response but only implements `vCont;c` (continue all) and `vCont;s:1` (step
hart 0). Per-hart independent control requires scheduler changes (pause individual hart
while others run) that are a Phase 1 dependency.

**Rationale:** Phase 0 targets single-hart SE mode binaries. Full `vCont` requires the
scheduler to support per-hart step/continue, per-hart halt detection, and per-hart
register access from the GDB thread — all requiring hart-level thread isolation that is
Phase 1 work. Advertising `vCont` in `qSupported` without full implementation is standard
practice (GDB falls back gracefully to `c`/`s` for single-thread targets).

**Impact:** `qSupported` response includes `vCont+`. `vCont` handler implements `c` (all) and `s` for hart 0 only. Unknown `vCont` actions respond with `OK` (treat as continue). Phase 1 tracker item created.

---

### Q81: LLDB support — qXfer:features:read for target XML?

**Context**

LLDB (and modern GDB) use `qXfer:features:read:target.xml:0,fff` to fetch a target
description XML that describes register layouts. Without this, LLDB uses a generic register
set and may misinterpret register sizes or names. For RISC-V, the target XML must declare
all 32 GPRs, CSRs, and FP registers with their GDB numbering. LLDB requires this packet
for correct register display.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Implement `qXfer:features:read` with static XML | LLDB works correctly; GDB enhanced register display | Must maintain per-ISA XML strings | QEMU, OpenOCD |
| Defer — LLDB uses generic register set | Zero Phase 0 cost | LLDB shows wrong register names/sizes | Phase 0 acceptable if LLDB not required |
| Generate XML from `RegisterIntrospect` data | Dynamic; stays in sync with register model | More implementation; XML generation code | Best long-term |

**Answer:** Phase 0 implements `qXfer:features:read` with static per-ISA XML strings.
RISC-V and AArch64 target XMLs are embedded as compile-time string constants in
`helm-debug/src/gdb/target_xml/`. Generated dynamically from `RegisterIntrospect` data is a Phase 1 improvement.

**Rationale:** LLDB is a primary target for researchers using macOS (where LLDB is the
system debugger). Without `qXfer:features:read`, LLDB cannot display RISC-V register names
correctly. Static XML strings are low-effort (copy from QEMU's riscv64-cpu.xml) and solve
the problem immediately. Dynamic generation from `RegisterIntrospect` is the right long-term
solution but adds scope to Phase 0.

**Impact:** `helm-debug/src/gdb/target_xml/riscv64.xml` and `aarch64.xml` embedded via `include_str!`. `qXfer` handler maps `target.xml` to the ISA-appropriate string. ISA determined from `HelmSim` variant at stub creation.

---

### Q82: Ring buffer capacity default? Configurable?

**Context**

`TraceLogger` uses a ring buffer to hold `TraceEvent` records before they are consumed
(written to file, sent to Python subscriber, or inspected in the debugger). The capacity
default affects memory usage. A 1M-event buffer at ~128 bytes/event = 128 MiB — large
for embedded simulations, modest for desktop research. Gem5's `TraceFlag` ring buffer is
not bounded (unlimited vector). SIMICS trace output is immediate (no ring buffer).

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Fixed default 64K events (~8 MiB at 128 B/event) | Reasonable default; bounded memory | May overflow fast for high-frequency trace | Typical embedded trace buffers |
| Fixed default 1M events | Captures longer traces before overflow | 128 MiB minimum memory committed | Desktop research use |
| Configurable at construction, no hard default | Maximum flexibility | User must always specify; no sensible default | Library design |
| Configurable with default 64K | Sensible default + flexibility | Slightly more API surface | Best practice |

**Answer:** Default 65,536 events (64K), configurable via `TraceLogger::with_capacity(n)` or
Python `TraceLogger(capacity=N)`. At construction time, capacity is rounded up to next power
of two for efficient ring index arithmetic. Maximum capacity capped at 16M events (2 GiB
guard).

**Rationale:** 64K events at 128 bytes each = 8 MiB — a sensible default that fits in L3
cache on modern CPUs and does not pressure simulation memory. Researchers running long
traces can increase capacity or use streaming output (write to file on overflow). The
power-of-two rounding is a standard ring buffer optimization (`index & (cap - 1)` instead
of `index % cap`).

**Impact:** `TraceLogger::new()` defaults to 65536. `TraceLogger::with_capacity(n: usize)` for custom capacity. Python: `helm_ng.TraceLogger(capacity=1_000_000)`. A `--trace-capacity N` CLI flag passes through to `World` construction.

---

### Q83: When the ring buffer fills, what is the policy — overwrite oldest, block, or drop new?

**Context**

When `TraceLogger`'s ring buffer is full and a new event arrives, three policies are
possible. Overwrite-oldest preserves the most recent trace (useful for post-mortem: "what
happened right before the crash"). Block-simulation guarantees no events are lost but stalls
the hot path. Drop-new preserves old context but loses recent events (wrong for most use cases).

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Overwrite oldest (circular buffer) | No simulation stall; most recent events preserved | Old events lost; cannot replay from beginning | Most hardware trace buffers (ETM, CoreSight) |
| Block simulation until consumer drains | Zero event loss | Stalls hot path; consumer must be fast | Logging frameworks (log4j blocking appender) |
| Drop new events | Simple; no stall; old context preserved | Loses recent events — wrong for debugging crashes | Not recommended |
| Configurable policy per-logger | Maximum flexibility | More API complexity; policy must be communicated to users | Some frameworks |

**Answer:** Overwrite oldest (circular buffer semantics) as the default. An optional
`on_overflow` callback (registered via `TraceLogger::set_overflow_callback`) is invoked on
each overwrite, allowing Python to flush to disk before the slot is reused. A
`TraceLogger::set_policy(OverflowPolicy::Block)` alternative is available for correctness-critical
analysis.

**Rationale:** Hardware trace buffers (ARM CoreSight ETM, RISC-V Trace Encoder) universally
use circular/overwrite semantics because they cannot stall the CPU. For post-mortem crash
analysis, the most recent N events are what matters — not the oldest. For researchers who
need complete traces, the `Block` policy and a faster consumer (streaming to disk) are the
correct tools. The `on_overflow` callback enables a middle ground: flush to disk on overflow
without blocking.

**Impact:** `RingBuffer<TraceEvent>` implements overwrite as default. `OverflowPolicy` enum: `Overwrite`, `Block`, `DropNew`. `TraceLogger::set_policy(p)` switches. `overflow_count` stat tracks total overwrites.

---

### Q84: Trace output format fixed or pluggable?

**Context**

`TraceEvent` records can be serialized to JSON Lines (human-readable, large), a custom
binary format (compact, fast), or made pluggable (researcher provides a serializer). gem5
uses a custom text format. SIMICS uses a Python `TraceConsumer` object. The output format
affects file size (a busy simulation generates millions of events/second), parse speed,
and tool compatibility.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Fixed JSON Lines | Human-readable; `jq`-parseable; universal tooling | Large (~200 bytes/event); slow serialization | Many research tools |
| Fixed binary (CBOR or custom) | Compact (~32 bytes/event); fast | Requires decoder tool; not human-readable | gem5 binary trace |
| Pluggable `TraceFormatter` trait | Maximum flexibility; user can write to Parquet, etc. | More API surface; formatter must be registered | Ideal long-term |
| JSON default + pluggable override | Best of both; zero cost if no custom formatter | Two serialization paths | Best practice |

**Answer:** Pluggable `TraceFormatter` trait with two built-in implementations: `JsonLinesFormatter`
(default) and `BinaryFormatter` (CBOR via `ciborium`). Researchers implement `TraceFormatter`
to output to custom formats (Parquet, Protocol Buffers, etc.). The formatter is set at
`TraceLogger` construction.

**Rationale:** Locking to JSON Lines would create file-size problems at scale: 1M events/sec
× 200 bytes = 200 MB/sec. Locking to binary prevents human inspection without a decoder.
The pluggable approach with two built-in formatters covers both use cases and allows future
extensibility (e.g., a Parquet formatter for pandas analysis). The trait interface is simple:
`fn format(&mut self, event: &TraceEvent, out: &mut dyn Write) -> io::Result<()>`.

**Impact:** `trait TraceFormatter: Send`. `JsonLinesFormatter` and `BinaryFormatter` in `helm-debug/src/trace/format/`. `TraceLogger::with_formatter(f: Box<dyn TraceFormatter>)`. Python: `TraceLogger(formatter="json")` or `TraceLogger(formatter="cbor")`.

---

### Q85: Python subscription to TraceEvent — GIL strategy?

**Context**

Same GIL challenge as Q57 but for `TraceEvent` subscriptions. `TraceLogger` fires events
from the simulation thread at high frequency (potentially millions/sec). Acquiring the GIL
for every event would serialize the simulation with the Python interpreter, producing a
massive slowdown. This is more critical than `HelmEventBus` subscriptions because trace
events are on the instruction-retired hot path.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `Python::with_gil()` per event | Simple; immediate delivery | Unacceptable overhead at high event rates | Too slow |
| Batch events in `Vec`, acquire GIL once per N events | Amortized cost; configurable batch size | Python sees delayed events; need to choose N | Best for high-frequency |
| `crossbeam` channel; Python polls | Zero GIL overhead in sim thread | Python must poll; events may accumulate | gem5-style bridge |
| Python thread blocks on channel; GIL released while waiting | Python sees events promptly; no polling | Requires Python to manage receive thread | PyO3 thread spawn |

**Answer:** Channel-based delivery. `TraceLogger` enqueues `TraceEvent` into a bounded
`crossbeam` channel (capacity = ring buffer size) without touching the GIL. A Python
`TraceSubscriber` object wraps the receiving end; Python calls `subscriber.drain()` which
acquires the GIL once and returns a list of events. For push-based Python callbacks, a
background Rust thread drains the channel and calls Python with `with_gil()` per batch.

**Rationale:** Direct `with_gil()` per trace event is infeasible at instruction-retired
rates. The channel approach decouples the simulation thread from Python entirely — the
simulation never waits for Python to process events. `drain()` batching amortizes GIL
acquisition. For researchers who need near-real-time Python callbacks, the background
thread + batch approach delivers batches at configurable intervals (e.g., every 1ms of
wall time).

**Impact:** `TraceSubscriber` in PyO3 bindings wraps `Receiver<TraceEvent>`. `subscriber.drain()` → `PyList`. Optional `subscribe_callback(fn, batch_ms=10)` spins a thread calling Python. A `dropped_events` counter tracks channel overflow.

---

### Q86: Checkpoint format versioned? Version mismatch handling?

**Context**

A checkpoint saved with Helm-ng 1.0 must either load correctly in Helm-ng 1.1, or fail
with a clear error. Without versioning, a format change silently produces incorrect
simulation state (wrong register values, wrong memory layout). gem5 checkpoints embed a
gem5 version string but do not enforce compatibility — loading a mismatched checkpoint
silently produces wrong results. SIMICS checkpoints include a version and refuse to load
on major mismatch.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Version in checkpoint header, refuse on mismatch | Safe; clear error | Old checkpoints unusable after breaking change | SIMICS |
| Version in header, warn on mismatch, attempt load | Lenient; may work for minor changes | Silent failures if "attempt load" produces wrong state | Pragmatic but risky |
| No version — load and hope | Zero implementation cost | Silent corruption on format changes | gem5 (effectively) |
| Semantic versioning (major.minor) + migration scripts | Forward compatibility possible | Migration script maintenance burden | Database migrations pattern |

**Answer:** Checkpoint header contains `schema_version: u32` (monotonically increasing integer).
Breaking format changes increment `schema_version`. On load, Helm-ng checks: exact match →
load normally; same major, minor ahead → load with warning; version behind → refuse with
`CheckpointError::IncompatibleVersion { checkpoint: X, simulator: Y }`. A `helm checkpoint-upgrade` CLI command applies migration scripts for N→N+1 upgrades.

**Rationale:** Silent corruption from version mismatch is unacceptable for research reproducibility.
SIMICS's refuse-on-mismatch is correct for major changes. A `helm checkpoint-upgrade` migration
path (similar to database migrations) allows old checkpoints to be used with new Helm versions
without rebuilding simulations. The monotonic `schema_version` integer is simpler than semantic
versioning and unambiguous about ordering.

**Impact:** Checkpoint header struct includes `schema_version: u32` and `helm_version: String` (informational). `CheckpointLoader::load()` checks version before deserialization. `helm-debug/src/checkpoint/migrations/` contains per-version migration closures.

---

### Q87: Checkpoint differential or full-state?

**Context**

Full-state checkpoints save all `SimObject` attributes every time — simple but potentially
large (hundreds of MB for big memory simulations). Differential checkpoints save only what
changed since the last checkpoint — small but require a base checkpoint and a chain to restore.
SIMICS uses differential: "only changed attributes saved". gem5 uses full-state: each
checkpoint is standalone. For Helm-ng, the tradeoff is restore latency (diff requires
replaying chain) vs. storage (full is large).

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Full-state (gem5 model) | Self-contained; simple restore; no chain dependency | Large files; slow to write for big memory | gem5 |
| Differential (SIMICS model) | Small deltas; fast write for incremental saves | Restore requires chain replay; chain corruption blocks restore | SIMICS |
| Full for memory, differential for register state | Practical middle ground | More complex format; two serialization paths | Custom |
| Differential with periodic full base | Bounded chain length; space-efficient | Periodic full checkpoint cost; GC needed | Backup systems |

**Answer:** Full-state checkpoints for Phase 0 and 1. Differential checkpoints are a Phase 2
feature. A full checkpoint is self-contained — restoring it does not require any other file.
Memory contents are written as a binary blob (not serialized as attributes). Register and
device state use the `HelmAttr` attribute format.

**Rationale:** gem5's full-state model is simpler to implement correctly and produces
self-contained checkpoints that researchers can share without worrying about base checkpoint
availability. The restore latency advantage of differential is only significant for simulations
with frequent checkpointing (every few seconds). For Phase 0, the primary use case is
save-once/restore-once for reproducibility, not rapid incremental checkpointing. Differential
is a Phase 2 optimization.

**Impact:** `Checkpoint::save()` serializes all `SimObject` attribute maps + memory binary blob to one file. No chain tracking needed. File size for a 256 MiB memory simulation ≈ 256 MiB + ~1 MiB device state. Compression (zstd) is applied automatically.

---

### Q88: Checkpoint format — JSON, CBOR, or custom?

**Context**

The checkpoint file format must be: (a) correct (no data loss from floating point, large
integers), (b) reasonably compact (256 MiB memory simulation + device state), (c)
debuggable (researcher can inspect a corrupt checkpoint), (d) fast to write/read.
gem5 uses INI-style text + binary memory image. SIMICS uses a custom binary format with
Python-readable attribute strings.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| JSON (attribute state) + raw binary (memory) | Human-readable attributes; memory not encoded | JSON is slow and large for many attributes | Debuggable but slow |
| CBOR (attribute state) + raw binary (memory) | Compact; fast; no encoding overhead for binary values | Not human-readable without decoder | Most compact option |
| Custom binary (entire file) | Maximum performance; full control | Maintenance burden; not ecosystem-compatible | gem5, SIMICS |
| JSON for all (including memory as base64) | Fully human-readable | Absurdly large; base64 adds 33% overhead | Not viable |

**Answer:** CBOR for attribute state (device registers, CPU state, configuration) via the
`ciborium` crate, concatenated with a raw binary memory image prefixed by a length header.
The file structure: `[CBOR header][CBOR attribute map][u64 memory_blob_len][raw memory bytes][zstd frame end]`.
The entire file is zstd-compressed. A `helm dump-checkpoint <file>` CLI command decodes
the CBOR to JSON for human inspection.

**Rationale:** CBOR is the right choice: it is binary (compact, fast), handles all Rust
primitive types without loss, is a published standard (RFC 8949), and has good Rust library
support (`ciborium`). The raw binary memory blob is necessary — encoding 256 MiB as CBOR
byte strings would work but adds unnecessary framing overhead. zstd compression achieves
~3–5× size reduction on typical register and memory content. The `helm dump-checkpoint`
tool provides human-readability when needed.

**Impact:** `ciborium` added to `helm-debug` dependencies. `Checkpoint` file format documented in `docs/design/checkpoint-format.md`. `helm dump-checkpoint` subcommand in `helm-cli`. zstd compression via `zstd` crate.

---

### Q89: HelmAttr sole checkpoint mechanism or manual checkpoint_save() also needed?

**Context**

If `HelmAttr` is the sole checkpoint mechanism, every piece of device state that must be
restored must be exposed as a named attribute. This is the SIMICS model — all checkpointable
state is an attribute, and `SIM_get_attribute`/`SIM_set_attribute` is used to save and restore.
The advantage: uniform save/restore path, Python-introspectable state. The disadvantage:
device authors must wrap every field in `HelmAttr`, which is verbose and may not suit
complex state (e.g., a large FIFO, a state machine with private invariants).

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `HelmAttr` only | Uniform; Python-introspectable; no extra API | Every field must be `HelmAttr`; verbose for complex state | SIMICS |
| Manual `checkpoint_save()/restore()` only | Full control; can serialize complex invariants efficiently | Not introspectable; each device must implement manually | gem5 (serialize/unserialize) |
| Both — `HelmAttr` default, manual override optional | Flexibility; simple devices use attributes; complex use manual | Two paths to test and maintain | Best practice |
| Macro-generated from struct fields | Zero boilerplate; attributes generated automatically | Macro must know which fields to include; #[checkpoint] annotation needed | Custom |

**Answer:** Both mechanisms, with `HelmAttr` as the default path and manual `checkpoint_save()`/`checkpoint_restore()` as an opt-in override. `register_bank!` generates `HelmAttr`-based checkpoint for all declared registers automatically. A device that needs to save complex non-`HelmAttr` state (FIFO contents, DMA scatter-gather tables) implements `CheckpointExt` trait with manual `save()`/`restore()` methods. The checkpoint system calls `CheckpointExt::save()` if implemented, otherwise falls back to collecting all `HelmAttr` values.

**Rationale:** `HelmAttr`-only is too restrictive for devices with complex internal state (FIFO queues, state machines, DMA tables). gem5's `serialize/unserialize` model proves that manual checkpoint is necessary for full correctness. The dual-path approach matches real-world practice: simple registers via `HelmAttr` (automatic, zero boilerplate), complex state via manual `CheckpointExt` (full control). The `register_bank!` macro handles the register-state case automatically, so authors only need to write `CheckpointExt` for the non-register state.

**Impact:** `trait CheckpointExt: SimObject { fn checkpoint_save(&self) -> AttrMap; fn checkpoint_restore(&mut self, AttrMap); }`. `Checkpoint::save()` calls `CheckpointExt::checkpoint_save()` if the device implements it, otherwise collects `HelmAttr` values. `register_bank!` emits `impl CheckpointExt` delegating to generated attribute accessors.

---

*End of Q55–Q89 enriched design questions.*

---

# Design Questions: helm-stats, helm-python, helm-engine/World (Q90–Q110)

> Enriched design questions with context, trade-off tables, answers, rationale, and impact.
> Cross-references: [`../helm-stats/HLD.md`](../helm-stats/HLD.md) · [`../helm-python/HLD.md`](../helm-python/HLD.md) · [`../helm-engine/LLD-world.md`](../helm-engine/LLD-world.md)

---

## helm-stats (Q90–Q93)

---

### Q90 — Should `PerfCounter` use `AtomicU64` (lock-free) or plain `u64` (requires lock)?

**Context**

`PerfCounter` is incremented on the simulation hot path — potentially millions of times per simulated second. Each hart's execute loop calls `counter.inc()` to record cache hits, instruction retirements, branch mispredictions, and similar micro-events. With multiple harts running concurrently (one OS thread per hart in the multi-threaded scheduler), any counter that is shared across harts must be safe under concurrent mutation. The choice of `AtomicU64` vs `u64 + Mutex` determines both the synchronization cost paid per-increment and the visibility of count values across threads. Gem5's `Stats::Scalar` uses an equivalent atomic approach; SIMICS uses per-thread stat buckets that merge at dump time.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `AtomicU64` (lock-free, `Relaxed` ordering) | Zero lock contention; one `fetch_add` instruction on x86/ARM; safe across harts; no deadlock risk | Relaxed ordering means values are not instantly visible across cores — only guaranteed consistent at dump time; slightly larger struct than bare `u64` | Gem5 Stats::Scalar, Linux kernel `atomic64_t` perf counters |
| `u64` + `Mutex<u64>` | Strict visibility; trivially correct | Lock acquisition on every increment; catastrophic contention with 8+ harts; cache-line bouncing | — (rejected in all high-performance simulators) |
| Per-hart `u64` (no sync) + merge at dump | Zero atomic overhead; best cache locality; no contention | Each hart must own its own counter instance; shared counters (e.g., LLC stats) cannot use this; merging adds dump complexity | SIMICS per-thread stats, some QEMU per-vCPU counters |
| `u64` + `RwLock` (read-heavy optimization) | Cheap concurrent reads | Writes still require exclusive lock; worse than atomic for increment-only counters | — (anti-pattern for write-heavy counters) |

**Answer:** `AtomicU64` with `Ordering::Relaxed` for `inc()`, `Ordering::SeqCst` for `get()` at dump time.

**Rationale:** Hot-path performance is non-negotiable. A single `fetch_add(1, Relaxed)` compiles to a single locked instruction on x86 and a `stlxr`/`ldadd` on ARM — a handful of nanoseconds. A mutex-based increment would serialize all harts on a shared counter, defeating multi-hart parallelism entirely. Relaxed ordering is correct for independent counters: each counter only needs a consistent snapshot at dump time, not real-time cross-core visibility. The `SeqCst` barrier on `get()` at dump time ensures all prior `Relaxed` stores are visible before the value is read. Per-hart splitting is considered only for counters proven to be a bottleneck in profiling.

**Impact:** The `AtomicU64` choice means `PerfCounter` is `Send + Sync` and can be held via `Arc<PerfCounter>` without additional wrapping. It eliminates any lock from the critical execute loop. Dump time acquires a consistent snapshot via `SeqCst` load, which is acceptable since dump is a cold-path operation.

---

### Q91 — What is the `PerfFormula` expression language? Can it reference other counters at dump time?

**Context**

Derived statistics — hit rate, CPI (cycles per instruction), bandwidth in GB/s — cannot be stored as raw counts. They are computed from two or more counter values. Gem5 uses a lazy expression tree evaluated at dump time: `Stats::Formula` holds a reference-counted AST; evaluation calls `.result()` on each subtree. The challenge in helm-ng is that formulas must reference other counters by name (e.g., `"system.cpu0.icache.hits"`) without creating reference cycles, and evaluation must be deferred until dump time so that counter values are final. The expression language must be simple enough to be constructed from Python (e.g., `Formula.div(Formula.counter("a"), Formula.counter("b"))`) and correct in the face of division by zero.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Lazy expression tree (Rust enum, heap-allocated) | Arbitrarily composable; no evaluation cost during simulation; division-by-zero safe via `f64::NAN`; serializable for debugging | Heap allocation at formula creation (one-time, cold path); evaluation traverses tree recursively | Gem5 Stats::Formula, RPerf |
| String expression parsed at dump time (e.g., `"hits / (hits + misses)"`) | Human-readable; easy Python authoring | Parser overhead at dump; fragile (typos silently wrong); harder to typecheck at registration | Some custom in-house simulators |
| Closure (`Fn() -> f64`) | Maximum flexibility; no AST overhead | Not serializable; cannot introspect formula structure for debugging; holds `Arc` to counters — manual lifetime management | QEMU (some derived stats) |
| Pre-computed formula (update alongside counters) | Zero dump cost | Requires atomic `f64` or separate lock; loses accuracy (value is stale by the time another counter increments) | — (rejected: defeats purpose of lazy evaluation) |

**Answer:** `PerfFormula` is a recursive Rust enum (expression tree) with node variants `Counter(String)`, `Literal(f64)`, `Add(Box<PerfFormula>, Box<PerfFormula>)`, `Sub`, `Mul`, `Div`. Evaluation at dump time calls `eval(&StatsRegistry) -> f64`, which resolves `Counter` nodes by looking up the named counter in the registry and loading its value with `SeqCst` ordering. Division by zero yields `f64::NAN` (never panics).

**Rationale:** The expression tree is the only design that is composable, debuggable, and zero-cost during simulation. Counter references are by name (string), not by `Arc<PerfCounter>`, which avoids reference cycles and makes formula construction order-independent (the counter need not be registered before the formula). The registry resolves names at eval time. Formulas themselves are registered at `elaborate()` time and are never evaluated during `run()`.

**Impact:** Formula construction is a cold-path operation confined to `elaborate()`. Python can build formulas using `PerfFormula::div(PerfFormula::counter("a"), PerfFormula::counter("b"))` or via a PyO3-exposed builder API. Dump output for `NAN` values emits `"nan"` in JSON and `NaN` in the terminal table, clearly signaling a counter that was never incremented.

---

### Q92 — Should stats output include per-interval snapshots or only final values?

**Context**

Simulation workloads often have a "warmup" phase followed by a region of interest (ROI). Researchers using Gem5 with SimPoint collect stats at checkpoint intervals — each interval is an independent sampling window. Per-interval stats enable phase analysis, IPC variance across program phases, and cache warmup studies. However, per-interval stats require either resetting counters between intervals (destructive) or taking a snapshot of all counter values at interval boundaries and computing deltas (non-destructive). SIMICS does not support per-interval stats natively; users implement them via Python callbacks that call `SIM_get_attribute()` at magic instruction boundaries.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Final values only (Phase 0) | Simple; zero overhead; no snapshot machinery; correct for SE-mode benchmarks that run start-to-finish | Cannot analyze phase behavior; warmup bias in final CPI/hit-rate values | Phase 0 default for all simulators |
| Snapshot at interval boundaries (non-destructive delta) | Preserves cumulative total; enables per-phase analysis without counter reset; composable with `until=roi_start` | Must store snapshot of all counter values at interval boundary; delta computation at dump | Gem5 SimPoint stats, academic simulators |
| Counter reset at interval boundary (destructive) | Simple delta = new value; each interval is independent | Loses cumulative total; no way to recover total from intervals after the fact | Rare; only useful if cumulative total is irrelevant |
| Streaming time-series (event-by-event) | Full trace; post-processing flexibility | Enormous output volume; not feasible for billion-instruction runs | Hardware PMU trace tools (perf record), not simulators |

**Answer:** Phase 0 outputs final values only. Per-interval snapshots are deferred to Phase 1 and will be triggered by `sim.run(until=roi_start)` → `registry.snapshot("warmup")` → `sim.run()` → `registry.snapshot("roi")` → `registry.dump_delta("warmup", "roi")`.

**Rationale:** The complexity of interval snapshots is non-trivial — it requires storing a full copy of all counter values and implementing delta arithmetic across potentially hundreds of counters. Phase 0's goal is correctness on SE-mode benchmarks that have no warmup distinction. The `StatsRegistry` is designed with snapshot support in mind (counters are `AtomicU64` loadable at any time), but the snapshot API is not implemented until the ROI/phase workflow is needed.

**Impact:** The Phase 0 dump API is `registry.dump_json(path)` and `registry.print_table()`, called once after `sim.run()` returns. No interval tracking code ships in Phase 0. The `snapshot()` method is stubbed but unimplemented, returning `Err(NotImplemented)`.

---

### Q93 — How are stats namespaced? By dot-path matching the component hierarchy?

**Context**

When a simulation has 8 harts each with a private L1I, L1D, and shared L2, there are dozens of cache-hit counters. Without a naming convention, `hits` from hart 0's L1I and hart 3's L1D are indistinguishable. Gem5 uses a hierarchical `Stats::Group` where each component owns a group and all counters registered in that group inherit a prefix. The resulting paths (`system.cpu0.icache.hits`) are unique and match the component tree. SIMICS uses a flat attribute namespace on each object (`SIM_get_attribute(obj, "stat_hits")`) — no global hierarchy.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Dot-path mirroring component hierarchy | Human-readable; unique by construction if component names are unique; matches `SimObject` naming; easy to filter with prefix (`system.cpu0.*`) | Path must be constructed correctly by each component; typos in paths lead to silent separate counters | Gem5 Stats::Group hierarchy |
| Flat global namespace (component registers `"cpu0_icache_hits"`) | Simple; no hierarchy machinery | Not scalable; collision-prone; hard to filter by component | SIMICS attributes (per-object, not global) |
| Structured key (component ID + counter name as tuple) | No string allocation; collision-impossible | Not human-readable; harder to query from Python; doesn't dump to text cleanly | Rare; internal profiling tools |
| Auto-generated paths from `HelmObjectId` + counter name | Unique without effort | Not stable across runs if IDs change; not human-readable | — (rejected: usability loss) |

**Answer:** Dot-path namespace mirroring the component hierarchy. Each `SimObject` component receives its path prefix (e.g., `"system.cpu0.icache"`) during `elaborate()` via the `WorldContext`. The component appends counter names as final segments: `format!("{}.hits", self.path)`. The `StatsRegistry` enforces uniqueness at registration time and panics on duplicate paths.

**Rationale:** The dot-path convention is the only one that is both human-readable and automatically unique (assuming unique component names, which the `World` already enforces). It directly maps to the JSON output structure and supports prefix-based filtering (`registry.dump_prefix("system.cpu0")`). Path construction is done once at `elaborate()` — no string formatting on the hot path.

**Impact:** Component implementations follow the pattern `self.hits = reg.perf_counter(format!("{}.hits", path), "...")`. The path prefix is passed through `WorldContext` and `SystemContext`. Wildcard queries like `system.*.icache.hits` (all harts' L1I hits) are a Phase 1 feature.

---

## helm-python (Q94–Q100)

---

### Q94 — Should `helm_ng` expose raw `HelmObject`/`World` API or a higher-level Python DSL?

**Context**

PyO3 can expose Rust types at two levels of abstraction. A raw binding layer exposes `PyWorld`, `PyHelmObjectId`, `PyAttrValue` — thin wrappers around Rust structs with minimal Python ergonomics. A high-level DSL (like Gem5's Python) provides `Cpu(isa=Isa.RiscV)`, `L1Cache(size="32KiB")`, `Simulation(root=cpu)` — objects that read like a platform configuration, not a Rust API. Gem5's entire configuration system is Python; SIMICS's CLI is Python. Both hide the C++ object model behind a DSL. QEMU exposes QMP (JSON protocol) rather than a Python API, placing the DSL burden on the caller.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| High-level Python DSL (`Cpu`, `Cache`, `Memory`, `Board`) | Familiar to Gem5 users; no Rust knowledge required; param names match hardware documentation; easy to extend in pure Python | Extra Python layer to maintain; DSL must stay in sync with Rust component registry | Gem5 Python config, SIMICS Python CLI |
| Raw `HelmObject`/`World` PyO3 API | Minimal maintenance; one-to-one with Rust | Rust terminology leaks into Python; users must know `HelmObjectId`, `AttrValue`, `elaborate()` sequence; not ergonomic | QEMU QMP (JSON, not Python) |
| Hybrid: raw bindings + optional DSL package | Advanced users use raw; casual users use DSL; both maintained | Two surfaces to test and document | helm-ng Phase 0 decision (both layers ship) |

**Answer:** High-level Python DSL as the primary interface, raw `_helm_ng` bindings as a private secondary layer. The DSL (`helm_ng/components.py`) provides `Simulation`, `Cpu`, `L1Cache`, `L2Cache`, `Memory`, `Board`. The raw extension (`_helm_ng`) is not imported by end users.

**Rationale:** The target user is a computer architecture researcher writing platform configurations. They think in `Cpu`, `Cache`, `Memory` — not `HelmObjectId` or `AttrValue`. The DSL enables configurations that read like specifications. The raw layer is preserved for tool authors and testing infrastructure. Separating the two means the DSL can be revised in pure Python without recompiling Rust.

**Impact:** Two deliverables per component type: a `DeviceRegistry` entry in Rust (factory function), and a Python class in `helm_ng/components.py`. The Python class is a thin dataclass that holds `Param.*`-typed fields and converts to a `(type_name, params_dict)` tuple at `elaborate()` time.

---

### Q95 — How are Rust `Result<T, E>` errors propagated to Python?

**Context**

Rust's error model is `Result<T, E>` — callers must explicitly handle errors. Python's error model is exceptions — callers use `try/except`. PyO3 bridges these via `impl From<MyError> for PyErr`. The question is whether Helm maps all errors to a generic `RuntimeError` (lowest friction, least informative) or to a typed exception hierarchy (`HelmMemFault`, `HelmConfigError`, `HelmDeviceError`) that Python code can catch selectively. SIMICS propagates errors via `SIM_clear_exception()` — a global error state that callers poll, not a typed exception. QEMU QMP uses JSON `{"class": "GenericError", "desc": "..."}` — untyped, string-only. Gem5 uses Python exceptions but they originate from `m5.fatal()` which raises `SystemExit`, not a catchable subclass.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Typed exception hierarchy (`HelmError` base with subclasses) | `try/except HelmMemFault` catches faults precisely; attributes carry structured data (`.addr`, `.pc`); Pythonic | More PyO3 boilerplate; each new error type requires registration; exception class must be defined in both Rust and Python | Designed for helm-ng |
| Single `RuntimeError` with structured message string | Minimal boilerplate | Users cannot distinguish error types without string parsing; structured data lost | QEMU QMP (JSON string), many quick-and-dirty bindings |
| Global error state (SIMICS style) | Matches SIMICS patterns | Entirely un-Pythonic; requires explicit poll after every call; misses errors on early return | SIMICS SIM_clear_exception() |
| Panic passthrough | No mapping code | Python crashes with a Rust backtrace; completely unusable in production | — (rejected) |

**Answer:** Typed exception hierarchy rooted at `HelmError`, with subclasses `HelmConfigError`, `HelmMemFault` (attrs: `addr: int`, `fault_kind: str`, `pc: int`), `HelmDeviceError` (attrs: `device_name: str`, `offset: int`), `HelmCheckpointError`. Mapping implemented in `crates/helm-python/src/errors.rs` via `impl From<HelmError> for PyErr`.

**Rationale:** Fault injection is a first-class use case: a test might deliberately write to an unmapped address and need to catch `HelmMemFault` to verify the fault behavior. `RuntimeError` makes this impossible without fragile string matching. Typed exceptions also carry structured attributes, so `except HelmMemFault as e: print(hex(e.addr))` works. The boilerplate is a one-time cost in `errors.rs`.

**Impact:** `HelmMemFault.addr` and `HelmMemFault.pc` are integer attributes set from Rust struct fields via `PyErr::new::<HelmMemFault, _>((addr, fault_kind, pc))`. Python subclasses `HelmError(Exception)` so `except HelmError` catches all helm exceptions. No Rust `panic!` propagates to Python; panics are bugs that crash the interpreter with a message.

---

### Q96 — Does `Simulation.run()` block the Python thread, or release the GIL?

**Context**

CPython's Global Interpreter Lock (GIL) ensures that only one thread executes Python bytecode at a time. When Python calls a native extension that runs a long computation, the GIL is held unless explicitly released. For a single-threaded simulator this doesn't matter — there are no competing Python threads. For helm-ng, which runs multiple harts on OS threads and may run for billions of instructions, holding the GIL means: (1) other Python threads (GUI, progress reporter, log consumer) are completely blocked during the simulation run, and (2) Python callbacks from within the Rust simulation loop would deadlock (the GIL is already held by the simulation thread). Gem5 does not release the GIL because it is single-threaded. PyO3 provides `py.allow_threads(|| { ... })` for GIL release.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Release GIL via `py.allow_threads()` | Other Python threads (progress, GUI, logging) run concurrently; no deadlock risk on Python callbacks from Rust; correct for multi-hart multi-thread | Rust simulation loop must not touch any Python objects after release; callbacks re-acquire GIL, which is expensive if frequent | PyO3 best practice for CPU-bound work |
| Hold GIL (no release) | Simplest; no threading concern | Blocks all Python threads for entire simulation duration; GUI freezes; progress reporting impossible | Gem5 (single-threaded, irrelevant) |
| Run simulation on a separate Python thread (thread + GIL release) | Python main thread remains responsive | Complexity: synchronization between threads; harder to reason about callback timing | — (over-engineering for Phase 0) |

**Answer:** `Simulation.run()` calls `py.allow_threads(|| helm_sim.run(n))`, releasing the GIL for the duration of the Rust execution loop. Python callbacks registered via `HelmEventBus` re-acquire the GIL via `Python::with_gil(|py| { ... })` when called.

**Rationale:** Helm-ng is explicitly multi-threaded (one OS thread per hart). If the GIL is held, the Rust simulation threads do not need GIL access, but Python callbacks from those threads would deadlock. More practically: users running a 10-billion-instruction simulation want to read progress in a second terminal without the interpreter being frozen. GIL release is the correct and standard PyO3 pattern for any CPU-bound native extension.

**Impact:** Every Python object used as a callback must be cloned into a `PyObject` before entering `allow_threads`, since `PyObject` is `Send`. All Rust types used inside `allow_threads` must not hold `Py<T>` references. Python callbacks that are called frequently (e.g., every instruction) will have high GIL re-acquisition overhead — users are advised to use Rust-side subscribers for high-frequency observation.

---

### Q97 — How does `Simulation.run(until="roi_start")` work — what triggers the "until" condition?

**Context**

A canonical simulation workflow is: boot to ROI start → take stats snapshot → simulate ROI → dump stats. The ROI boundary is signaled by the target program executing a "magic instruction" — an otherwise-illegal or NOP-like instruction that the simulator intercepts. SIMICS implements this as `Core_Magic_Instruction` HAP. Gem5 uses `m5ops` (special instructions that trap via `ExecContext::handleLockedWrite`). The "until" mechanism must allow Python to specify a stopping condition that is checked after each significant event without requiring a new `run_until_foo()` API for every condition type.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Python callback `until=fn(event) -> bool` | Maximally flexible; any event type, any field; user can implement counting, state machine, etc. | GIL re-acquisition on every event call; slow if events are dense | Designed for helm-ng; similar to SIMICS HAP model |
| Named string shorthand `until="roi_start"` → expands to a `MagicInsn` callback | Ergonomic for the common case | Limited to pre-defined condition names; not extensible without modifying Rust | Convenience wrapper, implemented in Python DSL layer |
| Synchronous polling (`while sim.last_event() != "MagicInsn": sim.step()`) | Simple; no callback machinery | Terrible performance; `sim.step()` acquires/releases GIL on every instruction | — (rejected) |
| Breakpoint address (`until={"pc": 0x8001000}`) | Hardware-debugger familiar | Only works for PC-based conditions; cannot express event-type conditions | GDB-style breakpoints |

**Answer:** `until` accepts a Python callable with signature `(event: HelmEvent) -> bool`. The callable is stored as a `PyObject` before `allow_threads`. After each `HelmEvent` fires on the event bus, the Rust side acquires the GIL and calls the callable. If it returns `True`, the simulation loop exits. The string shorthand `until="roi_start"` is syntactic sugar in the Python DSL that expands to `lambda e: e.kind == "MagicInsn"`.

**Rationale:** The callback pattern matches SIMICS's HAP model and is the only design that handles arbitrary stopping conditions without enumerating them in the Rust API. The string shorthand satisfies the 80% use case ergonomically. Calling the Python callback on every event is acceptable because `MagicInsn` events are rare (a few per run); the overhead of GIL re-acquisition is amortized over millions of instructions between magic instructions.

**Impact:** `sim.run(until=callback)` does not stop at exactly the first event where `callback` returns `True` in the middle of a quantum — it stops at the next quantum boundary after the callback returns `True`. For single-hart SE-mode simulations without temporal decoupling, the quantum boundary coincides with the event, giving effectively exact stopping. For multi-hart full-system simulations, there is a quantum-sized lag.

---

### Q98 — Are `Param.*` types validated at attribute-set time (Python) or `elaborate()` time (Rust)?

**Context**

Python component objects (`Cpu`, `L1Cache`, etc.) have typed parameters (`Param.Int`, `Param.MemorySize`, `Param.Hz`). Validation can happen at two points: when the user writes `cpu.clock_hz = -1` (immediate feedback), or when `sim.elaborate()` is called and Rust validates the fully assembled parameter set (deferred feedback). Early validation gives a fast traceback pointing to the exact assignment line. Late validation can cross-check inter-parameter constraints (e.g., "L1 must be smaller than L2") that require the full system context. Gem5 uses both: Python `Param` descriptors do type coercion at assignment; Gem5's C++ `SimObject::create()` does range checking.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Type-check at Python assignment (`Param.__set__`) + range-check at `elaborate()` (Rust) | Immediate feedback on type errors; range errors (requiring full context) correctly deferred; matches Gem5 split | Two validation sites; some validation logic duplicated or split | Gem5, designed for helm-ng |
| All validation at `elaborate()` (Rust only) | Single validation site; full context always available | Type errors (`cpu.isa = 42`) not caught until elaborate; traceback points to elaborate, not the bad assignment | Simpler but worse UX |
| All validation at assignment (Python only) | Immediate feedback for all errors | Cannot validate cross-parameter constraints; unit conversion (Hz to cycles) requires `MicroarchProfile.clock_hz` which is not known at assignment time | — (insufficient for unit conversion) |

**Answer:** Type and coercion validation at Python attribute-set time via `Param.*` descriptor `__set__`. Range and cross-parameter validation at `elaborate()` time in Rust, raising `HelmConfigError` for violations.

**Rationale:** Type errors (assigning a string to `Param.Int`) are immediately diagnosable — the Python traceback points exactly to the offending line. This is table-stakes UX. Range errors and unit conversions require cross-component context (`MicroarchProfile.clock_hz` must be known before `Param.Cycles` can validate a nanosecond value), so they must be deferred. The split matches both user expectations and system constraints.

**Impact:** `Param.Int.__set__` calls `int(value)` and raises `TypeError` immediately if the conversion fails. `Param.MemorySize.__set__` accepts `str`, `int` and stores the raw value; range check (`size > 0`, `size <= max_platform_ram`) happens in Rust. `HelmConfigError` at elaborate time includes the param name and component path in its message for diagnostic clarity.

---

### Q99 — How does `Param.MemorySize` parse — `"32KiB"`, `"32768"`, `32768` (int) all valid?

**Context**

Memory sizes appear in user scripts as human-readable strings (`"32KiB"`, `"4MiB"`, `"1GiB"`), raw integer strings (`"32768"`), or bare integers (`32768`). All three must be accepted. The parsing must be unambiguous: `"32K"` could mean 32,000 (SI) or 32,768 (IEC binary). Hardware simulators universally use IEC binary units (1 KiB = 1024 bytes). Gem5's `MemorySize` param accepts `"1kB"` (= 1024 in Gem5's convention) and integers. SIMICS uses `SIM_get_attribute` returning integers only — no string parsing. QEMU configuration uses `"32M"` (SI-ambiguous, treated as 2^20 by QEMU).

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| IEC suffixes only (`KiB`, `MiB`, `GiB`, `TiB`) + bare int | Unambiguous; matches memory spec documents; rejects SI ambiguity | Users used to `K`/`M`/`G` may be confused initially | Preferred for helm-ng |
| Accept both IEC (`KiB`) and SI-ambiguous (`K`, `KB`, `M`) mapping to powers-of-2 | Familiar; accepts sloppy input | `"32K"` = 32×1024 is surprising in SI context; could silently misinterpret | Gem5 `MemorySize`, QEMU |
| Integer-only (no string parsing) | Simple | Unusable — `32*1024*1024` everywhere | SIMICS (per-object attributes) |
| Rich DSL (`32 * KiB`, `4 * MiB` in Python expressions) | Very readable | Requires importing constants; more complex API surface | Some academic simulators |

**Answer:** `Param.MemorySize.__set__` accepts: (1) bare `int` (bytes), (2) `str` with IEC suffix (`"32KiB"`, `"4MiB"`, `"1GiB"`, `"512TiB"`) and optional space (`"32 KiB"`), (3) `str` of a bare integer (`"32768"`). Case-insensitive for suffix. SI ambiguous suffixes (`K`, `M`, `G`, `KB`, `MB`, `GB`) are also accepted and mapped to powers of 2 with a deprecation warning. Result is stored as `int` (bytes).

**Rationale:** Accepting all three forms (bare int, IEC string, numeric string) eliminates all common user friction. The IEC forms are unambiguous and should be preferred; the SI forms are accepted with a warning because many users come from Gem5/QEMU habits. The result is always an integer number of bytes, so downstream Rust code only sees a `u64`.

**Impact:** The parser lives entirely in `helm_ng/params.py` as a `MemorySize` descriptor class, roughly 30 lines. It raises `ValueError` with a clear message on unparseable input. The validated `int` is passed through `AttrValue::Int(i64)` at elaborate time. Values above `i64::MAX` (exotic but possible on future platforms) would need `AttrValue::Uint(u64)` — noted as a Phase 1 concern.

---

### Q100 — How does the Python param system handle unit conversion (Cycles vs Nanoseconds vs Hz)?

**Context**

A user configures `cpu.clock_hz = 2e9` (2 GHz). A cache latency is `cache.read_latency_ns = 4` (4 nanoseconds). An event deadline is `uart.baud_period_cycles = 12`. All three ultimately drive the same simulator timeline — the `VirtualClock` measured in ticks (cycles at the processor clock). Conversion between Hz, nanoseconds, and cycles requires `clock_hz` to be known, which is only available after `elaborate()` resolves the full `MicroarchProfile`. SIMICS uses `simtime_t` (double seconds) everywhere and converts to cycles internally. Gem5 uses `Tick` (picoseconds) everywhere — all values are converted at construction using a global `SimClock::Frequency`.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Defer conversion to `elaborate()` when `clock_hz` is known | Correct; conversion uses actual configured clock; no global state required | Param must carry units metadata through PyO3 boundary; Rust performs conversion | Designed for helm-ng |
| Global `SimClock.set(hz)` in Python before elaborate | Familiar (Gem5 pattern) | Global state; breaks multi-simulation-per-process use cases; must be set before any param | Gem5 (`m5.ticks.fixGlobalFrequency(hz)`) |
| User must always specify cycles (no unit conversion) | Simple | Unusable: `cache.read_latency = int(4e-9 * 2e9)` everywhere | — (rejected: terrible UX) |
| Accept all units, convert at set time using a provisional clock frequency | Can give early feedback | Clock frequency may not be known yet; conversion could be wrong if clock later changes | — (incorrect for reconfigurable clocks) |

**Answer:** `Param.Ns`, `Param.Hz`, `Param.Cycles` are distinct descriptor types that store the raw user value plus a unit tag. At `elaborate()` time, Rust receives `AttrValue::Tagged { value: f64, unit: "ns" | "Hz" | "cycles" }`. The Rust elaborate pass resolves `MicroarchProfile.clock_hz` first, then converts all `Ns` and `Hz` params to cycles using `cycles = ns * clock_hz / 1e9` and `cycles = 1.0 / hz * clock_hz`. The conversion result is stored as the param's runtime value.

**Rationale:** Conversion at `elaborate()` time is the only correct approach: `clock_hz` is a user-configurable parameter that may not be known until the full component tree is assembled. Doing conversion earlier risks using a wrong or default clock frequency. Gem5's global `SimClock` is a known footgun when running multiple simulations in one process. The unit-tagged `AttrValue` approach carries the unit information losslessly across the PyO3 boundary.

**Impact:** Users write `cpu.clock_hz = 2e9`, `cache.read_latency_ns = 4` and never think about ticks. Rust's `elaborate()` pass performs all conversions in a deterministic order (clock first, then everything that depends on clock). A `HelmConfigError` is raised if a `Param.Ns` or `Param.Hz` value is set on a component that has no associated clock (e.g., a standalone device in device-only mode without a `MicroarchProfile`).

---

## helm-engine/World (Q101–Q105)

---

### Q101 — Does `World` (full sim) and `World` (device-only) share code or are they separate types?

**Context**

The original question asked whether device-only simulation (no CPU, no ISA, no arch state) uses the same `World` type as full simulation (CPU + memory + devices + scheduler). The concern was code duplication: if they are separate types, the device MMIO dispatch, event queue, stats registry, and interrupt wiring would be duplicated. If they are the same type, device-only mode is simply the full `World` without a `HelmEngine` attached.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Single `World` type, no `HelmEngine` = device-only mode | Zero duplication; all device tests use the same `World`; feature set is identical | `World` is slightly heavier than a pure device substrate (includes stats, event bus always) | **Resolved: helm-ng** |
| Separate `DeviceWorld` and `SimWorld` types | Could be lighter-weight for device-only | Massive code duplication; divergence risk; two surfaces to test | QEMU (QOM objects vs. full machine) — different philosophy |
| Trait-based `WorldTrait` with two impls | Clean separation | Trait object overhead; harder to add methods without breaking the trait | — (unnecessary complexity) |

**Answer:** Resolved. Same `World` type. Device-only mode = `World` without a `HelmEngine` registered. Full simulation mode = `World` with one or more `HelmEngine` instances registered and driven by the `Scheduler`.

**Rationale:** The resolution was reached during World API design: the `World` already provides everything needed for device-only use — MMIO dispatch via `MemoryMap`, timer callbacks via `EventQueue`, observability via `HelmEventBus`, stats via `StatsRegistry`, and interrupt wiring via `wire_interrupt()`. Adding a CPU is `world.add_device("cpu0", Box::new(HelmEngine::new(...)))`. Not adding one is device-only. No separate type needed.

**Impact:** Device tests in `helm-devices/tests/` use `World` directly. There is no `DeviceWorld` type anywhere in the codebase. Documentation refers to "device-only mode" (operating mode) rather than a type distinction. Full simulation tests also use `World`, augmented with `Scheduler`.

---

### Q102 — In `World`, who is the "time master"? Is simulated time always exactly what `advance(cycles)` says?

**Context**

`World::advance(cycles)` is the primary time-advancement mechanism. It drains all events scheduled at or before `current_tick + cycles`, then advances the `VirtualClock` to `current_tick + cycles`. But what if an event callback calls `advance()` recursively? What if a device schedules an event in the past? Who is the authoritative source of current time during event processing? In SIMICS, time is managed by the clock object; devices schedule events relative to the clock and the simulator advances to the next event automatically. In Gem5, `EventQueue::serviceEvents()` is the loop that drives time, and the "current tick" is set by the event queue, not by a separate clock.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `VirtualClock` is set by `advance()` to each event's tick before calling its callback | `current_tick()` always reflects the exact moment of the current event; devices can read accurate time during callbacks | Slightly complex: clock is set twice (once per event, once to target after drain) | **helm-ng design (implemented)** |
| `VirtualClock` stays at `advance()` start tick until loop ends | Simple implementation | `current_tick()` during callback is wrong (shows pre-advance time, not event time) | — (incorrect) |
| No `VirtualClock`; devices compute time from event queue peek | No separate clock struct | Devices cannot query current time without the event queue | — (awkward API) |

**Answer:** `VirtualClock` is the single time source. During `advance(cycles)`, the clock is set to each event's scheduled tick before its callback runs, then set to `current_tick + cycles` after the drain loop. The clock never goes backward (enforced by `VirtualClock::set()` panic on backward movement). Events scheduled in the past (tick < current) by a device callback are processed immediately in the same `advance()` call if they fall within the window.

**Rationale:** Devices need to know the current simulated time to compute correct behavior (e.g., UART baud timing, timer register reads). If `current_tick()` returned a stale value during event callbacks, device timing would be subtly wrong. Setting the clock to the event tick before the callback is the correct semantic: the callback executes "at" that tick. The final advance to `target` after all events ensures the clock reflects the full window even if no events fired.

**Impact:** `World::advance(cycles)` always moves time forward exactly `cycles` ticks. No over- or under-shoot. Recursive calls to `advance()` from within an event callback are allowed but unusual; they process events within their own sub-window and then return, allowing the outer `advance()` to continue from the updated clock position.

---

### Q103 — How does `World::wire_interrupt(uart.irq_out, plic.input(33))` work when `plic.input(33)` is a port?

**Context**

The conceptual API `wire_interrupt(uart.irq_out, plic.input(33))` connects a UART's interrupt output pin to the PLIC's input 33. The challenge: `plic.input(33)` is a dynamic port — the PLIC has 32+ interrupt inputs, each of which is an `Arc<dyn InterruptSink>`. The PLIC device must expose an API to retrieve the sink for a given input index. The `World::wire_interrupt()` method takes `(from_device: HelmObjectId, pin_name: &str, to_sink: Arc<dyn InterruptSink>)`. So "getting" `plic.input(33)` means calling a method on the PLIC device that returns `Arc<dyn InterruptSink>`.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| PLIC exposes `plic.irq_sink(n: u32) -> Arc<dyn InterruptSink>` method | Clean; type-safe; PLIC controls its own sink instances | Requires downcasting `Box<dyn Device>` to `Plic` to call the method; awkward | **helm-ng (chosen)** |
| `Device::named_sink(name: &str) -> Option<Arc<dyn InterruptSink>>` trait method | Generic; no downcasting; `World` can implement `wire_interrupt` cleanly | All device types must implement the method (even those without sinks); string-based port name parsing | Extension of current design |
| Pre-wired at device construction: `Plic::new(connections: &[(device_id, pin)])` | No runtime wiring needed | Circular dependency: PLIC needs device IDs, devices need PLIC; configuration inflexible | — (rejected) |

**Answer:** The `Device` trait gains a default-no-op method `fn irq_input_sink(&self, name: &str) -> Option<Arc<dyn InterruptSink>> { None }`. The PLIC implements this to return `Some(Arc::clone(&self.inputs[n]))` where `name` is `"input_33"` (formatted from the input index). `World::wire_interrupt()` is extended to accept either a sink directly or a `(to_device: HelmObjectId, sink_name: &str)` form that calls `irq_input_sink` on the target device.

**Rationale:** The string-named sink approach avoids downcast (`as_any()` + `downcast_ref::<Plic>()`), which would require `Any` bounds on `Device` — a significant API change. A named sink method on `Device` is a small addition with a default no-op implementation, so existing device implementations are unaffected. The PLIC formats its input names as `"input_{n}"`.

**Impact:** The Python DSL exposes this as `world.wire_interrupt(uart_id, "irq_out", plic_id, "input_33")` — four arguments. The Rust signature becomes `wire_interrupt(from: HelmObjectId, from_pin: &str, to: HelmObjectId, to_sink: &str)`. The built-in `WorldInterruptSink` shorthand remains as `wire_interrupt_to_sink(from, from_pin, world.irq_sink())` for tests.

---

### Q104 — Can `World` be used with `HelmEventBus` for observability in device-only mode?

**Context**

`HelmEventBus` provides synchronous, zero-allocation event observation. It fires events such as `MemWrite`, `MemRead`, `DeviceSignal`, `InterruptAssert`. In device-only mode (no CPU), observability is even more important: without a CPU driving instructions, all observable events come from device MMIO interactions and timer callbacks. A test harness or verification tool needs to see every write and every interrupt assertion without modifying device code. The question is whether `HelmEventBus` is fully functional in device-only mode or whether it depends on CPU-side events.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `HelmEventBus` fully functional in all modes | Tests subscribe once; same API in both modes; no special-casing | Must ensure all device-side event kinds (`MemWrite`, `MemRead`, `DeviceSignal`) are fired by `World`, not by `HelmEngine` | **helm-ng (designed this way)** |
| Separate `DeviceEventBus` for device-only mode | Could be lighter | Two event bus types; code duplication; tests must know which mode they're in | — (rejected) |
| No event bus in device-only mode; use interrupt sink only | Simpler | Cannot observe MMIO traffic; insufficient for verification | — (insufficient) |

**Answer:** Yes. `World` owns `Arc<HelmEventBus>` unconditionally. All MMIO operations (`mmio_write`, `mmio_read`), signal operations (`signal_raise`, `signal_lower`), and interrupt wire events fire on the bus regardless of whether a `HelmEngine` is registered. Device-only tests can subscribe to `HelmEventKind::MemWrite` and see every MMIO write fired by the test harness or by device-to-device DMA.

**Rationale:** The event bus is owned by `World`, not by `HelmEngine`. CPU-side events (`InsnRetired`, `Exception`, `MagicInsn`) are fired by `HelmEngine` when it is present. Device-side events (`MemWrite`, `MemRead`, `DeviceSignal`) are fired by `World::mmio_write()` and `World::signal_raise()`. The two sets are orthogonal. Device-only mode gets the full device-side observable event set at zero additional cost.

**Impact:** Device tests can use `world.on_event(HelmEventKind::MemWrite, |e| { ... })` to verify every MMIO write without injecting code into devices. This is the primary test verification mechanism in `helm-devices/tests/`. The `EventHandle` drop guard ensures subscriptions are cleaned up after each test.

---

### Q105 — For fuzzing: how is `World` reset between fuzz iterations?

**Context**

Fuzz testing device models (e.g., `cargo-fuzz` or `libfuzzer-sys`) requires resetting device state between iterations. Each iteration feeds a different MMIO write sequence (from the fuzzer corpus) and checks for panics, assertion failures, or undefined behavior. The reset must be fast (millions of iterations/second is the goal) and complete (no state leakage between iterations). SIMICS resets via checkpoints — restoring a pre-configured snapshot. Gem5 supports `m5.instantiate()` re-run but full reset is done by process restart in fuzz contexts. QEMU's fuzz infrastructure re-instantiates QOM objects per iteration.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Re-instantiate `World` per iteration (drop + new) | Guaranteed clean state; Rust's `Drop` ensures all resources freed; simple | Constructor overhead per iteration (hash map allocation, event queue alloc) | helm-ng fuzzing (recommended pattern), QEMU fuzz |
| `World::reset()` method that re-initializes all fields in-place | Avoids allocator round-trip; reuses buffer capacity | Must reset every field correctly; devices must implement a `reset()` method; footgun if any field is missed | — (risky to implement correctly) |
| Checkpoint/restore (serialize + deserialize) | Exact state restore; works for complex pre-configured states | Slow (serialization overhead); requires serde on all device state | SIMICS checkpoint-based reset |
| Clone the initial `World` (`Clone` derive) | Fast if clone is cheap | `Box<dyn Device>` is not `Clone`; would require `dyn CloneDevice`; significant API complexity | — (not feasible with trait objects) |

**Answer:** Re-instantiate `World` per fuzz iteration. The fuzz harness calls a `setup_world()` function that creates a fresh `World`, calls `add_device()`, `map_device()`, `wire_interrupt()`, and `elaborate()` on every iteration. `World` drops at end of scope. The integration test in `LLD-world.md` demonstrates this pattern (`test_reset_via_re_instantiation`).

**Rationale:** Re-instantiation is the only approach that is provably correct: Rust's ownership system guarantees complete cleanup via `Drop`. The overhead is low: `World::new()` allocates a `HashMap` (default capacity ~4 entries), an `EventQueue` (heap-allocated `BinaryHeap`), and a `StatsRegistry`. For a device with a small number of registers, this is microseconds — well within fuzz iteration budget. `World::reset()` is explicitly rejected because maintaining a correct reset implementation across all device types is a maintenance burden and a source of hard-to-find test bugs.

**Impact:** Fuzz targets in `fuzz/fuzz_targets/` follow the pattern `let mut world = setup_world(); /* fuzz input drives mmio_write calls */ drop(world);`. No `World::reset()` method exists in the public API. This is documented as the canonical fuzz pattern. For coverage-guided fuzzing of complex initialized states, the pre-elaborated state can be captured by factoring `setup_world()` to run once and then using a cheaper per-iteration re-init of only the mutable device fields — but this optimization is deferred until profiling shows `setup_world()` is the bottleneck.

---

## Cross-Cutting (Q106–Q110)

---

### Q106 — What is the crate dependency order (DAG)? Which crates have zero helm-* deps?

**Context**

Rust's crate compilation model requires a strict DAG — circular dependencies are illegal. In a simulator, circular dependencies are easy to accidentally create: `helm-memory` wants to fire events (needs `helm-event`), `helm-event` wants to know object IDs (needs `helm-core`), `helm-core` wants memory access (needs `helm-memory`). Breaking these cycles requires careful layering. Gem5's SimObject hierarchy avoids this via C++ virtual dispatch and forward declarations. Rust's trait system provides similar decoupling if interfaces are defined in low-level crates.

| Layer | Crates | Depends On |
|-------|--------|-----------|
| Level 0 (no helm-* deps) | `helm-core`, `helm-event`, `helm-stats` | external crates only (`slotmap`, `atomic`, etc.) |
| Level 1 | `helm-memory`, `helm-arch` | `helm-core`, `helm-event` |
| Level 2 | `helm-devices` | `helm-core`, `helm-event`, `helm-memory`, `helm-stats` |
| Level 3 | `helm-engine` | `helm-core`, `helm-arch`, `helm-memory`, `helm-devices`, `helm-event`, `helm-stats`, `helm-timing`, `helm-debug` |
| Level 4 | `helm-python` | all of the above via PyO3 |

**Answer:** Zero helm-* dependencies: `helm-core` (defines `SimObject`, `AttrValue`, interfaces), `helm-event` (defines `EventQueue`, `HelmEventBus`, `HelmEvent`), `helm-stats` (defines `StatsRegistry`, `PerfCounter`, `PerfFormula`). These three are the foundation layer and may only depend on external crates (`slotmap`, `atomic`, `serde`, `thiserror`).

**Rationale:** Placing `HelmEventBus` in `helm-event` (level 0) prevents the circular dep: `helm-devices` fires events without `helm-devices` depending on `helm-engine`. `helm-core` defines `SimObject` and `AttrValue` without knowing about devices, memory, or the engine. `helm-stats` depends on nothing helm-internal because counter paths are plain strings and the registry is self-contained.

**Impact:** Adding a dependency from a level-0 crate to any other level-N crate must be treated as an architectural red flag and reviewed. The workspace `Cargo.toml` documents the expected dependency DAG, and `cargo deny` or a custom CI check enforces that no level-0 crate transitively depends on level-1+ crates.

---

### Q107 — How is the `World` struct structured — one monolithic struct or split?

**Context**

`World` coordinates MMIO dispatch, event scheduling, interrupt wiring, observability, and stats. A monolithic struct (`pub struct World { memory, event_queue, event_bus, clock, stats, irq_sink, objects, ... }`) has all fields in one allocation — simple, cache-friendly for fields accessed together. A split design (e.g., `World { context: WorldContext, devices: DeviceRegistry }`) separates concerns but adds indirection. Gem5's `System` is a monolithic SimObject. SIMICS's `conf_class_t` is a flat object with attribute tables.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Monolithic struct with all fields | One allocation; no indirection; simple `&mut self` borrowing; field access is direct | Large struct; borrow checker fights when two methods each need `&mut` on different fields simultaneously | **helm-ng (chosen)** |
| Split into sub-structs (`WorldContext`, `DeviceRegistry`) | Cleaner separation of concerns; can pass `&mut WorldContext` to devices without exposing device list | More indirection; `WorldContext` borrows must be carefully scoped to avoid conflicts with `objects` map | Possible refactor if borrow checker fights become common |
| `Arc<Mutex<WorldInner>>` everywhere | `Clone`-able handle; multi-threaded mutation | Lock on every operation; kills performance; unnecessary since `World` is single-threaded | — (rejected) |

**Answer:** Monolithic struct as implemented in `LLD-world.md`. `World` owns all subsystems as direct fields. The `elaborate()` method creates a temporary `WorldContext` (a struct of shared references) that is passed to each device's `elaborate_in_world()` — this avoids borrow conflicts during elaboration without splitting the struct permanently.

**Rationale:** The monolithic layout matches actual access patterns: MMIO operations need `memory`, `event_bus`, and `clock` together — no indirection saves anything. The borrow checker conflict (cannot borrow `objects` and `memory` simultaneously) is handled by the existing pattern of looking up the device ID from `memory` first, then looking up the device in `objects` — two separate borrows. The `WorldContext` borrow trick in `elaborate()` resolves the elaboration conflict cleanly.

**Impact:** `World` is approximately 8 fields + the `objects: HashMap`. All fields are initialized in `World::new()`. No heap indirection beyond the `HashMap` itself and `Arc` for shared subsystems (`event_bus`, `irq_sink`). The struct is `!Send` (single-threaded by design); `Arc<Mutex<World>>` would be needed for multi-threaded access, but `World` is not designed for that use case.

---

### Q108 — Is `HelmObjectId` a `u32` index, `u64` hash, or typed wrapper?

**Context**

`HelmObjectId` is the stable identifier for a registered device in `World`. It must be: stable across elaboration (not reassigned), unique within a `World` instance, fast to look up (O(1)), and serializable for checkpoints. The `slotmap` crate provides a generational index (`KeyData`: u32 index + u32 generation) that prevents use-after-free on object deletion. A plain `u64` monotonic counter is simpler but loses the generation safety. A hash (`u64` hash of the name string) is non-unique in theory and non-monotonic.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| `slotmap` generational index (u32 index + u32 generation) | Use-after-free detection; O(1) lookup; `serde`-compatible; well-tested crate | Adds `slotmap` dependency; slightly larger than bare `u32` | Designed for helm-ng (slotmap research in context) |
| Monotonic `u64` counter (as implemented in `LLD-world.md`) | Dead simple; no dependency; O(1) HashMap lookup; stable; no reuse risk (u64 never wraps) | No use-after-free detection (but `World` never removes devices after elaborate, so this is moot) | **helm-ng Phase 0 (current)** |
| `u32` index into `Vec` | Cache-friendly; O(1) by index | Reuse on deletion causes use-after-free; not stable if insertion order changes | — (rejected if devices can be removed) |
| Name-based `String` key | Human-readable; debuggable | Heap allocation; not `Copy`; slow comparison; not serializable efficiently | — (rejected for hot-path use) |

**Answer:** Monotonic `u64` counter wrapped in `struct HelmObjectId(pub(crate) u64)`. Assigned by `add_device()`, incremented from `next_id: u64` in `World`. Never reused. Stored in `HashMap<HelmObjectId, RegisteredDevice>` for O(1) lookup.

**Rationale:** `World` never removes a device after `elaborate()` — device lifecycle is add → elaborate → run → drop (with the entire `World`). Without removal, use-after-free is impossible, so the generation guard of `slotmap` is unnecessary. The monotonic u64 is simpler, smaller (no `slotmap` dependency), and fully sufficient. If device removal is added in the future (Phase N hotplug support), migration to `slotmap` is straightforward.

**Impact:** `HelmObjectId` is `Copy`, `Eq`, `Hash`, `Debug`. It can be stored in Python as a `PyObject` wrapping the u64 and compared by value. It is stable for the lifetime of a `World` instance but not across serialization boundaries (checkpoints must map IDs by device name, not raw ID value). The `next_id` field starts at 1; ID 0 is reserved and never issued, providing a sentinel value.

---

### Q109 — What is the minimum Rust edition and MSRV?

**Context**

Rust edition controls syntax and some language features (2018, 2021). MSRV (Minimum Supported Rust Version) determines which stable Rust features are available. Dependencies impose their own MSRVs. PyO3 0.20+ requires Rust 1.63+. `deku` 0.16+ requires Rust 1.60+. GATs (Generic Associated Types) stabilized in Rust 1.65. `slotmap` 1.0 requires Rust 1.49. `thiserror` 1.0 requires Rust 1.56. Async Rust features (if used) require 1.39+. The workspace must set a MSRV that satisfies all dependencies.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Edition 2021, MSRV 1.70 | Latest stable at design time; `std::io::ErrorKind` improvements; let-else syntax (1.65); GATs (1.65); `Arc::into_inner` (1.70); conservative enough for CI | May lag behind nightly features; some CI setups use older toolchains | **helm-ng (chosen)** |
| Edition 2021, MSRV 1.65 | GATs available; slightly wider compatibility | Misses `Arc::into_inner` and some `Iterator` methods added in 1.66–1.70 | Acceptable fallback |
| Edition 2021, MSRV 1.56 | Very wide compatibility | Cannot use GATs, let-else, or several `std` improvements; limits language expressiveness | Over-conservative |
| Nightly only | Access to all experimental features | Not suitable for a production simulator; CI instability | — (rejected) |

**Answer:** Rust edition 2021, MSRV 1.70. Set in `workspace.package.rust-edition = "2021"` and `workspace.package.rust-version = "1.70"`. Enforced in CI by `cargo +1.70 check` as a separate job.

**Rationale:** 1.70 is a Long-Term Support milestone in the Rust release train and is available on all major Linux distributions' Rust toolchain packages. It covers all features used: GATs (1.65), let-else (1.65), `std::io::Error::other()` (1.74 — must check), PyO3 0.20 (1.63), `deku` 0.16 (1.60). Any feature requiring > 1.70 must be gated behind a `#[rustversion::since(1.X)]` check or removed.

**Impact:** MSRV is checked in CI on every PR. Adding a dependency with MSRV > 1.70 requires a workspace-wide MSRV bump and a corresponding CI update. The `workspace.package.rust-version` field causes `cargo check` on older toolchains to error clearly rather than produce cryptic compile failures.

---

### Q110 — How are inter-crate feature flags managed?

**Context**

Feature flags in Rust allow conditional compilation of heavyweight features. `helm-engine` has timing-model variants: `timing-interval` (lightweight statistical model, default) and `timing-accurate` (out-of-order pipeline, heavyweight, pulls in a large OoO pipeline crate). `helm-arch` might have `riscv` and `aarch64` as separate features. Feature flags propagate through the crate dependency graph: if `helm-python` enables `helm-engine/timing-accurate`, it transitively enables the OoO pipeline. Workspace-level feature management ensures that users of the workspace don't accidentally enable heavyweight features by default.

| Option | Pros | Cons | Used By |
|--------|------|------|---------|
| Crate-level features with sane defaults | Standard Rust; `cargo build` with defaults works for most users; opt-in to heavyweight | Features must be carefully designed to avoid additive conflicts; `default-features = false` pitfalls | **helm-ng (standard practice)** |
| Workspace-level feature re-exports (`[workspace.dependencies]` with features) | One place to control features for all crates; consistent | Requires Cargo 1.64+; features still defined per-crate | Cargo workspace best practice |
| Build profiles (`--profile timing-accurate`) | Separate from features; no accidental enable | Not standard Rust; limited to `opt-level`, `debug`, `overflow-checks` — cannot toggle code | — (profiles cannot gate code) |
| Conditional compilation via env vars | Works without Cargo features | Not idiomatic; breaks `cargo check`; caching issues | — (rejected) |

**Answer:** Per-crate features with workspace-level dependency declarations. `helm-engine` defines:
- `default = ["timing-interval"]`
- `timing-interval` — lightweight statistical timing (always available)
- `timing-accurate` — full OoO pipeline (opt-in, large dependency)

`helm-python` re-exposes these as its own features: `helm-python/timing-accurate` enables `helm-engine/timing-accurate`. The workspace `Cargo.toml` uses `[workspace.dependencies]` to pin `helm-engine` version and declare which features are enabled by default in workspace members.

**Rationale:** The standard Rust feature flag system is the only supported mechanism for conditional compilation. The default of `timing-interval` ensures `cargo build` in the workspace produces a fast-to-compile, useful simulator without pulling in the OoO pipeline's dependencies. Power users who need `timing-accurate` add `--features timing-accurate` on the command line or set it in their top-level `Cargo.toml`. Workspace-level `[workspace.dependencies]` centralizes version pinning without centralizing feature selection.

**Impact:** `#[cfg(feature = "timing-accurate")]` gates the OoO pipeline structs and their imports in `helm-engine`. The CI matrix runs `cargo test` with and without `--features timing-accurate` to ensure both configurations compile and pass tests. Feature-additive: enabling `timing-accurate` never removes or breaks functionality available in `timing-interval`. Mutual exclusion between `timing-interval` and `timing-accurate` is not enforced at the Cargo level — if both are enabled, the scheduler selects the model via a runtime config parameter, not a compile-time branch.
