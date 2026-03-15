# LLD: HelmSim and build_simulator

> Low-Level Design for the `HelmSim` PyO3 boundary enum and the `build_simulator()` factory.

**Crate:** `helm-engine`
**Files:**
- `crates/helm-engine/src/sim.rs` — `HelmSim` enum and its `impl`
- `crates/helm-engine/src/factory.rs` — `build_simulator()` and `TimingChoice`
- `crates/helm-python/src/factory.rs` — `#[pyfunction]` wrapper (in `helm-python`, not `helm-engine`)

---

## Table of Contents

1. [HelmSim Enum Definition](#1-helmsim-enum-definition)
2. [impl HelmSim — Full Method Set](#2-impl-helmsim--full-method-set)
3. [build_simulator Factory](#3-build_simulator-factory)
4. [PyO3 #[pyclass] Wrapper Design](#4-pyo3-pyclass-wrapper-design)
5. [Python API: sim.read_reg(0) Resolution Chain](#5-python-api-simread_reg0-resolution-chain)
6. [Variant Exhaustiveness Guarantee](#6-variant-exhaustiveness-guarantee)
7. [Extending HelmSim: Adding a New Timing Model](#7-extending-helmsim-adding-a-new-timing-model)

---

## 1. HelmSim Enum Definition

`HelmSim` wraps the three concrete `HelmEngine<T>` variants in a single enum. This provides:

1. **One enum dispatch per Python call** — not per instruction.
2. **Compiler-enforced exhaustiveness** — adding a new timing model variant produces a compile error at every unhandled match site.
3. **`'static` safety** — the enum is `'static` because each variant owns its `HelmEngine<T>`, satisfying PyO3's requirement.
4. **No vtable** — `HelmSim` methods contain direct `match` arms; no `Box<dyn>` indirection.

```rust
// crates/helm-engine/src/sim.rs

use helm_timing::{Virtual, Interval, Accurate};
use helm_core::{ThreadContext, MemInterface};
use helm_memory::MemoryMap;
use helm_devices::bus::event_bus::HelmEventBus;

use crate::engine::HelmEngine;
use crate::StopReason;

/// PyO3 boundary enum. Wraps the three concrete timing-model specializations.
///
/// # Dispatch Model
///
/// Every method on `HelmSim` contains exactly one `match self { ... }` arm per variant.
/// This dispatch happens once per Python call — not per instruction.
///
/// After `HelmSim::run()` dispatches into `HelmEngine<T>::run()`, no further enum
/// dispatch occurs until Python makes another call. The inner loop inside
/// `HelmEngine<T>::run()` is a tight loop with no enum overhead at the `HelmSim` level.
///
/// # Exhaustiveness
///
/// The Rust compiler enforces that every `match self` arm covers all three variants.
/// Forgetting to update a match after adding a variant is a compile error, not a
/// runtime bug.
pub enum HelmSim {
    Virtual(HelmEngine<Virtual>),
    Interval(HelmEngine<Interval>),
    Accurate(HelmEngine<Accurate>),
}
```

---

## 2. impl HelmSim — Full Method Set

```rust
impl HelmSim {
    // ── Execution ─────────────────────────────────────────────────────────────

    /// Run for up to `budget` instructions. Returns the stop reason.
    ///
    /// This is the primary simulation entry point from Python.
    /// One enum dispatch here; then delegates to the tight inner loop.
    pub fn run(&mut self, budget: u64) -> StopReason {
        match self {
            Self::Virtual(k)   => k.run(budget),
            Self::Interval(k)  => k.run(budget),
            Self::Accurate(k)  => k.run(budget),
        }
    }

    /// Execute exactly one instruction. Equivalent to `run(1)`.
    ///
    /// Used by GDB `step` command and Python-level single-stepping.
    pub fn step_once(&mut self) -> StopReason {
        match self {
            Self::Virtual(k)   => k.step_once(),
            Self::Interval(k)  => k.step_once(),
            Self::Accurate(k)  => k.step_once(),
        }
    }

    // ── Architectural state inspection ────────────────────────────────────────

    /// Return a mutable reference to the hart's `ThreadContext`.
    ///
    /// `ThreadContext` is a cold-path trait for register inspection,
    /// used by GDB stub, Python `sim.read_reg()`, and syscall handler.
    ///
    /// The vtable call overhead of `dyn ThreadContext` is acceptable here —
    /// this is called from Python or GDB, not from the inner loop.
    pub fn thread_context(&mut self) -> &mut dyn ThreadContext {
        match self {
            Self::Virtual(k)   => k as &mut dyn ThreadContext,
            Self::Interval(k)  => k as &mut dyn ThreadContext,
            Self::Accurate(k)  => k as &mut dyn ThreadContext,
        }
    }

    // ── Memory access ─────────────────────────────────────────────────────────

    /// Shared reference to the memory map (for Python introspection, binary loading).
    pub fn memory(&self) -> &MemoryMap {
        match self {
            Self::Virtual(k)   => &k.memory,
            Self::Interval(k)  => &k.memory,
            Self::Accurate(k)  => &k.memory,
        }
    }

    /// Mutable reference to the memory map (for loading binaries, mapping regions).
    ///
    /// Called from `build_simulator()` and from Python config before `sim.run()`.
    pub fn memory_mut(&mut self) -> &mut MemoryMap {
        match self {
            Self::Virtual(k)   => &mut k.memory,
            Self::Interval(k)  => &mut k.memory,
            Self::Accurate(k)  => &mut k.memory,
        }
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    /// Reset the hart to power-on state (PC=0, registers zeroed, memory unchanged).
    ///
    /// Does not reset memory contents — the binary remains loaded.
    /// Resets: ArchState, timing model internal counters, stop_flag, insns_executed.
    pub fn reset(&mut self) {
        match self {
            Self::Virtual(k)   => k.reset(),
            Self::Interval(k)  => k.reset(),
            Self::Accurate(k)  => k.reset(),
        }
    }

    // ── Checkpoint ────────────────────────────────────────────────────────────

    /// Serialize all architectural state to a byte blob.
    ///
    /// The blob includes: ISA tag, ExecMode tag, ArchState, RAM contents.
    /// It does NOT include: performance counters, timing model internal state.
    pub fn checkpoint_save(&self) -> Vec<u8> {
        match self {
            Self::Virtual(k)   => k.checkpoint_save(),
            Self::Interval(k)  => k.checkpoint_save(),
            Self::Accurate(k)  => k.checkpoint_save(),
        }
    }

    /// Restore from a checkpoint blob. Panics on version or ISA mismatch.
    pub fn checkpoint_restore(&mut self, data: &[u8]) {
        match self {
            Self::Virtual(k)   => k.checkpoint_restore(data),
            Self::Interval(k)  => k.checkpoint_restore(data),
            Self::Accurate(k)  => k.checkpoint_restore(data),
        }
    }

    // ── Configuration ─────────────────────────────────────────────────────────

    /// Install a syscall handler. Required for `ExecMode::Syscall`.
    /// Panics if called after `run()` has started.
    pub fn set_syscall_handler(&mut self, h: Box<dyn helm_core::SyscallHandler>) {
        match self {
            Self::Virtual(k)   => k.syscall_handler = Some(h),
            Self::Interval(k)  => k.syscall_handler = Some(h),
            Self::Accurate(k)  => k.syscall_handler = Some(h),
        }
    }

    /// Replace the event bus. Used by the harness to install a shared bus
    /// before the simulation starts.
    pub fn set_event_bus(&mut self, bus: std::sync::Arc<HelmEventBus>) {
        match self {
            Self::Virtual(k)   => k.event_bus = bus,
            Self::Interval(k)  => k.event_bus = bus,
            Self::Accurate(k)  => k.event_bus = bus,
        }
    }

    // ── Metadata ──────────────────────────────────────────────────────────────

    /// Return the ISA this simulator was built for.
    pub fn isa(&self) -> helm_core::Isa {
        match self {
            Self::Virtual(k)   => k.isa,
            Self::Interval(k)  => k.isa,
            Self::Accurate(k)  => k.isa,
        }
    }

    /// Return the execution mode this simulator was built for.
    pub fn exec_mode(&self) -> helm_core::ExecMode {
        match self {
            Self::Virtual(k)   => k.mode,
            Self::Interval(k)  => k.mode,
            Self::Accurate(k)  => k.mode,
        }
    }

    /// Total instructions retired since construction (or last reset).
    pub fn insns_executed(&self) -> u64 {
        match self {
            Self::Virtual(k)   => k.insns_executed,
            Self::Interval(k)  => k.insns_executed,
            Self::Accurate(k)  => k.insns_executed,
        }
    }
}
```

---

## 3. build_simulator Factory

`build_simulator()` is the sole Rust-level creation path. It is called from the PyO3 `#[pyfunction]` wrapper in `helm-python`.

```rust
// crates/helm-engine/src/factory.rs

use helm_core::{Isa, ExecMode};
use helm_timing::{Virtual, Interval, Accurate};

use crate::engine::HelmEngine;
use crate::sim::HelmSim;

/// Timing model selection at factory time. Maps to the three `HelmSim` variants.
///
/// This is a Rust enum, not the PyO3-facing representation.
/// The PyO3 wrapper converts Python string/enum values to `TimingChoice`.
#[derive(Debug, Clone, Copy)]
pub enum TimingChoice {
    Virtual,
    Interval { interval_ns: u64 },
    Accurate,
}

/// Factory — the only way to create a `HelmSim` from Python.
///
/// Matches on `timing` to produce the correct monomorphized variant.
/// Configuration finalization (loading binaries, setting up syscall handler,
/// registering memory regions) happens after this call, before `sim.run()`.
///
/// # Panics
///
/// Does not panic. All inputs are validated before reaching this function
/// (done by the PyO3 `FromPyObject` conversion in `helm-python`).
pub fn build_simulator(isa: Isa, mode: ExecMode, timing: TimingChoice) -> HelmSim {
    match timing {
        TimingChoice::Virtual => {
            HelmSim::Virtual(HelmEngine::new(isa, mode, Virtual))
        }

        TimingChoice::Interval { interval_ns } => {
            HelmSim::Interval(HelmEngine::new(isa, mode, Interval { interval_ns }))
        }

        TimingChoice::Accurate => {
            HelmSim::Accurate(HelmEngine::new(isa, mode, Accurate))
        }
    }
}
```

### What Happens After build_simulator()

The factory creates a bare `HelmSim`. The caller is responsible for post-construction configuration before calling `sim.run()`:

```rust
// Typical construction sequence (shown in Rust; Python does this via PyO3):

let mut sim = build_simulator(Isa::RiscV, ExecMode::Syscall, TimingChoice::Virtual);

// 1. Map RAM
sim.memory_mut().add_ram(0x8000_0000, 128 * 1024 * 1024);  // 128 MiB at 0x80000000

// 2. Load ELF binary into RAM
elf_loader::load(&mut sim.memory_mut(), "hello_world");
sim.thread_context().write_pc(elf_loader::entry_point("hello_world"));

// 3. Install syscall handler (required for Syscall mode)
sim.set_syscall_handler(Box::new(LinuxSyscallHandler::new(Isa::RiscV)));

// 4. Optionally install event bus (for tracing, GDB)
let bus = Arc::new(HelmEventBus::new());
sim.set_event_bus(Arc::clone(&bus));

// 5. Run
let reason = sim.run(1_000_000_000);
```

---

## 4. PyO3 #[pyclass] Wrapper Design

The `#[pyclass]` wrapper lives in `helm-python`, not in `helm-engine`. `helm-engine` is a pure Rust crate with no PyO3 dependency. This separation:

- Keeps `helm-engine` compilable without PyO3 (important for test speed).
- Keeps the PyO3 binding surface isolated to one crate.
- Allows the Python API to evolve independently of the kernel.

```rust
// crates/helm-python/src/sim_wrapper.rs

use pyo3::prelude::*;
use helm_engine::{HelmSim, StopReason, TimingChoice};
use helm_core::{Isa, ExecMode};

/// Python-facing wrapper for HelmSim.
///
/// Owned by a Python `helm_ng.Simulation` object.
/// All method calls on this object dispatch through HelmSim's enum match,
/// which is the only overhead per Python call.
#[pyclass(name = "HelmSim")]
pub struct PyHelmSim {
    inner: HelmSim,
}

#[pymethods]
impl PyHelmSim {
    /// Run for `budget` instructions, releasing the GIL.
    ///
    /// GIL is released via `py.allow_threads()` so Python can run other threads
    /// concurrently with the simulation. The simulation does not call back into
    /// Python during `run()` unless an event bus subscriber is a Python callable
    /// (which re-acquires the GIL inside the subscriber).
    pub fn run(&mut self, py: Python<'_>, budget: u64) -> PyResult<PyStopReason> {
        let reason = py.allow_threads(|| self.inner.run(budget));
        Ok(PyStopReason::from(reason))
    }

    /// Single-step: execute one instruction.
    pub fn step_once(&mut self) -> PyResult<PyStopReason> {
        Ok(PyStopReason::from(self.inner.step_once()))
    }

    /// Read an integer register by index.
    /// For RISC-V: 0=x0 (zero), 1=ra, 2=sp, ..., 31=t6.
    pub fn read_reg(&mut self, idx: u32) -> u64 {
        self.inner.thread_context().read_int_reg(idx)
    }

    /// Write an integer register by index.
    pub fn write_reg(&mut self, idx: u32, val: u64) {
        self.inner.thread_context().write_int_reg(idx, val);
    }

    /// Read the program counter.
    pub fn read_pc(&mut self) -> u64 {
        self.inner.thread_context().read_pc()
    }

    /// Write the program counter.
    pub fn write_pc(&mut self, pc: u64) {
        self.inner.thread_context().write_pc(pc);
    }

    /// Read a CSR by 12-bit index.
    pub fn read_csr(&mut self, csr: u16) -> u64 {
        self.inner.thread_context().read_csr(csr)
    }

    /// Read `size` bytes from guest physical memory at `addr`.
    pub fn read_mem(&self, addr: u64, size: usize) -> PyResult<Vec<u8>> {
        self.inner.memory()
            .read_bytes_functional(addr, size)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    }

    /// Write bytes to guest physical memory at `addr`.
    pub fn write_mem(&mut self, addr: u64, data: &[u8]) -> PyResult<()> {
        self.inner.memory_mut()
            .write_bytes_functional(addr, data)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
    }

    /// Save a checkpoint. Returns a Python bytes object.
    pub fn checkpoint_save(&self) -> Vec<u8> {
        self.inner.checkpoint_save()
    }

    /// Restore from a checkpoint bytes object.
    pub fn checkpoint_restore(&mut self, data: &[u8]) {
        self.inner.checkpoint_restore(data);
    }

    /// Reset to power-on state (PC=0, registers=0, memory unchanged).
    pub fn reset(&mut self) {
        self.inner.reset();
    }

    /// Total instructions retired since construction or last reset.
    pub fn insns_executed(&self) -> u64 {
        self.inner.insns_executed()
    }
}

/// Python-facing `build_simulator` function.
///
/// Takes Python-typed arguments, converts to Rust types, calls the factory.
#[pyfunction]
pub fn build_simulator(
    isa: &str,
    mode: &str,
    timing: &str,
    interval_ns: Option<u64>,
) -> PyResult<PyHelmSim> {
    let isa = match isa {
        "riscv"   => Isa::RiscV,
        "aarch64" => Isa::AArch64,
        "aarch32" => Isa::AArch32,
        _         => return Err(pyo3::exceptions::PyValueError::new_err(
            format!("Unknown ISA: {isa}. Valid: riscv, aarch64, aarch32")
        )),
    };

    let mode = match mode {
        "functional" => ExecMode::Functional,
        "syscall"    => ExecMode::Syscall,
        "system"     => ExecMode::System,
        _            => return Err(pyo3::exceptions::PyValueError::new_err(
            format!("Unknown ExecMode: {mode}. Valid: functional, syscall, system")
        )),
    };

    let timing = match timing {
        "virtual"   => TimingChoice::Virtual,
        "interval"  => TimingChoice::Interval {
            interval_ns: interval_ns.unwrap_or(10_000)  // default: 10K ns per interval
        },
        "accurate"  => TimingChoice::Accurate,
        _           => return Err(pyo3::exceptions::PyValueError::new_err(
            format!("Unknown timing: {timing}. Valid: virtual, interval, accurate")
        )),
    };

    let inner = helm_engine::build_simulator(isa, mode, timing);
    Ok(PyHelmSim { inner })
}
```

---

## 5. Python API: sim.read_reg(0) Resolution Chain

The call chain from Python to hardware register is:

```
Python: sim.read_reg(0)
  │
  │  PyO3 dispatch — calls PyHelmSim::read_reg(0)
  ▼
PyHelmSim::read_reg(idx: u32) -> u64
  │
  │  self.inner.thread_context()
  │    ↓
  │  HelmSim::thread_context() — enum match (one branch)
  │    HelmSim::Virtual(k)  → k as &mut dyn ThreadContext
  │    HelmSim::Interval(k) → k as &mut dyn ThreadContext
  │    HelmSim::Accurate(k) → k as &mut dyn ThreadContext
  ▼
  &mut dyn ThreadContext  (vtable dispatch — cold path, acceptable)
  │
  │  .read_int_reg(0)
  ▼
HelmEngine<T>::read_int_reg(idx: u32) -> u64
  │
  │  self.arch.read_int(idx)
  ▼
ArchState::read_int(idx: u32) -> u64
  │
  │  self.int_regs[idx as usize]  (array indexing, no allocation)
  ▼
u64 value
```

**Total cost per `read_reg()` call from Python:**
1. PyO3 argument conversion: O(1), ~5 ns
2. `HelmSim::thread_context()` enum match: 1 branch, ~1 ns
3. `dyn ThreadContext` vtable call: 1 indirect jump, ~3 ns
4. `ArchState::read_int()` array index: ~1 ns

Total: ~10 ns per call. This is acceptable for a cold-path debugger/inspection API.

---

## 6. Variant Exhaustiveness Guarantee

Every method on `HelmSim` uses an exhaustive `match self { ... }` with all three variants. The Rust compiler enforces this at compile time.

When a new timing model is added (e.g., `RealTime` for wall-clock-synchronized simulation):

1. Add `RealTime(HelmEngine<RealTime>)` to the `HelmSim` enum.
2. The compiler immediately produces errors at every unhandled `match self` site.
3. Each error points to a specific method that needs a new arm.
4. The developer adds `Self::RealTime(k) => k.method()` to each site.

This compile-time enforcement is the reason `HelmSim` is an enum rather than a `Box<dyn HelmKernel>` trait object. A trait object would allow forgetting to implement a method for the new variant silently.

---

## 7. Extending HelmSim: Adding a New Timing Model

Step-by-step procedure:

```rust
// 1. Define the new timing model in helm-timing:
pub struct RealTime { pub scale_factor: f64 }
impl TimingModel for RealTime {
    #[inline(always)]
    fn on_memory_access(&mut self, addr: u64, is_write: bool, size: usize) { /* ... */ }
    #[inline(always)]
    fn on_branch_mispredict(&mut self, penalty_cycles: u64) { /* ... */ }
}

// 2. Add the variant to HelmSim in helm-engine:
pub enum HelmSim {
    Virtual(HelmEngine<Virtual>),
    Interval(HelmEngine<Interval>),
    Accurate(HelmEngine<Accurate>),
    RealTime(HelmEngine<RealTime>),   // ← new
}

// 3. Add the variant to TimingChoice:
pub enum TimingChoice {
    Virtual,
    Interval { interval_ns: u64 },
    Accurate,
    RealTime { scale_factor: f64 },  // ← new
}

// 4. Add the match arm to build_simulator():
TimingChoice::RealTime { scale_factor } => {
    HelmSim::RealTime(HelmEngine::new(isa, mode, RealTime { scale_factor }))
}

// 5. Fix every compile error (exhaustiveness failures in HelmSim impl).
// 6. Add the PyO3 binding arm in helm-python.
```

The compile errors in step 5 are exhaustive — the compiler lists every match site that needs updating. No silent regressions.

---

*See [`HLD.md`](HLD.md) for crate-level design context.*
*See [`LLD-helm-engine.md`](LLD-helm-engine.md) for the HelmEngine struct internals.*
*See [`LLD-scheduler.md`](LLD-scheduler.md) for multi-hart scheduling.*
*See [`TEST.md`](TEST.md) for HelmSim dispatch tests.*
