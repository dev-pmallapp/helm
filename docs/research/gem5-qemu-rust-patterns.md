# Simulator Architecture Research Report
## For: helm-ng Next-Generation Simulator Design

**Date:** 2026-03-13
**Scope:** Multi-mode execution, multi-ISA design, SE/FE/FS modes, timing models, Rust simulator patterns

---

## 1. Multi-Mode Simulation Architectures

### The Core Design Problem

The fundamental tension in simulator architecture is **accuracy vs. speed**. No single execution model serves all use cases: fast-forwarding to a region of interest, measuring precise cache miss rates, and booting a kernel all require different fidelity levels. The canonical solution, exemplified by gem5, is to support multiple CPU models that share the same memory system and SimObject infrastructure, selectable at configuration time.

### gem5's Three CPU Models

gem5 implements three primary CPU execution models, each representing a point on the accuracy/speed continuum:

#### AtomicSimpleCPU
- Uses **atomic memory accesses**: requests complete synchronously via function call return, with no modeled delay.
- Executes instructions in strict order with no simulated delays.
- Returns a latency **estimate** from the atomic access to compute cache access time, but this is approximate.
- Four execution stages: fetch, pre-execute/decode, execute, commit — all within one conceptual tick.
- Primary use: **fast-forwarding** to reach a region of interest (ROI). Not suitable for performance measurement.
- Architectural implication: the CPU drives the memory system synchronously; no callbacks, no event scheduling for memory responses.

#### TimingSimpleCPU
- Uses **timing memory accesses**: sends a request packet and then **stalls** — does not proceed until the memory system sends a response via callback.
- Still single-issue, in-order — no pipeline, no out-of-order parallelism.
- Every arithmetic instruction executes in 1 cycle; memory accesses take realistic multiple cycles.
- Does not model function unit latencies.
- Primary use: **simple timing measurement** where pipeline complexity is not the research focus.
- Architectural implication: the CPU is event-driven; when it sends a memory request, it registers a callback and yields. The memory system calls back when the response arrives.

#### O3CPU (Out-of-Order)
- Loosely based on the Alpha 21264. Full out-of-order pipeline with ROB, register renaming, branch prediction, speculative execution.
- Pipeline stages: Fetch → Decode → Rename → Dispatch → Issue → Execute → Writeback → Commit.
- Uses timing memory accesses — event-driven like TimingSimpleCPU.
- Executes instructions at the **execute stage**, not the commit stage, to correctly model out-of-order load-store interactions.
- Primary use: **detailed microarchitecture research** — cache effects, branch misprediction penalties, IPC measurement.
- Architectural implication: the most complex model; simulates every pipeline stage as separate simulation events.

### How Mode Switching Works

The mode switch between CPU models is a **configuration-time** decision, not a runtime toggle. In gem5's Python config:

```python
system.cpu = AtomicSimpleCPU()  # or TimingSimpleCPU() or DerivO3CPU()
```

However, gem5 supports a **simulation-time switch** between fast and detailed modes for checkpoint-based workflows:
1. Boot or fast-forward using AtomicSimpleCPU (very fast).
2. Serialize/checkpoint the complete machine state at a region of interest.
3. Restore the checkpoint and continue with O3CPU (detailed but slow).

This is not a hot switch of the CPU model mid-simulation — it is a checkpoint-and-restore workflow. The CPU models are not hot-swappable because their internal state representations differ fundamentally.

**MARSS** takes a different approach: QEMU and PTLsim are **tightly coupled in a single process**, sharing the same memory address space. When the simulator hits a ROI marker, it triggers a mode switch via an internal exception (`BARRIER`), which transfers control from QEMU's functional path to PTLsim's cycle-accurate pipeline. The virtual clock in QEMU is then incremented by simulated cycles for correctness.

### How the Same Memory System Serves Multiple CPU Models

gem5's memory system is built around a **port interface** that supports three access modes. Every CPU model uses the same port protocol regardless of its simulation depth:

**Timing access (used by TimingSimpleCPU, O3CPU):**
- CPU calls `sendTimingReq(pkt)` on its request port.
- The call may return `false` if the memory system cannot accept the packet (flow control).
- The memory system later calls `recvTimingResp(pkt)` on the CPU's port to deliver the response.
- This is fully asynchronous and event-driven.

**Atomic access (used by AtomicSimpleCPU):**
- CPU calls `sendAtomic(pkt)` — synchronous, returns estimated latency.
- No callbacks. No event scheduling. Runs at host CPU speed.

**Functional access (used for debugging and SE binary loading):**
- CPU calls `sendFunctional(pkt)` — always succeeds, reads/writes current true state.
- Traverses the entire cache hierarchy, reading the most up-to-date value from wherever it lives.
- Used heavily in SE mode to load the target binary into simulated memory.

