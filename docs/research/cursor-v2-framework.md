# cursor-v2 — Framework crates (`framework/`)

**Date:** 2026-04-07  
**Crates:** `helm-core`, `helm-memory`, `helm-timing`, `helm-event`, `helm-devices`, `helm-stats`, `helm-plugin`, `helm-decode`, `helm-jit`, `helm-diag`, `helm-probe`

---

## Summary

Framework code defines the **stable simulation contract**: memory, timing, events, devices SDK, stats, plugins, decode tables, JIT backends, and diagnostics hooks. The leaf crate `helm-core` stays dependency-free; complexity increases toward `helm-memory` (MMIO dispatch), `helm-jit` (unsafe + codegen), and `helm-plugin` (legacy callbacks vs newer probe stack).

**Top risks:** plugin callback bitmask omitting syscall/fault paths; `IntervalTiming` register indexing without bounds; dual memory surfaces (`FlatMem` vs `HelmAddressSpace` vs experimental `MemoryMap`); broad crate-level lint suppression (see [`cursor-cross-cutting.md`](cursor-cross-cutting.md)).

---

## Issues by taxonomy

### Design

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| FD1 | Experimental `MemoryMap` vs live `HelmAddressSpace` | Medium | Both implement `MemInterface`; easy to use the wrong one. Prefer feature-gating or crate split. |
| FD2 | `helm-memory` depends on `helm-devices` | Medium | Intentional for MMIO; prevents using the full address space in isolation. Document and optionally split `FlatMem`-only helpers. |
| FD3 | `helm-plugin` as legacy layer | Low | Docs already steer new work to probe/spy/report; avoid growing callback API without registry parity. |
| FD4 | `ByteMem` blanket impl O(n) per byte | Low–Med | Correct but slow; `HelmAddressSpace` overrides for bulk — callers must know which type they hold. |

### Correctness

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| FC1 | `IntervalTiming` `reg_ready[dst_reg as usize]` | High | Panic if register id ≥ 64; validate or use checked indexing. |
| FC2 | `FieldDesc::mask` for 64-bit field width | Medium | `1u64 << 64` is invalid; special-case full width. |
| FC3 | `EventQueue::post_after` tick wrap | Low–Med | `u64` overflow on add; use `checked_add` for safety. |
| FC4 | `FlatMem` silent zero on unmapped read | Medium | Differs from `HelmAddressSpace`; intentional for SE but easy to misuse in tests. |
| FC5 | `MemoryMap` alias/container | Low | Always faults until phase-1 resolution. |
| FC6 | `fetch32`/`fetch16` `unreachable!` on truncation | Low | Document invariants or return `MemFault`. |

### Completeness

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| FM1 | `MemoryMap` flattening / alias | Medium | TODOs in `helm-memory` for recursive flattening and alias resolution. |
| FM2 | `StatsRegistry` vs histograms | Medium | Counters only; `PerfHistogram` not integrated into registry enumeration. |
| FM3 | `helm-decode` tests | Medium | Largely stub coverage; expand as decode format evolves. |
| FM4 | `SubscriptionId` Drop / unsubscribe | Low | TODO in `event_bus.rs`; RAII not implemented. |

### Software engineering

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| FE1 | `HelmPluginRegistry::has_any_callbacks()` bitmask | High | Omits syscall/fault (and related) bits; fast-path can skip real work. |
| FE2 | Crate-level `#![allow(missing_docs)]` | Medium | Framework APIs should trend toward documented public surfaces. |
| FE3 | `EventQueue::cancel` semantics | Low | `HashSet::insert` truth value may surprise; document or tighten. |

### Software architecture

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| FA1 | Memory surface fragmentation | Medium | Three behaviors for unmapped access; see cross-cutting doc. |
| FA2 | JIT split (`helm-jit`) | Medium | Unsafe + backend-specific; keep boundary narrow (`JitBackend`) and test tiered features. |
| FA3 | Plugin vs probe migration | Low | Two observation paths; new features should not duplicate wiring (spy/report already called out in `helm-plugin` crate docs). |

### Idiomatic Rust

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| FI1 | Mixed error styles at framework boundary | Low | Prefer `thiserror` consistently (`MemFault`, `DecodeError` are good models). |
| FI2 | `const fn` bit math | Low | Full-width masks need explicit patterns (`u64::MAX >> …`). |

---

## Detailed guidelines (framework contributors)

### Memory (`helm-core`, `helm-memory`)

1. **Document which implementation backs your test or harness.** If you need faults on bad addresses, do not use `FlatMem` unless you accept silent-zero semantics or add a strict mode.
2. **Bulk access:** Prefer APIs that hit `HelmAddressSpace::read_bytes` / `write_bytes` or specialized device RAM paths; avoid relying on `ByteMem` default for large transfers.
3. **Experimental tree:** Treat `MemoryMap` as non-production until alias/container semantics are specified and tested.

### Timing (`helm-timing`)

1. **Guard register indices** before indexing fixed arrays (`reg_ready`, etc.). Tie to `TimingInsnInfo` contract: max register count and encoding.
2. **Document** what happens when interval metadata is incomplete or wrong (fallback to functional timing).

### Events (`helm-event`)

1. **Tick arithmetic:** Use checked math when combining delays with `current_tick` in long-running simulations.
2. **Cancel API:** Treat return value as “was this id newly marked cancelled,” not “did an event exist.”

### Devices SDK (`helm-devices`)

1. **`register_bank!`:** Audit field widths; full-register fields need safe mask computation.
2. **Event bus:** Either implement `Drop for SubscriptionId` or remove RAII wording from docs.

### Stats (`helm-stats`)

1. If you add a histogram, **register it** in the same place counters are registered, or document why histograms stay ad hoc until `StatsRegistry` grows.

### Plugins (`helm-plugin`)

1. **Any change to `has_any_callbacks` or callback registration** must update **all** callback kinds the engine may invoke — bitmask and tests.
2. Prefer **probe/spy/report** for new observability; extend legacy plugins only when needed for compatibility.

### Decode (`helm-decode`)

1. Treat generated/decoded tables as **security-sensitive** if fed untrusted input; add fuzzing if the parser accepts user files.

### JIT (`helm-jit`)

1. Centralize unsafe invariants in one module with comments; add tests for emitters and exit handling.
2. Keep `jit-tiered` feature boundaries clear in `Cargo.toml` and docs.

---

## Crate checklist (quick audit)

| Crate | Invariant to verify |
|-------|---------------------|
| `helm-core` | No `helm-*` deps; `MemInterface` semantics clear |
| `helm-memory` | MMIO path uses device offset only; no device self-placement |
| `helm-timing` | No panic on malformed insn metadata |
| `helm-event` | Monotonic tick ordering documented |
| `helm-devices` | `Device` impls respect interrupt pin abstraction |
| `helm-stats` | Namespaced dot-paths for counters |
| `helm-plugin` | Registry bitmask matches engine |
| `helm-decode` | Parser tests for representative `.decode` files |
| `helm-jit` | Unsafe reviewed per change; `jit-tiered` CI |
