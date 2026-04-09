# cursor-runtime -- Codebase Audit

Date: 2026-04-07

## Summary

The `runtime/` domain contains `helm-arch` (ISA decode/execute) and
`helm-engine` (simulation kernel, syscall handlers, JIT integration). This
is the highest-complexity area of the codebase. `helm-arch` carries ~800
AArch64 tests and follows the intentional "one large file" design for the
executor. `helm-engine` is a 3200+ line monolith that owns timing, events,
plugins, probes, FS stepping, syscall dispatch, and JIT. The most serious
issues are: `state()`/`state_mut()` only exposing vCPU 0 in system mode,
a panic path when the fault plugin fires with no AArch64 state, JIT FS
memory context potentially using the wrong vCPU, `unreachable!` in decode
dispatch where guest `IllegalInstruction` faults would be more correct,
and several unimplemented stubs (`Casp`, some SIMD, AArch32).

---

## Design Issues

### D1. `helm-engine` centralizes too much in one crate/file

**Severity: Medium**

`HelmEngine<T>` (`runtime/helm-engine/src/lib.rs`, 3200+ lines) owns timing,
events, plugins, probes, AArch64 decode cache, FS stepping, syscall dispatch,
JIT integration, ELF symbols, and instrumented memory recording. The crate
root allows `dead_code` globally:

```rust
// runtime/helm-engine/src/lib.rs:15-27
#![allow(
    missing_docs,
    dead_code,
    clippy::pedantic,
    ...
)]
```

This hides incomplete or unused API surface instead of narrowing visibility.

**Suggested fix:** Consider extracting JIT integration (`jit.rs`), dispatch
table (`dispatch.rs`), and FS-mode step loop (`fs.rs`) into their own modules
that take `&mut HelmEngine<T>` rather than being inherent impl blocks. Remove
`dead_code` allow and prune unused code.

---

### D2. `Aarch64Core::state()` / `state_mut()` only expose vCPU 0 in system mode

**Severity: Critical**

```rust
// runtime/helm-engine/src/session.rs:70-86
pub(crate) fn state(&self) -> Option<&Aarch64ArchState> {
    match self {
        Self::Disabled => None,
        Self::Functional(state) => Some(state),
        Self::Syscall { state, .. } => Some(state),
        Self::System(machine) => machine.vcpus.first().map(|vcpu| &vcpu.arch),
    }
}
pub(crate) fn state_mut(&mut self) -> Option<&mut Aarch64ArchState> {
    match self {
        // ...
        Self::System(machine) => machine.vcpus.first_mut().map(|vcpu| &mut vcpu.arch),
    }
}
```

Any code using `state()` for "current AArch64 PC" or register state in FS mode
silently reads vCPU 0 even when the engine is stepping a different vCPU.
The Python API, plugin fault context, and JIT state commit all flow through
this accessor.

**Suggested fix:** Accept a `vcpu_idx` parameter, or store the "currently
stepping" vCPU index on `HelmEngine<T>` and route through it.

---

### D3. Probe wiring via thread-local raw pointers

**Severity: Medium**

```rust
// runtime/helm-arch/src/probe_ctx.rs:1-10
//! Thread-local probe context for helm-arch execute functions.
//! ...
//! This is the "faster to ship" approach from the TODO: thread-local
//! `CURRENT_PROBES: RefCell<Option<*mut CpuProbes>>` set by the engine
//! before each step.
```

This is a global mutable pointer stored in a thread-local. It works in the
single-threaded simulation model but becomes unsound if execution is ever
multi-threaded (e.g., SMP simulation with thread-per-core).

**Suggested fix:** Acceptable for now. When SMP is added, pass probes as a
parameter to `execute()` or use a scoped borrow.

---

### D4. Platform description vs. realization split is asymmetric

**Severity: Low**

`helm-platform` defines address constants and device topology, but actual
device construction stays in `helm-engine` (because it depends on
engine-internal types). This is documented in
`runtime/helm-platform/src/aarch64/virt.rs:1-6` but confusing for new
contributors trying to add a second platform.

