# HELM Proposals

Proposed changes to HELM, organised by category. Each item describes the
problem, why it matters, and the recommended fix.

This document supersedes:
- `CONSOLIDATED_REFACTORING_PROPOSAL.md` (root)
- `CRATE_CONSOLIDATION_PROPOSAL.md` (root)
- `PLUGIN_CONSOLIDATION_PROPOSAL.md` (root)

---

## A. Architectural Problems

### A1. `helm-tcg` is orphaned

**Problem:** `helm-tcg` (TcgOp, TcgContext, TcgBlock) has no downstream
dependents. `helm-translate` uses `helm-core::MicroOp` directly and never
touches TCG IR. The TCG crate exists but plays no role.

**Impact:** Dead code in the workspace, misleading crate name implies it
is used.

**Fix:** Merge `helm-tcg` into `helm-translate` as `src/tcg/`. Wire
`Translator` to emit `TcgBlock` as an intermediate form, then lower to
`Vec<MicroOp>`. This clarifies the intended two-phase pipeline:

```
ISA frontend
  ──► TcgContext.emit(TcgOp::Add { .. })  ← TCG generation phase
  ──► lower_to_micro_ops(block)           ← lowering phase
  ──► Vec<MicroOp>                        ← pipeline input
```

Delete `crates/helm-tcg/` from the workspace once merged.

---

### A2. `helm-llvm` defines parallel IR types

**Problem:** `helm-llvm` defines its own `MicroOp` enum, `Error` type, and
`PhysReg = usize` that diverge from `helm-core`:

| Concept | helm-core | helm-llvm |
|---------|-----------|-----------|
| Micro-op | `ir::MicroOp` struct + `Opcode` enum | `micro_op::MicroOp` enum |
| Register | `types::RegId = u16` | `micro_op::PhysReg = usize` |
| Error | `HelmError` (thiserror) | local `Error` + `Result<T>` |

If `helm-llvm` output is fed into `helm-pipeline`, it won't type-check.

**Fix — Option A (recommended):** Expand `helm_core::ir::Opcode` to cover
the fine-grained categories `helm-llvm` needs (`IntAdd`, `IntSub`, `LogicAnd`,
etc.), then rewrite `llvm_to_micro_ops()` to return `helm_core::ir::MicroOp`.
Add `HelmError::Llvm(String)` and implement `From<helm_llvm::Error>`.

**Fix — Option B:** Keep the local enum, add
`impl From<helm_llvm::micro_op::MicroOp> for helm_core::ir::MicroOp`. Less
clean but lower churn.

---

### A3. `helm-decode` is unused

**Problem:** `helm-decode` correctly parses QEMU `.decode` files
(`%fields`, `&argsets`, `@formats`, `DecodeTree`). The QEMU `.decode` files
for ARM are checked in under
`crates/helm-isa/src/arm/decode_files/qemu/a64.decode`. Nothing connects
them.

**Impact:** The ISA frontend uses hardcoded pattern matching. The decode
tree is a parallel system that goes nowhere.

**Options:**
1. **Keep and document** — mark `publish = false`, add a note that it is
   reserved for machine-generated decoders. (Current recommendation.)
2. **Wire it in** — use `helm-decode` in a `build.rs` to generate Rust
   match arms from the `.decode` files at build time.
3. **Remove** — delete the crate, keep the `.decode` files as reference.

---

### A4. `Simulation::run_se()` is a stub

**Problem:** `helm-engine::Simulation::run_se()` logs a message and returns
immediately. The actual SE runner (`se::linux::run_aarch64_se_with_plugins`)
exists in the same crate but is never called from `run_se()`.

**Fix:** One line:

```rust
// helm-engine/src/sim.rs
pub fn run_se(&mut self) -> HelmResult<()> {
    match self.config.isa {
        IsaKind::Arm64 => crate::se::linux::run_aarch64_se_with_plugins(
            &self.binary_path, &self.args, &self.env,
            self.max_cycles, self.plugins.as_ref(),
        ),
        _ => Err(HelmError::Config(format!("SE mode: {:?} not supported", self.config.isa))),
    }
}
```

---

### A5. Dead syscall handler in `generic.rs`

**Problem:** `helm-syscall/src/os/linux/generic.rs` defines a
`SyscallHandler` that handles ~4 syscalls without libc. Nothing references
it. The real handler is `Aarch64SyscallHandler` in `handler.rs`.

