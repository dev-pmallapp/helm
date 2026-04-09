# cursor-cross-cutting -- Codebase Audit

Date: 2026-04-07

## Summary

This document covers systemic patterns and architectural issues that span
multiple domains in the helm-ng workspace. These are not localized to any
single crate but reflect design choices, conventions, or gaps that recur
across the debug, framework, hw, runtime, and python layers. The most
impactful cross-cutting concerns are: broad compiler suppression that hides
incomplete code, inconsistent error-type strategy, memory-surface
fragmentation, the engine's heavy dependency fan-out, and the pattern of
using `unreachable!` / `unwrap` / `expect` in paths where graceful
degradation would be more robust.

---

## 1. Broad `#![allow(...)]` Suppression on Crate Roots

**Severity: High** | **Affected: 15+ crates**

Nearly every crate in the workspace suppresses `missing_docs` at the crate
level. Several also suppress `dead_code`, `clippy::pedantic`, and multiple
specific clippy lints. This means the compiler cannot flag:

- **Undocumented public API** (missing_docs) -- affects all framework crates
- **Unreachable code / unused fields** (dead_code) -- affects `helm-engine`
- **Potential bugs** caught by clippy pedantic -- affects `helm-arch`, `helm-engine`

### Where it appears

| Crate | Suppressed lints |
|-------|-----------------|
| `helm-engine` | `missing_docs, dead_code, clippy::pedantic, clippy::collapsible_match, clippy::large_enum_variant, clippy::needless_range_loop, clippy::new_without_default, clippy::nonminimal_bool, clippy::ptr_arg, clippy::useless_vec` |
| `helm-arch` | `missing_docs, clippy::pedantic` |
| `helm-python` | `missing_docs, clippy::redundant_closure, clippy::semicolon_if_nothing_returned, clippy::unused_self, clippy::useless_conversion` |
| `helm-memory` | `missing_docs` |
| `helm-stats` | `missing_docs` |
| `helm-event` | `missing_docs` |
| `helm-spy` | `missing_docs, unsafe_code` |
| `helm-hw-intc` | `missing_docs` |
| `helm-hw-iommu` | `missing_docs` |

### Recommendation

1. Remove `dead_code` from `helm-engine` -- prune or `cfg`-gate unused code
2. Remove `clippy::pedantic` from `helm-arch` and `helm-engine` -- address
   the warnings or selectively allow specific lints at the item level
3. Keep `missing_docs` suppressed on internal crates (`helm-spy`, hw crates)
   but enable it on "stable API" framework crates (`helm-core`, `helm-memory`,
   `helm-timing`, `helm-event`)
4. Narrow `unsafe_code` in `helm-spy` to the specific `trace_ring` module

---

## 2. Inconsistent Error Type Strategy

**Severity: Medium** | **Affected: 6+ crates**

The codebase uses three different approaches to error types:

| Approach | Where used | Example |
|----------|-----------|---------|
| `thiserror` derive | `helm-core::MemFault`, `helm-arch::DecodeError` | `#[derive(Error)]` |
| Manual `Display` + `Error` | `helm-core::PowerError`, `helm-platform::PlatformError` | Hand-written `impl Display` |
| `String` / `&'static str` | `helm-hw-pci` construction functions, some engine paths | `Result<_, String>` |

This inconsistency means:
- Some errors compose with `?` naturally, others require `.map_err()`
- Library users cannot rely on a uniform error pattern
- New contributors pick whichever style they see first

### Recommendation

Standardize on `thiserror` for all error enums. It produces the same code
as manual impls but with less boilerplate. Specific fixes:

- `PowerError` in `helm-core/src/lib.rs:154-177`
- `PlatformError` in `helm-platform/src/lib.rs:44-71`
- `SinkError` in `helm-report` (already correct -- uses `Display` + `Error`)
- PCI string errors in `helm-hw-pci`

---

## 3. Memory Surface Fragmentation

**Severity: Medium** | **Affected: framework, runtime, hw**

Three distinct memory surfaces exist, each with different semantics:

| Surface | Location | Behavior on unmapped read |
|---------|----------|--------------------------|
| `FlatMem` | `helm-memory/src/flat_mem.rs` | Returns `0` (silent) |
| `HelmAddressSpace` | `helm-memory/src/address_space.rs` | Returns `MemFault::AccessFault` |
| `MemoryMap` (experimental) | `helm-memory/src/lib.rs` | Returns `MemFault::AccessFault` |

All three implement `MemInterface`, so code that accepts `&mut dyn MemInterface`
behaves differently depending on which surface backs it. SE mode uses `FlatMem`
(silent zeros), FS mode uses `HelmAddressSpace` (faults). Device models tested
against one surface may fail on the other.

Additionally, the `ByteMem` blanket impl over `MemInterface` does byte-by-byte
access, while `HelmAddressSpace` has its own bulk `read_bytes`/`write_bytes`
with aligned RAM fast paths. Code that goes through the trait gets O(n) per
byte; code that knows it has `HelmAddressSpace` gets the fast path.

