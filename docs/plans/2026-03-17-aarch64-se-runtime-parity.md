# AArch64 SE Runtime Parity Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bring the current AArch64 SE runtime path to parity with `../helm.git` for ISA execution, Linux syscall emulation, and ELF/TLS setup so that real static AArch64 binaries such as fish run correctly.

**Architecture:** Keep the engine on the current active AArch64 SE runtime path and port missing behavior into `decode.rs`, `execute.rs`, the AArch64 Linux syscall handler, and the ELF loader. Use `../helm.git` as the behavioral reference, and drive the work with failing parity tests before each implementation batch.

**Tech Stack:** Rust, PyO3-facing SE runtime, cargo tests, pytest, `qemu-aarch64`, old-repo reference in `../helm.git`

---

### Task 1: Build the Parity Inventory

**Files:**
- Create: `docs/plans/2026-03-17-aarch64-se-runtime-parity-matrix.md`
- Read: `../helm.git/crates/helm-isa/src/arm/aarch64/exec.rs`
- Read: `../helm.git/crates/helm-engine/src/loader/elf64.rs`
- Read: `../helm.git/crates/helm-engine/src/se/linux.rs`
- Read: `../helm.git/crates/helm-syscall/src/os/linux/mod.rs`
- Read: `../helm.git/crates/helm-syscall/src/tests/`
- Read: `crates/helm-arch/src/aarch64/decode.rs`
- Read: `crates/helm-arch/src/aarch64/execute.rs`
- Read: `crates/helm-engine/src/loader/elf64.rs`
- Read: `crates/helm-engine/src/se/linux_aarch64.rs`

**Step 1: Create the parity matrix skeleton**

Document sections for:
- instruction families
- syscall coverage
- ELF / auxv / brk / TLS setup
- known fish-path failures

**Step 2: Record old-repo instruction coverage**

Run:
```bash
rg -n "decode_aarch64_simd|exec_simd_dp|exec_ldst_simd|fn exec_" ../helm.git/crates/helm-isa/src/arm/aarch64
```

Expected: concrete old-path instruction families listed in the matrix.

**Step 3: Record current active-path gaps**

Run:
```bash
rg -n "Simd.*true|silently skip|Unsupported|PT_TLS|tls|auxv|brk" crates/helm-arch/src/aarch64 crates/helm-engine/src
```

Expected: current stubs and missing loader/TLS behaviors captured in the matrix.

**Step 4: Commit**

```bash
git add docs/plans/2026-03-17-aarch64-se-runtime-parity-matrix.md
git commit -m "docs: add aarch64 se parity matrix"
```

### Task 2: Port Old SIMD Decode/Execute Regression Tests

**Files:**
- Modify: `crates/helm-arch/tests/aarch64_decode.rs`
- Modify: `crates/helm-arch/tests/aarch64_exec.rs`
- Read: `../helm.git/crates/helm-isa/src/arm/aarch64/tests/decode_simd_generated.rs`
- Read: `../helm.git/crates/helm-isa/src/arm/aarch64/tests/exec_simd.rs`

**Step 1: Add one failing decode test for a missing old-repo mnemonic**

Start with one fish-observed opcode that is still stubbed or absent from the active path.

**Step 2: Run the single decode test**

Run:
```bash
cargo test -p helm-arch <new_decode_test_name> -- --exact
```

Expected: FAIL because the current decoder maps it incorrectly or too generically.

**Step 3: Add one failing exec test for the same instruction family**

Mirror old-repo semantics with concrete register / vector assertions.

**Step 4: Run the single exec test**

Run:
```bash
cargo test -p helm-arch <new_exec_test_name> -- --exact
```

Expected: FAIL because current execution is stubbed or semantically wrong.

**Step 5: Commit**

```bash
git add crates/helm-arch/tests/aarch64_decode.rs crates/helm-arch/tests/aarch64_exec.rs
git commit -m "test: add aarch64 parity regressions"
```

### Task 3: Implement Active-Path ISA Parity in Small Batches

**Files:**
- Modify: `crates/helm-arch/src/aarch64/insn.rs`
- Modify: `crates/helm-arch/src/aarch64/decode.rs`
- Modify: `crates/helm-arch/src/aarch64/execute.rs`
- Read: `crates/helm-arch/src/aarch64/step_simd.rs`
- Read: `../helm.git/crates/helm-isa/src/arm/aarch64/exec.rs`

