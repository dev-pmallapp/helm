# SE Host Threading Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a generic SE runtime host-threading model so supported guest thread-style `clone()` requests create native host threads, with AArch64 wired first and room for other ISAs such as RISC-V.

**Architecture:** Follow QEMU linux-user's runtime model: classify guest `clone()` calls into thread-style vs fork-style, validate flags, copy per-thread CPU state, apply TLS/thread-pointer state, and spawn a native host thread for supported thread-style clones. Keep the runtime generic in `helm-engine`; keep ISA-specific register/TLS wiring narrow.

**Tech Stack:** Rust, `std::thread` / synchronization primitives, cargo tests, old-repo references, local QEMU linux-user source references

---

### Task 1: Add Clone Classification Tests

**Files:**
- Create: `crates/helm-engine/tests/se_clone_flags.rs`
- Read: `../helm.git/assets/qemu/linux-user/syscall.c`

**Step 1: Write a failing test for thread-style clone flag recognition**

Use the QEMU thread flag set:
- `CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM`

The test should assert that this combination is recognized as thread-style.

**Step 2: Run the test to verify it fails**

Run:
```bash
cargo test -p helm-engine --test se_clone_flags thread_style_clone_flags_are_recognized
```

Expected: FAIL because no generic clone classifier exists yet.

**Step 3: Write a failing test for invalid thread-style flags**

Add a case that QEMU rejects as invalid.

**Step 4: Run the invalid-flags test**

Run:
```bash
cargo test -p helm-engine --test se_clone_flags invalid_thread_style_flags_are_rejected
```

Expected: FAIL.

### Task 2: Implement Generic Clone Classification

**Files:**
- Create: `crates/helm-engine/src/se/threading.rs`
- Modify: `crates/helm-engine/src/se/mod.rs`
- Test: `crates/helm-engine/tests/se_clone_flags.rs`

**Step 1: Implement the smallest shared clone classification helper**

Add a generic helper that returns:
- thread-style
- fork-style
- invalid

**Step 2: Run the first test**

Run:
```bash
cargo test -p helm-engine --test se_clone_flags thread_style_clone_flags_are_recognized
```

Expected: PASS.

**Step 3: Run the second test**

Run:
```bash
cargo test -p helm-engine --test se_clone_flags invalid_thread_style_flags_are_rejected
```

Expected: PASS.

### Task 3: Add AArch64 TLS Inheritance Tests

**Files:**
- Create: `crates/helm-engine/tests/se_thread_state.rs`
- Read: `../helm.git/crates/helm-engine/src/tests/thread.rs`

**Step 1: Write a failing test for `CLONE_SETTLS` semantics**

Assert that child thread state receives the requested TLS/thread-pointer value.

**Step 2: Run the test**

Run:
```bash
cargo test -p helm-engine --test se_thread_state clone_settls_sets_child_thread_pointer
```

Expected: FAIL because no generic thread runtime exists yet.

**Step 3: Write a failing test for inherited thread-pointer semantics**

Assert that without `CLONE_SETTLS`, the child inherits the parent thread pointer.

**Step 4: Run the test**

Run:
```bash
cargo test -p helm-engine --test se_thread_state clone_without_settls_inherits_parent_thread_pointer
```

Expected: FAIL.

### Task 4: Introduce Generic SE Thread State

**Files:**
- Modify: `crates/helm-engine/src/se/threading.rs`
- Modify: `crates/helm-engine/src/lib.rs`
- Test: `crates/helm-engine/tests/se_thread_state.rs`

**Step 1: Implement the minimal generic per-thread state needed by the TLS tests**

The state should include:
- thread id
- register snapshot / ISA-thread-state hook
- TLS/thread-pointer field

**Step 2: Run the TLS test**

Run:
```bash
cargo test -p helm-engine --test se_thread_state clone_settls_sets_child_thread_pointer
```

Expected: PASS.

**Step 3: Run the inheritance test**

Run:
```bash
cargo test -p helm-engine --test se_thread_state clone_without_settls_inherits_parent_thread_pointer
```