---

## Correctness Issues

### C1. Fault plugin context assumes RISC-V when AArch64 state is missing

**Severity: Critical**

```rust
// runtime/helm-engine/src/lib.rs:2105-2117
let context = if let Some(a) = self.session.aarch64().and_then(Aarch64Core::state) {
    helm_plugin::runtime::ArchContext::Aarch64 { ... }
} else {
    helm_plugin::runtime::ArchContext::RiscV {
        x: self.riscv().iregs,
        pc: self.riscv().pc,
    }
};
```

If AArch64 state is `None` but the active core is not RISC-V (e.g.,
`Aarch64Core::Disabled` with no RISC-V runtime), `self.riscv()` panics via
`.expect("riscv runtime missing")` at `lib.rs:596`.

**Suggested fix:** Check the active ISA first, or use `session.riscv()`
(which returns `Option`) and fall back gracefully.

---

### C2. Timer countdown truncates `u64` to `u32`

**Severity: Medium**

```rust
// runtime/helm-engine/src/lib.rs:147-151
if nearest == u64::MAX {
    TIMER_CHECK_INTERVAL
} else {
    (nearest as u32).clamp(1, TIMER_CHECK_MAX)
}
```

If `nearest` exceeds `u32::MAX`, the cast wraps before `clamp`. A timer
far in the future would be treated as an imminent timer, causing unnecessary
timer-check overhead.

**Suggested fix:** Use `nearest.min(TIMER_CHECK_MAX as u64) as u32`.

---

### C3. JIT FS memory context uses `board.next_vcpu` not current vCPU

**Severity: High**

```rust
// runtime/helm-engine/src/jit.rs:91-94
let mut fs_ctx = Some(helm_jit::helpers::JitFsContext {
    sys_mem: &mut board.sys_mem as *mut _,
    tlb: &mut board.vcpus[board.next_vcpu].fs.tlb as *mut _,
    mmu_cfg,
});
```

`next_vcpu` is advanced by the scheduling code elsewhere. If this ever
diverges from the vCPU whose state was flattened into `flat_regs`, the
JIT executes with the wrong TLB and MMU configuration.

**Suggested fix:** Store the currently-stepping vCPU index and use it
consistently, or pass it explicitly to `setup_aarch64_jit_memory_context`.

---

### C4. `Casp` (pair CAS) is a no-op stub

**Severity: High**

```rust
// runtime/helm-arch/src/aarch64/execute/ldst.rs:519
Casp => { /* pair CAS — stub, return current value */ }
```

No memory access and no register update occur. Guest code using `CASP`
(e.g., lock-free pair-CAS in glibc) silently gets no effect, leading to
incorrect synchronization and data corruption.

**Suggested fix:** Implement the full pair-CAS: read two consecutive
registers as a 128-bit pair, compare-and-swap, write results back.

---

### C5. `unreachable!` in decode/execute dispatch instead of guest fault

**Severity: High**

Seven execute modules use `unreachable!("wrong dispatch to ...")`:

```
runtime/helm-arch/src/aarch64/execute/ldst.rs:593
runtime/helm-arch/src/aarch64/execute/dp.rs:472
runtime/helm-arch/src/aarch64/execute/branch.rs:271
runtime/helm-arch/src/aarch64/execute/simd.rs:486
runtime/helm-arch/src/aarch64/execute/sysreg.rs:134
runtime/helm-arch/src/aarch64/execute/mul_div.rs:120
runtime/helm-arch/src/aarch64/execute/fp.rs:206
```

If an opcode is routed to the wrong group (e.g., due to a decode table
bug), the simulator panics instead of raising a guest `IllegalInstruction`
exception. The guest kernel would normally handle such faults gracefully.

**Suggested fix:** Return `HartException::IllegalInstruction` instead of
`unreachable!`. This makes the simulator more robust to decode-table drift
and mirrors real hardware behavior.

