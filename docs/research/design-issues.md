# helm-ng Design Issues & Mitigation Plan

> Deep crate-by-crate analysis of design issues, performance concerns, and
> alternate approaches. Generated from full source review of all 20+ workspace
> crates (April 2026).

---

## Executive Summary

The helm-ng codebase is architecturally sound. The 10 inviolable design rules
are consistently upheld across all crates. The monomorphized timing model,
enum-dispatched ISA, and single-threaded hot loop produce a clean, fast
simulator core.

This review identified **38 issues** across 4 severity tiers. **All 38 have
been resolved or triaged** (34 fixed, 2 deferred, 2 not applicable) across
7 commits in April 2026.

The three most impactful clusters were:

1. **Missing inline annotations on hot-path trait impls** — fixed in Phase A
   (`9f5cf08`): `HelmAddressSpace::read/write`, `ExecContext` methods, and
   `IntervalTiming` consume methods all annotated.

2. **Unnecessary heap allocations in per-tick device code** — fixed in
   Phase B (`ca14874`) + Phase C (`4eab4e1`): DMA buffer reuse, typed
   `EventData` enum replacing `Box<dyn Any>`, lazy event cancellation.

3. **Incomplete infrastructure** — fixed across Phases B–D: event
   cancellation implemented, MmioBus binary search, HashSet breakpoints,
   per-VA TLB invalidation, VirtioMem trait for queue I/O.

---

## Severity Definitions

| Tier | Label | Meaning |
|------|-------|---------|
| P0 | **Critical** | Measurable hot-path regression or safety concern; fix before next perf milestone |
| P1 | **High** | Will become a bottleneck in Phase 2–3 workloads; fix soon |
| P2 | **Medium** | Suboptimal but tolerable; fix when touching the file |
| P3 | **Low** | Minor or cosmetic; fix opportunistically |

---

## Issue Index (sorted by severity)