**Critical constraint:** Timing and atomic accesses **cannot coexist** in the same memory system simultaneously. This is a fundamental limitation — switching CPU modes requires draining all in-flight timing transactions first.

The memory system itself (Classic model or Ruby) is **CPU-model-agnostic**: it routes packets based on address ranges, handles cache coherence via MOESI snooping (Classic) or a configurable protocol (Ruby), and presents a uniform port interface to whatever CPU model is connected.

### Key Lessons from gem5's Multi-Mode Design

1. **Port abstraction is the correct seam.** Making the CPU/memory boundary a well-defined message-passing interface (packets + ports) allows any CPU model to connect to any memory model.
2. **Atomic and timing modes must not mix.** This is a runtime invariant that must be enforced architecturally — the system is in either atomic mode or timing mode, not both.
3. **Mode switching at checkpoint boundaries, not hot.** Hot CPU model switching requires complex state migration; checkpoint-restore is simpler and more reliable.
4. **The event queue is the common substrate.** All CPU models and the memory system operate on the same global event queue with simulated time in ticks — this is what makes them composable.

---

## 2. Multi-ISA Simulator Design

### gem5's ISA Abstraction Layer

gem5 supports Alpha, ARM (AArch32/AArch64), SPARC, MIPS, POWER, RISC-V, and x86 from a single codebase. The architecture of this is built on two core interfaces:

**Interface 1: `StaticInst` — The ISA→CPU Direction**
- Abstract base class for every instruction in every ISA.
- Contains: opcode class, instruction flags (isLoad, isStore, isBranch, isFloat, etc.), source and destination register descriptors, number of operands.
- Pure virtual `execute(ExecContext*, Trace::InstRecord*)` method — each ISA-specific instruction subclass implements this.
- CPU models call `decode()` to get a `StaticInst*` pointer, then call `execute()` on it without knowing the specific ISA.
- The ISA is compiled **into** the binary — gem5 is built for one ISA at a time (though recent versions support multi-ISA builds).

**Interface 2: `ExecContext` / `ThreadContext` — The CPU→ISA Direction**
- The ISA accesses CPU state via an `ExecContext` interface: read/write integer registers, read/write float registers, read/write PC, access memory.
- `ThreadContext` is the external-facing version: used by debuggers, OS syscall emulation, external tools.
- CPU models implement this interface; ISA code calls it without knowing which CPU model is running.

**The decode cache:** Each ISA decoder checks a `BasicDecodeCache` before decoding a new instruction. Once an instruction encoding is decoded, the resulting `StaticInst*` is cached — subsequent encounters of the same encoding hit the cache and avoid re-decoding. This is a key performance optimization for interpreted simulation.

### ISA Description Language and Code Generation

gem5 uses a custom **Domain-Specific Language (DSL)** to specify instruction encodings and execution semantics. The ISA parser processes `.isa` files and generates:
- C++ class definitions for each instruction (subclassing `StaticInst`)
- The `decodeInst(machInst)` function — a large `switch/case` tree on opcode fields

This means ISA authors write compact DSL descriptions; the parser expands them into thousands of lines of C++ automatically. The generated code is placed in `build/{ISA}/arch/{isa}/generated/decode-method.cc.inc`.

### AArch32 + AArch64 in a Single ISA Object

ARM's implementation is the most instructive case for multi-mode ISA design. The `ArmISA::ISA` class in `src/arch/arm/isa.hh` serves as the central container for ARM architectural state and handles both AArch32 and AArch64 within a single object:

- A single `miscRegs[NUM_MISCREGS]` array stores all 650+ system registers, covering both execution states.
- A mode-dependent pointer `intRegMap` handles register banking for AArch32 (banked registers per privilege mode).
- `initID32()` / `initID64()` initialize state for each sub-architecture separately.
- `clear32()` / `clear64()` set initial architectural state per mode.
- `flattenMiscIndex()` translates architectural register indices to physical storage, applying banking based on the current mode and security state.
- The `PCState` object carries both the program counter and the execution state (Thumb vs. ARM, ITSTATE for Thumb predication).
- Exception Level (EL0–EL3) is tracked via `CPSR.mode` and extracted by `opModeToEL()`.
- Interworking between AArch32 and AArch64 is fully supported in gem5 v20.0+.

**Key insight:** The ISA object is a **state container**, not an execution engine. All ISA-specific state lives there; the CPU models access it uniformly via `ExecContext`. The ISA object tracks which sub-architecture is active and adjusts register mapping accordingly.

### RISC-V Implementation in gem5

- `RiscvISA::ISA` inherits from `BaseISA` — the same pattern.
- The decoder handles both 32-bit standard instructions and 16-bit compressed (C extension) instructions.
- Decoder hierarchy: identify instruction quadrant → decode specific opcode and function codes.
- TLB supports Sv39, Sv48, and Sv57 virtual memory systems, plus hypervisor extension.
- Privilege modes: M-mode, S-mode, U-mode.
- Extensions are implemented as separate `.isa` format files that integrate into the decoder hierarchy.

