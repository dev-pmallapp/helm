# helm-debug — LLD: TraceLogger

> **Module:** `helm-debug::trace`
> **Output:** JSON Lines (`.jsonl`), one `TraceEvent` per line
> **Ring buffer:** Lock-free, `AtomicUsize` head/tail, configurable capacity (default 65 536)

---

## Table of Contents

1. [TraceEvent Enum](#1-traceevent-enum)
2. [Ring Buffer](#2-ring-buffer)
3. [TraceLogger API](#3-tracelogger-api)
4. [HelmEventBus Integration](#4-helmeventbus-integration)
5. [JSON Lines Output Format](#5-json-lines-output-format)
6. [Python Callback Integration](#6-python-callback-integration)
7. [Implementation Notes](#7-implementation-notes)

---

## 1. TraceEvent Enum

All eight variants are `serde::Serialize` so they can be written directly to the JSONL output.

```rust
use serde::{Deserialize, Serialize};

/// A single observable simulation event captured by the TraceLogger.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraceEvent {
    /// An instruction was fetched at `pc`.
    InsnFetch {
        /// Simulated cycle at which the fetch occurred.
        cycle: u64,
        /// Hart ID (0-indexed).
        hart: u32,
        /// Program counter of the fetched instruction.
        pc: u64,
        /// Raw encoding of the fetched instruction (4 bytes for RV32/64, 2 for compressed).
        bytes: u32,
    },

    /// A memory read was performed.
    MemRead {
        cycle: u64,
        hart: u32,
        /// Guest virtual address.
        addr: u64,
        /// Width of the access in bytes (1, 2, 4, or 8).
        size: u8,
        /// Value that was read.
        value: u64,
    },

    /// A memory write was performed.
    MemWrite {
        cycle: u64,
        hart: u32,
        addr: u64,
        size: u8,
        /// Value that was written.
        value: u64,
    },

    /// An exception or interrupt was taken.
    Exception {
        cycle: u64,
        hart: u32,
        /// Exception/interrupt vector number (RISC-V `mcause`, AArch64 ESR class).
        vector: u32,
        /// PC of the faulting instruction.
        pc: u64,
        /// Trap value (RISC-V `mtval`; address for load/store faults).
        tval: u64,
    },

    /// A syscall instruction was executed (SE mode only).
    Syscall {
        cycle: u64,
        hart: u32,
        /// System call number (from `a7` on RISC-V; `x8` on AArch64).
        nr: u64,
        /// Up to six syscall arguments.
        args: [u64; 6],
        /// Return value (recorded post-dispatch; 0 for entry-only capture).
        ret: i64,
    },

    /// A branch was mispredicted.
    BranchMiss {
        cycle: u64,
        hart: u32,
        /// PC of the branch instruction.
        pc: u64,
        /// Actual branch target.
        target: u64,
        /// Pipeline flush penalty in cycles.
        penalty: u32,
    },

    /// A device (MMIO) raised or lowered an interrupt signal.
    DeviceSignal {
        cycle: u64,
        /// Dot-path name of the device (e.g. `"system.uart0"`).
        device: String,
        /// Signal name (e.g. `"irq"`, `"dma_done"`).
        name: String,
        /// Signal level: 1 = asserted, 0 = deasserted.
        level: u8,
    },

    /// A user-defined custom event (e.g. from a Python HAP handler or magic instruction).
    Custom {
        cycle: u64,
        /// Arbitrary event name (namespaced by convention: `"mymodule.my_event"`).
        name: String,
        /// Arbitrary payload serializable as JSON.
        data: serde_json::Value,
    },
}
```

---

## 2. Ring Buffer

The ring buffer stores `TraceEvent` objects in a fixed-capacity heap allocation. It is designed so that `log()` — called on the hot simulation thread — never blocks and never allocates.

### Data Layout

```rust
pub struct RingBuffer {
    /// Fixed-size storage. Size is always a power of two for cheap modulo.
    slots: Box<[Option<TraceEvent>]>,
    /// Monotonically increasing write cursor. Index = head % capacity.
    head: AtomicUsize,
    /// Monotonically increasing read cursor. Index = tail % capacity.
    tail: AtomicUsize,
    capacity: usize,  // must be a power of two
}
```

### Capacity Invariant

`capacity` is always rounded up to the next power of two at construction time. Modulo is replaced with a bitmask: `idx & (capacity - 1)`.

### `log()` — Lock-Free Insert

```rust
impl RingBuffer {
    /// Insert an event. If the buffer is full, the oldest event is overwritten.
    /// This method is wait-free on the writer side.
    pub fn log(&self, event: TraceEvent) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) & (self.capacity - 1);
        // Safety: only one writer per hart; readers only advance tail.
        // In a multi-hart scenario, use a per-hart ring buffer or accept
        // occasional torn writes on the shared buffer.
        unsafe {
            let slot = self.slots.as_ptr().add(idx) as *mut Option<TraceEvent>;
            slot.write(Some(event));
        }
        // Advance tail if we just overwrote it (full buffer scenario).
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        if head.wrapping_sub(tail) >= self.capacity {
            self.tail.fetch_add(1, Ordering::Relaxed);
        }
    }
}
```

> **Note on multi-hart safety:** For maximum throughput, each hart should hold its own `TraceLogger` instance (and thus its own ring buffer). A shared logger requires an additional `AtomicUsize` for the head reservation pattern (compare-exchange loop). The per-hart model is preferred and is the default.

### `drain()` — Read All Pending Events

```rust
impl RingBuffer {
    /// Drain all unread events into a `Vec`. Called from the flush thread or on `flush_to_file`.
    pub fn drain(&self) -> Vec<TraceEvent> {
        let mut out = Vec::new();
        let head = self.head.load(Ordering::Acquire);
        let mut tail = self.tail.load(Ordering::Relaxed);
        while tail != head {
            let idx = tail & (self.capacity - 1);
            // Safety: we own this slot; head has moved past it.
            if let Some(event) = unsafe { &*self.slots.as_ptr().add(idx) }.clone() {
                out.push(event);
            }
            tail = tail.wrapping_add(1);
        }
        self.tail.store(tail, Ordering::Release);
        out
    }
}
```

---

## 3. TraceLogger API

```rust
pub struct TraceLogger {
    ring: Arc<RingBuffer>,
    subscribers: Mutex<Vec<Box<dyn Fn(&TraceEvent) + Send>>>,
    /// `HelmEventBus` subscription IDs so they can be unsubscribed on drop.
    event_bus_ids: Vec<SubscriberId>,
}

impl TraceLogger {
    /// Construct a TraceLogger with the given ring buffer capacity.
    /// `capacity` is rounded up to the next power of two.
    /// Default capacity: 65_536.
    pub fn new(capacity: usize) -> Self;

    /// Record an event into the ring buffer. Called from the simulation hot path
    /// via the HelmEventBus subscription (not directly by user code).
    pub fn log(&self, event: TraceEvent);

    /// Flush all events currently in the ring buffer to a JSON Lines file.
    /// Each line is one JSON-serialized `TraceEvent` followed by `\n`.
    /// Appends to the file if it already exists.
    pub fn flush_to_file(&self, path: &Path) -> io::Result<()>;

    /// Register a subscriber callback. Called synchronously from `log()`.
    /// Use sparingly — each callback is called on the simulation thread.
    pub fn subscribe(&self, f: Box<dyn Fn(&TraceEvent) + Send>);

    /// Return the last `n` events in chronological order.
    /// Returns fewer than `n` if fewer are available.
    pub fn recent(&self, n: usize) -> Vec<TraceEvent>;

    /// Return the total number of events that have been logged (including overwritten ones).
    pub fn total_logged(&self) -> u64;

    /// Return the number of events currently held in the ring buffer.
    pub fn buffered_count(&self) -> usize;
}
```

### `flush_to_file` Implementation

```rust
impl TraceLogger {
    pub fn flush_to_file(&self, path: &Path) -> io::Result<()> {
        use std::io::Write;
        let events = self.ring.drain();
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let mut writer = std::io::BufWriter::new(file);
        for event in &events {
            serde_json::to_writer(&mut writer, event)?;
            writer.write_all(b"\n")?;
        }
        writer.flush()
    }
}
```

### `recent` Implementation

```rust
impl TraceLogger {
    pub fn recent(&self, n: usize) -> Vec<TraceEvent> {
        let all = self.ring.drain();  // non-destructive if we re-snapshot
        // In practice, drain is destructive; use a secondary snapshot ring
        // or read from the slots directly without advancing tail.
        let start = all.len().saturating_sub(n);
        all[start..].to_vec()
    }
}
```

> **Implementation note:** A non-destructive `recent()` requires reading from the ring without advancing `tail`. Implement as a separate `peek(n)` that reads backward from `head` without touching `tail`. See the `ring.rs` module for the full implementation.

---

## 4. HelmEventBus Integration

`TraceLogger` subscribes to all relevant `HelmEventKind` variants during `elaborate()`. It holds the returned `SubscriberId` values so it can unsubscribe on drop.

```rust
impl SimObject for TraceLogger {
    fn elaborate(&mut self, system: &mut System) {
        let bus = system.event_bus_mut();
        let logger = Arc::clone(&self.inner);

        // Subscribe to each event kind and map to TraceEvent
        let id_insn = bus.subscribe(HelmEventKind::InsnFetch, {
            let l = Arc::clone(&logger);
            move |ev| {
                if let HelmEvent::InsnFetch { hart, pc, bytes, cycle } = ev {
                    l.log(TraceEvent::InsnFetch { cycle: *cycle, hart: *hart, pc: *pc, bytes: *bytes });
                }
            }
        });

        let id_memw = bus.subscribe(HelmEventKind::MemWrite, {
            let l = Arc::clone(&logger);
            move |ev| {
                if let HelmEvent::MemWrite { hart, addr, size, val, cycle } = ev {
                    l.log(TraceEvent::MemWrite { cycle: *cycle, hart: *hart, addr: *addr, size: *size as u8, value: *val });
                }
            }
        });

        let id_exc = bus.subscribe(HelmEventKind::Exception, {
            let l = Arc::clone(&logger);
            move |ev| {
                if let HelmEvent::Exception { cpu, vector, pc, tval, .. } = ev {
                    l.log(TraceEvent::Exception { cycle: 0, hart: 0, vector: *vector, pc: *pc, tval: *tval });
                }
            }
        });

        let id_sys = bus.subscribe(HelmEventKind::SyscallEnter, {
            let l = Arc::clone(&logger);
            move |ev| {
                if let HelmEvent::SyscallEnter { nr, args } = ev {
                    l.log(TraceEvent::Syscall { cycle: 0, hart: 0, nr: *nr, args: *args, ret: 0 });
                }
            }
        });

        self.event_bus_ids = vec![id_insn, id_memw, id_exc, id_sys];
    }
}
```

---

## 5. JSON Lines Output Format

Each line is a complete, self-contained JSON object. The `type` field identifies the variant.

**Example output:**

```jsonl
{"type":"insn_fetch","cycle":1024,"hart":0,"pc":4194304,"bytes":147}
{"type":"mem_read","cycle":1025,"hart":0,"addr":8388608,"size":4,"value":42}
{"type":"mem_write","cycle":1026,"hart":0,"addr":8388608,"size":4,"value":43}
{"type":"syscall","cycle":2048,"hart":0,"nr":64,"args":[1,4194560,14,0,0,0],"ret":14}
{"type":"exception","cycle":4096,"hart":0,"vector":12,"pc":4198400,"tval":0}
{"type":"branch_miss","cycle":8192,"hart":0,"pc":4194400,"target":4194312,"penalty":5}
{"type":"device_signal","cycle":16384,"device":"system.uart0","name":"irq","level":1}
{"type":"custom","cycle":32768,"name":"roi.start","data":{"label":"main loop"}}
```

Consumers can process this format with standard tools:

```bash
# Count all syscalls
jq 'select(.type == "syscall") | .nr' trace.jsonl | sort | uniq -c

# Extract all branch misses
jq 'select(.type == "branch_miss")' trace.jsonl
```

---

## 6. Python Callback Integration

Python subscribers are called synchronously from `log()`. The GIL is acquired once per callback invocation. Use filters to reduce callback frequency.

```rust
#[cfg(feature = "pyo3")]
impl TraceLogger {
    pub fn subscribe_python(&self, py: Python<'_>, callback: PyObject) {
        self.subscribe(Box::new(move |event| {
            Python::with_gil(|py| {
                // Convert TraceEvent to a Python dict via serde_json + PyAny
                let json = serde_json::to_string(event).unwrap();
                let py_dict = py.eval(&format!("__import__('json').loads('{}')", json), None, None);
                if let Ok(obj) = py_dict {
                    let _ = callback.call1(py, (obj,));
                }
            });
        }));
    }
}
```

**Python usage:**

```python
def on_trace_event(event):
    if event["type"] == "syscall":
        print(f"Syscall nr={event['nr']}")

sim.trace_logger.subscribe(on_trace_event)
sim.run(1_000_000)
sim.trace_logger.flush("trace.jsonl")
```

---

## 7. Implementation Notes

### Ring Buffer Sizing

Always round `capacity` to the next power of two:

```rust
fn next_pow2(n: usize) -> usize {
    if n.is_power_of_two() { n } else { n.next_power_of_two() }
}
```

### Subscriber Overhead

Subscriber callbacks run inline on the simulation thread. If the combined subscriber overhead exceeds ~50 ns/event, consider an async model where the ring buffer is drained by a background thread and callbacks run off-thread.

### Flushing Strategy

Two options:

1. **On-demand:** Call `flush_to_file()` explicitly from Python at ROI boundaries.
2. **Periodic background flush:** A background thread calls `flush_to_file()` every N milliseconds. This is the default in production use. The flush interval is configurable (default: 1 s).

### Memory Usage

At default capacity (65 536 events), each `TraceEvent` averages ~80 bytes (including `String` heap for `DeviceSignal` and `Custom` variants). Total ring buffer memory: ~5 MiB. For variants without heap allocation (`InsnFetch`, `MemRead`, `MemWrite`, `BranchMiss`), the actual size is much smaller.

Use a compact variant (`TraceEventCompact`) if memory is a concern:

```rust
/// Compact fixed-size variant for hot-path logging (no heap allocation).
#[repr(C)]
pub struct TraceEventCompact {
    pub kind: u8,   // discriminant
    pub hart: u8,
    pub size: u8,
    pub _pad: u8,
    pub cycle: u64,
    pub a: u64,     // pc / addr
    pub b: u64,     // value / target / vector / nr
}
```

---

## Design Decisions from Q&A

### Design Decision: Default ring buffer capacity 65,536 events (Q82)

`TraceLogger::new()` defaults to 65,536 events (64K), configurable via `TraceLogger::with_capacity(n)` or Python `TraceLogger(capacity=N)`. At construction time, capacity is rounded up to the next power of two for efficient ring index arithmetic (`index & (cap - 1)` instead of `index % cap`). Maximum capacity is capped at 16M events. At 65,536 events × ~80 bytes average = ~5 MiB — fits in L3 cache on modern CPUs.

### Design Decision: Overwrite oldest as default overflow policy (Q83)

The ring buffer uses **overwrite-oldest** semantics as the default (circular buffer). An optional `on_overflow` callback (registered via `TraceLogger::set_overflow_callback`) is invoked on each overwrite, allowing Python to flush to disk before the slot is reused. A `TraceLogger::set_policy(OverflowPolicy::Block)` alternative is available for correctness-critical analysis.

```rust
pub enum OverflowPolicy {
    /// Overwrite the oldest event (default). Hardware trace-buffer semantics.
    Overwrite,
    /// Block the simulation thread until the consumer drains space.
    Block,
    /// Drop the new event silently. Not recommended for most use cases.
    DropNew,
}
```

Rationale: hardware trace buffers (ARM CoreSight ETM, RISC-V Trace Encoder) universally use circular/overwrite semantics because they cannot stall the CPU. For post-mortem crash analysis, the most recent N events are what matters. The `overflow_count` stat tracks total overwrites.