---

### C6. Dispatch table silently caps index at 319

**Severity: Medium**

```rust
// runtime/helm-engine/src/dispatch.rs:46-50
let idx = insn.opcode as u16 as usize;
// SAFETY: table has 320 entries; u16 max is 65535 but we only have 304
// valid opcodes. The enum repr(u16) guarantees discriminants < 320.
let f = EXEC_TABLE[idx.min(319)];
```

If a future opcode variant exceeds 319 (enum extension without table
extension), the `.min(319)` silently routes it to whatever handler is at
index 319. This masks bugs rather than catching them.

**Suggested fix:** Use `EXEC_TABLE.get(idx).unwrap_or(&fallback_handler)`
where the fallback handler raises `IllegalInstruction`.

---

### C7. `MAX_INSTRUMENTED_ACCESSES` silently drops wide SIMD

**Severity: Medium**

```rust
// runtime/helm-engine/src/lib.rs:421-425
/// Maximum memory accesses recorded per instruction. 16 covers paired
/// load/store and basic SIMD (LD1/ST1 up to 4-register). SVE/SME with
/// wider vectors may exceed this — those accesses are silently dropped
/// since we don't yet instrument SVE element-wise.
const MAX_INSTRUMENTED_ACCESSES: usize = 16;
```

Plugin and trace consumers miss memory accesses for wide SIMD/SVE
instructions. This affects cache-miss analysis and memory-trace fidelity.

---

### C8. RISC-V64 JIT in FS mode falls back to interpreter silently

**Severity: Low**

```rust
// runtime/helm-engine/src/jit.rs:470-475
let (jit_mr, jit_mw) = if is_fs {
    // ... For now, fall back to interpreter if FS infrastructure isn't ready.
    return self.run(max_insns);
```

FS-mode RISC-V JIT is silently disabled. Users expecting JIT acceleration
in RV64 FS mode get interpreter speed with no warning.

**Suggested fix:** Log a one-time `sim_info!` message indicating JIT is
not available for RV64 FS mode.

---

## Completeness Issues

### P1. AArch32 is entirely unimplemented

```rust
// runtime/helm-arch/src/aarch32/mod.rs
pub fn decode(_raw: u32, _pc: u64) -> Result<Instruction, DecodeError> {
    Err(DecodeError::Unimplemented)
}
```

Phase 3 deliverable, acknowledged in the roadmap.

### P2. Legacy `LinuxSyscallHandler` in `se/mod.rs` is dead code

`LinuxSyscallHandler` (the Phase-0 stub) appears unused alongside the
real handlers in `linux_aarch64.rs` and `linux_riscv64.rs`. Its `write`
syscall is still a stub that does not copy from guest memory:

```rust
// runtime/helm-engine/src/se/mod.rs:90-95
64 => {
    // TODO(phase-0): access guest memory via ctx to read buf_addr..count
    // For now, stub returns count to keep programs happy
    Ok(args.a2 as i64)
}
```

### P3. Large SIMD / crypto surface is marked as stubs

Many SIMD opcodes are classified as `is_stub: true` in the engine's opcode
classifier. `SimdOther`, `SimdLd1`, `SimdSt1`, `Crc32`, `Crc32c`, and
others are silently skipped during execution.

### P4. `dispatch_aarch64_syscall` panics if state missing

```rust
// runtime/helm-engine/src/lib.rs:2128-2137
let (x0, x1, x2, x3, x4, x5) = {
    let a = self.session.aarch64().and_then(Aarch64Core::state)
        .expect("a64_state missing");
```

In a correctly wired engine this should never fail, but an `expect` here
turns a configuration error into a panic rather than a diagnostic.

---

## Software Engineering Issues

### E1. Broad `#![allow(...)]` on crate roots

- `helm-arch/lib.rs`: `missing_docs`, `clippy::pedantic`
- `helm-engine/lib.rs`: `missing_docs`, `dead_code`, `clippy::pedantic`,
  `clippy::collapsible_match`, `clippy::large_enum_variant`, and more