**Fix:** Delete `generic.rs` and its `mod generic;` declaration. If a
no-libc stub is needed for tests, add it as `#[cfg(test)]`.

---

### A6. Crate consolidation (optional, medium-term)

Three groups of small crates that are always used together could be merged
to reduce workspace overhead and enable `pub(crate)` encapsulation:

| Merge | Into | Reduction |
|-------|------|-----------|
| helm-tcg | helm-translate | −1 crate (also fixes A1) |
| helm-object + helm-stats | helm-core | −2 crates |
| helm-device + helm-timing + helm-syscall | helm-platform (new) | −2 crates |

Net: 18 → 13 crates. **Not blocking.** The current structure is fine at
this project's scale. Do after A1–A5 are resolved.

---

## B. Performance

### B1. TCG has no JIT backend

`helm-tcg` defines `TcgOp` IR but there is no JIT or interpreter. All SE
execution goes through `Aarch64Cpu.step()` — a direct interpreter with no
translation or compilation. For high-instruction-count workloads this is
the primary throughput ceiling.

**Options:**
- Implement a simple threaded-code interpreter (dispatch table on `TcgOp`
  variants) — moderate speedup, low complexity.
- Wire `inkwell` (LLVM) as a JIT backend for hot translated blocks —
  significant speedup, high complexity.
- Keep as-is until SE-mode functional completeness is achieved.

---

### B2. Cache model uses naive LRU

`helm-memory::Cache` evicts the first invalid line, or the last line if
all are valid. This is not LRU. For high-associativity caches (8-way,
16-way) this produces incorrect eviction behavior and inaccurate miss rates.

**Fix:** Implement PLRU (pseudo-LRU) using a 1-bit-per-way tree (standard
for set-associative cache models). LRU is acceptable for 2-way; PLRU is
standard for 4-way and above.

---

### B3. TLB eviction is naive

`helm-memory::Tlb` evicts the first entry it finds when at capacity. True
LRU or even random-replacement would be more accurate.

**Fix:** Track insertion order (a simple `VecDeque` keyed by VPN) and evict
the oldest entry. For the simulation sizes HELM targets, full LRU is
practical.

---

### B4. Translation cache has no size limit

`TranslationCache` is a `HashMap<Addr, TranslatedBlock>` with no capacity
bound. For long-running workloads with large code footprints, this grows
without bound.

**Fix:** Add a `max_blocks` cap and evict cold blocks using a simple LFU
or clock algorithm. For SE mode this matters once the binary footprint
exceeds a few thousand basic blocks.

---

### B5. `helm-llvm` parser is a custom text parser

The built-in LLVM IR text parser handles common accelerator patterns but
fails on void return types and complex IR (pre-existing bug, tracked via
18 `#[ignore]` tests). Full LLVM IR is handled by `inkwell` (LLVM 18
bindings), which requires LLVM to be installed.

**Fix (short-term):** Fix the `parse_label` / `consume_char(':')` bug in
`parser.rs` for the common `define void @f()` case.

**Fix (long-term):** Gate `inkwell` behind a feature flag (already
scaffolded as `features = ["inkwell-parser"]`) and prefer it when LLVM is
available.

---

### B6. Multi-core rayon dispatch not load-balanced

`helm-engine` uses `rayon` for multi-core simulation, but the current
`CoreSim::tick()` implementation is largely sequential within each core.
Cross-core synchronisation happens at every quantum boundary, which may
be too frequent for large quantum sizes.

**Fix:** Profile multi-core runs with 4+ cores and tune the
`TemporalDecoupler` quantum size. Consider per-core work-stealing for
event processing.

---

## C. Release / Usability

### C1. Python / Rust config field mismatch

**Problem:** Python's `Platform.to_dict()` emits field names
(`num_cores`, `l1_size`, `pipeline_width`) that don't exist on Rust's
`PlatformConfig` / `CoreConfig` / `CacheConfig`. Passing a Python-configured
platform to Rust fails silently or panics on deserialization.

**Fix:** Audit `python/helm/platform.py` and `python/helm/core.py`. Align
every field name 1-to-1 with `helm-core/src/config.rs`. Add a round-trip
test:

```python
# tests/test_config_roundtrip.py
config = Platform(...).to_dict()
json_str = json.dumps(config)
# call into Rust via PyO3 to deserialise PlatformConfig
result = _helm_core.validate_config(json_str)
assert result["ok"]
```

