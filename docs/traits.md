# helm-ng Trait Reference

This document is the canonical developer reference for all core traits in helm-ng. It covers purpose, dispatch mechanism, performance contract, method-level documentation, implementation examples, and common mistakes for each trait.

Cross-references: [object-model.md](object-model.md) | [api.md](api.md)

---

## Design Axiom

> **Monomorphize only timing (hot path via generic parameter). Dispatch everything else via enum or `Box<dyn>` on cold paths.**

This rule drives every dispatch decision in the table below and is the first thing to check when adding a new trait or modifying an existing one.

---

## Trait Dependency Diagram

```
Hart ──────────────────────────► SimObject
Hart ──────────────────────────► ExecContext  (concrete impl inside HelmEngine, static dispatch)
Hart ──────────────────────────► MemInterface (passed by caller at step() time)
HelmEngine<T> ─────────────────► TimingModel  (T: generic param, monomorphized)
HelmEngine<T> ─────────────────► SyscallHandler (Box<dyn>, cold path)
HelmEngine<T> ─────────────────► GdbTarget    (impl on HelmEngine itself)
MemoryRegion::Mmio ───────────► MmioHandler  (Box<dyn>, cold path)
ExecContext ──────────────────► MemFault     (error type shared across read_mem / write_mem)
SyscallHandler ───────────────► ThreadContext (receives &mut dyn ThreadContext per call)
```

---

## Dispatch Strategy Table

| Trait | Dispatch | Reason |
|---|---|---|
| `TimingModel` | Generic param (monomorphized) | Hot path: called per instruction and per memory access; zero overhead required |
| `SimObject` | `Box<dyn SimObject>` in System tree | Cold path: lifecycle only; object tree built at elaboration time |
| `MmioHandler` | `Box<dyn MmioHandler>` in `MemoryRegion::Mmio` | Cold path: fired only on device-register reads/writes, not on every memory op |
| `ExecContext` | Concrete type (static dispatch via `impl ExecContext for CpuState`) | Hot path: called by ISA `execute()`; no indirection; `ExecContext` is generic over the CPU state struct |
| `GdbTarget` | `impl GdbTarget for HelmEngine<T>` (static) | Debug-only path; called from GDB server thread; no perf requirement |
| `SyscallHandler` | `Box<dyn SyscallHandler>` inside `HelmEngine` | Cold path: one call per syscall, not per instruction; allows SE/FS swap at config time |
| `MemInterface` | `&mut dyn MemInterface` at call site | Passed into `Hart::step()`; functional/atomic modes are cold; timing mode is hot but indirection is unavoidable here given the layered cache hierarchy |
| `Hart` | `Box<dyn Hart>` inside `HelmEngine` core list | One call per hart per simulation tick; overhead negligible relative to per-instruction work |

---

## `TimingModel`

### Purpose

`TimingModel` is the sole monomorphized hot-path seam in helm-ng. Every instruction fetch, memory access, branch misprediction, and cache miss pumps through one of its methods. The kernel is `HelmEngine<T: TimingModel>`, making `T` a compile-time constant; the compiler inlines the timing calls directly into the execution loop with no virtual dispatch overhead. This is how helm-ng supports pluggable timing models (simple cycle counter, out-of-order pipeline, trace-driven) without paying a vtable cost.

### Who Implements It

Core team and advanced timing model authors. This is not an ISA or device author interface. Implementing `TimingModel` requires understanding the full instruction pipeline and cache hierarchy. It is the most performance-sensitive interface in the codebase.

### Dispatch Mechanism

Generic parameter on `HelmEngine<T: TimingModel>`. The entire kernel is monomorphized for each `T`. Binary size grows with the number of timing model variants, but runtime overhead is zero.

### Performance Contract

**Hot path.** Every method must be `#[inline(always)]`. A `TimingModel` method that is not inlined is a bug. These methods are called inside the per-instruction execution loop and inside the memory access critical path. Allocating, locking a mutex, or doing I/O in any `TimingModel` method is prohibited.

### Method Reference

```rust
pub trait TimingModel: Send + 'static {
    fn on_memory_access(&mut self, addr: u64, cycles: u64);
    fn on_branch_mispredict(&mut self, penalty_cycles: u64);
    fn on_cache_miss(&mut self, level: u8, addr: u64, penalty: u64);
    fn advance_clock(&mut self, cycles: u64);
    fn current_tick(&self) -> u64;
}
```

| Method | Preconditions | Postconditions | Error behavior |
|---|---|---|---|
| `on_memory_access(addr, cycles)` | `cycles >= 1`. Called after every simulated memory read or write. | Internal timing state updated. No return value. | Must not panic. Ignore unknown addresses silently. |
| `on_branch_mispredict(penalty_cycles)` | `penalty_cycles >= 1`. Called once per confirmed mispredicted branch. | Pipeline flush penalty recorded. | Must not panic. |
| `on_cache_miss(level, addr, penalty)` | `level` in `{1, 2, 3}`. Called when a cache lookup fails. | Miss penalty recorded; `advance_clock` is NOT called implicitly — caller handles that. | Must not panic. |
| `advance_clock(cycles)` | `cycles >= 1`. Caller guarantees monotonic progression. | `current_tick()` increases by at least `cycles`. | Must not panic. |
| `current_tick()` | None. | Returns the current simulated cycle count. Monotonically non-decreasing. | Infallible. |

### Implementation Example

```rust
/// Minimal fixed-CPI timing model — one cycle per instruction, configurable
/// memory and branch penalties. Suitable for fast functional simulation.
#[derive(Default)]
pub struct FixedCpiModel {
    tick: u64,
    mem_penalty: u64,
    branch_penalty: u64,
}

impl FixedCpiModel {
    pub fn new(mem_penalty: u64, branch_penalty: u64) -> Self {
        Self { tick: 0, mem_penalty, branch_penalty }
    }
}

impl TimingModel for FixedCpiModel {
    #[inline(always)]
    fn on_memory_access(&mut self, _addr: u64, cycles: u64) {
        self.tick += cycles + self.mem_penalty;
    }

    #[inline(always)]
    fn on_branch_mispredict(&mut self, _penalty_cycles: u64) {
        self.tick += self.branch_penalty;
    }

    #[inline(always)]
    fn on_cache_miss(&mut self, level: u8, _addr: u64, penalty: u64) {
        // L3 misses are more expensive; override penalty with level scaling
        let scaled = match level {
            1 => penalty,
            2 => penalty * 4,
            _ => penalty * 16,
        };
        self.tick += scaled;
    }

    #[inline(always)]
    fn advance_clock(&mut self, cycles: u64) {
        self.tick += cycles;
    }

    #[inline(always)]
    fn current_tick(&self) -> u64 {
        self.tick
    }
}
```