### What Must Be ISA-Specific vs. Generic

| Generic (CPU Model) | ISA-Specific |
|---|---|
| Pipeline stages (fetch, decode, issue, execute, commit) | Instruction encoding and decode tree |
| Event scheduling | Register file layout (int count, float count, vector registers) |
| ROB, reservation stations | System register definitions |
| Branch predictor | PC representation (e.g., Thumb bit for ARM) |
| Cache and memory hierarchy | Privilege levels and mode transitions |
| Statistics collection | MMU / TLB page table format |
| Port interface | Interrupt and exception model |
| Execution context interface | Syscall ABI (for SE mode) |

**The single most important rule:** CPU models never directly access ISA state. They go through `ExecContext`. ISA instructions never directly access CPU microarchitecture state. They go through `ExecContext`. This bidirectional abstraction is what makes multi-ISA possible.

### Lessons for helm-ng Multi-ISA Design

1. **Define the ISA interface first, before any ISA implementation.** The interface is the `ExecContext` equivalent — what the CPU model needs to read/write to execute an instruction.
2. **ISA state is a struct, not an execution engine.** The ISA object holds register files, PC, system registers — it does not implement the execution loop.
3. **The instruction is a closure over the ISA state.** Each decoded instruction should carry its decoded form and know how to execute itself, given an `ExecContext`.
4. **ARM's AArch32/AArch64 duality works because the ISA object routes register accesses.** A clean mode flag plus a mapping function is sufficient; you do not need two separate ISA objects.
5. **ISA DSL is worth the investment at scale.** For 200+ instructions per ISA, hand-writing decode trees is error-prone. A simple code generator saves development time and reduces bugs.

---

## 3. Syscall Emulation (SE) vs. Functional Emulation (FE) vs. Full-System (FS)

### The Three Simulation Scopes

These are not execution *speed* modes — they are scope modes defining how much of the target system is simulated.

### Functional Emulation (FE) / Pure ISS

**What it is:** Execute instructions correctly (produce the right architectural state changes) with no timing model at all. The simulator is an Instruction Set Simulator (ISS) — an interpreter over the ISA.

**What it models:**
- Architectural register state (integer, float, vector)
- Memory read/write (flat or with virtual-to-physical translation)
- Control flow (branches, calls, returns)
- Exception and interrupt handling (at the semantic level)

**What it does NOT model:**
- Clock cycles, pipeline stages, cache latencies
- Resource contention, stalls, hazards
- Any microarchitecture detail

**Where it's used:**
- ISA validation: verify that a new instruction implementation produces correct results
- Software bring-up before hardware exists
- Fast pre-silicon emulation for software teams
- As the "functional front-end" in a coupled simulator (Sniper's pin-based front-end, MARSS's QEMU front-end)
- RTL co-simulation: the ISS runs in parallel with RTL simulation and checks for divergence

**Speed:** Typically 10–100x faster than a cycle-accurate simulator. A simple ISS can execute 100M–1B instructions per second.

**Key architecture pattern:** The ISS is a simple `while(true) { fetch; decode; execute; } ` loop. No event queue needed. Time is measured in instruction count, not simulated cycles.

### Syscall Emulation (SE) Mode

**What it is:** A superset of FE — executes the target binary's user-space instructions, but intercepts privileged instructions (specifically the `syscall` instruction) and translates them to host OS calls.

**The mechanism:**
1. The simulator executes user-space instructions normally (functionally, or with timing).
2. When the target executes a `syscall` instruction, the simulator traps it.
3. The simulator reads syscall number and arguments from architectural registers.
4. It calls the corresponding **host OS syscall** (or an emulated version of it).
5. Results are written back to target architectural registers.
6. Execution continues.

**What SE mode provides over FE:**
- `read()`, `write()`, `open()`, `mmap()`, `brk()`, `exit()` — standard POSIX calls — work transparently.
- Dynamic memory allocation, file I/O, and basic threading work.
- No OS kernel needs to be present in the simulation.

**Limitations of SE mode:**
- Only implements a **subset** of Linux syscalls — uncommon syscalls may be missing.
- Cannot model OS scheduling, context switching, or kernel code paths.
- Cannot model page faults triggered by the kernel.
- Dynamically linked binaries require host-compatible dynamic libraries.
- IO-intensive or multi-process workloads are less accurate.
- GPU applications and specialty drivers are typically not possible in SE mode.
- Until recently, gem5's SE mode required statically linked binaries.

**Key design pattern for codebase separation:** In gem5, SE mode has a dedicated `Process` class that handles binary loading, address space setup, and syscall dispatch. The CPU model calls `syscall(tc)` on the process object when it encounters a syscall instruction. The process object dispatches to a table of syscall handlers. The CPU model is completely unaware of SE vs. FS — this is handled by the `Process` vs. `System` object attached to the thread context.

