# HELM Consolidated Refactoring Proposal

> **Date**: June 2025
> **Scope**: Full codebase analysis of all 19 crates, Python layer, and build system
> **Method**: Independent line-by-line source review of every file in the workspace

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Current Architecture](#2-current-architecture)
3. [Crate Inventory & Health Assessment](#3-crate-inventory--health-assessment)
4. [Refactoring Recommendations](#4-refactoring-recommendations)
   - R1: Unify the Translation Stack
   - R2: Merge helm-plugin-api into helm-plugins
   - R3: Deduplicate Type Registries
   - R4: Remove Dead Syscall Handler
   - R5: Wire SE-Mode Entry Point
   - R6: Resolve helm-decode's Unused Status
   - R7: Unify helm-llvm IR Types with helm-core
   - R8: Fix Python/Rust Config Mismatch
   - R9: Add Integration Test Coverage
5. [helm-llvm Integration Strategy](#5-helm-llvm-integration-strategy)
6. [Optional Consolidations (Crate Count Reduction)](#6-optional-consolidations-crate-count-reduction)
7. [Dependency Graph](#7-dependency-graph)
8. [Priority & Sequencing](#8-priority--sequencing)

---

## 1. Executive Summary

HELM (Hybrid Emulation Layer for Microarchitecture) is a well-structured Rust workspace of 19 crates (~9,500 lines of Rust) with a Python configuration layer. The codebase has excellent hygiene: zero `todo!()` / `unimplemented!()`, 52 test files, clean dependency DAG, and consistent style.

**The workspace does not need a major restructuring.** The folder layout, crate boundaries, and dependency graph are sound. What it needs are nine targeted refactorings that fix concrete problems—duplicate types, dead code paths, disconnected wiring, and the integration of the new `helm-llvm` crate.

The most impactful item is **R7: unify helm-llvm's IR types with helm-core**. The new crate defines its own `MicroOp`, `Error`, and register types that diverge from the shared IR, which will fragment the pipeline if left unaddressed.

### What This Document Supersedes

This document replaces:
- `RESTRUCTURE_PROPOSAL.md`
- `CRATE_CONSOLIDATION_PROPOSAL.md`
- `LLVM_IR_TCG_INTEGRATION.md`
- `LLVM_IR_UNIFIED_MODEL.md`
- `GEM5_SALAM_ANALYSIS.md`

Those documents contain useful research and design rationale; this document is the single, actionable plan.

---

## 2. Current Architecture

### Crate Map (19 crates)

```
Foundation:
  helm-core         — IR (MicroOp, Opcode), types (Addr, RegId, Cycle), config, events, errors

ISA & Translation:
  helm-isa           — ISA frontends (ARM 16 files, RISC-V, x86); Aarch64Cpu with step()
  helm-decode        — QEMU .decode file parser (fields, formats, patterns, trees)
  helm-tcg           — QEMU-style TCG IR (TcgOp, TcgContext, TcgBlock)
  helm-translate     — Dynamic binary translation (Translator, TranslatedBlock, TranslationCache)

LLVM Accelerator Frontend:
  helm-llvm          — LLVM IR parser, instruction scheduler, accelerator model (NEW)

Microarchitecture:
  helm-pipeline      — OOO pipeline stages, ROB, rename, dispatch
  helm-memory        — Cache hierarchy, TLB, coherence, address space

Platform:
  helm-device        — Bus, MMIO, IRQ controller
  helm-timing        — Timing models, event queue, temporal sampling
  helm-syscall       — Linux syscall emulation (Aarch64, fd_table)
  helm-object        — QOM-style object model, property system, type registry
  helm-stats         — Statistics collector, counters

Engine & Orchestration:
  helm-engine        — Simulation, CoreSim, SE-mode runner, ELF loader

Plugin System:
  helm-plugin-api    — Stable plugin API (HelmComponent, ComponentRegistry)
  helm-plugins       — Plugin framework + built-ins (trace, memory plugins)

Frontends:
  helm-cli           — CLI entry point (clap)
  helm-python        — PyO3 bindings (maturin)
  helm-systemc       — SystemC co-simulation bridge
```

### Dependency Flow

```
helm-core
  ├── helm-object, helm-stats, helm-decode, helm-tcg
  ├── helm-isa (→ helm-core)
  ├── helm-translate (→ helm-core, helm-isa)
  ├── helm-llvm (→ helm-core)             ← NEW
  ├── helm-pipeline (→ helm-core)
  ├── helm-memory (→ helm-core)
  ├── helm-device (→ helm-core, helm-object)
  ├── helm-timing (→ helm-core)
  ├── helm-syscall (→ helm-core, libc)
  ├── helm-plugin-api (→ helm-core)
  ├── helm-plugins (→ helm-core, helm-plugin-api)
  └── helm-engine (→ helm-core, helm-isa, helm-pipeline, helm-memory,
                     helm-translate, helm-syscall, helm-object, helm-stats)
        ├── helm-cli (→ helm-engine)
        ├── helm-python (→ helm-engine, pyo3)
        └── helm-systemc (→ helm-engine)
```

---

## 3. Crate Inventory & Health Assessment

| Crate | Lines | Status | Issues |
|---|---:|---|---|
| helm-core | ~320 | Healthy | Foundational, no issues |
| helm-isa | ~2,362 | Healthy | ARM frontend most mature |
| helm-decode | ~779 | **Unused** | No crate depends on it; ISA frontends do hardcoded matching |
| helm-tcg | ~395 | **Orphaned** | No crate depends on it; helm-translate uses helm-core::MicroOp directly |
| helm-translate | ~208 | Healthy | Small but functional |
| helm-llvm | ~1,200+ | **New, has issues** | Duplicate MicroOp/Error types; see R7 |
| helm-pipeline | ~850 | Healthy | Clean OOO implementation |
| helm-memory | ~900 | Healthy | Cache, TLB, coherence |
| helm-device | ~400 | Healthy | Bus, MMIO, IRQ |
| helm-timing | ~500 | Healthy | Event queue, sampling |
| helm-syscall | ~600 | Has dead code | generic.rs `SyscallHandler` is unused; see R4 |
| helm-object | ~350 | Healthy | `TypeRegistry` duplicated in helm-plugin-api; see R3 |
| helm-stats | ~250 | Healthy | Small utility |
| helm-engine | ~800 | Has wiring bug | `Simulation::run_se()` is a stub; see R5 |
| helm-plugin-api | ~230 | **Premature** | No external consumers; duplicates helm-object; see R2 |
| helm-plugins | ~988 | Healthy | Good plugin framework |
| helm-cli | ~100 | Healthy | Thin clap wrapper |
| helm-python | ~200 | Healthy | PyO3 bindings |
| helm-systemc | ~150 | Healthy | SystemC bridge |

---

## 4. Refactoring Recommendations

### R1: Unify the Translation Stack

**Priority: HIGH** | **Effort: Medium** | **Risk: Low**

#### Problem

Three crates serve the translation function but are disconnected:

- `helm-tcg` (395 lines) defines `TcgOp`, `TcgContext`, `TcgBlock` — nothing depends on it
- `helm-translate` (208 lines) defines `Translator`, `TranslatedBlock`, `TranslationCache` — uses `helm-core::MicroOp`, **not** `TcgOp`
- `helm-decode` (779 lines) defines a QEMU .decode parser — nothing depends on it

`helm-translate` completely ignores `helm-tcg`. The TCG IR exists but plays no role in the actual translation pipeline.

#### Proposed Fix

Merge `helm-tcg` into `helm-translate` as a submodule:

```
crates/helm-translate/src/
├── lib.rs
├── translator.rs      (existing)
├── block.rs           (existing TranslatedBlock)
├── cache.rs           (existing TranslationCache)
└── tcg/               (moved from helm-tcg)
    ├── mod.rs
    ├── ir.rs           (TcgOp, TcgTemp)
    ├── context.rs      (TcgContext)
    └── block.rs        (TcgBlock)
```

Wire `Translator` to produce `TcgBlock` → lower to `Vec<MicroOp>`, or document the intended two-phase pipeline (ISA → TCG → MicroOp) and connect them.

Delete `crates/helm-tcg/` and remove it from workspace members.

#### What Not To Do

Do **not** merge `helm-decode` into `helm-translate` yet. It has value as a QEMU-compatible .decode parser for future use, but forcing it into the dependency chain of helm-translate adds 779 lines of unused code. Handle `helm-decode` separately in R6.

---

### R2: Merge helm-plugin-api into helm-plugins

**Priority: Medium** | **Effort: Low** | **Risk: Low**

#### Problem

`helm-plugin-api` (230 lines) defines `HelmComponent` and `ComponentRegistry` for external plugin authors. However:
- No external consumer exists
- `HelmComponent` is never implemented by anything
- `ComponentRegistry` duplicates functionality in `helm-object::TypeRegistry`
- `helm-plugins` already defines `HelmPlugin` trait and `PluginRegistry` which are the actual plugin interface

Two separate "plugin registries" existing in parallel creates confusion about which is canonical.

#### Proposed Fix

Move `helm-plugin-api`'s `ComponentRegistry::register()` / `create()` functionality into `helm-plugins`. Keep a `pub mod api` submodule inside `helm-plugins` if you want a clean separation for future external consumers:

```
crates/helm-plugins/src/
├── lib.rs
├── plugin.rs          (HelmPlugin trait — the real API)
├── registry.rs        (PluginRegistry)
├── api/               (moved from helm-plugin-api, adapted)
│   ├── mod.rs
│   └── component.rs   (HelmComponent, if still wanted)
├── trace/
└── memory/
```

Delete `crates/helm-plugin-api/`.

---

### R3: Deduplicate Type Registries

**Priority: Medium** | **Effort: Low** | **Risk: Low**

#### Problem

Two type registries exist:
- `helm-object::TypeRegistry` — QOM-style, used by `helm-device`
- `helm-plugin-api::ComponentRegistry` — plugin components, unused

Both store type name → constructor mappings with nearly identical APIs.

#### Proposed Fix

After R2 merges `helm-plugin-api` into `helm-plugins`, make `helm-plugins` depend on `helm-object` and use `TypeRegistry` as its underlying component store instead of re-implementing. Alternatively, if `helm-plugins` needs different semantics (e.g., dynamic library loading), document why and keep the separation.

---

### R4: Remove Dead Syscall Handler

**Priority: Medium** | **Effort: Low** | **Risk: None**

#### Problem

`crates/helm-syscall/src/os/linux/generic.rs` defines a `SyscallHandler` that handles ~4 syscalls without libc. Nothing references it. The real handler is `Aarch64SyscallHandler` in `handler.rs` (~50 syscalls with libc passthrough).

#### Proposed Fix

Delete `generic.rs` and its `mod generic;` declaration. If a "stub" handler is needed for tests, add it behind `#[cfg(test)]`.

---

### R5: Wire SE-Mode Entry Point

**Priority: HIGH** | **Effort: Low** | **Risk: Low**

#### Problem

`helm-engine::Simulation::run_se()` logs "SE mode: fast functional emulation (stub)" and returns immediately. The **real** SE runner (`se::linux::run_aarch64_se()`) exists in the same crate but is never called from `run_se()`.

This means anyone calling `Simulation::run_se()` gets nothing.

#### Proposed Fix

```rust
// helm-engine/src/sim.rs
pub fn run_se(&mut self) -> HelmResult<()> {
    use crate::se::linux::run_aarch64_se;
    run_aarch64_se(&self.config)
}
```

If multi-ISA SE is intended, add a match on `config.isa`:

```rust
pub fn run_se(&mut self) -> HelmResult<()> {
    match self.config.isa {
        IsaKind::Arm64 => crate::se::linux::run_aarch64_se(&self.config),
        _ => Err(HelmError::Unsupported(format!("SE mode for {:?}", self.config.isa))),
    }
}
```

---

### R6: Resolve helm-decode's Unused Status

**Priority: Low** | **Effort: Low** | **Risk: None**

#### Problem

`helm-decode` parses QEMU `.decode` files into structured decode trees. It works correctly but no crate consumes it. The ISA frontends in `helm-isa` use hardcoded pattern matching instead.

QEMU `.decode` files *are* checked in under `crates/helm-isa/src/arm/decode_files/qemu/`, but they're inert assets.

#### Options

1. **Keep and document** — Add a `NOTE.md` to `helm-decode` explaining it's a future dependency for machine-generated decoders, and mark it `publish = false` in Cargo.toml.
2. **Wire it in** — Have `helm-isa`'s ARM frontend use `helm-decode` to parse the `.decode` files at build time (via `build.rs`) to generate match arms. This is the ideal end-state but significant work.
3. **Remove** — Delete the crate if there's no plan to use it. The QEMU `.decode` files in `helm-isa` can stay as reference documentation.

**Recommendation**: Option 1 for now. Wire it in later when the ARM frontend matures.

---

### R7: Unify helm-llvm IR Types with helm-core

**Priority: CRITICAL** | **Effort: Medium** | **Risk: Medium**

#### Problem

`helm-llvm` defines its own parallel type system that diverges from the shared IR:

| Concept | helm-core | helm-llvm |
|---|---|---|
| Micro-operation | `ir::MicroOp` (struct with `Opcode` enum) | `micro_op::MicroOp` (enum with data variants) |
| Register ID | `types::RegId = u16` | `micro_op::PhysReg = usize` |
| Error type | `error::HelmError` / `HelmResult<T>` | `error::Error` / `Result<T>` |
| Opcode families | `Opcode::{IntAlu, IntMul, FpAlu, Load, Store, Branch, …}` | `MicroOp::{IntAdd, IntSub, IntMul, FPAdd, Load, Store, Branch, …}` |

If `helm-llvm`'s `MicroOp` is fed into `helm-pipeline`, it won't work—`helm-pipeline` expects `helm_core::ir::MicroOp`.

The `helm-llvm::micro_op::MicroOp` enum is richer (per-operation variants like `IntAdd`, `IntSub`, `Xor`, `Shl`, etc.) and carries source/dest registers inline. The `helm-core::ir::MicroOp` is a flat struct with an `Opcode` tag and `Vec<RegId>` for sources. Both designs have merits.

#### Proposed Fix

**Option A (Recommended): Extend helm-core's MicroOp to be the unified type**

Expand `helm_core::ir::Opcode` to cover the finer-grained categories that `helm-llvm` needs:

```rust
// helm-core/src/ir.rs — expanded Opcode
pub enum Opcode {
    IntAdd, IntSub, IntMul, IntDiv,
    FpAdd, FpSub, FpMul, FpDiv,
    LogicAnd, LogicOr, LogicXor,
    Shift,  // or ShiftLeft, ShiftRight
    Load, Store,
    Branch, CondBranch,
    Compare,
    Move, LoadImm,
    Conversion,  // trunc, zext, sext
    Syscall, Nop, Fence,
    /// GEP address calculation (LLVM-specific but useful generically)
    AddrCalc,
    Other(u16),
}
```

Then make `helm-llvm` produce `helm_core::ir::MicroOp` directly:

```rust
// helm-llvm/src/micro_op.rs — rewrite to produce helm-core MicroOps
use helm_core::ir::{MicroOp, Opcode, MicroOpFlags};
use helm_core::types::RegId;

pub fn llvm_to_micro_ops(inst: &LLVMInstruction, ctx: &mut ConversionContext) -> Vec<MicroOp> {
    match inst {
        LLVMInstruction::Add(dest, src1, src2) => vec![MicroOp {
            guest_pc: 0,
            opcode: Opcode::IntAdd,
            sources: vec![ctx.reg(src1), ctx.reg(src2)],
            dest: Some(ctx.reg(dest)),
            immediate: None,
            flags: MicroOpFlags::default(),
        }],
        // ...
    }
}
```

**Option B: Trait-based bridge**

Keep both `MicroOp` types but add `impl From<helm_llvm::MicroOp> for helm_core::ir::MicroOp`. This is less clean but lower-churn.

**Error type fix**: Have `helm-llvm` use `HelmError` (add an `Llvm(String)` variant) or wrap its `Error` with `impl From<helm_llvm::Error> for HelmError`.

**Register type fix**: Use `RegId = u16` everywhere, or change `helm-core` to `RegId = u32` if 16 bits is too narrow for LLVM's virtual register numbering.

---

### R8: Fix Python/Rust Config Mismatch

**Priority: Medium** | **Effort: Low** | **Risk: Low**

#### Problem

Python `Platform.to_dict()` emits fields like `"num_cores"`, `"l1_size"`, `"pipeline_width"` that don't exist on Rust's `PlatformConfig`, `CacheConfig`, or `CoreConfig`. The Rust side expects `rob_size`, `num_entries`, etc. If a user constructs a platform in Python and passes it to Rust (via `helm-python` PyO3 bridge), the config will silently lose fields or fail.

#### Proposed Fix

Audit `python/helm/platform.py` and `python/helm/core.py`. Align field names 1:1 with Rust's `PlatformConfig`/`CoreConfig`/`CacheConfig`/`MemoryConfig` in `helm-core/src/config.rs`. Add a round-trip test: Python `to_dict()` → JSON → Rust `serde_json::from_str::<PlatformConfig>()` → assert success.

---

### R9: Add Integration Test Coverage

**Priority: Medium** | **Effort: Medium** | **Risk: None**

#### Problem

The existing test suite is mostly unit tests inside each crate. There is one e2e test (`helm-engine/tests/e2e_aarch64.rs`) that exercises the full ISA → pipeline → memory path. Coverage gaps:
- No cross-crate test for LLVM IR → MicroOp → pipeline
- No test that `Simulation::run_se()` actually runs code
- No test for Python config → Rust pipeline round-trip

#### Proposed Fix

Add a `tests/` directory at workspace root (or in `helm-engine/tests/`) with:
1. `e2e_llvm_accel.rs` — Parse a simple LLVM IR function, generate MicroOps, run through pipeline
2. `e2e_se_arm.rs` — Call `Simulation::run_se()` with a minimal AArch64 ELF
3. `config_roundtrip.rs` — Python dict → JSON → Rust PlatformConfig → JSON → compare

---

## 5. helm-llvm Integration Strategy

### Where helm-llvm Fits

`helm-llvm` is a **frontend** analogous to `helm-isa`. Both produce `MicroOp`s consumed by `helm-pipeline`:

```
CPU path:       Binary → helm-isa → helm-translate → helm-core::MicroOp → helm-pipeline
Accelerator:    LLVM IR → helm-llvm ───────────────→ helm-core::MicroOp → helm-pipeline
```

The key architectural insight (documented in LLVM_IR_TCG_INTEGRATION.md) is correct: TCG and LLVM IR serve different purposes, and both should map directly to MicroOps. There is no need for LLVM IR → TCG translation.

### Current State of helm-llvm

The crate is now complete (all 7 files exist):

| File | Lines | Status |
|---|---:|---|
| `lib.rs` | 53 | Module declarations and re-exports |
| `ir.rs` | ~368 | LLVM IR types (LLVMModule, LLVMFunction, LLVMBasicBlock, LLVMInstruction, LLVMValue, LLVMType). `from_string()` has a text parser stub. |
| `micro_op.rs` | ~352 | Local `MicroOp` enum + `ConversionContext` + `llvm_to_micro_ops()`. **Uses divergent types (see R7).** |
| `error.rs` | ~40 | Local `Error` / `Result` types. **Diverges from HelmError.** |
| `accelerator.rs` | ~220 | `Accelerator` + `AcceleratorBuilder` (gem5-SALAM style builder pattern) |
| `scheduler.rs` | ~250 | `InstructionScheduler` with reservation table, compute/load/store queues, dependency tracking |
| `functional_units.rs` | ~250 | `FunctionalUnitPool` with per-type configurable units, pipelined/non-pipelined modes |

### Dependencies

```toml
[dependencies]
helm-core = { workspace = true }
thiserror = { workspace = true }
log = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
# llvm-sys and inkwell are commented out (for future full LLVM IR parsing)
```

### Immediate Actions for helm-llvm

1. **R7** — Unify MicroOp/Error/RegId types with helm-core (critical)
2. Add `helm-llvm` to `helm-engine`'s dependencies and create an `ExecutionMode::Accelerator` variant
3. Add a Python `helm.LLVMAccelerator` class in `python/helm/` that wraps `AcceleratorBuilder`
4. Add unit tests for `llvm_to_micro_ops()` and `InstructionScheduler::tick()`

### Future Work (Not Blocking)

- Uncomment `llvm-sys` / `inkwell` for real LLVM bitcode parsing (currently text-only stub)
- Add scratchpad memory support to `helm-memory`
- Add DMA controller to `helm-device`
- Wire `helm-llvm` scheduler to `helm-pipeline` for unified microarchitecture simulation

---

## 6. Optional Consolidations (Crate Count Reduction)

The previous `CRATE_CONSOLIDATION_PROPOSAL.md` suggested reducing 18 → 12 crates. With `helm-llvm` added (19 crates), a similar consolidation would yield ~13.

**These are optional.** The current structure is fine for a project this size. Premature merging can hurt modularity. Consider these only if compile times or cognitive overhead become a problem.

### If You Do Consolidate

| Merge | Into | Rationale |
|---|---|---|
| helm-tcg | helm-translate | Same domain, tcg is unused standalone (R1) |
| helm-plugin-api | helm-plugins | No external consumers (R2) |
| helm-object + helm-stats | helm-core | Small utilities, always pulled in together |
| helm-device + helm-timing | helm-platform (new) | Both model platform-level concerns |

This would reduce 19 → 14 crates. Further merging `helm-syscall` into `helm-platform` gets to 13.

### If You Don't Consolidate

That's fine. Fix R1 through R9 and the codebase is in excellent shape. The 19-crate layout has clean boundaries and a clear dependency DAG. Crate count is not a problem at this project's scale.

---

## 7. Dependency Graph

After applying R1 (merge tcg → translate) and R2 (merge plugin-api → plugins):

```
                    helm-core
                   /    |     \
                  /     |      \
         helm-object  helm-stats  helm-decode (unused, keep for now)
              |
         helm-device
              |
         helm-timing
                  \
                   \
    helm-isa    helm-translate (includes tcg)    helm-llvm
        \              |                          /
         \             |                         /
          helm-pipeline    helm-memory    helm-syscall
                \           |           /
                 \          |          /
                  helm-engine
                 /     |      \
                /      |       \
          helm-cli  helm-python  helm-systemc
                        |
                  helm-plugins (includes plugin-api)
```

---

## 8. Priority & Sequencing

### Phase 1: Critical Path (Do First)

| # | Item | Effort | Impact |
|---|---|---|---|
| R7 | Unify helm-llvm types with helm-core | Medium | Prevents type fragmentation across the entire pipeline |
| R5 | Wire Simulation::run_se() | Low | Fixes a broken public API |

### Phase 2: Code Health (Do Soon)

| # | Item | Effort | Impact |
|---|---|---|---|
| R1 | Merge helm-tcg → helm-translate | Medium | Eliminates orphaned crate, clarifies translation pipeline |
| R2 | Merge helm-plugin-api → helm-plugins | Low | Removes premature abstraction |
| R4 | Delete dead syscall handler | Low | Removes dead code |
| R3 | Deduplicate registries | Low | Reduces confusion |

### Phase 3: Polish (Do When Convenient)

| # | Item | Effort | Impact |
|---|---|---|---|
| R8 | Fix Python/Rust config mismatch | Low | Prevents silent data loss in Python → Rust path |
| R6 | Document helm-decode unused status | Low | Clarifies intent |
| R9 | Add integration tests | Medium | Catches cross-crate regressions |

---

*This document is the single source of truth for HELM refactoring decisions. Previous analysis documents (RESTRUCTURE_PROPOSAL.md, CRATE_CONSOLIDATION_PROPOSAL.md, LLVM_IR_TCG_INTEGRATION.md, LLVM_IR_UNIFIED_MODEL.md, GEM5_SALAM_ANALYSIS.md) contain background research and can be archived.*