### Common Mistakes

1. **Forgetting `#[inline(always)]` on every method.** The compiler may not inline across crate boundaries without it. Profile before shipping any timing model; missing inline annotations are a frequent source of unexpected slowdowns.

2. **Calling `advance_clock` inside `on_cache_miss`.** `on_cache_miss` records a miss; `advance_clock` is called separately by the kernel. Double-counting cycles here silently inflates all timing output.

3. **Holding a `Mutex` inside timing model state.** Any lock inside a hot-path method serializes the simulation loop. Use `UnsafeCell` or per-hart state instead, or defer aggregation to a post-step callback.

---

## `SimObject`

### Purpose

`SimObject` is the lifecycle interface for every named component in the System object tree. It mirrors the gem5 `SimObject` design: each component registers itself, connects to peers during `elaborate`, and transitions through `init → elaborate → startup` before simulation begins. `checkpoint_save`/`checkpoint_restore` enable snapshot and restore of simulation state.

### Who Implements It

Core team for built-in components (caches, memory controllers, interconnects). ISA authors for CPU cores. Device authors for peripheral models. Anyone adding a persistent component to the System tree implements `SimObject`.

### Dispatch Mechanism

`Box<dyn SimObject>` stored in the System tree. The System holds a `Vec<Box<dyn SimObject>>` and calls lifecycle methods on each in order. Dynamic dispatch here is correct: lifecycle calls happen once at startup, not per instruction.

### Performance Contract

**Cold path.** All lifecycle methods (`init`, `elaborate`, `startup`, `reset`, `checkpoint_save`, `checkpoint_restore`) are called at simulation startup or on explicit user request. Allocating, locking, and doing I/O are all fine here.

### Method Reference

```rust
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

| Method | Preconditions | Postconditions | Error behavior |
|---|---|---|---|
| `name()` | None. | Returns a unique, stable component name. Must not allocate on every call (return `&str` into `self`). | Infallible. |
| `init()` | Object has been registered in the System tree. Other objects may not yet be reachable. | Internal state initialized to defaults. No cross-object calls. | Should not panic. Log and no-op if dependencies are missing. |
| `elaborate(system)` | All `init()` calls have completed. `system` provides peer lookup. | All cross-object connections established. Port bindings done. | Panicking here is acceptable for fatal misconfiguration (e.g., missing required peer). |
| `startup()` | All `elaborate()` calls have completed. | Object is ready to participate in simulation. | Panicking acceptable for unrecoverable startup failure. |
| `reset()` | Simulation is paused or stopped. | Object returns to post-`startup` state. Cycle counters reset. | Should not panic. |
| `checkpoint_save()` | Simulation is paused. | Returns a serialized snapshot of all mutable state needed for restore. | Panicking acceptable if state is unsaveable. |
| `checkpoint_restore(data)` | Simulation is paused. `data` was produced by `checkpoint_save` on a compatible build. | Object state restored from snapshot. | Panic or return if data is corrupt or incompatible. |

### Implementation Example

```rust
pub struct SimpleRam {
    name: String,
    base: u64,
    data: Vec<u8>,
}

impl SimpleRam {
    pub fn new(name: impl Into<String>, base: u64, size: usize) -> Self {
        Self { name: name.into(), base, data: vec![0u8; size] }
    }
}

impl SimObject for SimpleRam {
    fn name(&self) -> &str {
        &self.name
    }

    fn init(&mut self) {
        // Nothing to do; data is zeroed at construction.
    }

    fn elaborate(&mut self, system: &mut System) {
        // Register our address range with the memory map.
        system.register_mmio(self.base, self.data.len() as u64, Box::new(RamHandler::new(&self.data)));
    }

    fn startup(&mut self) {
        log::info!("{}: {} bytes at {:#x}", self.name, self.data.len(), self.base);
    }

    fn reset(&mut self) {
        self.data.fill(0);
    }