### Full System (FS) Mode

**What it is:** Simulates the complete hardware platform — CPU, memory, interrupt controllers, timers, storage controllers, network interfaces, etc. — and boots an unmodified OS kernel.

**What's different from SE mode:**
- A real kernel boots (Linux, bare-metal firmware, etc.).
- The simulator must model all hardware devices the kernel expects.
- Page table walks happen in simulated hardware (not elided as in SE mode).
- Interrupts, timers, and DMA work correctly.
- Multi-process workloads, threads, and context switching work.
- OS kernel code paths execute normally.

**Configuration complexity:** FS mode requires specifying: BIOS/bootloader, disk image, physical memory layout, interrupt controller (APIC, GIC), timer devices (HPET, ARM timer), storage controller, and more. This is architecture-specific (x86 FS config is completely different from ARM FS config).

**Performance penalty:** Waiting for the OS to boot before reaching the workload adds significant overhead. This is mitigated via:
- **Checkpointing**: boot once with AtomicSimpleCPU, checkpoint at login, restore for repeated experiments.
- **KVM acceleration**: boot using KVM (native host execution) for the OS boot phase, switch to detailed simulation for the ROI.

### The Clean Codebase Separation

gem5's approach to separating these three modes in code:

```
CPU model (TimingSimpleCPU, O3CPU, etc.)
    ↓ ExecContext interface
ThreadContext
    ↓ attached to
Process (SE mode) OR System (FS mode)
    - Process: handles binary loading, syscall dispatch, address space
    - System: handles kernel loading, device tree, interrupt routing
```

The CPU model itself is mode-agnostic. It calls `tc->syscall()` when it hits a syscall instruction. The `ThreadContext` routes this to the attached `Process` (SE) or to the simulated kernel interrupt handler (FS). This is the correct architectural seam.

**For helm-ng:** Define a `SystemInterface` trait (or equivalent) that the CPU model calls when it hits a syscall or memory fault. In SE mode, this interface is backed by a `SyscallEmulator`. In FS mode, this interface is backed by the interrupt handler and the simulated OS. The CPU model is completely unaware of which is attached.

---

## 4. Timing Models: Simulated Time vs. Interval vs. Cycle-Accurate

### Model 1: Simulated Time / Event-Driven (gem5's approach)

**Core concept:** There is one global virtual clock, measured in ticks (typically picoseconds or nanoseconds, depending on configuration). The simulator maintains a priority queue (the event queue) sorted by tick count. The simulation loop processes events in tick order.

**Architecture:**

```
EventQueue (priority queue sorted by Tick)
    |
    ↓ next event
EventHandler::process()
    |
    ↓ may schedule new events at future Tick values
EventQueue
```

**How it works in practice:**
- CPU fetch: scheduled as event at current tick.
- Memory access: the CPU sends a packet, the cache responds by scheduling a `recvTimingResp` event at `current_tick + cache_latency`.
- CPU resumes execution when its response event fires.
- Branch misprediction: the CPU squashes in-flight events and reschedules from the corrected PC.

**gem5 specifics:**
- Time measured in ticks; `sim_seconds` = total simulated time.
- Every component that models time delay schedules events — there is no background clock incrementing.
- The `simulate(max_ticks)` function drives the loop: dequeue minimum-tick event, advance simulated time to that tick, call `event.process()`, repeat.
- Multiple event queues exist (one per CPU core in parallel simulations), with barrier synchronization at quantum boundaries.

**Strengths:**
- Precise: every inter-component latency is modeled explicitly.
- Composable: any component can schedule events; the queue handles ordering.
- Accurate idle modeling: if no events are scheduled for 1000 ticks, the clock jumps forward instantly — no wasted host cycles.

**Weaknesses:**
- Slow: each simulated cycle requires event scheduling and queue operations.
- Memory: in-flight events can accumulate, using significant memory.
- Hard to parallelize: event queues require synchronization across cores.

**Typical simulation speed:** 1–10 million simulated instructions per second (MIPS) on a modern host for a detailed O3CPU model.

### Model 2: Interval Simulation (Sniper's approach)

**Core concept:** Rather than simulating every pipeline cycle, interval simulation identifies **miss events** (cache misses, branch mispredictions, TLB misses) and computes timing analytically between miss events. Between miss events, the processor executes at the theoretical IPC limited by instruction-level parallelism.

**Architecture:**

```
Functional Front-End (Pin/DynamoRIO/QEMU)
    ↓ instruction stream
Timing Back-End
    |
    ├─ Instruction Window (simulates ROB)
    │     - tracks dependencies
    │     - builds critical path
    ├─ Miss Event Detector
    │     - I-cache miss → add miss latency to simulated time
    │     - D-cache miss → check for overlapping misses, add penalty
    │     - Branch mispredict → add flush + refill penalty
    └─ Memory Hierarchy Model (cache + DRAM)
          - functional cache simulation
          - coherence protocol simulation
```