---

### C2. No integration tests crossing crate boundaries

Unit tests exist in every crate. Missing end-to-end tests:

| Test | What it validates |
|------|-----------------|
| `e2e_se_arm.rs` | `Simulation::run_se()` actually runs a binary (blocked by A4) |
| `e2e_llvm_accel.rs` | LLVM IR text → MicroOp → scheduler drains (blocked by A2) |
| `config_roundtrip.rs` | Python dict → JSON → `PlatformConfig` → JSON round-trip (blocked by C1) |
| `e2e_plugin_trace.rs` | Plugin callbacks fire and accumulate correct counts on a real binary |

Add these under `tests/` at the workspace root or in
`crates/helm-engine/tests/`.

---

### C3. ELF loader: static AArch64 only

`load_elf()` requires static AArch64 ELF64. It rejects:
- ELF32 (AArch32 binaries)
- x86-64 ELF (even though `X86Frontend` exists)
- Dynamically linked binaries (no `PT_INTERP` handling)
- PIE binaries that rely on base address randomisation

**Fix:** Add at minimum:
- ELF32 support when AArch32 frontend is implemented.
- A clear error message for dynamic binaries ("dynamic linking not
  supported; build with `-static -static-libgcc`").
- Base address override for PIE (randomise or fix at 0x400000).

---

### C4. Missing ISA frontend implementations

`RiscVFrontend` and `X86Frontend` are stubs that emit a single NOP and
consume bytes without decoding. They compile and pass type checks but
execute nothing correctly.

**Fix priority order:**
1. RISC-V RV64GC — standard Linux ABI, most used after AArch64.
2. x86-64 — large existing binary ecosystem but complex variable-length encoding.
3. AArch32 — needed for ARMv7 embedded targets.

Each frontend should follow the same TDD approach used for AArch64:
register file → decoder → executor → syscall table → e2e test.

---

### C5. Python bindings lack type hints and docstrings

`helm-python` exposes `_helm_core` as a native extension but provides no
`.pyi` stub files and no docstrings on exported functions. Users relying on
the Python API get no IDE completion or inline documentation.

**Fix:** Generate `.pyi` stub files using `maturin`'s PyO3 annotation
support, or write them manually. Add `#[doc = "..."]` attributes to all
PyO3 `#[pyfunction]` and `#[pyclass]` items.

---

### C6. `helm-cli` has no integration tests

The `helm` and `helm-arm` binaries have no automated tests. Any regression
in argument parsing, binary loading, or output formatting goes undetected.

**Fix:** Add `crates/helm-cli/tests/integration.rs` using
`std::process::Command`:

```rust
#[test]
fn helm_help_exits_zero() {
    let out = Command::new(env!("CARGO_BIN_EXE_helm"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success());
}
```

See `crates/helm-cli/TODO-COVERAGE-TESTS.md` for the full test plan.

---

### C7. No plugin ABI stability guarantees

The `helm-plugin` crate exports a `HelmPlugin` trait that external dynamic
plugins depend on. There is no versioning, no deprecation policy, and no
documentation of which parts of the API are stable.

**Fix:**
- Add `API_VERSION: &str = "0.1"` constant.
- Document in `plugin-system.md` which traits and types are stable.
- Version-check at dynamic load time: reject plugins compiled against a
  mismatched major version.

---

## Priority Summary

| # | Item | Effort | Impact |
|---|------|--------|--------|
| A4 | Wire `run_se()` | Low | Fixes broken public API |
| A5 | Delete dead syscall handler | Low | Removes dead code |
| C1 | Fix Python/Rust config mismatch | Low | Prevents silent data loss |
| A2 | Unify helm-llvm IR types | Medium | Prevents type fragmentation |
| A1 | Merge helm-tcg → helm-translate | Medium | Eliminates orphaned crate |
| B2 | Fix cache LRU | Low | Improves APE/CAE accuracy |
| B1 | TCG interpreter | Medium | 2-5× SE throughput |
| C2 | Integration tests | Medium | Catches cross-crate regressions |
| C4 | RISC-V frontend | High | New ISA support |
| A3 | Resolve helm-decode | Low | Clarifies intent |
| A6 | Crate consolidation | Medium | Workspace ergonomics |
| C3 | ELF loader improvements | Medium | Broader binary support |