    fn checkpoint_save(&self) -> Vec<u8> {
        // Simple: serialize base address (8 bytes) then raw data.
        let mut out = Vec::with_capacity(8 + self.data.len());
        out.extend_from_slice(&self.base.to_le_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    fn checkpoint_restore(&mut self, data: &[u8]) {
        assert!(data.len() >= 8 + self.data.len(), "checkpoint data too short");
        // Skip base address (first 8 bytes); restore data.
        self.data.copy_from_slice(&data[8..8 + self.data.len()]);
    }
}
```

### Common Mistakes

1. **Doing cross-object calls in `init()`.** At `init` time, peers are not yet guaranteed to be initialized. Move all cross-object wiring to `elaborate()`.

2. **Not restoring all mutable fields in `checkpoint_restore`.** If a field accumulates state during simulation (counters, dirty bits, FIFO queues) and is omitted from `checkpoint_save`, the restored simulation will diverge silently.

3. **Returning a non-unique name from `name()`.** The System tree uses names for peer lookup. Duplicate names cause wrong-object lookups during `elaborate` that are hard to debug.

---

## `MmioHandler`

### Purpose

`MmioHandler` is the read/write interface for memory-mapped I/O devices. When the memory subsystem decodes an address into a `MemoryRegion::Mmio` region, it calls `read` or `write` on the stored `Box<dyn MmioHandler>`. This isolates device register semantics (side effects, byte-enable masks, read-clears) from the main memory path.

### Who Implements It

Device authors. Every peripheral (UART, interrupt controller, timer, DMA controller, GPU model) implements `MmioHandler`. This is one of the most commonly implemented traits in helm-ng.

### Dispatch Mechanism

`Box<dyn MmioHandler>` stored inside `MemoryRegion::Mmio`. Dynamic dispatch is correct: MMIO accesses occur when guest software touches device registers, which is orders of magnitude less frequent than normal memory accesses.

### Performance Contract

**Cold path.** MMIO accesses are rare relative to normal memory traffic. Dynamic dispatch overhead is negligible. Device authors may allocate, lock, or do I/O (e.g., forwarding to a host socket for a virtual device).

### Method Reference

```rust
pub trait MmioHandler: Send + Sync {
    fn read(&self, offset: u64, size: usize) -> u64;
    fn write(&mut self, offset: u64, size: usize, value: u64);
    fn addr_range(&self) -> (u64, u64); // (base, size)
}
```

| Method | Preconditions | Postconditions | Error behavior |
|---|---|---|---|
| `read(offset, size)` | `offset < size` from `addr_range()`. `size` in `{1, 2, 4, 8}`. | Returns the register value at `offset`, zero-extended to `u64`. May have device-side effects (e.g., read-clear status bits). | Unimplemented offsets should return `0` and log a warning. Must not panic in release builds. |
| `write(offset, size, value)` | `offset < size` from `addr_range()`. `size` in `{1, 2, 4, 8}`. `value` is right-aligned in `u64`. | Device state updated. Side effects (DMA triggers, interrupt assertion) may occur. | Writes to read-only registers should be silently ignored or logged. Must not panic in release builds. |
| `addr_range()` | None. | Returns `(base, size)` where the device occupies `[base, base+size)`. Must be stable after `elaborate`. | Infallible. |

### Implementation Example

```rust
/// Minimal 16550-compatible UART stub. Supports TX-only for boot output.
pub struct Uart16550 {
    base: u64,
    tx_buf: std::collections::VecDeque<u8>,
    divisor_latch: u16,
    lcr: u8,
    irq_line: Option<Box<dyn Fn(bool) + Send + Sync>>,
}

impl Uart16550 {
    pub fn new(base: u64) -> Self {
        Self {
            base,
            tx_buf: Default::default(),
            divisor_latch: 1,
            lcr: 0,
            irq_line: None,
        }
    }

    pub fn with_irq(mut self, irq: impl Fn(bool) + Send + Sync + 'static) -> Self {
        self.irq_line = Some(Box::new(irq));
        self
    }
}

impl MmioHandler for Uart16550 {
    fn read(&self, offset: u64, _size: usize) -> u64 {
        match offset {
            0 => 0, // RBR: no RX data
            5 => 0x60, // LSR: TX empty, THR empty
            _ => 0,
        }
    }

    fn write(&mut self, offset: u64, size: usize, value: u64) {
        match offset {
            0 if (self.lcr & 0x80) == 0 => {
                // THR: transmit character
                let byte = (value & 0xFF) as u8;
                self.tx_buf.push_back(byte);
                // Drain to stdout for host visibility
                if let Ok(s) = std::str::from_utf8(&[byte]) {
                    print!("{}", s);
                }
            }
            3 => self.lcr = (value & 0xFF) as u8, // LCR
            _ => {
                log::trace!("uart: unhandled write offset={:#x} size={} val={:#x}", offset, size, value);
            }
        }
    }

    fn addr_range(&self) -> (u64, u64) {
        (self.base, 8) // 8 byte-wide registers
    }
}
```

### Common Mistakes

1. **Returning garbage for unimplemented registers instead of `0`.** Guest firmware often probes registers to detect features. Returning non-zero for unknown offsets causes spurious feature detection and subtle boot failures.

2. **Panicking on out-of-range offsets in release builds.** The memory subsystem should guard against out-of-range, but defense in depth matters. Log a warning and return `0` / ignore the write.

3. **`addr_range` returning different values after `elaborate`.** The memory map is built once. If `addr_range` changes, the device will no longer match its registered region and accesses will fall through to the wrong handler or generate faults.

---

## `ExecContext`

### Purpose

`ExecContext` is the **hot-path** interface between ISA instruction implementations and the CPU state they manipulate. An ISA `execute()` function receives a `&mut impl ExecContext` (or a concrete CPU state type that implements the trait) and calls methods to read/write registers, access memory, and raise exceptions: `read_int_reg`, `write_int_reg`, `read_pc`, `write_pc`, `read_mem`, `write_mem`, `raise_exception`. This decouples ISA logic from the specific CPU microarchitectural state struct, enabling the same ISA code to run on different CPU models.

For external/cold-path access — GDB inspection, syscall handlers, Python API, checkpointing — use `ThreadContext` instead. `ThreadContext` exposes the same state but is designed for use outside the instruction execution loop.

### Who Implements It

Core team (implements `ExecContext` on each concrete `CpuState` struct for each CPU model). ISA authors consume `ExecContext`; they do not implement it.

### Dispatch Mechanism

Static dispatch via concrete type. ISA `execute()` functions are generic over `C: ExecContext`, so the compiler monomorphizes them for each CPU model. This avoids vtable overhead on a path that runs once per instruction. For external/cold-path access (syscall handlers, GDB, Python inspection, checkpointing), `ThreadContext` is used instead. The `SyscallHandler` interface receives `&mut dyn ThreadContext`; dynamic dispatch there is acceptable.

### Performance Contract

**Hot path when used from ISA execute().** All register read/write methods and `read_mem`/`write_mem` are called per instruction. The concrete implementation must be lightweight — typically a direct struct field access or a small array index. Memory access methods may call into the cache hierarchy but must not do heap allocation.

### Method Reference

```rust
pub trait ExecContext {
    fn read_int_reg(&self, idx: usize) -> u64;
    fn write_int_reg(&mut self, idx: usize, val: u64);
    fn read_float_reg(&self, idx: usize) -> f64;
    fn write_float_reg(&mut self, idx: usize, val: f64);
    fn read_csr(&self, csr: u16) -> u64;
    fn write_csr(&mut self, csr: u16, val: u64);
    fn read_pc(&self) -> u64;
    fn write_pc(&mut self, val: u64);
    fn read_mem(&self, addr: u64, size: usize) -> Result<u64, MemFault>;
    fn write_mem(&mut self, addr: u64, size: usize, val: u64) -> Result<(), MemFault>;
    fn raise_exception(&mut self, vector: u32, tval: u64);
}
```

| Method | Preconditions | Postconditions | Error behavior |
|---|---|---|---|
| `read_int_reg(idx)` | `idx < NREGS` for the ISA. | Returns the current value of integer register `idx`. Register 0 (x0 on RISC-V) must always return `0`. | Panic on out-of-bounds `idx` in debug; UB-free but unspecified in release. |
| `write_int_reg(idx, val)` | `idx < NREGS`. | Register `idx` updated to `val`. Writes to register 0 are silently discarded. | Panic on out-of-bounds in debug. |
| `read_float_reg(idx)` | `idx < NFPREGS`. | Returns the float register as `f64`. NaN-boxing rules are the caller's responsibility. | Panic on out-of-bounds in debug. |
| `write_float_reg(idx, val)` | `idx < NFPREGS`. | Float register updated. | Panic on out-of-bounds in debug. |
| `read_csr(csr)` | `csr` is a valid CSR address for the ISA. | Returns CSR value. Side-effect CSRs (e.g., `instret`) may update internal counters. | Return `0` and log for unknown CSR in permissive mode; raise illegal-instruction exception in strict mode. |
| `write_csr(csr, val)` | `csr` is a valid, writable CSR. | CSR updated; side effects (mode change, interrupt enable) applied immediately. | Raise illegal-instruction for read-only or unknown CSR in strict mode. |
| `read_pc()` | None. | Returns the program counter of the current instruction. | Infallible. |
| `write_pc(val)` | `val` is a valid instruction address (alignment enforced by ISA). | PC updated; next fetch from `val`. | Misaligned addresses should raise `InstructionAddressMisaligned` via `raise_exception`. |
| `read_mem(addr, size)` | `size` in `{1, 2, 4, 8}`. | Returns memory value at `addr`, zero-extended. May trigger cache hierarchy. | Returns `Err(MemFault)` on page fault, access fault, or misalignment. Caller must propagate via `raise_exception`. |
| `write_mem(addr, size, val)` | `size` in `{1, 2, 4, 8}`. | Memory at `addr` updated. | Returns `Err(MemFault)` on fault. |
| `raise_exception(vector, tval)` | `vector` is a valid exception code for the ISA. | Exception state set; PC redirected to exception handler. Marks the current instruction as non-completing. | Must not return; subsequent `read_pc()` should return the trap vector address. |

### Implementation Example

```rust
/// Minimal RISC-V 64-bit execution context backed by a flat register file.
pub struct Rv64CpuState {
    xregs: [u64; 32],
    fpregs: [f64; 32],
    csrs: std::collections::HashMap<u16, u64>,
    pc: u64,
    pending_exception: Option<(u32, u64)>,
}

impl Rv64CpuState {
    pub fn new(entry: u64) -> Self {
        Self {
            xregs: [0u64; 32],
            fpregs: [0f64; 32],
            csrs: Default::default(),
            pc: entry,
            pending_exception: None,
        }
    }
}

impl ExecContext for Rv64CpuState {
    fn read_int_reg(&self, idx: usize) -> u64 {
        if idx == 0 { 0 } else { self.xregs[idx] }
    }

    fn write_int_reg(&mut self, idx: usize, val: u64) {
        if idx != 0 { self.xregs[idx] = val; }
    }

    fn read_float_reg(&self, idx: usize) -> f64 { self.fpregs[idx] }
    fn write_float_reg(&mut self, idx: usize, val: f64) { self.fpregs[idx] = val; }

    fn read_csr(&self, csr: u16) -> u64 {
        *self.csrs.get(&csr).unwrap_or(&0)
    }

    fn write_csr(&mut self, csr: u16, val: u64) {
        self.csrs.insert(csr, val);
    }

    fn read_pc(&self) -> u64 { self.pc }
    fn write_pc(&mut self, val: u64) { self.pc = val; }

    fn read_mem(&self, _addr: u64, _size: usize) -> Result<u64, MemFault> {
        // Delegate to memory subsystem (abbreviated here)
        todo!("wire to MemInterface")
    }

    fn write_mem(&mut self, _addr: u64, _size: usize, _val: u64) -> Result<(), MemFault> {
        todo!("wire to MemInterface")
    }

    fn raise_exception(&mut self, vector: u32, tval: u64) {
        self.pending_exception = Some((vector, tval));
        // Redirect PC to trap vector (simplified; real impl reads mtvec CSR)
        self.pc = *self.csrs.get(&0x305).unwrap_or(&0);
    }
}
```

### Common Mistakes

1. **Not silently discarding writes to x0.** Every RISC-V instruction that writes `rd = 0` must be a no-op. Forgetting this produces architecturally illegal behavior that only manifests in programs that rely on x0 always reading zero.

2. **Not setting `pending_exception` before returning from `raise_exception`.** The ISA execute loop must check for pending exceptions after each instruction. If the exception is not recorded, the instruction appears to complete normally and execution continues at the wrong PC.

3. **Using `HashMap` for CSRs in a hot-path production implementation.** The example above uses `HashMap` for clarity. In a real implementation, CSRs must be a fixed-size array (indexed by CSR address) to avoid per-instruction heap allocation.

---

## `GdbTarget`

### Purpose

`GdbTarget` is the interface between the GDB Remote Serial Protocol (RSP) server and the simulation kernel. `GdbServer` runs on a dedicated thread, decodes GDB RSP packets, and calls `GdbTarget` methods to read/write registers and memory, single-step, continue, and manage breakpoints/watchpoints. This isolates the GDB wire protocol from the simulation engine.

### Who Implements It

Core team only. `GdbTarget` is implemented once on `HelmEngine<T>`. ISA and device authors do not implement this trait.

### Dispatch Mechanism

`impl GdbTarget for HelmEngine<T>` — static dispatch. The GDB server holds a reference to `HelmEngine` through a trait object `Box<dyn GdbTarget>` only at the server boundary, which is fine since GDB sessions are not performance-critical.

### Performance Contract

**Cold/debug path.** GDB commands arrive over a socket at human or script speed. No inlining or allocation restrictions. Blocking is acceptable. `step()` and `continue()` execute the simulation engine but that cost is the simulation itself, not the trait dispatch.

### Method Reference

```rust
pub trait GdbTarget {
    fn read_register(&self, reg: GdbReg) -> u64;
    fn write_register(&mut self, reg: GdbReg, val: u64);
    fn read_memory(&self, addr: u64, len: usize) -> Result<Vec<u8>, MemFault>;
    fn write_memory(&mut self, addr: u64, data: &[u8]) -> Result<(), MemFault>;
    fn step(&mut self) -> StopReason;
    fn r#continue(&mut self) -> StopReason;
    fn set_breakpoint(&mut self, addr: u64, kind: BreakpointKind) -> bool;
    fn clear_breakpoint(&mut self, addr: u64) -> bool;
    fn set_watchpoint(&mut self, addr: u64, size: usize, kind: WatchKind) -> bool;
    fn clear_watchpoint(&mut self, addr: u64) -> bool;
}
```

| Method | Preconditions | Postconditions | Error behavior |
|---|---|---|---|
| `read_register(reg)` | Simulation is paused. `reg` is a valid `GdbReg` for the current ISA. | Returns the register value in target byte order. | Return `0` and log for unknown registers. |
| `write_register(reg, val)` | Simulation is paused. | Register updated. | Log and ignore for unknown or read-only registers. |
| `read_memory(addr, len)` | `len > 0`. | Returns `len` bytes starting at `addr` using functional (non-timing) memory access. | `Err(MemFault)` for unmapped or inaccessible addresses. GDB server sends error packet. |
| `write_memory(addr, data)` | `data.len() > 0`. | Memory updated at `addr`. | `Err(MemFault)` for unmapped addresses. |
| `step()` | Simulation is paused. | Executes exactly one instruction. Returns `StopReason` describing why execution stopped (breakpoint hit, exception, etc.). | Must always return a valid `StopReason`, even on exception. |
| `continue()` | Simulation is paused. | Runs until a breakpoint, watchpoint, exception, or explicit halt. Returns `StopReason`. | Must return when a stop condition is met. Must not busy-loop without yielding if running in a multithreaded host. |
| `set_breakpoint(addr, kind)` | None. | Breakpoint installed at `addr`. Returns `true` if successful, `false` if the breakpoint table is full or address is invalid. | Never panics. |
| `clear_breakpoint(addr)` | None. | Breakpoint removed. Returns `true` if a breakpoint existed at `addr`. | Never panics. |
| `set_watchpoint(addr, size, kind)` | `size > 0`. | Watchpoint installed. Returns `true` on success. | Never panics. |
| `clear_watchpoint(addr)` | None. | Watchpoint removed at `addr`. Returns `true` if one existed. | Never panics. |

### Implementation Example

```rust
impl<T: TimingModel> GdbTarget for HelmEngine<T> {
    fn read_register(&self, reg: GdbReg) -> u64 {
        self.hart.get_int_reg(reg.index())
    }

    fn write_register(&mut self, reg: GdbReg, val: u64) {
        self.hart.set_int_reg(reg.index(), val);
    }

    fn read_memory(&self, addr: u64, len: usize) -> Result<Vec<u8>, MemFault> {
        let mut buf = vec![0u8; len];
        for (i, byte) in buf.iter_mut().enumerate() {
            *byte = self.mem.read_functional(addr + i as u64, 1) as u8;
        }
        Ok(buf)
    }

    fn write_memory(&mut self, addr: u64, data: &[u8]) -> Result<(), MemFault> {
        for (i, &byte) in data.iter().enumerate() {
            self.mem.write_functional(addr + i as u64, 1, byte as u64);
        }
        Ok(())
    }

    fn step(&mut self) -> StopReason {
        match self.hart.step(&mut self.mem) {
            Ok(()) => {
                let pc = self.hart.get_pc();
                if self.breakpoints.contains(&pc) {
                    StopReason::Breakpoint(pc)
                } else {
                    StopReason::Step
                }
            }
            Err(e) => StopReason::Exception(e),
        }
    }

    fn r#continue(&mut self) -> StopReason {
        loop {
            let reason = self.step();
            if !matches!(reason, StopReason::Step) {
                return reason;
            }
        }
    }

    fn set_breakpoint(&mut self, addr: u64, _kind: BreakpointKind) -> bool {
        self.breakpoints.insert(addr);
        true
    }

    fn clear_breakpoint(&mut self, addr: u64) -> bool {
        self.breakpoints.remove(&addr)
    }

    fn set_watchpoint(&mut self, addr: u64, size: usize, kind: WatchKind) -> bool {
        self.watchpoints.insert(addr, (size, kind));
        true
    }

    fn clear_watchpoint(&mut self, addr: u64) -> bool {
        self.watchpoints.remove(&addr).is_some()
    }
}
```

### Common Mistakes

1. **Using timing memory access in `read_memory`/`write_memory`.** GDB memory reads must use functional access (`read_functional`) so they do not perturb simulation timing state or wait on pending cache operations.

2. **`continue()` busy-looping without yielding on the host thread.** If `HelmEngine` runs on the GDB server thread, a tight `continue()` loop blocks the thread from receiving the GDB interrupt packet (`\x03`). Check for a stop flag set by the GDB server's reader side.

3. **Not checking breakpoints after `step()` in `continue()`.** Reusing `step()` in `continue()` is correct only if `step()` checks breakpoints. If they are checked separately at a different level, breakpoints will be missed on the instruction immediately after the initial stop.

---

## `SyscallHandler`

### Purpose

`SyscallHandler` enables the simulation to support both Syscall-Emulation (SE) mode (intercept system calls and service them on the host OS) and Full-System (FS) mode (pass system calls to a simulated OS kernel) through a swappable `Box<dyn SyscallHandler>` stored in `HelmEngine`. This is the primary mechanism for SE vs FS mode selection at configuration time.

### Who Implements It

Core team for built-in SE mode (Linux ABI emulation) and FS mode (pass-through to guest kernel). Advanced users may implement custom syscall handlers for specialized workloads (e.g., a syscall counter, a fuzzing harness, or a deterministic replay handler).

### Dispatch Mechanism

`Box<dyn SyscallHandler>` stored in `HelmEngine`. Swapped at simulation startup based on Python config (`mode = "se"` or `mode = "fs"`). Dynamic dispatch is correct: `handle` is called once per `ecall`/`syscall` instruction, which is orders of magnitude less frequent than per-instruction work.

### Performance Contract

**Cold path.** Syscalls are rare in typical workloads. Dynamic dispatch, allocation, and host OS calls are all acceptable inside `handle`. The SE handler in particular calls `libc` functions on the host.

### Method Reference

```rust
pub trait SyscallHandler: Send {
    fn handle(&mut self, nr: u64, args: [u64; 6], ctx: &mut dyn ThreadContext) -> SyscallResult;
    fn supported_syscalls(&self) -> &[u64];
}
```

| Method | Preconditions | Postconditions | Error behavior |
|---|---|---|---|
| `handle(nr, args, ctx)` | `ctx` is the CPU state at the point of the `ecall`/`syscall` instruction. `nr` is the syscall number from the appropriate register (a7 on RISC-V). `args` are the six argument registers. | Returns `SyscallResult` carrying the return value and optional error code. The handler may modify `ctx` (e.g., write return value to a0, advance PC past the syscall instruction). | Return `SyscallResult::Err(ENOSYS)` for unsupported syscalls. Must not panic. |
| `supported_syscalls()` | None. | Returns a slice of syscall numbers this handler services. Used for validation and debug tooling. May be incomplete (i.e., returning fewer than all supported numbers is safe). | Infallible. |

### Implementation Example

```rust
/// Minimal SE-mode syscall handler: supports exit, write to stdout only.
pub struct MinimalSeHandler;

impl SyscallHandler for MinimalSeHandler {
    fn handle(&mut self, nr: u64, args: [u64; 6], ctx: &mut dyn ThreadContext) -> SyscallResult {
        match nr {
            // write(fd, buf, count)
            64 => {
                let fd = args[0];
                let buf_addr = args[1];
                let count = args[2] as usize;

                if fd != 1 && fd != 2 {
                    return SyscallResult::Err(libc::EBADF as u64);
                }

                // Read bytes from guest memory via functional access
                let mut bytes = Vec::with_capacity(count);
                for i in 0..count as u64 {
                    match ctx.read_mem(buf_addr + i, 1) {
                        Ok(b) => bytes.push(b as u8),
                        Err(_) => return SyscallResult::Err(libc::EFAULT as u64),
                    }
                }
                let written = unsafe { libc::write(fd as i32, bytes.as_ptr() as *const _, bytes.len()) };
                SyscallResult::Ok(written as u64)
            }
            // exit_group(code)
            94 => {
                std::process::exit(args[0] as i32);
            }
            _ => SyscallResult::Err(libc::ENOSYS as u64),
        }
    }