**How timing is computed:**
- Between miss events, the processor is modeled as making forward progress at the critical-path-limited IPC.
- On a D-cache miss, the interval model scans the ROB window for independent misses that can be overlapped (memory-level parallelism).
- On a branch misprediction, it adds the branch resolution latency plus the front-end pipeline flush penalty.
- The ROB is simulated to track dependencies but not cycle-by-cycle; only the critical path through the dependency graph is computed.

**Accuracy:** Average error ~4.6% vs. cycle-accurate simulation for SPEC CPU2000 and PARSEC on 8-core systems. Max error ~11% for multithreaded workloads.

**Speed:** ~10x faster than cycle-accurate simulation. Sniper achieves 1M–10M simulated instructions per second — approximately one order of magnitude better than gem5 O3CPU.

**Key insight — what interval simulation sacrifices:**
- Precise pipeline hazard modeling (structural hazards, data hazards across specific functional units)
- Exact IPC in short intervals (errors cancel out over longer runs)
- Precise simulation of highly timing-sensitive code sequences

**Key insight — what interval simulation preserves:**
- Cache miss rates (the memory hierarchy is fully simulated)
- Memory-level parallelism effects
- Core-uncore interaction (cache coherence, interconnect latency effects)
- Multi-core behavior (thread synchronization timing)

**MARSS's approach (related):** MARSS uses QEMU for functional execution and switches to PTLsim (cycle-accurate) only for the ROI. This is not interval simulation — it is **mode-based simulation switching**: functional fast-forward, then cycle-accurate for the target region. The key mechanism is the `BARRIER` exception that triggers the transition.

### Model 3: Cycle-Accurate Simulation

**Core concept:** Every pipeline stage is simulated cycle by cycle. Every clock cycle, the state of every pipeline register, every cache way, every ROB entry, every reservation station entry is updated. The simulated time advances by exactly one cycle at each step.

**Architecture:**

```
Per cycle (every tick):
    Fetch stage: select thread, access I-cache, handle branch prediction
    Decode stage: decode instructions, identify hazards
    Rename stage: rename registers, update RAT
    Dispatch stage: send to reservation stations
    Issue stage: wake up ready instructions, select from RS
    Execute stage: execute in functional units
    Memory stage: access D-cache, handle TLBs
    Writeback stage: update physical register file
    Commit stage: retire instructions from ROB, update arch state
```

**What cycle-accurate simulation correctly models:**
- Exact structural hazards (two instructions competing for the same functional unit)
- Precise pipeline flush depths (exactly how many cycles a branch misprediction costs given the current pipeline state)
- Exact cache fill timing, including MSHR (Miss Status Holding Register) effects
- Memory-level parallelism effects from specific access patterns
- Power/energy estimation (power models are parameterized per pipeline stage activity)

**Cost:**
- 10–100x slower than interval simulation.
- 100–1000x slower than purely functional ISS.
- At 1M simulated instructions/second, a 1-billion-instruction workload takes 17 minutes.
- SPEC CPU2006 workloads at gem5 O3CPU speeds can take hours to days per benchmark.

**When to use it:**
- Evaluating a specific microarchitectural feature (new branch predictor design, new prefetcher)
- Power modeling (requires cycle-level activity factors)
- RTL correlation (comparing against actual hardware)
- Timing-sensitive correctness checking (e.g., verifying that a lock-free algorithm works correctly under specific pipeline reorderings)

### Tradeoff Summary

| Model | Speed (MIPS) | Accuracy | Development Cost | Use Case |
|---|---|---|---|---|
| Functional ISS | 100–1000 | Correct outputs only | Low | Software bring-up, ISA validation |
| SE mode + functional | 10–100 | Correct + syscall behavior | Medium | Software testing, SE workloads |
| Interval (Sniper-style) | 1–10 | ~5% error | High | Design space exploration |
| Event-driven timing (gem5 Timing) | 0.5–5 | ~10% error | Medium | Memory/cache research |
| Cycle-accurate (gem5 O3CPU) | 0.1–1 | <2% error | Very High | Microarchitecture research |

### For helm-ng: Recommended Approach

The clearest path for a new simulator is to build execution modes **independently** and **composably**:

1. **Functional core first:** Build a correct ISS. No timing. Fast. This validates the ISA implementation and provides the functional reference.
2. **Event-driven timing layer second:** Add a discrete event queue and port interface. Connect the functional core to a timing-enabled memory system. This is the TimingSimpleCPU equivalent.
3. **Interval timing as the performance model:** Rather than building a full O3CPU, implement an interval model. This gives ~5% accurate performance estimates at 10x the speed of cycle-accurate simulation. Much more practical for a solo developer.
4. **Cycle-accurate as a future module:** Implement cycle-accurate as a pluggable execution backend for specific ROIs.