Expected: PASS.

### Task 5: Add Host-Thread Spawn Regression

**Files:**
- Create: `crates/helm-engine/tests/se_host_threads.rs`
- Modify: `crates/helm-engine/src/se/threading.rs`

**Step 1: Write a failing test for host-thread creation on thread-style clone**

Use a test hook or observable side effect proving a native host thread is created.

**Step 2: Run the host-thread test**

Run:
```bash
cargo test -p helm-engine --test se_host_threads thread_style_clone_spawns_host_thread
```

Expected: FAIL.

### Task 6: Implement Native Host Thread Spawning

**Files:**
- Modify: `crates/helm-engine/src/se/threading.rs`
- Modify: `crates/helm-engine/src/lib.rs`
- Modify: `crates/helm-engine/src/se/linux_aarch64.rs`
- Test: `crates/helm-engine/tests/se_host_threads.rs`

**Step 1: Implement the minimal host-thread spawn path**

For supported thread-style clone:
- validate flags
- create child thread state
- spawn a native host thread
- synchronize parent/child startup like QEMU does

**Step 2: Run the host-thread test**

Run:
```bash
cargo test -p helm-engine --test se_host_threads thread_style_clone_spawns_host_thread
```

Expected: PASS.

**Step 3: Re-run TLS/state tests**

Run:
```bash
cargo test -p helm-engine --test se_thread_state
```

Expected: PASS.

### Task 7: Wire AArch64 Clone Syscall Into the Generic Runtime

**Files:**
- Modify: `crates/helm-engine/src/se/linux_aarch64.rs`
- Modify: `crates/helm-engine/src/lib.rs`
- Read: `../helm.git/assets/qemu/linux-user/syscall.c`

**Step 1: Add a failing test for AArch64 `clone()` thread path**

Assert that supported thread-style clone no longer returns `ENOSYS`.

**Step 2: Run the test**

Run:
```bash
cargo test -p helm-engine --test se_host_threads aarch64_clone_thread_path_is_supported
```

Expected: FAIL.

**Step 3: Implement the minimal AArch64 syscall wiring**

Translate AArch64 clone arguments into the generic runtime request.

**Step 4: Run the AArch64 clone test**

Run:
```bash
cargo test -p helm-engine --test se_host_threads aarch64_clone_thread_path_is_supported
```

Expected: PASS.

### Task 8: Add Futex / Thread Exit Regression

**Files:**
- Create: `crates/helm-engine/tests/se_futex_threads.rs`
- Read: `../helm.git/crates/helm-engine/src/tests/thread.rs`

**Step 1: Write a failing test for futex wake across guest threads**

Use the generic runtime, not AArch64-specific code, where possible.

**Step 2: Run the futex test**

Run:
```bash
cargo test -p helm-engine --test se_futex_threads futex_wake_unblocks_waiting_thread
```

Expected: FAIL.

### Task 9: Implement Futex / Exit Integration

**Files:**
- Modify: `crates/helm-engine/src/se/threading.rs`
- Modify: `crates/helm-engine/src/se/linux_aarch64.rs`
- Test: `crates/helm-engine/tests/se_futex_threads.rs`

**Step 1: Implement minimal futex wake / thread-exit bookkeeping**

Focus only on behavior required by the failing test.

**Step 2: Run the futex test**

Run:
```bash
cargo test -p helm-engine --test se_futex_threads futex_wake_unblocks_waiting_thread
```

Expected: PASS.

### Task 10: End-to-End Verification

**Files:**
- Read: `examples/se/run_binary.py`
- Read: `assets/binaries/fish`

**Step 1: Run focused engine tests**

Run:
```bash
cargo test -p helm-engine --test se_clone_flags --test se_thread_state --test se_host_threads --test se_futex_threads
```

Expected: PASS.

**Step 2: Re-run fish**

Run:
```bash
cargo run --release --bin helm-aarch64 -- examples/se/run_binary.py
```

Expected: either successful progress beyond the current allocator failure or a new concrete failure deeper into the multithreaded runtime path.
