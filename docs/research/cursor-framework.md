# cursor-framework -- Codebase Audit

Date: 2026-04-07

## Summary

The `framework/` domain contains the stable API and shared primitive crates.
The leaf crate `helm-core` is clean with zero internal dependencies. `helm-memory`
is solid for the live `HelmAddressSpace`/`FlatMem` paths but carries an
experimental `MemoryMap` that coexists ambiguously. `helm-timing` has good
unit coverage for the interval model. The biggest risks are register-index
bounds in `IntervalTiming`, an incomplete `StatsRegistry` that cannot track
histograms, a plugin callback bitmask that omits syscall/fault subscribers,
and the `FieldDesc::mask` overflow at full-register width. `helm-decode` has
only stub tests.

---

## Design Issues

### D1. Experimental `MemoryMap` coexists with live `HelmAddressSpace`

**Severity: Medium**

`MemoryRegion` / `MemoryMap` are documented as experimental and incomplete
(`helm-memory/src/lib.rs:28-32`) while `HelmAddressSpace` is the live
runtime surface. Both implement `MemInterface`, so callers can accidentally
depend on the wrong type. No compile-time gate distinguishes them.

```rust
// framework/helm-memory/src/lib.rs:28-32
/// This model is not the live runtime memory surface today. `HelmAddressSpace`
/// remains authoritative for current RAM/MMIO behavior while this tree model
/// still lacks complete alias/container/remap semantics.
```

**Suggested fix:** Gate `MemoryMap` behind a feature flag (`experimental-memmap`)
to prevent accidental production use, or move it to a separate crate.

---

### D2. `helm-memory` depends on `helm-devices`

**Severity: Medium**

The physical-memory crate is tied to `Device` + `AddressMap` because
`HelmAddressSpace` dispatches MMIO to devices. This means "framework memory"
is not an independent leaf -- testing plain RAM requires pulling in the device
SDK.

**Suggested fix:** This is structural and intentional for MMIO dispatch. If
reuse without devices becomes a goal, consider splitting `FlatMem` into
`helm-core` (it has no device dependency) and keeping MMIO dispatch in
`helm-memory`.

---

### D3. `ByteMem` default impl is byte-by-byte O(n)

**Severity: Low**

The blanket `impl<T: MemInterface> ByteMem for T` reads/writes one byte at a
time via the scalar `MemInterface`:

```rust
// framework/helm-core/src/mem.rs:118-131
impl<T: MemInterface + ?Sized> ByteMem for T {
    fn read_bytes(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), MemFault> {
        for (offset, byte) in buf.iter_mut().enumerate() {
            *byte = self.read(addr + offset as u64, 1, AccessType::Load)? as u8;
        }
        Ok(())
    }
    // ...
}
```

For ELF loading or VirtIO descriptor walks this incurs N scalar reads where
a bulk `memcpy` would suffice. `HelmAddressSpace` has its own `read_bytes`
override that handles aligned RAM differently, creating duplicated paths.

**Suggested fix:** Accept as-is for correctness, but document that hot-path
bulk transfers should use the `HelmAddressSpace` override directly or provide
a `ByteMemExt` with an optimized default that tries wide reads first.

---

### D4. `HelmPluginRegistry::has_any_callbacks()` omits syscall/fault/vcpu callbacks

**Severity: High**

The fast-path bitmask only checks `CB_INSN | CB_MEM | CB_BRANCH | CB_TIMER`:

```rust
// framework/helm-plugin/src/runtime/registry.rs:96-98
pub fn has_any_callbacks(&self) -> bool {
    self.cb_mask & (CB_INSN | CB_MEM | CB_BRANCH | CB_TIMER) != 0
}
```

`on_syscall` and `on_syscall_ret` never set the mask (`registry.rs:50-55`).
`on_fault` sets `CB_FAULT` but `has_any_callbacks` does not check it.
Engine call sites that consult `has_any_callbacks()` as a "skip all plugins"
guard will bypass syscall, fault, and vcpu lifecycle callbacks.

**Suggested fix:** Include `CB_FAULT` in the mask check, and add `CB_SYSCALL`
and `CB_VCPU` bits for the remaining callback types.

---

### D5. `SubscriptionId` documents Drop-based unsubscribe but does not implement it

**Severity: Low**

```rust
// framework/helm-devices/src/bus/event_bus.rs:86-87
/// A subscription handle. Drop to unsubscribe (TODO: implement Drop).
pub struct SubscriptionId(u64);
```

Manual `unsubscribe` exists but the RAII pattern is not wired.

**Suggested fix:** Implement `Drop for SubscriptionId` that calls
`unsubscribe`, or remove the doc claim.

---

### D6. `StatsRegistry` tracks counters but not histograms

**Severity: Medium**

Module docs mention histograms, but `StatsRegistry` only holds counters:

```rust
// framework/helm-stats/src/lib.rs:112-114
pub struct StatsRegistry {
    counters: HashMap<String, (PerfCounter, String)>,
}
```