**Step 1: Implement the minimal decode change for the failing test**

Only add the opcode / operand decode needed by the new failing test.

**Step 2: Run the decode test**

Run:
```bash
cargo test -p helm-arch <new_decode_test_name> -- --exact
```

Expected: PASS.

**Step 3: Implement the minimal execute change for the failing exec test**

Port semantics from the old repo or reuse matching logic from `step_simd.rs`.

**Step 4: Run the exec test**

Run:
```bash
cargo test -p helm-arch <new_exec_test_name> -- --exact
```

Expected: PASS.

**Step 5: Run the relevant AArch64 test file**

Run:
```bash
cargo test -p helm-arch --test aarch64_exec
```

Expected: the edited test file passes.

**Step 6: Repeat for the next parity gap**

Keep batches narrow. Prioritize fish-observed SIMD, then remaining old-repo-implemented SIMD/FP gaps.

**Step 7: Commit**

```bash
git add crates/helm-arch/src/aarch64/insn.rs crates/helm-arch/src/aarch64/decode.rs crates/helm-arch/src/aarch64/execute.rs crates/helm-arch/tests/aarch64_decode.rs crates/helm-arch/tests/aarch64_exec.rs
git commit -m "feat: port aarch64 isa parity batch"
```

### Task 4: Add ELF Loader and TLS Regression Tests

**Files:**
- Create: `crates/helm-engine/tests/aarch64_loader_tls.rs`
- Read: `crates/helm-engine/src/loader/elf64.rs`
- Read: `../helm.git/crates/helm-engine/src/loader/elf64.rs`
- Read: `assets/binaries/fish`

**Step 1: Write a failing loader test for old-repo-visible behavior**

Cover one behavior at a time:
- `brk_base` placement
- auxv contents
- PT_TLS parsing
- initial stack invariants

**Step 2: Run the single loader test**

Run:
```bash
cargo test -p helm-engine --test aarch64_loader_tls <test_name> -- --exact
```

Expected: FAIL because the current loader lacks the required field or setup behavior.

**Step 3: Add a failing TLS-specific test**

Prefer a direct PT_TLS / TLS-template assertion rather than relying only on fish.

**Step 4: Run the TLS test**

Run:
```bash
cargo test -p helm-engine --test aarch64_loader_tls <tls_test_name> -- --exact
```

Expected: FAIL.

**Step 5: Commit**

```bash
git add crates/helm-engine/tests/aarch64_loader_tls.rs
git commit -m "test: add aarch64 loader tls regressions"
```

### Task 5: Implement ELF / TLS Parity

**Files:**
- Modify: `crates/helm-engine/src/loader/elf64.rs`
- Modify: `crates/helm-engine/src/lib.rs`
- Read: `../helm.git/crates/helm-engine/src/loader/elf64.rs`
- Read: `../helm.git/crates/helm-engine/src/se/session.rs`
- Read: `../helm.git/crates/helm-engine/src/se/linux.rs`

**Step 1: Implement the smallest loader data-structure change needed**

Add only the fields required by the failing loader/TLS tests.

**Step 2: Run the targeted loader test**

Run:
```bash
cargo test -p helm-engine --test aarch64_loader_tls <test_name> -- --exact
```

Expected: PASS.

**Step 3: Implement the smallest TLS initialization change needed**

Match old-repo observable behavior for PT_TLS capture and SE startup state.

**Step 4: Run the TLS test**

Run:
```bash
cargo test -p helm-engine --test aarch64_loader_tls <tls_test_name> -- --exact
```

Expected: PASS.

**Step 5: Run the full loader test file**

Run:
```bash
cargo test -p helm-engine --test aarch64_loader_tls
```

Expected: PASS.

**Step 6: Commit**

```bash
git add crates/helm-engine/src/loader/elf64.rs crates/helm-engine/src/lib.rs crates/helm-engine/tests/aarch64_loader_tls.rs
git commit -m "feat: add aarch64 elf tls parity"
```

### Task 6: Add Linux AArch64 Syscall Regression Tests

**Files:**
- Create: `crates/helm-engine/tests/aarch64_syscalls.rs`
- Read: `crates/helm-engine/src/se/linux_aarch64.rs`
- Read: `../helm.git/crates/helm-syscall/src/tests/handler.rs`
- Read: `../helm.git/crates/helm-syscall/src/tests/aarch64.rs`
- Read: `examples/se/compare_syscalls.py`