The critical architectural requirement: the **functional core is separate from the timing model**. The timing model wraps or drives the functional core; it does not embed timing into the instruction execution itself.

---

## 5. Rust Simulator Projects: Patterns and Lessons

### Existing Rust RISC-V Projects

**rvemu (d0iasm/rvemu):**
- Full RISC-V emulator targeting RV64GC (G = IMAFD base + standard extensions, C = compressed).
- Supports xv6 and Linux boot.
- Written as a book/tutorial: https://book.rvemu.app/
- Architecture: CPU struct containing registers, PC, CSRs, and a memory bus. Decode/execute via large `match` on opcode.
- No timing model — purely functional ISS.
- Compiles to WebAssembly for browser use.

**riscv-rust (takahirox):**
- RISC-V processor + peripheral devices (CLINT, PLIC, UART, VirtIO disk) in Rust + WASM.
- Can boot Linux or xv6 in a browser.
- Architecture: single `Cpu` struct with `tick()` method, `Memory` trait for device dispatch.
- Notable: implements a virtual UART and VirtIO block device — demonstrating that device modeling in Rust via traits is practical.

**rrs (Greg Chadwick):**
- Designed as a well-structured simulator for RISC-V research, not just an emulator.
- Key architectural decision: separate `HartState` (architectural state) from `InstructionExecutor` (execution logic).
- `InstructionProcessor` trait defines what an instruction handler must implement.
- `Memory` trait abstracts over different memory backends.
- Blog series documents the design decisions in detail.

**riscv-harmony (brettcannon):**
- Explicitly designed as an ISA simulator, not a system emulator.
- Chosen Rust specifically for systems programming fit.

**riscv-5stage-simulator (djanderson):**
- Implements the 5-stage pipeline (IF/ID/EX/MEM/WB) from Patterson and Hennessy.
- Demonstrates that cycle-accurate pipeline simulation in Rust is straightforward with struct-per-stage.

### Key Architectural Patterns in Rust Simulators

#### Pattern 1: Trait-based Device Abstraction

The dominant pattern for memory buses and device models:

```rust
trait MemoryDevice {
    fn read(&self, addr: u64, size: usize) -> Result<u64, MemFault>;
    fn write(&mut self, addr: u64, size: usize, val: u64) -> Result<(), MemFault>;
    fn address_range(&self) -> (u64, u64);
}

struct Bus {
    devices: Vec<(u64, u64, Box<dyn MemoryDevice>)>,  // (base, end, device)
}
```

The bus iterates devices to find the one whose range covers the target address. `Box<dyn MemoryDevice>` uses dynamic dispatch — a runtime vtable lookup. For a simulator, this overhead is acceptable (one vtable dispatch per memory access vs. hundreds of cycles of instruction simulation).

**Tradeoff:** `Box<dyn MemoryDevice>` requires heap allocation and prevents inlining. For extremely hot paths (every instruction fetch), you may want monomorphization via generics. For most device accesses (UART, PLIC), dynamic dispatch is fine.

#### Pattern 2: Rc\<RefCell\<T\>> vs Arc\<Mutex\<T\>> for Shared Devices

When multiple components need to reference the same device (e.g., a DMA controller and a CPU both accessing the same memory controller):

- Single-threaded simulation: `Rc<RefCell<Device>>` — zero-cost for borrowing, borrow checker enforces exclusive access at runtime.
- Multi-threaded simulation: `Arc<Mutex<Device>>` — necessary for parallel simulation of multiple cores, but adds locking overhead.

Most Rust RISC-V simulators use single-threaded `Rc<RefCell<T>>` — simulating one hart at a time. Multi-hart/multi-core simulation requires more careful design.

#### Pattern 3: Hart as the Fundamental Abstraction

RISC-V defines a **hart** (hardware thread) as the fundamental execution unit — a set of state against which instructions execute. This maps cleanly to Rust:

```rust
struct Hart {
    registers: [u64; 32],
    pc: u64,
    csr: CsrFile,
    privilege: PrivilegeLevel,
}
```

For multi-ISA design, `Hart` becomes a trait:

```rust
trait Hart {
    fn step(&mut self, mem: &mut dyn MemoryDevice) -> Result<(), HartException>;
    fn read_reg(&self, idx: usize) -> u64;
    fn write_reg(&mut self, idx: usize, val: u64);
    fn pc(&self) -> u64;
}
```

Different ISA implementations implement this trait. The simulator loop calls `hart.step(mem)` without knowing which ISA it is.

#### Pattern 4: Instruction Decoding via Match + Bit Manipulation

Standard Rust approach — no special framework needed for simple ISAs:

```rust
fn decode(word: u32) -> Instruction {
    let opcode = word & 0x7F;
    let funct3 = (word >> 12) & 0x7;
    let funct7 = (word >> 25) & 0x7F;
    match opcode {
        0b0110011 => decode_r_type(funct3, funct7, word),
        0b0010011 => decode_i_type(funct3, word),
        0b0000011 => decode_load(funct3, word),
        _ => Instruction::Invalid,
    }
}
```