| ID | Sev | Crate | Title | Status | Commit |
|----|-----|-------|-------|--------|--------|
| [DI-01](#di-01) | P0 | helm-memory | `HelmAddressSpace::read/write` missing `#[inline]` | **Fixed** | `9f5cf08` Phase A |
| [DI-02](#di-02) | P0 | helm-hw-dma | `vec![]` allocation on every `tick()` call | **Fixed** | `ca14874` Phase B |
| [DI-03](#di-03) | P0 | helm-event | Event cancellation not implemented | **Fixed** | `ca14874` Phase B |
| [DI-04](#di-04) | P0 | helm-core | No `#[inline]` on `ExecContext` trait methods | **Fixed** | `9f5cf08` Phase A |
| [DI-05](#di-05) | P1 | helm-event | `Box<dyn Any + Send>` type erasure per event | **Fixed** | `4eab4e1` Phase C |
| [DI-06](#di-06) | P1 | helm-timing | `reg_ready: [Tick; 128]` oversized for RISC-V | **Fixed** | `4eab4e1` Phase C |
| [DI-07](#di-07) | P1 | helm-timing | `consume_pending_loads/stores()` not inlined | **Fixed** | `9f5cf08` Phase A |
| [DI-08](#di-08) | P1 | helm-devices | MmioBus child lookup is O(n) linear scan | **Fixed** | `ca14874` Phase B |
| [DI-09](#di-09) | P1 | helm-engine | Decode cache direct-mapped with silent collision | **Fixed** | `60b6f6b` Phase C |
| [DI-10](#di-10) | P1 | helm-spy | BranchPredictor behind `Mutex` in hot loop | Deferred | Probe API requires `Fn`; Mutex needed for interior mutability |
| [DI-11](#di-11) | P1 | helm-jit | `CompiledBlock::new()` lifetime not enforced by type system | **Fixed** | `6f448b7` Phase D |
| [DI-12](#di-12) | P1 | helm-python | GIL thrashing in `instantiate()` child discovery | **N/A** | Already mitigated by freeze pattern |
| [DI-13](#di-13) | P1 | helm-hw-intc | GIC `Arc<Mutex>` serializes all IRQ accesses | Deferred | Requires SMP threading; uncontended in single-thread sim |
| [DI-14](#di-14) | P2 | helm-debug | GDB RSP checksum not validated | **Fixed** | `ca14874` Phase B |
| [DI-15](#di-15) | P2 | helm-debug | Breakpoint/watchpoint engines O(n) per insn | **Fixed** | `ca14874` Phase B |
| [DI-16](#di-16) | P2 | helm-devices | InterruptPin uses `SeqCst` ordering | **Fixed** | `9f5cf08` Phase A |
| [DI-17](#di-17) | P2 | helm-devices | IrqRouter `route()` is O(n) linear scan | **Fixed** | `ca14874` Phase B |
| [DI-18](#di-18) | P2 | helm-devices | HelmEventBus string key per fire | **Fixed** | `60b6f6b` Phase C |
| [DI-19](#di-19) | P2 | helm-memory | Device index bounds unchecked in address_space.rs | **Fixed** | `9f5cf08` Phase A |
| [DI-20](#di-20) | P2 | helm-memory | Page table rebuild O(n²) during ELF load | **Fixed** | `60b6f6b` Phase C |
| [DI-21](#di-21) | P2 | helm-memory | Non-page-aligned regions always use slow path | **N/A** | Sub-page MMIO routes through HelmAddressSpace, not FlatMem |
| [DI-22](#di-22) | P2 | helm-arch | Decoder monolithic 1,498-line single file | **Fixed** | `bddf4a8` Phase C |
| [DI-23](#di-23) | P2 | helm-arch | SIMD stubs silently succeed instead of faulting | **Fixed** | `9f5cf08` Phase A |
| [DI-24](#di-24) | P2 | helm-arch | TLB flush is all-or-nothing | **Fixed** | `4eab4e1` Phase D |
| [DI-25](#di-25) | P2 | helm-engine | TIMER_CHECK_INTERVAL fixed at 1024 | **Fixed** | `4eab4e1` Phase C |
| [DI-26](#di-26) | P2 | helm-engine | InstrumentedMem fixed 8-entry access limit | **Fixed** | `1ba8f57` Sweep |
| [DI-27](#di-27) | P2 | helm-python | SimObject circular references may leak | **Fixed** | `4eab4e1` Phase D |
| [DI-28](#di-28) | P2 | helm-python | `HelmSpy.snapshot()` allocates Vec every call | **Fixed** | `1ba8f57` Sweep |
| [DI-29](#di-29) | P2 | helm-plugin | HelmScoreboard `&mut` from `&self` via UnsafeCell | **Fixed** | `1ba8f57` Sweep |
| [DI-30](#di-30) | P2 | helm-plugin | Plugin callback dispatch is linear, unordered | **Fixed** | `1ba8f57` Sweep |
| [DI-31](#di-31) | P2 | helm-hw-rtc | PL031 `tick(cycles)` loops N times | **Fixed** | `ca14874` Phase B |
| [DI-32](#di-32) | P2 | helm-hw-virtio | `queue_notify()` can't access guest memory | **Fixed** | `6f448b7` Phase D |
| [DI-33](#di-33) | P2 | helm-report | PythonSink callback GIL management undocumented | **N/A** | Already documented with Arc<Mutex> GIL-safe pattern |
| [DI-34](#di-34) | P2 | helm-jit | JIT cache collision rate not tracked | **Fixed** | `ca14874` Phase B |
| [DI-35](#di-35) | P3 | helm-stats | `PerfCounter::inc/add` missing `#[inline]` | **Fixed** | `9f5cf08` Phase A |
| [DI-36](#di-36) | P3 | helm-platform | `AffinityMap::register()` panics on duplicate | **Fixed** | `1ba8f57` Sweep |
| [DI-37](#di-37) | P3 | helm-devices | `Box::leak()` in params.rs error path | **Fixed** | `1ba8f57` Sweep |
| [DI-38](#di-38) | P3 | helm-event | No capacity pre-allocation or memory budget | **Fixed** | `1ba8f57` Sweep |

---

## Detailed Issue Descriptions

### P0 — Critical

<a id="di-01"></a>
#### DI-01: `HelmAddressSpace::read/write` missing `#[inline]`

**Crate:** `helm-memory` — `address_space.rs:77-104`

**Problem:** The `MemInterface` impl for `HelmAddressSpace` — called on every
instruction fetch, load, and store — lacks `#[inline]` annotations. The
compiler may not inline these through trait boundaries, adding virtual call
overhead on every memory access.

**Impact:** Estimated 5–15% throughput regression in tight interpreter loops.
At 3 MIPS, that is ~150K–450K wasted instructions/second.

**Evidence:** `FlatMem::read_inner/write_inner` are correctly `#[inline]`, but
the `HelmAddressSpace` wrapper that calls them is not.

**Fix:**
```rust
// address_space.rs
#[inline]
fn read(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault> { ... }

#[inline]
fn write(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault> { ... }
```

---

<a id="di-02"></a>
#### DI-02: DMA `vec![]` allocation on every `tick()`

**Crate:** `helm-hw-dma` — `dma.rs:140`

**Problem:** Every `tick()` call allocates a temporary buffer:
```rust
let mut buf = vec![0u8; transfer as usize];
```
At 100 MHz with 64 bytes/tick: ~1.6M heap allocations/second.

**Impact:** Heap fragmentation, allocator contention, cache pollution. This is
the single worst allocation pattern in the codebase.

**Fix:** Pre-allocate a reusable buffer in the `DmaEngine` struct:
```rust
pub struct DmaEngine {
    transfer_buf: Vec<u8>,  // reused across ticks
    // ...
}

fn tick(&mut self, port: &mut dyn DmaPort) {
    self.transfer_buf.resize(transfer as usize, 0);
    // use self.transfer_buf instead of vec![]
}
```

---

<a id="di-03"></a>
#### DI-03: Event cancellation not implemented

**Crate:** `helm-event` — `lib.rs:199-202`

**Problem:** `cancel()` is a stub that always returns `false`. This violates
the `TimerScheduler` trait contract from `helm-core`. Devices cannot remove
scheduled events, leading to stale events accumulating on the heap.

**Impact:** Timer devices (SP804, PL031) that change reload values leave dead
events on the queue. In long-running FS-mode simulations, the event heap grows
unbounded with cancelled-intent events.

**Fix:** Implement lazy cancellation with a `HashSet<EventId>`:
```rust
pub struct EventQueue {
    heap: BinaryHeap<PendingEvent>,
    cancelled: HashSet<EventId>,  // lazy deletion
    // ...
}

pub fn cancel(&mut self, event_id: EventId) -> bool {
    self.cancelled.insert(event_id)
}

// In drain_until(), skip cancelled events:
fn drain_until(...) {
    while let Some(ev) = self.heap.peek() {
        if ev.fire_at > target { break; }
        let ev = self.heap.pop().unwrap();
        if self.cancelled.remove(&ev.seq) { continue; }  // skip
        handler(ev.class_id, ev.owner_id, ev.data);
    }
}
```

---

<a id="di-04"></a>
#### DI-04: No `#[inline]` on `ExecContext` trait methods

**Crate:** `helm-core` — `lib.rs:58-95`

**Problem:** `ExecContext` methods (`read_int_reg`, `write_int_reg`,
`read_mem`, `write_mem`) are the hottest methods in the simulator — called
billions of times. None have `#[inline]` annotations.

**Impact:** Compiler heuristics usually inline small trait methods, but there
is no guarantee across crate boundaries without LTO. Adding annotations ensures
the compiler never generates an indirect call.

**Fix:** Add `#[inline]` to all `ExecContext` methods in trait definition and
all implementations.

---

### P1 — High

<a id="di-05"></a>
#### DI-05: `Box<dyn Any + Send>` type erasure per event

**Crate:** `helm-event` — `lib.rs:30`

**Problem:** Every scheduled event allocates a `Box<dyn Any + Send>` for its
payload, requiring heap allocation + vtable dispatch + `downcast_ref()` at
handler time.

**Impact:** For high event rates (accurate timing mode with thousands of events
per quantum), this adds measurable allocation pressure.

**Mitigation:** Use a small-buffer optimization or typed event channels:
```rust
enum EventData {
    Tick,                          // zero-sized, no alloc
    Timer { reload: u64 },         // inline, 8 bytes
    Callback(Box<dyn FnOnce()>),   // only callbacks allocate
}
```

---

<a id="di-06"></a>
#### DI-06: `reg_ready: [Tick; 128]` oversized for RISC-V

**Crate:** `helm-timing` — `lib.rs:273`

**Problem:** `OpenInterval` contains `reg_ready: [Tick; 128]` (1,024 bytes) to
track register readiness. RISC-V needs only 64 entries (32 int + 32 FP),
wasting 512 bytes per interval.

**Impact:** Cache pressure in IntervalTiming mode. The `OpenInterval` struct
is ~1,150 bytes; half of it unused for RISC-V.

**Mitigation:** Parameterize by ISA or use a smaller base array:
```rust
const REG_READY_SLOTS: usize = 64;  // covers RISC-V
reg_ready: [Tick; REG_READY_SLOTS],
// AArch64 vector regs tracked in separate overflow array
```

---

<a id="di-07"></a>
#### DI-07: `consume_pending_loads/stores()` not inlined

**Crate:** `helm-timing` — `lib.rs:434, 458`

**Problem:** These functions are called from `on_insn()` (per-instruction hot
path) but lack `#[inline]` annotations. They contain `enumerate().min_by_key()`
which is branch-intensive.

**Fix:** Add `#[inline(always)]` and unroll the 2-element min search:
```rust
#[inline(always)]
fn consume_pending_loads(&mut self) {
    let slot = if self.load_slots_ready[0] <= self.load_slots_ready[1] { 0 } else { 1 };
    // ...
}
```

---

<a id="di-08"></a>
#### DI-08: MmioBus child lookup is O(n) linear scan

**Crate:** `helm-devices` — `bus/mmio.rs:84-91`

**Problem:** `find_child()` iterates all children to find the one containing a
given offset. Called on every MMIO transaction through the bus.

**Impact:** With 8–16 devices on a bus, this is 4–8 cache-line loads per MMIO
access. Negligible at functional speed but adds up in accurate timing.

**Fix:** Sort children by offset at attach time, use binary search:
```rust
fn find_child(&self, offset: u64) -> Option<usize> {
    let idx = self.children.partition_point(|c| c.offset + c.size <= offset);
    self.children.get(idx).filter(|c| offset >= c.offset).map(|_| idx)
}
```

---

<a id="di-09"></a>
#### DI-09: Decode cache direct-mapped with silent collision

**Crate:** `helm-engine` — `aarch64_decode_cache.rs`

**Problem:** 4096-entry direct-mapped cache indexed by `(pc >> 2) & 0xFFF`.
Collisions silently overwrite entries. No collision statistics.

**Impact:** Tight loops at addresses with the same lower 14 bits thrash the
cache. Two functions 16 KB apart constantly evict each other's entries.

**Mitigation:** Increase associativity to 2-way or 4-way:
```rust
struct DecodeCacheEntry {
    entries: [(u64, u32, DecodedInsn); 2],  // 2-way
}
```
Add a collision counter for workload characterization.

---

<a id="di-10"></a>
#### DI-10: BranchPredictor behind `Mutex` in hot loop

**Crate:** `helm-spy` — `session.rs:20`

**Problem:** `Arc<Mutex<BranchPredictor>>` is locked on every branch event in
the hot loop. For branch-heavy workloads (>50% of instructions), this adds
lock/unlock overhead per branch.

**Fix:** Use per-core BranchPredictor (no sharing needed) or replace Mutex with
atomic state transitions in the predictor.

---

<a id="di-11"></a>
#### DI-11: `CompiledBlock::new()` lifetime not enforced

**Crate:** `helm-jit` — `block.rs:44-56`

**Problem:** `CompiledBlock` stores a raw function pointer + `Box<dyn Any>` for
the backing buffer. The type system does not enforce that the function pointer
points into the buffer. If `_buf` is dropped first, the function pointer
becomes dangling.

**Impact:** Use-after-free if buffer and code pointer lifetime diverge. Current
code is safe by construction but fragile.

**Fix:** Wrap in a self-referential struct or use `owning_ref`:
```rust
pub struct CompiledBlock {
    _buf: Pin<Box<[u8]>>,
    entry: NonNull<()>,  // points into _buf
}
```

---

<a id="di-12"></a>
#### DI-12: GIL thrashing in `instantiate()` child discovery

**Crate:** `helm-python` — `system.rs:83-104`

**Problem:** `instantiate()` holds `PyRefMut` during child discovery, calling
`extract::<PyRef>()` for each child. With 10+ devices, this causes repeated
GIL acquisition/release.

**Fix:** Extract all children into a Rust `Vec` in one pass, then process
without holding `PyRefMut`:
```rust
let children: Vec<FrozenChild> = py_children
    .iter()
    .map(|c| freeze_child(c, py))
    .collect();
drop(py_ref);  // release GIL
// process children in pure Rust
```

---

<a id="di-13"></a>
#### DI-13: GIC `Arc<Mutex>` serializes all IRQ accesses

**Crate:** `helm-hw-intc` — `gicv2/distributor.rs`, `cpu_interface.rs`

**Problem:** Every MMIO register access (IAR, EOIR, priority) locks
`Arc<Mutex<GicSharedState>>`. In SMP with multiple vCPUs handling interrupts
simultaneously, this serializes all cores.

**Impact:** Not a blocker for Phase 1 (single-core) but limits Phase 3 SMP
scaling.

**Mitigation (Phase 3):** Partition GIC state:
- Per-CPU interface state: lock-free (one writer per vCPU)
- Distributor shared state: fine-grained locks per IRQ group (32 IRQs each)
- SGI/PPI banks: already per-CPU, move out of shared Mutex

---

### P2 — Medium

<a id="di-14"></a>
#### DI-14: GDB RSP checksum not validated

**Crate:** `helm-debug` — `rsp.rs:82`

**Problem:** Incoming GDB packets have checksums that are read but never
validated. Corrupt packets are accepted silently.

**Fix:** Compute running checksum during packet read, compare with trailer
bytes, send NAK on mismatch.

---

<a id="di-15"></a>
#### DI-15: Breakpoint/watchpoint engines O(n) per instruction

**Crate:** `helm-debug` — `breakpoint.rs`, `watchpoint.rs`

**Problem:** Both engines iterate all entries (Vec) on every instruction step
or memory access. Fine for <10 breakpoints; degrades for heavy debugging
sessions.

**Fix:** Use `HashSet<u64>` keyed by address for O(1) breakpoint lookup. For
watchpoint ranges, use a sorted Vec with binary search.

---

<a id="di-16"></a>
#### DI-16: InterruptPin uses `SeqCst` ordering

**Crate:** `helm-devices` — `interrupt.rs:171`

**Problem:** `InterruptWire::asserted` uses `Ordering::SeqCst` for
`AtomicBool::swap()`. SeqCst is the most expensive ordering, generating
full memory fences.

**Impact:** On multi-socket NUMA systems, SeqCst adds 10–20 ns per IRQ
assert/deassert vs. 2–5 ns for Release/Acquire.

**Fix:** Use `Ordering::Release` for `swap()` and `Ordering::Acquire` for
`load()`. SeqCst is only needed when multiple atomics must be observed in
a total order, which is not the case here.

---

<a id="di-17"></a>
#### DI-17: IrqRouter `route()` is O(n) linear scan

**Crate:** `helm-devices` — `irq_router.rs:102-109`

**Problem:** Route lookup scans all routes linearly. Systems with 20+ IRQ
sources pay per-assertion lookup cost.

**Fix:** Build a `HashMap<(DeviceId, u32), Route>` after all routes are added
(freeze at elaborate time).

---

<a id="di-18"></a>
#### DI-18: HelmEventBus string key per fire

**Crate:** `helm-devices` — `bus/event_bus.rs`

**Problem:** `fire(event: &str, val: u64)` hashes the event name string on
every call. String hashing is ~10 ns for typical event names.

**Fix:** Intern event names as `u64` IDs at subscribe time. Provide
`fire_id(id: u64, val: u64)` for hot-path use.

---

<a id="di-19"></a>
#### DI-19: Device index bounds unchecked

**Crate:** `helm-memory` — `address_space.rs:84, 97`

**Problem:** `self.devices[entry.device_id.0 as usize]` has no bounds check.
If AddressMap state is corrupted, this panics the simulator.

**Fix:** Add `debug_assert!((entry.device_id.0 as usize) < self.devices.len())`.

---

<a id="di-20"></a>
#### DI-20: Page table rebuild O(n²) during ELF load

**Crate:** `helm-memory` — `flat_mem.rs:119-152`

**Problem:** Each `map()` call triggers a full page table rebuild. Loading an
ELF with 100 segments causes 100 rebuilds, each scanning all regions.

**Fix:** Add a `batch_map()` method or defer rebuild until first access:
```rust
pub fn batch_map(&mut self, regions: &[(u64, Vec<u8>)]) {
    for (base, data) in regions {
        self.regions.push(FlatMemRegion { base: *base, size: data.len() as u64, data: data.clone() });
    }
    self.rebuild_page_table();  // once
}
```

---

<a id="di-21"></a>
#### DI-21: Non-page-aligned regions always use slow path

**Crate:** `helm-memory` — `flat_mem.rs:104-105`

**Problem:** Regions not page-aligned or smaller than 4 KB are skipped during
page table construction. All accesses to these regions fall through to O(n)
linear scan.

**Impact:** Sub-page MMIO device windows (common in FS mode) are slower than
necessary.

**Fix:** Pad sub-page regions to page boundaries during `map()`, or maintain a
secondary hash map for small regions.

---

<a id="di-22"></a>
#### DI-22: Decoder monolithic 1,498-line single file

**Crate:** `helm-arch` — `aarch64/decode.rs`

**Problem:** Single file hurts compile-time parallelism and readability.

**Fix:** Split into `decode/dp_imm.rs`, `decode/ldst.rs`, `decode/simd.rs`,
`decode/branch.rs`. No runtime impact.

---

<a id="di-23"></a>
#### DI-23: SIMD stubs silently succeed

**Crate:** `helm-arch` — `aarch64/execute/simd.rs`

**Problem:** Unimplemented SIMD opcodes mapped to `SimdOther` log a warning
and return `Ok(false)` instead of raising `IllegalInstruction`. Guest programs
silently compute wrong results.

**Fix:** Return `Err(HartException::IllegalInstruction { ... })` for
unimplemented opcodes, or at minimum track statistics of stub invocations.

---

<a id="di-24"></a>
#### DI-24: TLB flush is all-or-nothing

**Crate:** `helm-arch` — `mmu.rs`, `arch_state.rs:120`

**Problem:** TLBI instructions flush the entire 1024-entry TLB. Per-entry
invalidation (TLBI VAE1) is not implemented.

**Impact:** Context-switch-heavy workloads (many processes) pay full TLB
refill cost after every TLBI, even when only one entry is stale.

**Fix (Phase 2):** Implement per-VA invalidation by matching `va_page` in
the TLB entry and clearing only the matching slot.

---

<a id="di-25"></a>
#### DI-25: TIMER_CHECK_INTERVAL fixed at 1024

**Crate:** `helm-engine` — `fs.rs`

**Problem:** Timer deadline check happens every 1024 instructions. If a timer
fires at tick T and the next check is at T+1023, the interrupt is delayed by
up to 1023 instructions.

**Fix:** Compute next timer deadline dynamically and set countdown accordingly:
```rust
let next_deadline = min(cntp_cval, cntv_cval);
self.timer_countdown = (next_deadline - self.tick).min(MAX_INTERVAL) as u32;
```

---

<a id="di-26"></a>
#### DI-26: InstrumentedMem fixed 8-entry access limit

**Crate:** `helm-engine` — `lib.rs`

**Problem:** Memory access instrumentation records up to 8 accesses per
instruction. SVE/SME vector instructions with >8 element accesses lose data.

**Fix:** Use `SmallVec<[MemAccess; 8]>` which spills to heap only when needed.

---

<a id="di-27"></a>
#### DI-27: SimObject circular references may leak

**Crate:** `helm-python` — `simobject.rs`

**Problem:** `HelmSystem` holds `children: IndexMap<String, PyObject>`, and
each child may hold a back-reference. Python's GC handles cycles in pure Python
objects, but PyO3 `#[pyclass]` instances use reference counting without Python
cycle detection by default.

**Fix:** Implement `__traverse__` and `__clear__` for GC support, or use weak
references for parent pointers.

---

<a id="di-28"></a>
#### DI-28: `HelmSpy.snapshot()` allocates Vec every call

**Crate:** `helm-python` — `spy.rs:172-199`

**Problem:** Every `snapshot()` call allocates a new `PyDict` and
`Vec<(String, u64, f64)>` for the instruction mix table.

**Fix:** Cache the instruction mix as `Vec<(&'static str, u64, f64)>` (insn
class names are static strings) and return a reference or iterator.

---

<a id="di-29"></a>
#### DI-29: HelmScoreboard `&mut` from `&self` via UnsafeCell

**Crate:** `helm-plugin` — `runtime/scoreboard.rs`

**Problem:** `get_mut(&self, idx)` returns `&mut T` from `&self` without any
synchronization. Safe by the per-vCPU invariant (one writer per slot), but not
enforced by the type system.

**Fix:** Document the invariant prominently. Consider using `Cell<T>` for Copy
types or `RefCell<T>` for debug-mode borrow checking.

---

<a id="di-30"></a>
#### DI-30: Plugin callback dispatch is linear, unordered

**Crate:** `helm-plugin` — `runtime/registry.rs`

**Problem:** All registered callbacks fire sequentially with no priority. For
100+ plugins, latency accumulates.

**Fix:** Add priority tiers (Critical/Normal/Low). Pre-partition callbacks.
Use a bitmask `AtomicU32` for `has_any_callbacks()` instead of checking 4 Vec
emptiness.

---

<a id="di-31"></a>
#### DI-31: PL031 `tick(cycles)` loops N times

**Crate:** `helm-hw-rtc` — `pl031.rs:108-112`

**Problem:** `TickableDevice::tick(cycles)` calls internal `tick()` N times in
a loop instead of accumulating. For large cycle counts, this is O(N).

**Fix:** Direct accumulation with alarm check:
```rust
fn tick(&mut self, cycles: u64) {
    let old = self.counter;
    self.counter = self.counter.wrapping_add(cycles as u32);
    if old < self.match_reg && self.counter >= self.match_reg { fire_irq(); }
}
```

---

<a id="di-32"></a>
#### DI-32: VirtIO `queue_notify()` can't access guest memory

**Crate:** `helm-hw-virtio` — `blk.rs:86-91`

**Problem:** `VirtioBackend::queue_notify()` signature doesn't include a memory
interface parameter. Caller must synchronously call `process_queue()` with
memory access — incompatible with EventQueue-deferred callbacks.

**Fix:** Extend `queue_notify()` to accept `&mut dyn MemInterface`, or
restructure to use a DMA-port pattern (like DmaEngine).

---

<a id="di-33"></a>
#### DI-33: PythonSink callback GIL handling undocumented

**Crate:** `helm-report` — `sink/python.rs`

**Problem:** PythonSink likely calls back to Python via PyObject. GIL
management is not documented. Could deadlock if Python callback re-enters
HelmSpy.

**Fix:** Document that Python callbacks must not call back into the simulator.
Add `Python::with_gil()` wrapper with deadlock detection.

---

<a id="di-34"></a>
#### DI-34: JIT cache collision rate not tracked

**Crate:** `helm-jit` — `cache.rs:114`

**Problem:** Direct-mapped 4096-entry cache silently evicts on collision. No
statistics to diagnose performance regressions from cache thrashing.

**Fix:** Add `eviction_count: u64` counter. Expose via stats registry.

---

### P3 — Low

<a id="di-35"></a>
#### DI-35: `PerfCounter::inc/add` missing `#[inline]`

**Crate:** `helm-stats` — `lib.rs`

Not inlined but should be — single `AtomicU64::fetch_add(Relaxed)` call.

---

<a id="di-36"></a>
#### DI-36: `AffinityMap::register()` panics on duplicate

**Crate:** `helm-platform` — `affinity.rs`

Should return `Result` instead of `assert!()`.

---

<a id="di-37"></a>
#### DI-37: `Box::leak()` in params.rs error path

**Crate:** `helm-devices` — `params.rs:261-262`

Leaks `Box<str>` to create `&'static str` for error messages. Bounded (only
during invalid device instantiation) but technically unbounded.

---

<a id="di-38"></a>
#### DI-38: No capacity pre-allocation or memory budget

**Crate:** `helm-event` — `lib.rs`

EventQueue starts at capacity 0. Should expose `reserve(n)` for workloads
with known event volume.

---

## Cross-Cutting Concerns

### 1. Inline Annotation Discipline

Multiple hot-path trait impls lack `#[inline]`. The pattern:

| Location | Method | Status |
|----------|--------|--------|
| `helm-core` ExecContext | read_int_reg, write_int_reg, read_mem, write_mem | Missing |
| `helm-memory` HelmAddressSpace | MemInterface::read, MemInterface::write | Missing |
| `helm-memory` FlatMem | MemInterface::read, MemInterface::write | Present ✓ |
| `helm-timing` VirtualTiming | on_insn, on_mem_access, on_branch | Present ✓ |
| `helm-timing` IntervalTiming | consume_pending_loads/stores | Missing |
| `helm-stats` PerfCounter | inc, add, get | Missing |

**Recommendation:** Establish a `#[inline]` policy: all trait method impls
called from the instruction step loop must be annotated. Add a clippy lint
or CI check.

### 2. Atomic Ordering Consistency

| Crate | Atomic | Current | Recommended |
|-------|--------|---------|-------------|
| helm-stats | PerfCounter | Relaxed | Relaxed ✓ |
| helm-diag | GLOBAL_MONITOR_ACTIVE | Acq/Rel | Acq/Rel ✓ |
| helm-devices | InterruptWire | SeqCst | Release/Acquire |
| helm-plugin | cache counters | Relaxed | Relaxed ✓ |

Only InterruptWire is overspecified.

### 3. Allocation-Free Hot Path

The hot loop (fetch-decode-execute-timing) should be allocation-free. Current
violations:

| Allocation | Frequency | Crate |
|------------|-----------|-------|
| DMA `vec![buf]` | Per tick | helm-hw-dma |
| `Box<dyn Any>` per event | Per schedule | helm-event |
| `Vec` in event drain | Per drain | helm-engine |
| `HashSet::insert` for stubs | Per stub hit | helm-engine |
| HelmSpy snapshot | Per snapshot | helm-spy |

DMA and event boxing are the most impactful. The others are negligible at
current frequencies.

### 4. SMP Readiness

The codebase is designed for single-threaded simulation (design rule 8). Three
areas need attention for Phase 3 SMP:

1. **GIC Mutex** (DI-13) — serializes all interrupt handling across cores
2. **EventQueue** — single queue, no per-core partitioning
3. **FlatMem** — `unsafe impl Send` is correct but needs per-core TLB

---

## Mitigation Plan — Resolution Summary

All 38 issues resolved or triaged across 7 commits (April 2026).

### Phase A — Quick Wins ✅ `9f5cf08`

7 items: DI-01, DI-04, DI-07, DI-16, DI-19, DI-23, DI-35 — `#[inline]` annotations,
Acquire/Release ordering, debug_assert bounds check, SIMD stub correctness.

### Phase B — Targeted Fixes ✅ `ca14874`

8 items: DI-02, DI-03, DI-08, DI-14, DI-15, DI-17, DI-31, DI-34 — DMA buffer reuse,
lazy event cancellation, binary search MmioBus, HashSet breakpoints, IrqRouter HashMap,
PL031 direct arithmetic, RSP checksum validation, JIT eviction counter.

### Phase C — Structural Improvements ✅ `60b6f6b` + `4eab4e1` + `bddf4a8`

9 items: DI-05, DI-06, DI-09, DI-12, DI-18, DI-20, DI-22, DI-25, (DI-10 deferred) —
EventData typed enum, reg_ready 128→64, 2-way decode cache, event name interning,
deferred page table rebuild, decoder split into 6 sub-modules, dynamic timer countdown.

### Phase D — SMP Preparation ✅ `4eab4e1` + `6f448b7`

5 items: DI-11, DI-24, DI-27, DI-32, (DI-13 deferred) — Pin<Box> for CompiledBlock,
per-VA TLB invalidation, SimObject GC traversal, VirtioMem trait for queue_notify.

### Final Sweep ✅ `1ba8f57`

7 items: DI-26, DI-28, DI-29, DI-30, DI-36, DI-37, DI-38 — InstrumentedMem 8→16,
snapshot() String elision, Scoreboard safety docs, callback bitmask, AffinityMap Result,
Box::leak elimination, EventQueue::reserve().

### Deferred (2 items)

| ID | Reason |
|----|--------|
| DI-10 | Probe::subscribe requires `Fn`; Mutex needed for interior mutability. Fix when Probe API supports `FnMut`. |
| DI-13 | GIC acknowledge path needs atomic dist+redist access. Partition when SMP multi-threading is added. |

### Not Applicable (2 items)

| ID | Reason |
|----|--------|
| DI-21 | Sub-page MMIO routes through HelmAddressSpace (device dispatch), not FlatMem page table. |
| DI-33 | PythonSink already uses correct Arc<Mutex> GIL-safe pattern with documentation. |

---

## Appendix: Crate Health Summary

| Crate | Lines | Issues | P0 | P1 | P2 | P3 | Unsafe | Verdict |
|-------|-------|--------|----|----|----|----|--------|---------|
| helm-core | 683 | 1 | 1 | 0 | 0 | 0 | 0 | Solid foundation |
| helm-memory | 681 | 4 | 1 | 0 | 3 | 0 | 5 (sound) | Needs inline fix |
| helm-timing | 871 | 2 | 0 | 2 | 0 | 0 | 0 | Good; reg_ready oversized |
| helm-event | 203 | 3 | 1 | 1 | 0 | 1 | 0 | Cancellation critical |
| helm-devices | 4,300 | 5 | 0 | 1 | 4 | 1 | 0 | Solid SDK |
| helm-arch | 5,000+ | 3 | 0 | 0 | 3 | 0 | 0 | Production quality |
| helm-engine | 7,000+ | 3 | 0 | 1 | 2 | 0 | ~83 (FFI) | Core is excellent |
| helm-stats | 118 | 1 | 0 | 0 | 0 | 1 | 0 | Minimal, correct |
| helm-diag | 550 | 0 | 0 | 0 | 0 | 0 | 0 | Exemplary |
| helm-probe | 225 | 0 | 0 | 0 | 0 | 0 | 2 (sound) | Exemplary |
| helm-decode | 1,500 | 0 | 0 | 0 | 0 | 0 | 0 | Correct |
| helm-plugin | 1,800 | 2 | 0 | 0 | 2 | 0 | 1 (risky) | Scoreboard needs docs |
| helm-debug | 800 | 2 | 0 | 0 | 2 | 0 | 0 | RSP needs checksum |
| helm-python | 1,500 | 3 | 0 | 1 | 2 | 0 | 0 | GIL discipline needed |
| helm-platform | 1,000 | 1 | 0 | 0 | 0 | 1 | 0 | Clean |
| helm-spy | 2,000 | 1 | 0 | 1 | 0 | 0 | 1 (docs) | Mutex bottleneck |
| helm-report | 1,500 | 1 | 0 | 0 | 1 | 0 | 0 | GIL docs needed |
| helm-jit | 3,000 | 2 | 0 | 1 | 1 | 0 | 3 (careful) | Lifetime concern |
| helm-cli | 400 | 0 | 0 | 0 | 0 | 0 | 0 | Simple, correct |
| helm-hw-char | 447 | 0 | 0 | 0 | 0 | 0 | 0 | Clean |
| helm-hw-timer | 423 | 0 | 0 | 0 | 0 | 0 | 0 | Clean |
| helm-hw-rtc | 274 | 1 | 0 | 0 | 1 | 0 | 0 | Minor inefficiency |
| helm-hw-dma | 385 | 1 | 1 | 0 | 0 | 0 | 0 | Allocation fix critical |
| helm-hw-intc | 1,500+ | 1 | 0 | 1 | 0 | 0 | 0 | SMP concern |
| helm-hw-pci | 612 | 0 | 0 | 0 | 0 | 0 | 0 | Spec-compliant |
| helm-hw-virtio | 1,000+ | 1 | 0 | 0 | 1 | 0 | 0 | API mismatch |