**Step 1: Write one failing syscall test for a fish-path syscall**

Start with the earliest syscall whose behavior differs or is unimplemented on the fish path.

**Step 2: Run the single syscall test**

Run:
```bash
cargo test -p helm-engine --test aarch64_syscalls <test_name> -- --exact
```

Expected: FAIL with the current incorrect behavior.

**Step 3: Add one more failing regression around memory-management or TLS-adjacent behavior**

Prefer `mmap`, `mprotect`, `brk`, `set_tid_address`, or similar startup-critical calls.

**Step 4: Run the second syscall test**

Run:
```bash
cargo test -p helm-engine --test aarch64_syscalls <second_test_name> -- --exact
```

Expected: FAIL.

**Step 5: Commit**

```bash
git add crates/helm-engine/tests/aarch64_syscalls.rs
git commit -m "test: add aarch64 syscall parity regressions"
```

### Task 7: Implement Syscall Parity in Small Batches

**Files:**
- Modify: `crates/helm-engine/src/se/linux_aarch64.rs`
- Read: `../helm.git/crates/helm-syscall/src/os/linux/`
- Read: `../helm.git/crates/helm-engine/src/se/linux.rs`

**Step 1: Implement the minimal fix for the first failing syscall test**

Preserve the current architecture and match old observable behavior.

**Step 2: Run the first syscall test**

Run:
```bash
cargo test -p helm-engine --test aarch64_syscalls <test_name> -- --exact
```

Expected: PASS.

**Step 3: Implement the minimal fix for the second failing syscall test**

Prefer correctness and explicit errno handling over broad refactors.

**Step 4: Run the second syscall test**

Run:
```bash
cargo test -p helm-engine --test aarch64_syscalls <second_test_name> -- --exact
```

Expected: PASS.

**Step 5: Run the full syscall test file**

Run:
```bash
cargo test -p helm-engine --test aarch64_syscalls
```

Expected: PASS.

**Step 6: Commit**

```bash
git add crates/helm-engine/src/se/linux_aarch64.rs crates/helm-engine/tests/aarch64_syscalls.rs
git commit -m "feat: port aarch64 syscall parity batch"
```

### Task 8: Use Fish as the Runtime Gate After Each Batch

**Files:**
- Read: `examples/se/run_binary.py`
- Read: `assets/binaries/fish`

**Step 1: Run fish after each parity batch**

Run:
```bash
cargo run --release --bin helm-aarch64 -- examples/se/run_binary.py
```

Expected: either progress beyond the prior fault PC or a new concrete stop reason.

**Step 2: Run fish with debug plugins when it still stops**

Run:
```bash
cargo run --release --bin helm-aarch64 -- examples/se/run_binary.py --stub-trace --plugin fault-detect
```

Expected: reduced stub count and a new actionable fault if still failing.

**Step 3: Compare with qemu-user when behavior is unclear**

Run:
```bash
qemu-aarch64 assets/binaries/fish --no-config -c 'echo hello'
```

Expected: `hello`.

**Step 4: Commit**

```bash
git add <only files from the completed parity batch>
git commit -m "test: verify fish runtime after parity batch"
```

### Task 9: Final Verification Sweep

**Files:**
- Modify as needed from previous tasks only

**Step 1: Run the focused Python regression tests**

Run:
```bash
pytest -q tests/test_run_binary.py
```

Expected: PASS.

**Step 2: Run the AArch64 ISA tests**

Run:
```bash
cargo test -p helm-arch
```

Expected: PASS.

**Step 3: Run the engine parity tests**

Run:
```bash
cargo test -p helm-engine --test aarch64_loader_tls --test aarch64_syscalls
```

Expected: PASS.

**Step 4: Run the end-to-end fish command**

Run:
```bash
cargo run --release --bin helm-aarch64 -- examples/se/run_binary.py
```

Expected: fish progresses past the old allocator abort and completes successfully.

**Step 5: Commit**

```bash
git add crates/helm-arch crates/helm-engine examples/se/run_binary.py tests/test_run_binary.py docs/plans/2026-03-17-aarch64-se-runtime-parity-matrix.md
git commit -m "feat: complete aarch64 se runtime parity"
```
