# cursor-v2 — Runtime crates (`runtime/` except Python)

**Date:** 2026-04-07  
**Crates:** `helm-arch`, `helm-engine`, `helm-debug`, `helm-platform`, `helm-cli`

---

## Resolution Status (2026-04-18)

- [x] RD2 / RA2 — Active vCPU tracking done; `state()` / `state_mut()` now use explicit vCPU cursor instead of hardcoded vCPU 0
- [x] RC2 — JIT FS context fixed; `JitFsContext` now threads explicit vCPU id and validates it matches the active hart
- [x] RD5 — Timer check interval truncation fixed; `nearest as u32` replaced with saturating/checked arithmetic
- [x] RC5 — Dispatch table fallback done; `idx.min(319)` silent clamp replaced with bounds check and illegal-opcode handler
- [x] RC3 — CASP behavior implemented; atomic pair semantics now correct for lock-free code
- [x] RC1 / RA3 — Fault plugin ArchContext made safe; builds context from session + active ISA instead of `expect`-based fallback chain
- [ ] RD1 — `helm-engine` monolith (`lib.rs` 3000+ lines) not yet split
- [ ] RD3 — Thread-local probe pointer in `helm-arch` still single-threaded only
- [ ] RC4 — `unreachable!` in execute dispatch still present in 7 modules; not yet replaced with `IllegalInstruction`
- [ ] RC6 — `MAX_INSTRUMENTED_ACCESSES` still drops wide SIMD access lists
- [ ] RC7 — SE legacy syscall stub `write` still returns count without copying guest memory
- [ ] RM2 — SIMD / crypto stubs still incomplete
- [ ] RM4 — RISC-V SE syscall surface expansion ongoing per project plan
- [ ] RE2 — `Mutex` poison `expect` patterns in engine not yet addressed
- [ ] RE3 — `expect("a64_state missing")` in syscall dispatch not yet replaced with structured error

---

## Summary

**`helm-arch`** is ISA decode + execute only, with intentional large files and exhaustive `match` coverage. **`helm-engine`** is the simulation kernel: timing, events, plugins, FS/SE modes, JIT, syscall handlers. **`helm-platform`** describes ARM virt topology and constants without constructing the full machine. **`helm-debug`** provides GDB/trace/checkpoint utilities. **`helm-cli`** embeds Python and launches binaries.

**Top risks:** `Aarch64Core::state()` / `state_mut()` returning **only vCPU 0** in system mode; fault plugin **ArchContext** fallback when AArch64 missing; **JIT FS** TLB pointer keyed off `next_vcpu`; **CASP** stub; **`unreachable!`** in execute paths; **dispatch table** silent clamp.

---

## Issues by taxonomy

### Design

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| RD1 | `helm-engine` monolith (`lib.rs` 3000+ lines) | Medium | Many concerns in one crate; harder review and testing. |
| RD2 | `Aarch64Core::state` / `state_mut` — vCPU 0 only in System mode | Critical | Any consumer thinks it sees “current” CPU; wrong for SMP / round-robin. |
| RD3 | Thread-local probe pointer in `helm-arch` | Medium | OK for single-threaded today; incompatible with thread-per-core SMP without refactor. |
| RD4 | Platform describe vs build split | Low | Documented asymmetry: `helm-platform` metadata vs `helm-engine` construction. |
| RD5 | Timer check interval truncation `nearest as u32` | Medium | Large `nearest` can wrap; see first-pass audit. |

### Correctness

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| RC1 | Fault plugin: AArch64 missing → RISC-V context | Critical | Can panic if RISC-V runtime also missing (`expect` on `riscv()`). |
| RC2 | JIT `JitFsContext` uses `board.next_vcpu` for TLB | High | Must match vCPU whose arch state / regs are active. |
| RC3 | `Casp` execute stub | High | No atomic pair behavior; breaks lock-free code. |
| RC4 | `unreachable!` in execute dispatch | High | Decode bugs become host panic; prefer guest `IllegalInstruction`. |
| RC5 | Dispatch table `idx.min(319)` | Medium | Opcodes with index above 318 can map to wrong handler silently. |
| RC6 | `MAX_INSTRUMENTED_ACCESSES` drops wide SIMD | Medium | Plugins see incomplete access lists. |
| RC7 | SE legacy syscall stub in `se/mod.rs` | Medium | `write` returns count without copying guest memory (TODO). |