For larger ISAs (ARM AArch64 has ~1000 instruction encodings), this approach becomes verbose. The `deku` crate provides declarative bit-level parsing via derive macros — the struct fields map to bit ranges in the encoding, which mirrors ISA spec tables directly.

#### Pattern 5: Event Loop Architecture

For an event-driven timing simulator in Rust:

```rust
use std::collections::BinaryHeap;

struct EventQueue {
    queue: BinaryHeap<TimedEvent>,
}

struct TimedEvent {
    tick: u64,        // Ord on tick for BinaryHeap ordering
    callback: Box<dyn FnOnce(&mut SimState)>,
}

impl EventQueue {
    fn schedule(&mut self, tick: u64, cb: impl FnOnce(&mut SimState) + 'static) {
        self.queue.push(TimedEvent { tick, callback: Box::new(cb) });
    }

    fn step(&mut self, state: &mut SimState) -> Option<u64> {
        let event = self.queue.pop()?;
        (event.callback)(state);
        Some(event.tick)
    }
}
```

`BinaryHeap` in Rust is a max-heap by default; wrap `tick` in `Reverse<u64>` for a min-heap (earliest event first). The callback is `Box<dyn FnOnce>` for one-shot events.

**Challenge in Rust:** The closure captures references to simulator state, creating borrow checker complexity. The standard solution is to pass a mutable reference to the entire `SimState` into the callback, giving it access to everything it needs. This avoids the need for the closure to capture references at scheduling time.

#### Pattern 6: Separate Architectural State from Microarchitectural State

Following gem5's `StaticInst` / `DynInst` separation:

```rust
// Architectural state — what the ISA defines
struct ArchState {
    registers: [u64; 32],
    pc: u64,
    csrs: CsrFile,
}

// Microarchitectural state — what the timing model adds
struct MicroState {
    in_flight: VecDeque<InFlightInst>,
    rob: RingBuffer<RobEntry>,
    reservation_stations: Vec<ResStation>,
}
```

The functional simulation works entirely on `ArchState`. The timing model adds `MicroState` and wraps the functional execution with pipeline logic.

### rv8: The Most Complete Rust-Adjacent RISC-V Simulator Reference

rv8 (C++, but directly relevant architecturally) is the most complete reference design:
- High-performance x86-64 binary translator (JIT)
- User-mode simulator (syscall emulation, SE-equivalent)
- Full system emulator (FS-equivalent)
- ELF binary analysis tool
- ISA metadata library

rv8 demonstrates that a single project can span the entire continuum from pure ISS to JIT translator to full system emulator, sharing ISA metadata and decode infrastructure across all modes.

### Lessons for helm-ng Rust Implementation

1. **`trait MemoryDevice` is the right abstraction for the memory bus.** It handles both memory and devices uniformly. Use `Box<dyn MemoryDevice>` for flexibility.
2. **`Rc<RefCell<T>>` for single-core simulation; `Arc<Mutex<T>>` when adding multi-core.** Don't pay the multi-core tax until you need it.
3. **`Hart` as a trait enables multi-ISA naturally.** Each ISA implements the trait; the simulation loop is ISA-agnostic.
4. **`BinaryHeap<Reverse<TimedEvent>>` as the event queue.** Standard Rust, no external dependencies.
5. **Separate `ArchState` from `MicroState` from the start.** Adding timing later requires inserting between these layers; if they're merged, refactoring is expensive.
6. **`deku` for instruction decoding if encodings are complex.** For RISC-V's regular encoding, `match` + bit manipulation is sufficient. For ARM AArch64's irregular encoding, `deku` pays for itself.
7. **Avoid `async` in the hot path.** Async runtimes (Tokio, async-std) are designed for IO-bound tasks. The simulator's inner loop is CPU-bound — async adds overhead with no benefit. Use threads for parallelism.
8. **The `#[inline(always)]` attribute matters.** Frequent virtual dispatch (`dyn Trait`) and small function calls in the simulation hot path benefit from explicit inlining. Profile before assuming.

---

## 6. Cross-Cutting Architectural Lessons

### The Indispensable Interfaces

