# LLD: Scheduler

> Low-Level Design for the `Scheduler` — temporal decoupling and multi-hart coordination.

**Crate:** `helm-engine`
**File:** `crates/helm-engine/src/scheduler.rs`

---

## Table of Contents

1. [Struct Definition and Ownership](#1-struct-definition-and-ownership)
2. [Execute Trait](#2-execute-trait)
3. [Temporal Decoupling: Quantum Loop](#3-temporal-decoupling-quantum-loop)
4. [Hypersimulation: can_skip_to()](#4-hypersimulation-can_skip_to)
5. [Breakpoint Interruption](#5-breakpoint-interruption)
6. [Multi-Hart Synchronization at Quantum Boundary](#6-multi-hart-synchronization-at-quantum-boundary)
7. [Single-Hart Fast Path](#7-single-hart-fast-path)
8. [Scheduler Ownership and Lifecycle](#8-scheduler-ownership-and-lifecycle)
9. [Integration with EventQueue](#9-integration-with-eventqueue)
10. [Invariants and Error Conditions](#10-invariants-and-error-conditions)

---

## 1. Struct Definition and Ownership

```rust
// crates/helm-engine/src/scheduler.rs

use std::sync::Arc;
use helm_event::{EventQueue, Tick};
use helm_devices::bus::event_bus::HelmEventBus;

use crate::sim::HelmSim;
use crate::StopReason;

/// Multi-hart temporal decoupling scheduler.
///
/// # Ownership
///
/// The `Scheduler` is owned by its caller — either `World` (in a full system
/// configuration) or the test harness (in unit tests). It is NOT owned by any
/// individual `HelmSim`/`HelmEngine`.
///
/// In single-hart configurations (Phase 0), the `Scheduler` is not used at all.
/// `HelmSim::run()` is called directly.
///
/// # Temporal Decoupling
///
/// Each hart runs its quantum independently before the scheduler synchronizes.
/// Harts' local `current_tick` values may diverge by up to `quantum_size` ticks
/// during a quantum. At the quantum boundary, the scheduler:
///   1. Determines the minimum local tick across all harts.
///   2. Advances the global clock to that minimum.
///   3. Processes all EventQueue events scheduled before the new global time.
///   4. Delivers any pending IRQs to harts.
///   5. Starts the next quantum.
pub struct Scheduler {
    /// The harts being scheduled. Round-robin order.
    /// Phase 0: always length 1. Phase 3: N harts.
    harts: Vec<HelmSim>,

    /// Instruction budget per hart per quantum.
    /// Default: 10,000 (Q17). Configurable per Scheduler at construction
    /// via `World::set_quantum(n)`. Per-hart override not supported in Phase 0.
    quantum_size: u64,

    /// Global simulation tick. Advances at each quantum boundary.
    /// Monotonically non-decreasing.
    global_tick: u64,

    /// Shared discrete event queue. Owned by Scheduler.
    /// In `Virtual` timing mode, this queue drives device timers, DMA
    /// completion events, and interrupt delivery.
    event_queue: EventQueue,

    /// Shared event bus. Same instance as held by each HelmSim.
    /// Used by the scheduler to broadcast synchronization events.
    event_bus: Arc<HelmEventBus>,

    /// Breakpoint/pause flag. Set when any hart fires a stop event.
    /// Causes the scheduler to pause all harts after the current quantum.
    pause_requested: bool,
}

impl Scheduler {
    /// Construct a new scheduler with one or more harts.
    ///
    /// `harts`: must be non-empty. Single-element vec = single hart (still valid).
    /// `quantum_size`: instruction budget per quantum per hart. Default: 10,000 (Q17).
    pub fn new(harts: Vec<HelmSim>, quantum_size: u64, event_bus: Arc<HelmEventBus>) -> Self {
        assert!(!harts.is_empty(), "Scheduler requires at least one hart");

        // Register the pause subscriber on the event bus.
        // When any hart fires HelmEvent::Breakpoint, the scheduler pauses.
        // This is handled via the stop_flag mechanism inside HelmEngine —
        // the scheduler additionally notes that a pause was requested.

        Self {
            harts,
            quantum_size,
            global_tick: 0,
            event_queue: EventQueue::new(),
            event_bus,
            pause_requested: false,
        }
    }

    /// Add a hart to the scheduler. Must be called before the first `run()`.
    pub fn add_hart(&mut self, hart: HelmSim) {
        self.harts.push(hart);
    }

    /// Current global simulation tick.
    pub fn global_tick(&self) -> u64 {
        self.global_tick
    }

    /// Set the quantum size. Can be changed between `run()` calls.
    pub fn set_quantum_size(&mut self, q: u64) {
        assert!(q > 0, "Quantum size must be positive");
        self.quantum_size = q;
    }

    /// Borrow a hart by index for inspection (e.g., from Python).
    pub fn hart(&self, idx: usize) -> &HelmSim {
        &self.harts[idx]
    }

    /// Mutably borrow a hart by index (e.g., to load a binary).
    pub fn hart_mut(&mut self, idx: usize) -> &mut HelmSim {
        &mut self.harts[idx]
    }
}
```

---

## 2. Execute Trait

The `Scheduler` implements the same `Execute` trait as `HelmEngine<T>`. This allows a harness to treat a single hart and a multi-hart scheduler uniformly:

```rust
impl Execute for Scheduler {
    /// Run until `budget` total instructions have been retired across all harts,
    /// or until a stop condition is reached.
    ///
    /// `budget` is a global instruction count ceiling. Each quantum consumes
    /// up to `quantum_size * num_harts` instructions from the budget.
    fn run(&mut self, budget: u64) -> StopReason {
        let mut total_retired = 0u64;

        loop {
            // Check pause flag (set by breakpoint subscriber).
            if self.pause_requested {
                self.pause_requested = false;
                return StopReason::Breakpoint { pc: self.harts[0].thread_context().read_pc() };
            }

            // Check total budget.
            if total_retired >= budget {
                return StopReason::QuantumExhausted;
            }

            // Remaining budget for this quantum round.
            let remaining = budget - total_retired;
            let per_hart_budget = self.quantum_size.min(remaining);

            // Run one quantum per hart (round-robin).
            let reason = self.run_one_quantum(per_hart_budget);
            total_retired += per_hart_budget * self.harts.len() as u64;

            // Propagate terminal stop reasons immediately.
            match reason {
                StopReason::QuantumExhausted => {
                    // Normal: synchronize and continue.
                    self.synchronize();
                }
                StopReason::Breakpoint { .. } => return reason,
                StopReason::SimExit { .. }    => return reason,
                StopReason::Exception { .. }  => return reason,
            }
        }
    }

    /// Single-step across all harts: each hart executes one instruction.
    fn step_once(&mut self) -> StopReason {
        self.run(self.harts.len() as u64)
    }
}
```

---

## 3. Temporal Decoupling: Quantum Loop

```rust
impl Scheduler {
    /// Execute one full quantum round — each hart runs for `per_hart_budget` instructions.
    ///
    /// Harts run sequentially (not in parallel — single host thread in Phase 0).
    /// Multi-threaded hart execution is a Phase 3+ concern; the Scheduler API
    /// is designed to accommodate it without interface changes.
    fn run_one_quantum(&mut self, per_hart_budget: u64) -> StopReason {
        for hart in self.harts.iter_mut() {
            let reason = hart.run(per_hart_budget);

            match reason {
                StopReason::QuantumExhausted => {
                    // Normal completion — continue to next hart.
                    continue;
                }
                StopReason::Breakpoint { pc } => {
                    // A breakpoint fired during this hart's quantum.
                    // Pause all remaining harts (do not run them this quantum).
                    // Return to caller for handling.
                    self.pause_requested = false;
                    return StopReason::Breakpoint { pc };
                }
                StopReason::SimExit { code } => {
                    return StopReason::SimExit { code };
                }
                StopReason::Exception { vector, pc } => {
                    // Unhandled architectural exception in Functional mode.
                    return StopReason::Exception { vector, pc };
                }
            }
        }

        StopReason::QuantumExhausted
    }

    /// Synchronize all harts at quantum boundary.
    ///
    /// Responsibilities:
    ///   1. Advance global_tick to min(hart.current_tick) across all harts.
    ///   2. Drain EventQueue events scheduled before global_tick.
    ///   3. Deliver pending IRQs from drained events to harts.
    ///
    /// In Phase 0 (single-hart SE, no devices, no event queue events),
    /// this reduces to just advancing global_tick. Near-zero cost.
    fn synchronize(&mut self) {
        // 1. Compute minimum local tick.
        let min_tick = self.harts.iter()
            .map(|h| h.insns_executed())  // proxy for local tick in Virtual mode
            .min()
            .unwrap_or(0);

        // 2. Advance global clock.
        self.global_tick = min_tick;

        // 3. Drain event queue up to global_tick.
        // `drain_until(tick)` fires all callbacks scheduled at tick <= global_tick.
        // Callbacks may: deliver IRQs to harts, schedule future events, update device state.
        self.event_queue.drain_until(Tick(self.global_tick), |event| {
            // Execute the event callback.
            // The callback receives `&mut self.harts` indirectly via the EventQueue
            // callback closure captured at `schedule()` time.
            (event.callback)();
        });
    }
}
```

### Temporal Decoupling Accuracy Contract

In SE mode (Phase 0-2), harts do not communicate via shared memory (each hart has an independent address space). The temporal decoupling approximation is exact: no causal dependency between harts exists within a quantum.

In FS mode (Phase 3), harts share physical memory. A write by hart A in quantum N is not visible to hart B until the synchronization point at the end of quantum N. This is the *temporal decoupling approximation*: the maximum visibility latency is one quantum (1000 instructions by default = ~1 µs at 1 GHz). For most workloads, this is within the timing model's overall accuracy budget.

---

## 4. Hypersimulation: can_skip_to()

Hypersimulation allows the scheduler to advance the simulation clock without executing instructions when no hart is doing useful work (e.g., all harts are blocked waiting for a timer event).

```rust
impl Scheduler {
    /// Check if the simulation can skip ahead to `target_tick` without executing instructions.
    ///
    /// Returns `true` if all harts are blocked (e.g., waiting in a WFI/WFE instruction)
    /// AND the next scheduled event is at `target_tick`.
    ///
    /// If true, the caller may call `skip_to(target_tick)` to advance the clock.
    pub fn can_skip_to(&self, target_tick: u64) -> bool {
        // All harts must be idle (halted at WFI or similar).
        let all_idle = self.harts.iter().all(|h| h.is_halted());

        // The next event in the queue must be at or before target_tick.
        let next_event = self.event_queue.peek_tick();
        let event_ready = next_event.map_or(false, |t| t.0 <= target_tick);

        all_idle && event_ready
    }

    /// Skip the simulation clock to `target_tick`.
    ///
    /// Precondition: `can_skip_to(target_tick)` returned `true`.
    /// Advances global_tick, drains events up to target_tick, wakes harts if IRQ delivered.
    ///
    /// This allows the simulation to skip over idle periods at zero CPU cost —
    /// equivalent to clock gating in real hardware.
    pub fn skip_to(&mut self, target_tick: u64) {
        debug_assert!(
            self.can_skip_to(target_tick),
            "skip_to() called without checking can_skip_to()"
        );

        self.global_tick = target_tick;

        // Drain all events up to target_tick. This may wake sleeping harts.
        self.event_queue.drain_until(Tick(target_tick), |event| {
            (event.callback)();
        });
    }

    /// Find the next event tick for use in hypersimulation.
    /// Returns `None` if the event queue is empty.
    pub fn next_event_tick(&self) -> Option<u64> {
        self.event_queue.peek_tick().map(|t| t.0)
    }
}
```

### Hypersimulation Usage Pattern

```rust
// Typical harness using hypersimulation:
loop {
    let reason = scheduler.run(1_000_000);  // run 1M instructions

    if reason == StopReason::QuantumExhausted {
        // Check if we can skip to next event (all harts idle).
        if let Some(next_tick) = scheduler.next_event_tick() {
            if scheduler.can_skip_to(next_tick) {
                scheduler.skip_to(next_tick);
                // Clock advanced; events fired; harts may now be awake.
                continue;
            }
        }
    }

    break;  // StopReason::SimExit, Breakpoint, etc.
}
```

---

## 5. Breakpoint Interruption

A breakpoint can be set from Python or GDB. The mechanism flows through the event bus without requiring the scheduler to poll:

### Setting a Breakpoint

```rust
// GDB stub or Python calls:
sim.event_bus.fire(HelmEvent::Breakpoint { pc: 0x8000_1000 });

// Inside HelmEventBus::fire():
// → The stop_flag subscriber in HelmEngine::new() fires
// → stop_flag.store(true, Ordering::Relaxed)
// → HelmEngine::run() exits at next instruction loop top
// → Returns StopReason::Breakpoint { pc }
// → Scheduler::run_one_quantum() sees Breakpoint and returns immediately
// → Scheduler::run() propagates StopReason::Breakpoint to the Python caller
```

### Hardware Breakpoints (Address-Based)

The GDB stub implements hardware breakpoints by registering a `MemWrite` or `InsnFetch` subscriber that fires when the target address is accessed:

```rust
// GDB stub installs a breakpoint at 0x8000_1000:
let bus = Arc::clone(&sim.event_bus);
bus.subscribe(HelmEventKind::InsnFetch, move |event| {
    if let HelmEvent::InsnFetch { pc, .. } = event {
        if *pc == 0x8000_1000 {
            bus.fire(HelmEvent::Breakpoint { pc: *pc });
        }
    }
});
```

This fires the stop_flag chain described above. The `InsnFetch` event is only fired if a subscriber is registered (the opt-in check described in `LLD-helm-engine.md` section 7).

### Pause from Python

```python
# Python: pause the simulation asynchronously
# (e.g., user presses Ctrl+C in a Jupyter notebook)
import threading
def stop_after_1s():
    time.sleep(1.0)
    sim.thread_context().pause()  # sets stop_flag via ThreadContext::pause()

threading.Thread(target=stop_after_1s, daemon=True).start()
sim.run(1_000_000_000_000)  # will return StopReason::Breakpoint after ~1s
```

---

## 6. Multi-Hart Synchronization at Quantum Boundary

The synchronization protocol at the quantum boundary handles four concerns:

### 1. Clock Advancement

```
Before synchronize():
  hart0.current_tick = 1050  (ran 50 insns into next quantum before breakpoint)
  hart1.current_tick = 1000  (completed exactly one quantum)
  hart2.current_tick = 1000
  hart3.current_tick = 1000

After synchronize():
  global_tick = min(1050, 1000, 1000, 1000) = 1000
```

The global clock advances to the minimum hart tick. Harts that ran further ahead are within the temporal decoupling window — their excess instructions are committed but their timing effects are attributed to the next synchronization.

### 2. Shared Memory Coherence (Phase 3 Only)

In Phase 3 FS mode, harts share a `MemoryMap`. Write ordering is enforced at the quantum boundary:

```rust
fn synchronize(&mut self) {
    // In Phase 3: flush each hart's store buffer into the shared MemoryMap.
    // Store buffers are per-hart queues of pending writes accumulated during the quantum.
    // Phase 0-2: store buffers don't exist; skipped.
    #[cfg(feature = "fs-mode")]
    for hart in self.harts.iter_mut() {
        hart.flush_store_buffer();
    }

    // ... advance global_tick, drain events ...
}
```

### 3. IRQ Delivery

When an event queue callback fires an interrupt (e.g., a timer fires and raises IRQ line 5), the interrupt is delivered to the target hart by setting a pending IRQ flag:

```rust
// In the event callback (captured at schedule time):
let hart_ref = Arc::clone(&hart0_pending_irqs);
event_queue.schedule(Tick(10_000), move || {
    hart_ref.set_irq(5, true);  // Timer IRQ 5 now pending on hart0
});

// In synchronize(), after drain_until():
// hart0 sees pending_irq[5] = true at the start of its next quantum.
// step_riscv() checks pending_irq at quantum start and enters the trap vector.
```

### 4. Quantum Boundary Order

```
Quantum boundary sequence:
  1. All harts complete their quantum (or stop early on breakpoint).
  2. synchronize():
     a. Compute min_tick.
     b. Advance global_tick.
     c. Flush store buffers (Phase 3 only).
     d. drain_until(global_tick) — process device timer events, DMA completions.
     e. Deliver pending IRQs to target harts.
  3. Start next quantum.
```

---

## 7. Single-Hart Fast Path

When only one hart is registered, the `Scheduler` is not used. `HelmSim::run()` is called directly. This eliminates the quantum loop overhead:

```rust
// In the harness (or Python via PyO3):
if harts.len() == 1 {
    // Fast path: no Scheduler, no synchronization overhead.
    let reason = sim.run(budget);
    return reason;
} else {
    // Multi-hart: go through Scheduler.
    let mut sched = Scheduler::new(harts, quantum_size, event_bus);
    let reason = sched.run(budget);
    return reason;
}
```

The single-hart fast path is the primary path for Phase 0 (MVP). It avoids:
- The quantum loop iteration
- The `synchronize()` call (min_tick computation, event drain)
- The `Vec<HelmSim>` iteration overhead

---

## 8. Scheduler Ownership and Lifecycle

The `Scheduler` is created and owned by the **caller** — either the simulation harness, `World`, or the test. It is not owned by `HelmSim` or `HelmEngine`.

```
Ownership chain (Phase 3 full system):

World
  └── Scheduler (owned by World)
        ├── HelmSim::Virtual(HelmEngine<Virtual>)  ← hart0
        ├── HelmSim::Virtual(HelmEngine<Virtual>)  ← hart1
        ├── HelmSim::Virtual(HelmEngine<Virtual>)  ← hart2
        └── HelmSim::Virtual(HelmEngine<Virtual>)  ← hart3
```

```
Ownership chain (Phase 0 single-hart SE):

Test harness / Python
  └── HelmSim::Virtual(HelmEngine<Virtual>)  (no Scheduler)
```

The `Scheduler` does not implement `SimObject`. It is not in the component tree.

### Lifecycle

```
1. Construct: Scheduler::new(harts, quantum_size, event_bus)
2. Configure: add_hart(), set_quantum_size(), schedule events in event_queue
3. Run: scheduler.run(budget) — may be called multiple times
4. Inspect: scheduler.hart(idx).thread_context().read_pc()
5. Drop: Scheduler is dropped, which drops all HelmSim instances
```

---

## 9. Integration with EventQueue

The `Scheduler` owns the `EventQueue` from `helm-event`. Components and devices schedule future events via:

```rust
scheduler.event_queue.schedule(Tick(target_tick), callback);
```

During `synchronize()`, `event_queue.drain_until(global_tick)` fires all callbacks with `tick <= global_tick`. Callbacks are `Box<dyn FnMut()>` closures that capture device references.

### Event Queue and Temporal Decoupling

The event queue operates on the **global tick**, not individual hart ticks. Events scheduled at tick T fire at the first synchronization point where `global_tick >= T`. This means an event can be delayed by at most one quantum (1000 instructions * ~1 ns/instruction = ~1 µs).

For `Virtual` timing mode, this is within the timing model's accuracy budget. Timer events are typically scheduled hundreds of microseconds in the future; a 1 µs quantization error is negligible.

---

## 10. Invariants and Error Conditions

### Invariants

| Invariant | Enforcement |
|---|---|
| `global_tick` is monotonically non-decreasing | `synchronize()` only advances, never retracts |
| Each quantum, every hart runs the same `per_hart_budget` | `run_one_quantum()` uses the same arg for all harts |
| No hart's `current_tick` exceeds `global_tick + quantum_size` | Quantum budget ceiling enforces this |
| Event queue callbacks are fired in tick order | `EventQueue::drain_until()` drains in ascending tick order |
| The breakpoint stop_flag is cleared at the start of every `run()` call | `HelmEngine::run()` clears `stop_flag` on entry |

### Error Conditions

| Condition | Behavior |
|---|---|
| `Scheduler::new(vec![], ...)` | `panic!` in debug and release — empty hart set is invalid |
| `skip_to()` without `can_skip_to()` returning true | `debug_assert!` panic in debug builds; undefined in release |
| Hart returns `StopReason::Exception` in Functional mode | Propagated immediately; remaining harts not run this quantum |
| Two harts both fire `Breakpoint` in the same quantum | First one encountered (round-robin order) takes priority |

---

## Design Decisions from Q&A

### Design Decision: World owns Scheduler (Q14)

`World` owns the `Scheduler`. Configuration (quantum size, hart registration) happens before `world.run()` is called; the scheduler is not exposed directly after construction. `World::add_hart(hart: Box<dyn Hart>)` registers a hart with the internal scheduler. `World::set_quantum(n: u64)` sets the quantum before running. `World::run(until: Option<u64>)` drives the scheduler loop. The `Scheduler` type is not `pub` outside `helm-engine`. Rationale: the World-owns-Scheduler model is universal in production simulators — it is the only design where checkpoint, reset, and synchronization are guaranteed to be consistent.

### Design Decision: Default quantum is 10,000 instructions (Q17)

Default quantum is **10,000** instructions globally, configurable via `World::set_quantum(n: u64)`. Per-hart override is not supported in Phase 0 but the scheduler API reserves it. Rationale: 10,000 instructions is a reasonable default — large enough to amortize synchronization overhead, small enough to keep debuggability acceptable (a breakpoint fires within 10K instructions of its target in the worst case). SIMICS's 500K–1M defaults are tuned for throughput benchmarks on server silicon; helm-ng's primary early use case is correctness testing where 10K is more appropriate.

---

*See [`HLD.md`](HLD.md) for system-level design context.*
*See [`LLD-helm-engine.md`](LLD-helm-engine.md) for the HelmEngine inner loop.*
*See [`LLD-helm-sim.md`](LLD-helm-sim.md) for the HelmSim enum.*
*See [`TEST.md`](TEST.md) for quantum exhaustion and breakpoint tests.*