`PerfHistogram` exists as a standalone type with no registry integration.
There is no way to enumerate or dump all histograms from the central registry.

**Suggested fix:** Add a `histograms: HashMap<String, Arc<PerfHistogram>>`
field and corresponding `histogram()` / `dump_json()` support.

---

## Correctness Issues

### C1. `IntervalTiming::on_insn` -- register index out of range

**Severity: High**

`dst_reg as usize` indexes into `reg_ready` (a fixed 64-slot array) without
bounds checking:

```rust
// framework/helm-timing/src/lib.rs:510-511
for &dst_reg in info.dst_regs() {
    self.open_interval.reg_ready[dst_reg as usize] = complete_at;
}
```

Similarly, `src_ready_cycle` (`lib.rs:413-418`) maps `reg` directly to the
array index. If `TimingInsnInfo` ever contains a register ID >= 64 (e.g.
from an extended register file or malformed metadata), this panics.

**Suggested fix:** Add `debug_assert!(dst_reg < 64)` or `.get_mut()` with
a saturating fallback.

---

### C2. `FieldDesc::mask` overflows when width is 64

**Severity: Medium**

```rust
// framework/helm-devices/src/framework/register_bank.rs:62-65
pub const fn mask(&self) -> u64 {
    let width = self.msb - self.lsb + 1;
    ((1u64 << width) - 1) << self.lsb
}
```

If `msb=63, lsb=0`, then `width=64` and `1u64 << 64` is undefined behavior
in C and a panic in debug Rust (overflow in `const` context). No device
currently uses a full-64-bit field, but the API allows it.

**Suggested fix:** Special-case `width == 64` to return `u64::MAX << lsb`,
or use `u64::MAX >> (64 - width) << lsb`.

---

### C3. `EventQueue::post_after` -- tick overflow

**Severity: Medium**

```rust
// framework/helm-event/src/lib.rs:145
self.post_at(self.current_tick + delay, class_id, owner_id, data)
```

`current_tick + delay` can silently wrap `u64`. Extremely unlikely in practice
(requires ~584 years at 1 GHz tick rate) but violates the monotonicity
assumption of the min-heap.

**Suggested fix:** Use `checked_add` and panic/saturate on overflow.

---

### C4. `EventQueue::cancel` returns true for non-existent IDs

**Severity: Low**

```rust
// framework/helm-event/src/lib.rs:172-174
pub fn cancel(&mut self, event_id: EventId) -> bool {
    self.cancelled.insert(event_id)
}
```

`HashSet::insert` returns `true` when the key was new. Cancelling a
never-posted ID returns `true`, which is counterintuitive. The test at
`lib.rs:270-273` explicitly documents this behavior.

**Suggested fix:** Document clearly or return `false` for IDs >= `next_seq`.

---

### C5. `FlatMem` returns 0 for unmapped reads instead of faulting

**Severity: Medium**

Reads to unmapped addresses fall through the region scan and return `0`:

```rust
// framework/helm-memory/src/flat_mem.rs (read_inner)
// After iterating regions with no match:
0
```

This differs from `MemoryMap` and `HelmAddressSpace` which return
`MemFault::AccessFault`. SE-mode code that expects faults on invalid
addresses will get silent zeros from `FlatMem`.

**Suggested fix:** This is documented as intentional (SE fast path where
unmapped reads are benign). Add a `strict` mode or `FlatMemPolicy` enum
to optionally return faults.

---

### C6. `MemoryMap` alias/container regions always fault

**Severity: Low (experimental code)**

```rust
// framework/helm-memory/src/lib.rs:175-178
MemoryRegion::Alias { .. } | MemoryRegion::Container { .. } => {
    // TODO(phase-1): alias/container resolution
    Err(MemFault::AccessFault { addr })
}
```

Any address mapping through an alias or container node will produce a
spurious access fault.

---

### C7. `fetch32` / `fetch16` use `unreachable!` on truncation failure

**Severity: Low**

```rust
// framework/helm-core/src/mem.rs:75-78
.map(|v| match u32::try_from(v) {
    Ok(word) => word,
    Err(_) => unreachable!("4-byte fetch must fit in u32"),
})
```

If a broken `MemInterface` implementation returns a value > `u32::MAX` from
a 4-byte read, this panics. The `unreachable!` is correct by contract but
fragile if a third-party implements `MemInterface` incorrectly.

**Suggested fix:** Accept as-is (contract enforcement) or replace with
`(v & 0xFFFF_FFFF) as u32` for defense-in-depth.

---

### C8. `PerfHistogram::new` does not validate sorted boundaries

**Severity: Medium**

```rust
// framework/helm-stats/src/lib.rs:83-90
pub fn new(boundaries: Vec<u64>) -> Arc<Self> {
    let n = boundaries.len() + 1;
    let buckets = (0..n).map(|_| AtomicU64::new(0)).collect();
    Arc::new(Self { buckets, boundaries })
}
```