Every multi-mode, multi-ISA simulator needs exactly two clean interfaces:
1. **CPU → ISA:** What the CPU model calls to access ISA state (gem5's `ExecContext`). Must be ISA-agnostic.
2. **CPU → Memory:** What the CPU model calls to access memory (gem5's Port interface). Must support both synchronous (atomic/functional) and asynchronous (timing) access modes.

Get these interfaces right before implementing anything else. They are the hardest to change later.

### Fast-Forward + Checkpoint Is Not Optional

For any simulator targeting real workloads, the ability to fast-forward through initialization and checkpoint at a region of interest is not a nice-to-have — it is a practical necessity. Workloads like SPEC CPU take billions of instructions to reach stable execution. Without fast-forward, every experiment starts over.

The gem5 pattern (AtomicSimpleCPU for fast-forward + serialize/restore + O3CPU for ROI) should be designed in from the start. The functional core must be able to serialize its complete state to disk and restore from it. Memory state, register state, and device state must all be captured.

### The Event Queue Is the Universal Synchronization Primitive

Every timing-aware component in a well-designed simulator communicates via the event queue — not via direct function calls. Direct calls create temporal coupling (the caller and callee are synchronized in simulated time). Event-queue-mediated communication allows components to be decoupled in simulated time, which is essential for accurate modeling of propagation delays, memory access latencies, and pipeline stages.

### Interval Simulation Is the Practical Middle Ground

For a small team or solo developer building a general-purpose simulator, interval simulation (Sniper-style) offers the best return on investment:
- ~5% error vs. cycle-accurate
- ~10x faster than cycle-accurate
- Much simpler to implement than a full out-of-order pipeline
- Memory hierarchy simulation is the same (full cache + DRAM model)

A functional ISS + interval timing core + full memory hierarchy model is a viable, publishable research simulator. Cycle-accurate OoO simulation is a much larger engineering project with diminishing returns for most research questions.

### AArch32/AArch64 Interworking Is a Special Case

Supporting AArch32 and AArch64 in a single ISA object requires:
- A mode flag indicating current execution state (AArch32 or AArch64)
- Register banking for AArch32 (banked integer registers per privilege mode)
- Two separate decode trees (A32/T32 encoding is completely different from A64 encoding)
- Exception level tracking (EL0–EL3) and mode transitions on exceptions
- `PCState` must carry sub-architecture state (Thumb bit, ITSTATE)

This is substantially more complex than a single-mode ISA. If helm-ng's initial target is RISC-V only, deferring AArch32/AArch64 support is reasonable. The RISC-V ISA abstraction is much cleaner (no register banking, simpler privilege model).

---

## 7. Key References

- [Anatomy of gem5 Simulator (arXiv 2508.18043)](https://arxiv.org/abs/2508.18043) — AtomicSimpleCPU, TimingSimpleCPU, O3CPU, Ruby interaction
- [gem5 Simple CPU Models documentation](https://www.gem5.org/documentation/general_docs/cpu_models/SimpleCPU)
- [gem5 O3CPU documentation](https://www.gem5.org/documentation/general_docs/cpu_models/O3CPU)
- [gem5 ARM Implementation](https://www.gem5.org/documentation/general_docs/architecture_support/arm_implementation/)
- [gem5 RISC-V Architecture (DeepWiki)](https://deepwiki.com/gem5/gem5/6.3-risc-v-architecture)
- [gem5 ISA Parser documentation](https://www.gem5.org/documentation/general_docs/architecture_support/isa_parser/)
- [gem5 Memory System documentation](https://www.gem5.org/documentation/general_docs/memory_system/)
- [gem5 Event-driven programming tutorial](https://www.gem5.org/documentation/learning_gem5/part2/events/)
- [Interval Simulation — Sniper](https://snipersim.org/w/Interval_Simulation)
- [Interval Simulation HPCA 2010 paper](https://snipersim.org/w/Paper:Hpca2010Genbrugge)
- [MARSS full system simulator (ACM DAC 2011)](https://dl.acm.org/doi/10.1145/2024724.2024954)
- [MARSS GitHub (QEMU + PTLsim)](https://github.com/avadhpatel/marss)
- [QEMU-CAS cycle-accurate framework (CARRV 2023)](https://carrv.github.io/2023/papers/CARRV2023_paper_5_Cao.pdf)
- [TQSIM cycle-approximate QEMU-based simulator](https://www.sciencedirect.com/science/article/abs/pii/S1383762116300297)
- [Writing a RISC-V Emulator in Rust (rvemu book)](https://book.rvemu.app/)
- [Building a RISC-V Simulator in Rust — Greg Chadwick](https://gregchadwick.co.uk/blog/building-rrs-pt1/)
- [riscv-rust GitHub](https://github.com/takahirox/riscv-rust)
- [rv8 RISC-V simulator for x86-64](https://github.com/michaeljclark/rv8)
- [Rust emulator bus modeling — Rust forum](https://users.rust-lang.org/t/modeling-a-bus-and-its-components-in-rust-emulator/92583)
- [How to implement an ISA in gem5](http://old.gem5.org/How_to_implement_an_ISA.html)
- [gem5 Execution Basics](https://www.gem5.org/documentation/general_docs/cpu_models/execution_basics)
- [gem5 Full System Simulation tutorial](http://learning.gem5.org/book/part5/intro.html)
- [parti-gem5: gem5's Timing Mode Parallelised](https://arxiv.org/html/2308.09445v2)