    fn supported_syscalls(&self) -> &[u64] {
        &[64, 94]
    }
}
```

### Common Mistakes

1. **Writing the return value directly into a register inside `handle` and also having `HelmEngine` write it again.** Decide once: either `handle` writes the return register and advances PC, or `HelmEngine` does it using `SyscallResult`. Doing both corrupts the return value.

2. **Panicking on unsupported syscall numbers.** Many programs probe for optional syscalls (e.g., `getrandom`). Return `SyscallResult::Err(ENOSYS)` so the guest can handle the error gracefully.

3. **Not advancing PC past the `ecall` instruction.** If `handle` or `HelmEngine` does not move PC forward by the instruction size after a syscall, the simulator will re-execute the `ecall` indefinitely.

---

## `MemInterface`

### Purpose

`MemInterface` provides three distinct memory access modes on a single interface, covering the full range of simulation needs:

- **Timing mode** — asynchronous, tag-based, models realistic cache and bus latency. Used by the in-order and out-of-order CPU models during timed simulation.
- **Atomic mode** — synchronous, returns value plus estimated latency. Used by simple CPU models that want timing feedback without full async complexity.
- **Functional mode** — instantaneous, always succeeds, bypasses timing entirely. Used for binary loading, GDB memory access, and checkpoint save/restore.

### Who Implements It

Core team for built-in memory models (flat memory, banked DRAM, cache hierarchy). ISA and device authors consume `MemInterface` but do not implement it unless they are building a custom memory model.

### Dispatch Mechanism

`&mut dyn MemInterface` passed into `Hart::step()` at each call. Dynamic dispatch at the `Hart`/`MemInterface` boundary is unavoidable given the layered cache hierarchy; the overhead is dominated by the memory operation itself (TLB lookup, cache lookup) rather than the vtable call.

### Performance Contract

**Mixed.** Functional mode is cold (debug, load time). Atomic mode is warm (simple CPU models). Timing mode is hot (full-system timed simulation). The timing-mode methods (`read_timing`, `write_timing`, `on_read_complete`) are in the per-instruction hot path but the trait dispatch overhead is negligible relative to the cache hierarchy traversal they invoke.

### Method Reference

```rust
pub trait MemInterface {
    // Timing mode
    fn read_timing(&mut self, addr: u64, size: usize, tag: u64) -> Result<(), MemFault>;
    fn write_timing(&mut self, addr: u64, size: usize, val: u64, tag: u64) -> Result<(), MemFault>;
    fn on_read_complete(&mut self, tag: u64, val: u64, cycles: u64);
    // Atomic mode
    fn read_atomic(&self, addr: u64, size: usize) -> Result<(u64, u64), MemFault>;
    fn write_atomic(&mut self, addr: u64, size: usize, val: u64) -> Result<u64, MemFault>;
    // Functional mode
    fn read_functional(&self, addr: u64, size: usize) -> u64;
    fn write_functional(&mut self, addr: u64, size: usize, val: u64);
}
```

| Method | Preconditions | Postconditions | Error behavior |
|---|---|---|---|
| `read_timing(addr, size, tag)` | `size` in `{1,2,4,8}`. `tag` is a caller-assigned request ID, unique among in-flight requests. | Initiates an async memory read. Completion is signaled by a later call to `on_read_complete(tag, ...)`. Returns `Ok(())` immediately if the request was accepted. | `Err(MemFault)` if the address is unmapped (immediate). Never blocks. |
| `write_timing(addr, size, val, tag)` | Same as `read_timing`. | Initiates an async memory write. Write completion may be signaled via `on_read_complete` if the implementation tracks write ACKs, or ignored. | `Err(MemFault)` for unmapped addresses. |
| `on_read_complete(tag, val, cycles)` | `tag` matches a previously issued `read_timing` tag. `cycles` is the total latency from request to completion. | CPU state updated by caller using `val`. | Must not be called with an unrecognized `tag`. Caller is responsible for pairing tags. |
| `read_atomic(addr, size)` | `size` in `{1,2,4,8}`. | Returns `(value, latency_cycles)`. Synchronous; completes immediately. | `Err(MemFault)` for faults. |
| `write_atomic(addr, size, val)` | `size` in `{1,2,4,8}`. | Returns estimated write latency in cycles. Memory updated synchronously. | `Err(MemFault)` for faults. |
| `read_functional(addr, size)` | Any mapped or unmapped address. | Returns memory value. Returns `0` for unmapped addresses (no fault). | Infallible. |
| `write_functional(addr, size, val)` | Any address. | Memory updated. No-op for unmapped addresses. | Infallible. |

### Implementation Example

```rust
/// Flat memory model — no cache, fixed latency, synchronous backing store.
pub struct FlatMemory {
    data: Vec<u8>,
    base: u64,
    read_latency: u64,
    write_latency: u64,
}

impl FlatMemory {
    pub fn new(base: u64, size: usize, read_latency: u64, write_latency: u64) -> Self {
        Self { data: vec![0u8; size], base, read_latency, write_latency }
    }