### Recommendation

1. Document the semantic contract differences prominently in `MemInterface`
   trait docs
2. Add a `strict` flag to `FlatMem` to optionally fault on unmapped access
3. Consider a `BulkMemInterface` trait with an optimized `read_bytes` default
   that `HelmAddressSpace` overrides, to avoid the byte-by-byte fallback
4. Long-term: converge on the `MemoryMap` region tree once alias/container
   resolution is complete

---

## 4. Engine Dependency Fan-Out

**Severity: Medium** | **Affected: helm-engine, helm-python**

`helm-engine` directly depends on:
- All framework crates (core, memory, timing, event, devices, stats, plugin)
- All HW crates (intc, pci, virtio -- through `platform/arm_virt.rs`)
- `helm-debug`, `helm-platform`, `helm-arch`
- Optionally: `helm-jit`

`helm-python` adds PCI and VirtIO imports through `instantiate.rs`.

This means:
- Any change to a HW crate triggers recompilation of the engine + Python module
- The engine is difficult to test in isolation from device models
- Adding a new device type requires touching `helm-python/src/instantiate.rs`

### Recommendation

1. Feature-gate HW crate dependencies in `helm-engine` (e.g.,
   `arm-virt = ["helm-hw-intc", "helm-hw-pci", ...]`)
2. Move device construction out of `helm-python/src/instantiate.rs` into the
   engine or platform layer
3. Feature-gate `helm-debug` in `helm-engine`
4. Consider a `helm-machine` crate that owns platform realization, sitting
   between `helm-platform` (metadata) and `helm-engine` (simulation kernel)

---

## 5. `unreachable!` / `unwrap` / `expect` in Guest-Facing Paths

**Severity: High** | **Affected: helm-arch, helm-engine, hw crates**

The project uses three panic-inducing patterns in paths that can be triggered
by guest behavior:

### Pattern A: `unreachable!` in decode/execute dispatch

Seven execute modules use `unreachable!("wrong dispatch to ...")`. If a decode
table bug routes an opcode to the wrong group, the simulator panics instead of
raising a guest `IllegalInstruction` fault.

- `runtime/helm-arch/src/aarch64/execute/ldst.rs:593`
- `runtime/helm-arch/src/aarch64/execute/dp.rs:472`
- `runtime/helm-arch/src/aarch64/execute/branch.rs:271`
- `runtime/helm-arch/src/aarch64/execute/simd.rs:486`
- `runtime/helm-arch/src/aarch64/execute/sysreg.rs:134`
- `runtime/helm-arch/src/aarch64/execute/mul_div.rs:120`
- `runtime/helm-arch/src/aarch64/execute/fp.rs:206`

### Pattern B: `unwrap()` on guest memory access

VirtIO virtqueue helpers panic on `MemFault`:
- `hw/helm-hw-virtio/src/proto/virtqueue.rs:88-98`

SMMU hides faults with `unwrap_or(0)`:
- `hw/helm-hw-iommu/src/smmu/mod.rs:399-401`

### Pattern C: `expect()` on internal state that depends on configuration

- `self.riscv().expect("riscv runtime missing")` at `lib.rs:596`
- `self.session.aarch64().and_then(Aarch64Core::state).expect("a64_state missing")`

### Recommendation

1. Replace `unreachable!` with `IllegalInstruction` returns in execute dispatch
2. Make virtqueue helpers return `Result` and propagate faults
3. Replace SMMU `unwrap_or(0)` with proper fault propagation
4. Replace `expect` on ISA state with `Option` returns or match on active ISA

---

## 6. Mutex Poison Handling

**Severity: Low** | **Affected: framework, runtime, hw**

The standard pattern throughout the codebase is:

```rust
mutex.lock().expect("... mutex poisoned")
// or
mutex.lock().unwrap()
```

This is acceptable for a single-process simulator where a poisoned mutex
indicates a previous panic (which should already have aborted). However,
in long-running simulation sessions or when embedding the engine as a
library, poison propagation would be more robust.

### Affected locations

- `SharedEventQueue` in `helm-event/src/lib.rs`
- `SharedDmaPort` in `helm-memory/src/dma.rs`
- `helm-diag` global context
- PCI bus state in `helm-hw-pci`
- VirtIO PCI transport
- Branch predictor in `helm-spy`

### Recommendation

Accept the current pattern for now. If the engine is ever embedded as a
library where caller code may panic in callbacks, consider switching to
`Mutex::lock().unwrap_or_else(|poison| poison.into_inner())` to recover
from poison.

---

## 7. Test Coverage Gaps

**Severity: Medium** | **Affected: multiple crates**

| Crate | Gap |
|-------|-----|
| `helm-decode` | Only stub test module; no executable tests |
| `helm-stats` | No unit tests for `StatsRegistry`, `PerfCounter`, `PerfHistogram` |
| `helm-hw-char` / `helm-hw-timer` / `helm-hw-rtc` | Unit tests only; no integration tests through `HelmAddressSpace` |
| `helm-report` | `OnCounter` trigger untested; CSV column semantics not validated |
| `helm-spy` | `snapshot()` does not round-trip `fault_history`; no release-vs-debug probe test |
| `helm-python/instantiate.rs` | PCI/VirtIO device attachment path not integration-tested |