### Completeness

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| RM1 | AArch32 | N/A (roadmap) | Explicitly unimplemented decode. |
| RM2 | SIMD / crypto stubs | High | Many opcodes classified or executed as stubs; document matrix vs guest needs. |
| RM3 | RISC-V FS JIT | Low | Falls back to interpreter; should log once (see audit). |
| RM4 | RISC-V SE syscall surface | High (project focus) | Code: `linux_riscv64.rs`; plan hub: `docs/plans/cursor-plan-00-roadmap.md` § RISC-V SE. |

### Software engineering

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| RE1 | Crate-level allows (`dead_code`, `clippy::pedantic`, …) | High | Especially `helm-engine` and `helm-arch`; see cross-cutting. |
| RE2 | `Mutex` poison `expect` in engine | Medium | Prefer `map_err` or recovery for diagnosability. |
| RE3 | `expect("a64_state missing")` in syscall dispatch | Medium | Configuration errors should return structured failure. |

### Software architecture

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| RA1 | Engine dependency fan-out | Medium | Engine pulls arch, memory, jit, hw for built-in platforms — slows builds and couples layers. |
| RA2 | “Current vCPU” as implicit global | Critical | Needs first-class cursor (`active_vcpu: usize`) across session, JIT, plugins, Python. |
| RA3 | Plugin fault context selection | High | Must key off **active ISA + active vCPU**, not fallback chains. |

### Idiomatic Rust

| ID | Topic | Severity | Notes |
|----|--------|----------|--------|
| RI1 | Panics for guest-visible conditions | Medium | Prefer `Result` or exception injection into guest state. |
| RI2 | `unreachable!` for “should not happen” decode | Medium | Treat as simulator bug + guest fault for resilience. |

---

## Detailed guidelines (runtime contributors)

### `helm-arch`

1. **Do not split `execute.rs` / main decode dispatch** without maintainer buy-in — project policy treats single-file exhaustiveness as an audit feature.
2. **New instructions:** Add decode + execute + classifier hook in `helm-engine` if opcodes are enumerated there.
3. **Replace `unreachable!`** in group dispatch with a path that surfaces `IllegalInstruction` (or internal `debug_assert!` + guest fault in release).
4. **Stubs:** If an instruction is stubbed, make behavior obviously wrong in tests (e.g., deterministic sentinel) or gate behind feature until correct — silent no-op CAS is worse than `unimplemented!` for triage.

### `helm-engine`

1. **Never use `Aarch64Core::state()` for “current PC” in FS mode** until it accepts vCPU index or reads `active_vcpu`. Audit all call sites (Python, plugins, JIT commit).
2. **JIT memory context:** Thread **explicit vCPU id** into `JitFsContext`; assert it matches the hart being decoded/executed.
3. **Fault / syscall plugins:** Build `ArchContext` from **session + active ISA**; avoid `expect` on subsystems that may be absent in hybrid configs.
4. **Timer / event scheduling:** Use saturating or checked arithmetic when mixing `u64` ticks with `u32` intervals.
5. **Instrumentation:** If widening `MAX_INSTRUMENTED_ACCESSES`, document plugin/trace tradeoffs.

### `helm-platform`

1. Keep **constants and attachment slots** here; avoid pulling in `HelmEngine` types.
2. When adding a new machine, duplicate the **documented split**: metadata in `helm-platform`, wiring in `helm-engine`.

### `helm-debug`

1. GDB/trace/checkpoint code must respect **same vCPU cursor** as engine when exposing registers.
2. Do not introduce wall-clock timing into deterministic replay paths.

### `helm-cli`

1. Argument parsing and `--sim-trace` stripping must stay consistent with AGENTS.md.
2. Embedded Python entry points should surface engine errors without double panic.

---

## Verification priorities (ordered)

1. Fix or guard **vCPU-scoped state** (`state()` / JIT TLB / plugin context) before expanding SMP.
2. **CASP** and other atomic stubs — either implement or hard-fail with clear message when executed.
3. **Dispatch table** — bounds check with illegal-opcode handler.
4. **RISC-V SE** completeness per project plan.
5. Narrow **lint allows** incrementally (`dead_code` first in engine).