    fn offset(&self, addr: u64, size: usize) -> Result<usize, MemFault> {
        let off = addr.checked_sub(self.base).ok_or(MemFault::AccessFault(addr))? as usize;
        if off + size > self.data.len() {
            return Err(MemFault::AccessFault(addr));
        }
        Ok(off)
    }

    fn load(&self, off: usize, size: usize) -> u64 {
        let mut val = 0u64;
        for i in 0..size {
            val |= (self.data[off + i] as u64) << (8 * i);
        }
        val
    }

    fn store(&mut self, off: usize, size: usize, val: u64) {
        for i in 0..size {
            self.data[off + i] = (val >> (8 * i)) as u8;
        }
    }
}

impl MemInterface for FlatMemory {
    // --- Timing mode (simplified: immediate completion via on_read_complete) ---
    fn read_timing(&mut self, addr: u64, size: usize, tag: u64) -> Result<(), MemFault> {
        let off = self.offset(addr, size)?;
        let val = self.load(off, size);
        // In a real impl, enqueue completion event. Here we call back immediately.
        self.on_read_complete(tag, val, self.read_latency);
        Ok(())
    }

    fn write_timing(&mut self, addr: u64, size: usize, val: u64, _tag: u64) -> Result<(), MemFault> {
        let off = self.offset(addr, size)?;
        self.store(off, size, val);
        Ok(())
    }