### Recommendation

Prioritize by risk:
1. Add `helm-stats` unit tests (trivial)
2. Add CSV column-order semantic test in `helm-report`
3. Add integration tests for devices through `HelmAddressSpace` (medium)
4. Write `helm-decode` tests (aligns with decode-tree work)

---

## 8. MMIO `size` Parameter Systemically Ignored

**Severity: High** | **Affected: all hw crates**

The `Device::read/write` trait provides an access `size` parameter (1, 2, 4,
or 8 bytes), but nearly every device ignores it. This is documented in detail
in `cursor-hw.md` but rises to a cross-cutting concern because:

- It affects **every** device in the `hw/` layer
- Guest drivers performing narrow accesses get incorrect results
- PCI config space (where byte-enables matter) is affected

### Recommendation

1. Add a helper function in `helm-devices` that masks read values and write
   values to the requested width, usable by all devices
2. Start enforcement with PCI (most impactful)
3. Document per-device whether `size` is intentionally ignored (ARM MMIO
   devices often return 32-bit values regardless)

---

## 9. `cfg(debug_assertions)` as Feature Gate

**Severity: Medium** | **Affected: helm-spy, helm-probe, helm-arch**

Probe wiring (`HelmSpy::subscribe`, `ProbePluginBridge::wire`) is gated on
`cfg(debug_assertions)`. This means:
- Release builds cannot collect performance data
- Tests pass in debug but behavior differs in release
- Users cannot opt in to profiling in release builds

### Recommendation

Replace with a Cargo feature (e.g., `instrumentation`) that is enabled by
default in dev profiles and opt-in for release profiling. This gives users
explicit control.

---

## 10. Documentation Drift

**Severity: Low** | **Affected: docs, Cargo.toml descriptions**

| Location | Issue |
|----------|-------|
| `helm-hw-intc/Cargo.toml` | Says "future PLIC" but ships GICv3 |
| `helm-jit` lib docs | Describe AArch64-only; RISC-V paths exist |
| `helm-hw-virtio/src/blk.rs` | Doc examples reference non-existent module paths |
| `helm-memory/src/lib.rs` | `MemoryMap` described as "experimental" but implements `MemInterface` like a production type |
| `ARCHITECTURE.md` | Phased build plan shows Phase 0/1/2/3 status that may lag behind actual state |

### Recommendation

Add a CI step that greps for `TODO`, `FIXME`, and stale descriptions in
`Cargo.toml` files and flags drift during PR review.

---

## Priority Matrix

### Critical (fix before next release)

| ID | Issue | Domain |
|----|-------|--------|
| HW-C1 | GICv2 GICC RPR wrong for private IRQs | hw |
| HW-C3 | Virtqueue `unwrap()` on guest MemFault | hw |
| RT-C1 | Fault plugin panics when no RISC-V state | runtime |
| RT-D2 | `state()` only exposes vCPU 0 | runtime |

### High (address in next sprint)

| ID | Issue | Domain |
|----|-------|--------|
| CC-5A | `unreachable!` in 7 execute modules | runtime |
| CC-8 | MMIO `size` ignored systemically | hw |
| FW-D4 | Plugin `has_any_callbacks` omits syscall/fault | framework |
| HW-C2 | SMMU faults become zero | hw |
| HW-C4 | IOMMU TLB ignores ASID | hw |
| DB-C1 | CSV column order vs header | debug |
| CC-1 | `dead_code` allow hides unused code | all |

### Medium (address over coming weeks)

| ID | Issue | Domain |
|----|-------|--------|
| CC-2 | Inconsistent error types | all |
| CC-3 | Memory surface fragmentation | framework |
| CC-4 | Engine dependency fan-out | runtime/python |
| CC-9 | `cfg(debug_assertions)` as feature gate | debug/framework |
| FW-C1 | IntervalTiming register index bounds | framework |
| FW-D6 | StatsRegistry no histogram tracking | framework |
| RT-C3 | JIT FS vCPU mismatch | runtime |
| RT-C4 | Casp unimplemented | runtime |

### Low (track for future)

| ID | Issue | Domain |
|----|-------|--------|
| CC-6 | Mutex poison handling | all |
| CC-10 | Documentation drift | all |
| FW-P1 | AccurateTiming placeholder | framework |
| FW-P2 | helm-decode no tests | framework |
| RT-P1 | AArch32 unimplemented | runtime |

---

## Cross-Reference to Per-Domain Documents

- `docs/research/cursor-debug.md` -- helm-spy and helm-report
- `docs/research/cursor-framework.md` -- all framework/ crates
- `docs/research/cursor-hw.md` -- all hw/ device crates
- `docs/research/cursor-runtime.md` -- helm-arch and helm-engine
- `docs/research/cursor-platform-python.md` -- helm-platform and helm-python