This reduces signal from the compiler and hides incomplete code.

### E2. `Mutex` poison handling via `expect()` throughout

`shared.lock().unwrap()` / `.expect()` appears in `fs.rs`, `lib.rs`, and
`arm_virt.rs` tests. Panics on poison rather than propagating errors.

### E3. `PlatformError` manually implements `Display` / `Error`

```rust
// runtime/helm-platform/src/lib.rs:44-71
#[derive(Debug)]
pub enum PlatformError { ... }
impl std::fmt::Display for PlatformError { ... }
impl std::error::Error for PlatformError {}
```

Inconsistent with `helm-arch`'s `DecodeError` which uses `thiserror`.

### E4. Test-only `unwrap` / `expect` proliferation in production paths

Several production-path `expect()` calls exist (event payloads, platform
realization, state accessors). These should be `Result` returns.

---

## Architecture Issues

### A1. `helm-engine` depends on `helm-debug`

The simulation kernel pulls in the debugging crate as a non-optional
dependency. This ties "simulator core" to "debug infrastructure". Consider
gating behind a feature flag.

### A2. `helm-engine` depends on all HW crates

`maybe_realize_builtin_platform` hardcodes ARM virt with specific HW crate
types (`helm_hw_intc`, `helm_hw_virtio`, etc.). Adding a second platform
requires either widening this coupling or extracting platform realization.

### A3. `helm-platform` cannot construct machines

By design, but it means `helm-engine` must know all platform details. Each
new platform duplicates the bring-up pattern.

---

## Idiomatic Rust Issues

### I1. `unsafe` in JIT and probe context

`jit.rs` uses raw pointers to `JitCache`, `JitBackend`, slice from raw parts.
`probe_ctx.rs` uses `*mut CpuProbes` in thread-local. Both are justified by
domain constraints (JIT codegen requires raw memory manipulation, probes need
global access without signature changes) but represent the densest unsafe
surface in the project.

### I2. `libc` FFI in syscall handlers

`linux_aarch64.rs` / `linux_riscv64.rs` use extensive `unsafe { libc::... }`.
Normal for syscall-emulation FFI but each errno/result path must stay audited.

### I3. `unreachable!` for supposedly exhaustive matches

See C5. Correct by invariant but brittle. Prefer returning
`IllegalInstruction` for guest-facing dispatch.

---

## Recommendations

### Quick Wins (< 1 hour each)

1. **Fix fault context panic** (C1) -- check active ISA before assuming RISC-V
2. **Fix timer truncation** (C2) -- use `nearest.min(TIMER_CHECK_MAX as u64) as u32`
3. **Log RV64 FS JIT fallback** (C8)
4. **Use `thiserror` for `PlatformError`** (E3)
5. **Remove `dead_code` allow from `helm-engine`** and prune unused code (E1)
6. **Remove dead `LinuxSyscallHandler` in `se/mod.rs`** (P2)

### Medium Effort (1-4 hours each)

7. **Replace `unreachable!` with `IllegalInstruction` return** in all 7 execute modules (C5)
8. **Add `vcpu_idx` parameter to `Aarch64Core::state()`** or store current vCPU (D2)
9. **Fix JIT FS memory context vCPU selection** (C3)
10. **Add bounds-checked dispatch table lookup** (C6)
11. **Implement `Casp` pair-CAS** (C4)
12. **Increase `MAX_INSTRUMENTED_ACCESSES`** or emit a warning on overflow (C7)

### Structural (> 4 hours)

13. **Extract JIT, dispatch, FS-mode into focused modules** (D1)
14. **Feature-gate `helm-debug` dependency** in `helm-engine` (A1)
15. **Extract platform realization from `helm-engine`** to reduce HW coupling (A2)
16. **Implement AArch32 decode** (P1 -- Phase 3)
17. **Complete SIMD/crypto execute stubs** (P3)