    fn on_read_complete(&mut self, _tag: u64, _val: u64, _cycles: u64) {
        // Flat memory delivers immediately; the CPU polls or receives this callback.
        // Real implementations deliver to a completion queue.
    }

    // --- Atomic mode ---
    fn read_atomic(&self, addr: u64, size: usize) -> Result<(u64, u64), MemFault> {
        let off = self.offset(addr, size)?;
        Ok((self.load(off, size), self.read_latency))
    }

    fn write_atomic(&mut self, addr: u64, size: usize, val: u64) -> Result<u64, MemFault> {
        let off = self.offset(addr, size)?;
        self.store(off, size, val);
        Ok(self.write_latency)
    }

    // --- Functional mode ---
    fn read_functional(&self, addr: u64, size: usize) -> u64 {
        self.offset(addr, size).map(|off| self.load(off, size)).unwrap_or(0)
    }

    fn write_functional(&mut self, addr: u64, size: usize, val: u64) {
        if let Ok(off) = self.offset(addr, size) {
            self.store(off, size, val);
        }
    }
}
```

### Common Mistakes

1. **Calling `on_read_complete` from inside `read_timing` in an async model.** In a model with a real latency queue, `on_read_complete` should be called from the timing event queue, not immediately from `read_timing`. Calling it immediately defeats timing accuracy and can cause reentrant borrow issues.

2. **Using timing-mode methods in functional contexts (binary loading, GDB).** Always use `read_functional`/`write_functional` for non-simulation accesses. Using `read_atomic` or `read_timing` during binary load distorts timing statistics.

3. **Not handling the case where `addr` is below `base` in `offset()`.** A guest may legally probe below the base address. `checked_sub` is required; wrapping subtraction causes a panic or silently accesses wrong memory.

---

## `Hart`

### Purpose

`Hart` (hardware thread) is the multi-ISA CPU interface. Each `Hart` represents one hardware execution context. `HelmEngine` holds a list of `Box<dyn Hart>` and drives them each simulation tick by calling `step`. `Hart` extends `SimObject`, so each hart participates in the full component lifecycle. The `isa()` and `exec_mode()` methods allow runtime ISA and mode introspection for GDB register mapping, trace logging, and Python-config validation.

### Who Implements It

ISA authors. A new ISA port provides a concrete `Hart` implementation (e.g., `Rv64Hart`, `Aarch64Hart`, `X86Hart`). Core team may also implement thin wrapper harts for special purposes (tracing hart, record/replay hart).

### Dispatch Mechanism

`Box<dyn Hart>` in `HelmEngine`'s hart list. `step` is called once per hart per simulation tick. The per-instruction cost dominates the vtable dispatch cost by orders of magnitude, so dynamic dispatch here is correct and simplifies multi-ISA support.

### Performance Contract

**Warm path.** `step()` is called per hart per tick, but each call executes a full instruction (fetch, decode, execute, writeback), so the vtable overhead is negligible. The hot path is inside `step()` where `ExecContext` methods are called per instruction (static dispatch). Do not inline `step` aggressively across the `Box<dyn Hart>` boundary — the compiler cannot do so anyway.

### Method Reference

```rust
pub trait Hart: SimObject {
    fn step(&mut self, mem: &mut dyn MemInterface) -> Result<(), HartException>;
    fn get_pc(&self) -> u64;
    fn get_int_reg(&self, idx: usize) -> u64;
    fn set_int_reg(&mut self, idx: usize, val: u64);
    fn isa(&self) -> Isa;
    fn exec_mode(&self) -> ExecMode;
}
```

| Method | Preconditions | Postconditions | Error behavior |
|---|---|---|---|
| `step(mem)` | Hart is in a runnable state. `mem` provides access to the full simulated address space. | Fetches, decodes, and executes one instruction. PC advanced. Registers and memory updated. Any pending exceptions handled (trap entry). | Returns `Err(HartException)` for unrecoverable simulation errors (e.g., double fault, unimplemented instruction that cannot be emulated). Normal architectural exceptions (page fault, illegal instruction) are handled internally and do not propagate as `Err`. |
| `get_pc()` | None. | Returns the current PC. | Infallible. |
| `get_int_reg(idx)` | `idx < NREGS` for the ISA. | Returns register value. | Panic in debug on out-of-bounds. |
| `set_int_reg(idx, val)` | `idx < NREGS`. | Register updated. | Panic in debug on out-of-bounds. |
| `isa()` | None. | Returns the `Isa` enum variant for this hart (e.g., `Isa::Rv64Gc`, `Isa::Aarch64`). Stable for the lifetime of the hart. | Infallible. |
| `exec_mode()` | None. | Returns `ExecMode::Se` or `ExecMode::Fs`. | Infallible. |

### Implementation Example

```rust
pub struct Rv64Hart {
    name: String,
    state: Rv64CpuState,
    isa_variant: Isa,
    mode: ExecMode,
}

impl Rv64Hart {
    pub fn new(name: impl Into<String>, entry: u64, mode: ExecMode) -> Self {
        Self {
            name: name.into(),
            state: Rv64CpuState::new(entry),
            isa_variant: Isa::Rv64Gc,
            mode,
        }
    }
}

// SimObject impl (required by Hart: SimObject)
impl SimObject for Rv64Hart {
    fn name(&self) -> &str { &self.name }
    fn init(&mut self) { /* nothing */ }
    fn elaborate(&mut self, _system: &mut System) { /* register with system if needed */ }
    fn startup(&mut self) { log::info!("{}: RISC-V 64 hart starting at {:#x}", self.name, self.state.read_pc()); }
    fn reset(&mut self) { self.state = Rv64CpuState::new(0); }
    fn checkpoint_save(&self) -> Vec<u8> { todo!() }
    fn checkpoint_restore(&mut self, _data: &[u8]) { todo!() }
}

impl Hart for Rv64Hart {
    fn step(&mut self, mem: &mut dyn MemInterface) -> Result<(), HartException> {
        let pc = self.state.read_pc();

        // Instruction fetch (functional — timing is handled by TimingModel separately)
        let raw = mem.read_functional(pc, 4) as u32;

        // Decode
        let insn = rv64_decode(raw).map_err(|_| HartException::IllegalInstruction(raw as u64))?;

        // Execute (static dispatch: ExecContext is Rv64CpuState)
        rv64_execute(&insn, &mut self.state, mem);

        Ok(())
    }