`record()` uses `partition_point` which assumes sorted order. Unsorted
boundaries produce wrong bucket assignments with no diagnostic.

**Suggested fix:** `debug_assert!(boundaries.windows(2).all(|w| w[0] <= w[1]))`.

---

### C9. `StatsRegistry::dump_json` silently returns empty string on error

**Severity: Low**

```rust
// framework/helm-stats/src/lib.rs:137
serde_json::to_string_pretty(&map).unwrap_or_default()
```

Serialization failure (e.g. `serde_json` internal error) silently returns `""`.

---

## Completeness Issues

### P1. `AccurateTiming` is a placeholder delegating to `VirtualTiming`

The cycle-accurate pipeline model is declared as a Phase 3 deliverable and
currently wraps `VirtualTiming` with no pipeline stages.

### P2. `helm-decode` tests are stubs

The test module exists (`framework/helm-decode/src/tests/mod.rs`) but contains
no executable test cases.

### P3. `MemoryMap` recursive flattening is incomplete

`build_flat_view` does not recurse into containers or resolve aliases. Marked
with `TODO(phase-1)` at `helm-memory/src/lib.rs:131`.

### P4. `helm-plugin` API is marked legacy

The registry crate doc notes migration to probes/spy/report is ongoing.
Feature parity between old plugins and the new stack is not claimed.

---

## Software Engineering Issues

### E1. `#![allow(missing_docs)]` on most framework crate roots

`helm-memory`, `helm-stats`, `helm-event`, `helm-devices`, `helm-decode`
all suppress missing-docs. For "stable API" crates this hides undocumented
public surface.

### E2. Duplication: `ByteMem` default vs `HelmAddressSpace::read_bytes`

Both provide byte-level read/write loops with subtly different fast paths.
Changes to one may not propagate to the other.

### E3. `helm-stats` has no tests

`StatsRegistry`, `PerfCounter`, and `PerfHistogram` have no unit tests in
the crate.

### E4. `helm-plugin` builtins use `Box::leak` for global state

`StubTracer` leaks memory for process-lifetime state. Intentional but makes
testing and cleanup awkward.

---

## Architecture Issues

### A1. `helm-memory` -> `helm-devices` coupling

The memory framework crate depends on the device SDK crate. This is the only
upward dependency in the framework layer. Justified by MMIO dispatch but
worth noting for future refactoring.

### A2. `helm-timing` -> `helm-stats` dependency

`IntervalTiming` tracks stats via `helm-stats` counters. The timing model
is tied to the stats framework. If stats evolve, timing must follow.

### A3. `helm-probe` unsafe `Send`/`Sync`

`Probe<T>` implements `unsafe impl Send + Sync` based on single-threaded
simulation invariants. Future SMP would break these assumptions.

---

## Idiomatic Rust Issues

### I1. `PowerError` -- manual `Display` + `Error` instead of `thiserror`

```rust
// framework/helm-core/src/lib.rs:154-177
#[derive(Debug)]
pub enum PowerError { ... }
impl std::fmt::Display for PowerError { ... }
impl std::error::Error for PowerError {}
```

The rest of `helm-core` uses `thiserror` for `MemFault`. `PowerError` should
follow the same pattern.

### I2. `Histogram` edges not validated as sorted

See C8. Idiomatic Rust would validate invariants in the constructor.

### I3. Mutex poison handling via `expect()`

`SharedEventQueue` (`helm-event/src/lib.rs:221,228,236,243`) uses
`.expect("EventQueue mutex poisoned")`. Standard practice but panics on
poison rather than propagating. Acceptable for single-simulation process.

---

## Recommendations

### Quick Wins (< 1 hour each)

1. **Add `CB_FAULT | CB_SYSCALL` to `has_any_callbacks` bitmask** (D4)
2. **Validate histogram boundaries in constructor** (C8)
3. **Add `debug_assert!` for register index bounds in IntervalTiming** (C1)
4. **Use `thiserror` for `PowerError`** (I1)
5. **Document `cancel` semantics for non-existent IDs** (C4)
6. **Narrow `#![allow(missing_docs)]`** on framework crates (E1)

### Medium Effort (1-4 hours each)

7. **Feature-gate `MemoryMap`** behind `experimental-memmap` (D1)
8. **Add histogram tracking to `StatsRegistry`** (D6)
9. **Fix `FieldDesc::mask` for 64-bit width** (C2)
10. **Add unit tests for `helm-stats`** (E3)
11. **Implement `Drop` for `SubscriptionId`** or remove the doc claim (D5)

### Structural (> 4 hours)

12. **Complete `MemoryMap` recursive flattening** (P3)
13. **Implement `AccurateTiming`** pipeline model (P1)
14. **Write `helm-decode` test suite** (P2)
15. **Evaluate splitting `FlatMem` out of `helm-memory`** to break device coupling (D2)