    fn get_pc(&self) -> u64 { self.state.read_pc() }

    fn get_int_reg(&self, idx: usize) -> u64 { self.state.read_int_reg(idx) }
    fn set_int_reg(&mut self, idx: usize, val: u64) { self.state.write_int_reg(idx, val); }

    fn isa(&self) -> Isa { self.isa_variant }
    fn exec_mode(&self) -> ExecMode { self.mode }
}
```

### Common Mistakes

1. **Propagating architectural exceptions as `Err(HartException)` from `step()`.** Page faults, illegal instructions, and misaligned access are architectural events handled by the trap mechanism inside `step()` (call `raise_exception` on `ExecContext`). `Err` is reserved for simulation-level failures that prevent execution from continuing at all.

2. **Not implementing `SimObject::reset()` correctly.** If the hart is reset but `Rv64CpuState` is not re-initialized, checkpoint restore and simulation reset tests will run with stale register values. Reset must restore all mutable state to the post-`startup` baseline.

3. **Using `read_timing` inside `step()` for the instruction fetch.** Instruction fetch timing should be reported to the `TimingModel` (via the `HelmEngine` call after `step()` returns), not handled by calling `read_timing` directly inside `step()`. Calling `read_timing` in `step()` mixes timing-mode async semantics with the synchronous step loop, causing incorrect timing attribution.

---

## Quick Reference

| Trait | Implementors | Consumer | Dispatch | Path |
|---|---|---|---|---|
| `TimingModel` | Core team / timing model authors | `HelmEngine<T>` | Generic param | Hot |
| `SimObject` | Everyone (ISA, device, core) | System tree | `Box<dyn>` | Cold |
| `MmioHandler` | Device authors | `MemoryRegion::Mmio` | `Box<dyn>` | Cold |
| `ExecContext` | Core team (one per CPU model) | ISA `execute()` | Concrete / static | Hot |
| `GdbTarget` | Core team (`HelmEngine`) | `GdbServer` | `impl` on kernel | Debug-only |
| `SyscallHandler` | Core team (SE/FS), advanced users | `HelmEngine<T>` | `Box<dyn>` | Cold |
| `MemInterface` | Core team (memory models) | `Hart::step()` | `&mut dyn` | Mixed |
| `Hart` | ISA authors | `HelmEngine<T>` | `Box<dyn>` | Warm |

---

*Cross-references: [object-model.md](object-model.md) | [api.md](api.md)*
