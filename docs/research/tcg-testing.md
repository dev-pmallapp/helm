# TCG Testing — QEMU Research and HELM Strategy

**Date:** June 2025
**Status:** Research document

---

## 1. Executive Summary

This document analyses how QEMU tests its Tiny Code Generator (TCG) subsystem
across seven distinct testing layers, then maps those layers onto an equivalent
HELM testing strategy.  QEMU's approach has evolved over 20+ years into a
mature, multi-tier framework that covers everything from IR semantics to
full-system kernel boot.  HELM can adopt the same layered model, substituting
Rust unit tests for QEMU's ad-hoc C harnesses and leveraging the QEMU guest
test corpus directly.

---

## 2. How QEMU Tests TCG

QEMU's TCG testing is not a single framework — it is a stack of seven
complementary layers, each catching a different class of bug.

```
Layer 7 │ Functional Tests          Linux kernel boot with plugins
Layer 6 │ GDB Stub Tests            Debugger attach, register read, step
Layer 5 │ System-Mode Guest Tests   Bare-metal C programs on virt machine
Layer 4 │ User-Mode Guest Tests     Statically linked C programs under linux-user
Layer 3 │ Plugin Tests              TCG plugin API: insn count, mem trace, syscall
Layer 2 │ Decode Tree Tests         Parser correctness: succ_*/err_* .decode files
Layer 1 │ TCG IR / TCI Unit Tests   Interpreter correctness, op semantics
```

### 2.1 Layer 1 — TCG IR and TCI (Tiny Code Interpreter)

**What:** The Tiny Code Interpreter (`tcg/tci.c`) is QEMU's software
fallback that executes TCG IR without JIT compilation.  It serves as
both a portability layer (for hosts without a native TCG backend) and
an implicit test oracle: if TCI produces different results from the
native JIT backend, there is a bug.

**How QEMU tests it:**
- TCI is enabled via `--enable-tcg-interpreter` at configure time.
- The entire `make check-tcg` suite runs under TCI, exercising every
  IR opcode through the interpreter dispatch loop.
- TCI includes its own assertion framework (`tci_assert`) enabled by
  `CONFIG_DEBUG_TCG`, which validates operand counts, register indices,
  and memory alignment at runtime.
- There is no separate unit test file for individual TCG ops.  Instead,
  correctness is established by running real guest programs and comparing
  output against expected results.

**Key source files:**
- `tcg/tci.c` — interpreter dispatch loop (~60 opcodes)
- `tcg/tcg.c` — IR construction, optimization, register allocation
- `tcg/optimize.c` — constant folding, dead-code elimination

**Insight for HELM:** QEMU relies on integration testing (guest programs)
rather than per-opcode unit tests for TCG IR correctness.  HELM already
has per-opcode unit tests in `helm-tcg/src/tests/ir.rs` and
`helm-tcg/src/tests/interp.rs`, which is a stronger foundation.

### 2.2 Layer 2 — Decode Tree Parser Tests

**What:** QEMU's decode tree generator (`scripts/decodetree.py`) parses
`.decode` specification files and generates C switch/case decoders.  The
test suite validates the parser itself — not instruction semantics.

**How QEMU tests it:**
- 46 `.decode` files in `tests/decode/`:
  - 8 `succ_*.decode` — valid syntax that must parse without error
  - 38 `err_*.decode` — invalid syntax that must produce a diagnostic
- Each `succ_` file tests a specific parser feature: argsets, functions,
  field inference, group nesting (4 levels), multi-segment fields.
- Each `err_` file tests a specific failure mode: duplicate fields,
  overlapping patterns, width mismatches, invalid groups, cyclic field
  references.
- Tests are run via `make check` through the meson build system, which
  invokes `decodetree.py` on each `.decode` file and asserts exit code
  0 (success) or non-zero (error) as appropriate.

**Key asset files (in HELM repo):**
- `assets/qemu/tests/decode/succ_*.decode` (8 files)
- `assets/qemu/tests/decode/err_*.decode` (38 files)
- `assets/qemu/target/arm/tcg/a64.decode` (1927 lines, canonical A64 spec)

**Insight for HELM:** HELM's `helm-decode` crate already uses these
assets.  The existing `docs/decode-test-plan.md` maps them in detail.

### 2.3 Layer 3 — TCG Plugin Tests

**What:** QEMU's plugin API (`qemu-plugin.h`) allows dynamically loaded
shared libraries to instrument TCG execution: count instructions, trace
memory accesses, intercept syscalls, and modify translation blocks.

**How QEMU tests it:**
- 10 test plugins in `tests/tcg/plugins/`:

| Plugin | Purpose |
|--------|---------|
| `insn.c` | Instruction counting (inline and callback modes) |
| `mem.c` | Memory access tracing with region validation |
| `bb.c` | Basic block counting |
| `empty.c` | Null plugin — tests load/unload lifecycle |
| `inline.c` | Inline instrumentation (no callback overhead) |
| `syscall.c` | Syscall interception and argument logging |
| `patch.c` | Translation block patching (self-modifying code) |
| `reset.c` | Plugin reset/re-registration |
| `discons.c` | Discontinuity detection in execution |

- Each multiarch guest test can be paired with any plugin via the
  `run-plugin-<test>-with-<plugin>` make target.  The Makefile
  rotates plugins across tests to avoid combinatorial explosion.
- Plugin output is validated with `check-plugin-output.sh`, which
  runs regex checks on the plugin log file.
- The `test_tcg_plugins.py` functional test boots a full Linux kernel
  with the `libinsn` plugin enabled and validates instruction counts.

**Insight for HELM:** HELM's plugin system (`helm-plugin`) has 81 tests
covering the API, runtime, and callbacks.  The plugin test approach
(pairing real workloads with instrumentation plugins) is directly
applicable for validating HELM's TCG backend.

### 2.4 Layer 4 — User-Mode Guest Tests (`linux-user`)

**What:** Small C programs compiled for the guest ISA, executed under
QEMU's linux-user (syscall emulation) mode.  These test instruction
semantics by running real code and checking output.

**How QEMU tests it:**
- `make check-tcg` builds and runs all guest tests for each configured
  target, using cross-compilers or Docker containers.
- **Multiarch tests** (`tests/tcg/multiarch/`) run on all targets:

| Test | What it validates |
|------|-------------------|
| `sha1.c` | Integer ALU, shifts, memory (SHA-1 golden vector) |
| `sha512.c` | 64-bit arithmetic, vectorisation under -O3 |
| `float_convs.c` | Float-to-int and int-to-float conversions |
| `float_madds.c` | Fused multiply-add precision |
| `signals.c` | Signal delivery and handler execution |
| `threadcount.c` | pthread creation and join |
| `test-mmap.c` | Memory mapping, protection, and unmapping |
| `segfault.c` | SIGSEGV delivery on invalid access |
| `overflow.c` | Integer overflow detection |
| `tb-link.c` | Translation block chaining correctness |

- **AArch64-specific tests** (`tests/tcg/aarch64/`):

| Test | What it validates |
|------|-------------------|
| `fcvt.c` | Float conversion against golden reference (`fcvt.ref`) |
| `pauth-1..5.c` | Pointer Authentication (PAC) sign/verify |
| `bti-1..3.c` | Branch Target Identification (ELF notes, PROT_BTI) |
| `mte-1..8.c` | Memory Tagging Extension (tag check, sync/async) |
| `sme-*.c` | Scalable Matrix Extension outer products |
| `sve-ioctls.c` | SVE vector length control via ioctl |
| `sysregs.c` | System register reads (MRS) from user mode |
| `test-aes.c` | AES instruction correctness |
| `dcpop.c` / `dcpodp.c` | Data cache prefetch operations |
| `pcalign-a64.c` | PC alignment enforcement |
| `lse2-fault.c` | Large System Extensions atomic fault handling |
| `test-826.c` etc. | Regression tests for specific QEMU bugs |

- The test framework is minimal: tests are compiled with
  `-Wall -Werror -O0 -g` and statically linked.  Output goes to
  `<test>.out`, which is compared against `.ref` files where they exist.
- A 120-second timeout (`TIMEOUT=120`) kills hung tests.

**Execution model:**
```
cross-compile test.c → aarch64-linux-gnu-gcc -static test.c -o test
run under QEMU:      → qemu-aarch64 [opts] ./test > test.out
validate:            → diff test.out test.ref  (if reference exists)
                     → exit code 0 = pass
```

**Insight for HELM:** These user-mode tests are directly reusable.  HELM's
SE mode can run the same statically linked binaries that QEMU uses.  The
AArch64-specific tests (especially `fcvt.c` with golden references) are
high-value regression tests.

### 2.5 Layer 5 — System-Mode Guest Tests (`softmmu`)

**What:** Bare-metal C programs that run on QEMU's full-system emulation
without an OS.  These test system-level features that are invisible in
user mode: exception levels, timers, MMU, interrupt handling.

**How QEMU tests it:**
- Each architecture provides a `boot.S` (startup assembly), `kernel.ld`
  (linker script), and `minilib/` (tiny printf/memcpy without libc).
- Tests are compiled as bare-metal ELF images and loaded directly by
  QEMU's `-kernel` option.
- QEMU's `-semihosting` or `-chardev file` captures output.

**AArch64 system tests** (`tests/tcg/aarch64/system/`):

| Test | What it validates |
|------|-------------------|
| `vtimer.c` | Generic timer: `cntvoff_el2`, `cntv_cval_el0`, `cntv_ctl_el0` reads/writes |
| `pauth-3.c` | PAC in system mode (EL1 key configuration) |
| `mte.S` | MTE with EL1 tag control register setup |
| `feat-xs.c` | FEAT_XS (non-XS barriers) feature detection |
| `asid2.c` | ASID-based TLB invalidation |
| `semiconsole.c` | Semihosting console I/O |
| `semiheap.c` | Semihosting heap allocation |

**Multiarch system tests** (`tests/tcg/multiarch/system/`):

| Test | What it validates |
|------|-------------------|
| `memory.c` | Cross-page memory access, aligned/unaligned reads at all sizes |
| `hello.c` | Minimal "Hello, World" via semihosting |
| `interrupt.c` | External interrupt delivery and handler invocation |

**Execution model:**
```
aarch64-linux-gnu-gcc -ffreestanding -nostdlib -T kernel.ld \
    boot.S test.c minilib/*.c -o test
qemu-system-aarch64 -M virt -cpu max -nographic \
    -semihosting -kernel test
```

**Insight for HELM:** System-mode tests are the critical gap in HELM's
current test suite.  Once FS mode is operational, these bare-metal tests
validate exception handling, timer access, and MMU configuration — exactly
the areas identified in `docs/tcg-system-instructions-gap.md`.

### 2.6 Layer 6 — GDB Stub Tests

**What:** Python scripts that attach GDB to QEMU and exercise the
gdbstub interface: single-step, register read/write, breakpoints,
watchpoints, and architecture-specific register sets (SVE, SME, MTE).

**How QEMU tests it:**
- Scripts live in `tests/tcg/<arch>/gdbstub/` (e.g., `test-sve.py`,
  `test-mte.py`, `test-sme.py`).
- A helper `run-test.py` launches QEMU with `-g <port>`, attaches GDB,
  runs the Python test script, and reports pass/fail.
- Tests validate that TCG correctly exposes architectural state through
  the debug interface, catching bugs where TCG optimizations (e.g.,
  register caching, block chaining) hide state from the debugger.

**Insight for HELM:** GDB stub testing is a future opportunity.  HELM
does not yet have a gdbstub, but when added, QEMU's test scripts can be
reused with minimal modification.

### 2.7 Layer 7 — Functional Tests (Full Kernel Boot)

**What:** Python-based tests that boot a full Linux kernel under QEMU
and validate end-to-end behaviour.  These are the highest-level TCG
tests, catching subtle interaction bugs across thousands of instructions.

**How QEMU tests it:**
- `tests/functional/aarch64/test_tcg_plugins.py` boots an AArch64
  Linux kernel on the `virt` machine with TCG plugins enabled.
- The test:
  1. Downloads a pre-built kernel from `storage.tuxboot.com`.
  2. Boots with `libinsn` plugin to count instructions.
  3. Waits for the console message `"Please append a correct root=
     boot option"` (indicating successful kernel init).
  4. Validates plugin output: instruction count > 0, proper per-vCPU
     breakdown, memory-mapped regions accessed.
- `tests/docker/test-tcg` runs the entire `check-tcg` suite inside
  Docker containers with cross-compilation toolchains for all targets.

**Insight for HELM:** Full kernel boot is the ultimate validation for
HELM's TCG backend.  The existing `assets/` directory contains an Alpine
rootfs image.  Once FS mode and the system instruction gap (Phases 1-5)
are closed, a kernel boot test becomes the gold standard.

---

## 3. QEMU TCG Architecture — How It All Fits Together

Understanding the testing layers requires understanding the TCG pipeline
that they exercise:

```
Guest binary (ELF or raw)
    │
    ▼
┌──────────────────────────────────────────────────────────┐
│ Frontend: target/<arch>/tcg/translate-*.c                │
│   Reads guest instructions                               │
│   Calls TCG op-emission functions (tcg_gen_*)            │
│   Uses .decode files for structured instruction parsing   │
└─────────────────────────┬────────────────────────────────┘
                          │ TCG IR (SSA-like intermediate repr.)
                          ▼
┌──────────────────────────────────────────────────────────┐
│ Middle-end: tcg/optimize.c                               │
│   Constant folding                                        │
│   Dead-code elimination                                   │
│   Copy propagation                                        │
│   Memory operation fusion                                 │
└─────────────────────────┬────────────────────────────────┘
                          │ Optimized TCG IR
                          ▼
          ┌───────────────┴───────────────┐
          ▼                               ▼
┌──────────────────┐           ┌──────────────────────┐
│ Native Backend   │           │ TCI (Interpreter)    │
│ tcg/<host>/      │           │ tcg/tci.c            │
│ x86_64, aarch64, │           │ Software fallback    │
│ arm, ppc, s390x, │           │ ~10× slower          │
│ riscv, loongarch │           │ Full IR coverage     │
└────────┬─────────┘           └──────────┬───────────┘
         │ Host machine code               │ Interpreted
         ▼                                 ▼
┌──────────────────────────────────────────────────────────┐
│ Execution Engine: accel/tcg/cpu-exec.c                   │
│   Translation block (TB) cache management                 │
│   TB chaining (direct jumps between blocks)               │
│   Exception handling and TB invalidation                  │
│   Self-modifying code detection                           │
│   icount mode (deterministic instruction counting)        │
└─────────────────────────┬────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────┐
│ Memory Subsystem: accel/tcg/cputlb.c                    │
│   Software TLB for guest→host address translation        │
│   MMIO dispatch to device models                          │
│   Exclusive load/store monitor (LDXR/STXR)               │
│   Watchpoint and breakpoint support                       │
└──────────────────────────────────────────────────────────┘
```

**Each testing layer targets specific pipeline stages:**

| Layer | Pipeline Stage | Bug Class |
|-------|---------------|-----------|
| 1 (IR/TCI) | Middle-end + interpreter | Wrong opcode semantics, optimization bugs |
| 2 (Decode) | Frontend parser | Wrong bit-field extraction, missing patterns |
| 3 (Plugins) | Execution engine hooks | Incorrect instrumentation callbacks |
| 4 (User guest) | Full pipeline, user mode | Wrong instruction semantics end-to-end |
| 5 (System guest) | Full pipeline + MMU + devices | Exception handling, timer, TLB bugs |
| 6 (GDB stub) | Debug interface | State visibility, single-step correctness |
| 7 (Kernel boot) | Everything | Subtle interaction bugs across millions of insns |

---

## 4. QEMU TCG Testing — Key Design Decisions

### 4.1 No Per-Opcode Unit Tests

QEMU does **not** have isolated unit tests for individual TCG IR opcodes
(e.g., "test that `TCG_OP_ADD_I64` computes 3 + 4 = 7").  Instead,
opcode correctness is validated transitively through guest programs that
exercise those opcodes.  The rationale:

- There are ~60 TCG ops × multiple operand widths × several addressing
  modes = hundreds of combinations.  Maintaining unit tests for each is
  impractical.
- Real bugs tend to involve **interactions** between ops (e.g., flag
  setting after a subtract followed by a conditional branch), not
  isolated op semantics.
- Guest programs like `sha1.c` and `sha512.c` exercise dozens of ops in
  realistic combinations, with known-good output to validate against.

**HELM diverges here:** HELM *does* have per-opcode unit tests in
`helm-tcg/src/tests/ir.rs` (12 tests) and `helm-tcg/src/tests/interp.rs`
(15 tests).  This is a strength — it catches IR-level bugs early, before
they propagate to integration tests.  The recommendation is to keep these
and add integration tests on top.

### 4.2 Golden-Reference Testing

Several QEMU tests use pre-computed reference output files:

- `fcvt.ref` — expected output from float conversion test
- `float_convd.ref`, `float_convs.ref` — double/single conversion tables
- `float_madds.ref` — fused multiply-add results
- `sha1.out` — SHA-1 digest of known input

The test runner executes the guest binary, captures stdout, and diffs
against the `.ref` file.  Any difference is a failure.  This approach is
simple, deterministic, and catches subtle floating-point rounding bugs
that are invisible in pass/fail tests.

### 4.3 Conditional Compilation by Feature

AArch64 tests use feature-detection at build time:

```makefile
$(call cc-option,-march=armv8.1-a+sve, CROSS_CC_HAS_SVE);
ifneq ($(CROSS_CC_HAS_SVE),)
  AARCH64_TESTS += sve-ioctls
endif
```

Tests for optional features (SVE, SVE2, MTE, SME, BTI, PAuth) are only
built and run if the cross-compiler supports the required `-march` flag.
This prevents build failures on older toolchains while still testing new
features when available.

### 4.4 Plugin-Test Rotation

Rather than running every plugin with every test (N×M explosion), QEMU
rotates plugins across tests using modular arithmetic:

```makefile
$(eval _plugin := $(word $(call mod_plus_one, $(_idx), $(NUM_PLUGINS)), $(PLUGINS)))
```

Each test gets exactly one plugin, cycling through the list.  Specific
test+plugin pairings (e.g., `patch.c` with `libpatch.so`) are handled
via `EXTRA_RUNS_WITH_PLUGIN`.

### 4.5 Docker-Based Cross-Compilation

`tests/docker/test-tcg` runs the entire test suite inside Docker
containers that provide cross-compilation toolchains for all supported
targets.  This ensures CI can test all architectures regardless of the
host.

---

## 5. HELM TCG Testing Strategy

Mapping QEMU's seven layers onto HELM, accounting for HELM's Rust
codebase and existing test infrastructure:

### 5.1 Layer 1 — IR and Interpreter Unit Tests (Existing)

**Current state:** 12 tests in `ir.rs`, 15 in `interp.rs`, 8 in
`context.rs` = 35 tests.

**Recommendation:** Extend with:
- Edge cases for each `TcgOp` variant (overflow, zero, max u64).
- System-instruction ops: `ReadSysReg`, `WriteSysReg`, `SvcExc`,
  `Eret`, `Wfi`, `DcZva`, `Tlbi`, `At`, `Barrier`, `Clrex`,
  `DaifSet/Clr`, `SetSpSel`, `Cfinv`, `HvcExc`, `SmcExc`, `BrkExc`,
  `HltExc` — already covered in `system.rs` (30+ tests).
- Threaded interpreter parity tests: same block, same inputs, assert
  identical output from both `exec_block` and `exec_threaded`.

**Priority:** ✅ Solid foundation.  Add edge cases incrementally.

### 5.2 Layer 2 — Decode Tree Parser Tests (Existing)

**Current state:** 74 tests in `helm-decode`, including QEMU compat.

**Recommendation:** Follow `docs/decode-test-plan.md` Phase 1 — import
all 46 `succ_`/`err_` decode files as test cases.  Most are already
covered or planned.

**Priority:** ✅ Well-covered.

### 5.3 Layer 3 — Plugin Integration Tests (Existing)

**Current state:** 81 tests in `helm-plugin`.

**Recommendation:** Add end-to-end tests that:
1. Run a small SE binary with `ExecLog` plugin enabled.
2. Assert instruction count matches expected value.
3. Run with `CacheSim` plugin and assert non-zero hit rate.

These mirror QEMU's `run-plugin-<test>-with-<plugin>` pattern.

**Priority:** 🟡 Medium — useful once TCG backend is primary.

### 5.4 Layer 4 — SE Guest Tests (Partially Existing)

**Current state:** `helm-tcg/src/tests/e2e_a64.rs` has end-to-end tests
that translate individual instructions, execute via TCG interpreter, and
compare against the reference `Aarch64Cpu` interpreter.

**Recommendation — Phase A (unit-level E2E):**
- Expand `e2e_a64.rs` to cover all instruction groups:
  - Data processing (immediate): ADD/SUB/ADDS/SUBS, MOV, bitfield, extract
  - Data processing (register): shifts, multiply, divide, CRC, CLZ/RBIT
  - Branches: B, BL, BR, BLR, RET, CBZ/CBNZ, TBZ/TBNZ, B.cond
  - Loads/stores: LDR/STR (imm, reg, pre, post), LDP/STP, LDRB/STRB,
    LDXR/STXR, LDAR/STLR
  - FP/SIMD: FMOV, FADD, FMUL, FCVT

**Recommendation — Phase B (binary-level E2E):**
- Compile minimal test programs (equivalent to QEMU's `sha1.c`,
  `sha512.c`) as static AArch64 binaries.
- Run through HELM's SE mode with both interpretive and TCG backends.
- Assert identical output (stdout capture) and exit code.
- Store golden reference outputs alongside binaries.

**Recommendation — Phase C (QEMU test reuse):**
- The C sources in `assets/qemu/tests/tcg/aarch64/` and
  `assets/qemu/tests/tcg/multiarch/` can be cross-compiled and run
  under HELM's SE mode once the syscall surface is sufficient.
- Start with the simplest tests (`sha1.c`, `sha512.c`, `fcvt.c`) and
  progressively enable more as syscall coverage grows.
- Use the existing `.ref` files for golden-reference comparison.

**Priority:** 🔴 High — this is the most impactful gap.

### 5.5 Layer 5 — FS System Tests (Future)

**Current state:** FS mode is planned but not yet implemented.

**Recommendation:** When FS mode is available:
1. Port QEMU's `minilib/` (tiny printf, memcpy) and `boot.S` startup
   assembly for AArch64.
2. Build bare-metal test images using the same `kernel.ld` linker
   script pattern.
3. Start with `vtimer.c` (tests system registers and generic timer)
   and `memory.c` (tests cross-page access and alignment).
4. Add an `interrupt.c` equivalent once GIC integration is complete.

These tests directly validate the system instruction implementations
tracked in `docs/tcg-system-instructions-gap.md`.

**Priority:** 🟠 Medium — blocked on FS mode.

### 5.6 Layer 6 — GDB Stub Tests (Future)

**Current state:** No gdbstub in HELM.

**Recommendation:** When a gdbstub is added, reuse QEMU's Python test
scripts (`test-sve.py`, `test-mte.py`, etc.) with HELM's gdbstub
endpoint.

**Priority:** ⚪ Low — not on the near-term roadmap.

### 5.7 Layer 7 — Full Kernel Boot (Future)

**Current state:** Alpine rootfs exists in `assets/`.  FS mode is
planned.

**Recommendation:**
1. First milestone: boot Linux kernel to the point where it prints
   `"Starting init:"` — validates exception handling, MMU setup, timer,
   and basic device I/O.
2. Second milestone: reach shell prompt — validates syscalls from
   user-space, context switching, and interrupt-driven scheduling.
3. Integrate as a `#[test] #[ignore]` test that can be run manually
   or in CI with a longer timeout.

**Priority:** 🟠 Medium — blocked on FS mode and GIC.

---

## 6. HELM TCG Test Matrix

Cross-referencing test layers with TCG pipeline components:

```
                    │ Decode │ Emit  │ IR    │ Interp │ Block  │ Plugin │ SE   │ FS
                    │ Tree   │ (A64) │ Ops   │        │ Cache  │        │ E2E  │ Boot
────────────────────┼────────┼───────┼───────┼────────┼────────┼────────┼──────┼──────
L1 IR unit tests    │        │       │  ✅   │   ✅   │        │        │      │
L2 Decode tests     │   ✅   │       │       │        │        │        │      │
L3 Plugin tests     │        │       │       │        │        │   ✅   │      │
L4 SE guest tests   │   ✅   │  ✅   │  ✅   │   ✅   │   ✅   │   ✅   │  ✅  │
L5 System tests     │   ✅   │  ✅   │  ✅   │   ✅   │   ✅   │        │      │  ✅
L6 GDB tests        │        │       │       │   ✅   │        │        │      │  ✅
L7 Kernel boot      │   ✅   │  ✅   │  ✅   │   ✅   │   ✅   │   ✅   │      │  ✅
────────────────────┼────────┼───────┼───────┼────────┼────────┼────────┼──────┼──────
HELM status         │  Done  │ Part. │ Done  │  Done  │ Part.  │  Done  │ Part.│ Plan
```

---

## 7. Concrete Next Steps

### 7.1 Immediate (No Blockers)

| # | Action | Target | Tests Added |
|---|--------|--------|-------------|
| 1 | Expand `e2e_a64.rs` with all DP-imm, DP-reg, branch, load/store instruction groups | `helm-tcg` | ~40 |
| 2 | Add threaded-vs-interpreter parity tests | `helm-tcg` | ~10 |
| 3 | Add `a64_emitter.rs` coverage for every `TranslateAction` variant | `helm-tcg` | ~15 |
| 4 | Import remaining `succ_`/`err_` decode files per decode-test-plan | `helm-decode` | ~30 |
| 5 | Cross-compile `sha1.c` and `sha512.c` as golden SE test binaries | `helm-engine` | 2 |

### 7.2 Near-Term (After SE Syscall Expansion)

| # | Action | Target | Tests Added |
|---|--------|--------|-------------|
| 6 | Run `fcvt.c` with golden `.ref` comparison | `helm-engine` | 1 |
| 7 | Run QEMU multiarch tests (`sha1`, `sha512`, `overflow`) | `helm-engine` | ~5 |
| 8 | Plugin E2E: run SE binary with `ExecLog`, validate insn count | `helm-plugin` | ~3 |

### 7.3 Medium-Term (After FS Mode)

| # | Action | Target | Tests Added |
|---|--------|--------|-------------|
| 9 | Port `minilib` + `boot.S` for bare-metal AArch64 tests | `helm-engine` | — |
| 10 | `vtimer.c` system test | `helm-engine` | 1 |
| 11 | `memory.c` cross-page access test | `helm-engine` | 1 |
| 12 | Linux kernel boot to `"Starting init:"` | `helm-engine` | 1 |

---

## 8. Test Infrastructure Requirements

### 8.1 Golden Binary Storage

Pre-compiled static AArch64 test binaries should live in
`assets/tests/se/aarch64/` alongside `.ref` output files.  The build
system should not require a cross-compiler for routine `make test` runs —
binaries are checked in.

### 8.2 SE Binary Test Harness

A reusable test helper in `helm-engine/src/tests/` that:
1. Loads a static ELF binary from `assets/`.
2. Runs it under SE mode with a specified backend (interpretive or TCG).
3. Captures stdout via the fd table.
4. Returns the exit code and stdout as a `String`.
5. Optionally compares stdout against a `.ref` file.

```rust
fn run_se_binary(path: &str, backend: SeBackend) -> (i32, String) { ... }

#[test]
fn sha1_tcg_matches_reference() {
    let (code, output) = run_se_binary("assets/tests/se/aarch64/sha1", SeBackend::Tcg);
    assert_eq!(code, 0);
    let expected = include_str!("assets/tests/se/aarch64/sha1.ref");
    assert_eq!(output.trim(), expected.trim());
}
```

### 8.3 Backend Comparison Harness

For maximum confidence, run every SE test binary with **both** backends
and assert identical results:

```rust
#[test]
fn sha1_backends_agree() {
    let (code_interp, out_interp) = run_se_binary("...", SeBackend::Interpretive);
    let (code_tcg, out_tcg) = run_se_binary("...", SeBackend::Tcg);
    assert_eq!(code_interp, code_tcg);
    assert_eq!(out_interp, out_tcg);
}
```

This mirrors QEMU's TCI-vs-JIT validation strategy, using the interpretive
backend as the reference oracle.

### 8.4 CI Timeout Configuration

Following QEMU's precedent (`TIMEOUT=120`), long-running tests (kernel
boot, large binary execution) should use `#[ignore]` and be run in CI
with explicit `cargo test -- --ignored --test-threads=1` and a
per-test timeout.

---

## 9. Comparison: QEMU vs HELM Testing Coverage

| Dimension | QEMU | HELM (Current) | HELM (Target) |
|-----------|------|----------------|---------------|
| IR opcode unit tests | None (implicit via guests) | 27 (ir.rs + interp.rs) | 40+ |
| System instruction tests | Implicit via kernel boot | 30+ (system.rs) | 50+ |
| Decode parser tests | 46 (.decode files) | 74 (helm-decode) | 100+ |
| A64 emitter tests | None (implicit via guests) | ~20 (a64_emitter.rs + system.rs) | 50+ |
| E2E instruction tests | None (implicit via guests) | ~15 (e2e_a64.rs) | 60+ |
| User-mode guest binaries | 50+ per arch | 0 binary tests | 10+ |
| System-mode bare-metal | 10+ per arch | 0 | 5+ |
| Plugin integration | 10 plugins × N tests | 81 plugin unit tests | 90+ |
| GDB stub tests | 10+ Python scripts | 0 | Future |
| Kernel boot test | Standard CI target | 0 | 1 |
| Backend comparison | TCI vs JIT | 0 | 10+ |
| Golden reference files | 5+ (.ref files) | 0 | 5+ |

---

## 10. Key Takeaways

1. **QEMU tests TCG primarily through guest programs**, not IR unit tests.
   HELM's explicit IR unit tests are a superset of QEMU's approach and
   should be maintained.

2. **Golden-reference testing** (capturing stdout and diffing against
   `.ref` files) is QEMU's most effective technique for catching subtle
   semantic bugs, especially in floating-point and flag-setting
   instructions.  HELM should adopt this pattern.

3. **Backend comparison** (TCI vs JIT in QEMU; interpretive vs TCG in
   HELM) is a powerful oracle that catches implementation bugs without
   requiring manually written expected values.

4. **System-mode tests are the gap.**  HELM has strong unit-level TCG
   tests but no bare-metal or kernel boot tests.  These are blocked on
   FS mode implementation but should be the first tests added when FS
   mode lands.

5. **QEMU's test assets are directly reusable.**  The C sources in
   `assets/qemu/tests/tcg/` can be cross-compiled and run under HELM's
   SE mode.  The `.decode` files are already used by `helm-decode`.
   The bare-metal test infrastructure (`minilib/`, `boot.S`, `kernel.ld`)
   can be ported for FS mode testing.

6. **Plugin-based testing** is under-utilised in HELM.  QEMU pairs every
   guest test with a randomly selected plugin, catching instrumentation
   bugs opportunistically.  HELM's plugin system is mature enough to
   adopt this pattern.

---

## Appendix A — Detailed Gap Analysis: HELM TCG Tests vs QEMU

*Added after comprehensive audit of all 221 HELM TCG tests.*

### A.1 Current HELM TCG Test Inventory (221 tests)

| File | Count | Scope |
|------|-------|-------|
| `ir.rs` | 12 | TcgOp variant construction and field access |
| `interp.rs` | 15 | Interpreter execution of core ops (ALU, load/store, branch, sext/zext, cmp) |
| `context.rs` | 8 | TcgContext emission helpers (temp allocation, op builders) |
| `block.rs` | 3 | TcgBlock struct construction, cloning |
| `a64_emitter.rs` | 35 | A64→TcgOp emitter: `TranslateAction` for 30+ instruction forms |
| `e2e_a64.rs` | 73 | Full pipeline: insn→emitter→interp, compared against `Aarch64Cpu` reference |
| `system.rs` | 40 | System instruction ops: MRS/MSR, DAIF, SVC/ERET, WFI, DC ZVA, TLBI, AT, barriers, HVC/SMC/BRK/HLT, CFINV |
| `parity.rs` | 35 | Threaded-vs-interpreter backend comparison (QEMU TCI-vs-JIT analogue) |
| **Total** | **221** | |

### A.2 TcgOp Coverage (47 variants, 47 tested)

Every `TcgOp` variant has at least one test across the combined test suite:

| Op Category | Variants | Files Covering |
|-------------|----------|----------------|
| Moves/consts | `Movi`, `Mov` | ir, interp, parity |
| Arithmetic | `Add`, `Sub`, `Mul`, `Div`, `Addi` | ir, interp, parity |
| Bitwise | `And`, `Or`, `Xor`, `Not`, `Shl`, `Shr`, `Sar` | interp, parity |
| Memory | `Load`, `Store` | ir, interp, parity |
| Control flow | `Br`, `BrCond`, `Label`, `GotoTb`, `ExitTb` | interp, parity |
| Comparisons | `SetEq`, `SetNe`, `SetLt`, `SetGe` | interp, parity |
| Extension | `Sext`, `Zext` | interp, parity |
| Registers | `ReadReg`, `WriteReg` | ir, interp, parity |
| Syscall | `Syscall` | ir, interp, parity |
| Sysregs | `ReadSysReg`, `WriteSysReg` | system |
| PSTATE | `DaifSet`, `DaifClr`, `SetSpSel`, `Cfinv` | system, parity |
| Exceptions | `SvcExc`, `Eret`, `HvcExc`, `SmcExc`, `BrkExc`, `HltExc` | system, parity |
| Cache/TLB | `DcZva`, `Tlbi`, `At`, `Barrier`, `Clrex` | system, parity |
| Hints | `Wfi` | system, parity |

**Gap: None** — all 47 TcgOp variants are exercised.

### A.3 AArch64 Instruction Coverage in E2E Tests

The `e2e_a64.rs` and `a64_emitter.rs` files cover:

**Well-tested (with reference comparison):**
- DP-immediate: `ADD`, `SUB`, `ADDS`, `SUBS`, `MOVZ`, `MOVN`, `AND`, `ORR`, `UBFM`/LSR, `SBFM`/ASR
- DP-register: `ADD`, `SUB`, `AND`, `ORR`, `LSLV`, `LSRV`, `MADD`/MUL, `UDIV`
- Conditional: `CSEL`, `CSINC`, `CSINV`, `CSNEG`, `CCMP`
- Branch: `B`, `BL`, `BR`, `BLR`, `CBZ`, `CBNZ`, `TBZ`, `TBNZ`, `B.cond` (EQ/NE/MI/VS)
- Load/store: `LDR`/`STR` (imm offset, pre-index, post-index, register), `LDP`/`STP`, `LDRB`/`STRB`, `LDRH`/`STRH`, `LDRSB`, `LDRSH`, `LDRSW`
- 32-bit: `ADD W`, `MOVZ W`

**Emitter-tested (TranslateAction only, no semantic check):**
- `NOP`, `DSB`, `LDXR`, `STXR`, `MOVK`, `ADD_ext`, `LDR_pre`, `STR_post`, `LDP`, `STP`, `LDR_reg_offset`, `B.cond`

**Not tested at all:**

| Instruction Group | Missing | QEMU Tests? |
|-------------------|---------|-------------|
| `EOR` immediate | No E2E or emitter test | Covered by guest programs |
| `ANDS` immediate (TST alias) | No E2E | Covered by guest programs |
| `MOVK` semantic execution | Emitter-only, no E2E value check | Covered by guest programs |
| `EXTR` / `ROR` | Not tested | Covered by guest programs |
| `ADR` / `ADRP` | Not tested | Covered by guest programs |
| `CLZ`, `CLS`, `RBIT`, `REV` | Not tested | Covered by guest programs |
| `SDIV` | Not tested | Covered by guest programs |
| `ASR`/`ROR` register | Not tested (only `LSLV`/`LSRV`) | Covered by guest programs |
| `ADC`/`SBC`/`ADCS`/`SBCS` | Not tested | Covered by guest programs |
| `SMULH`/`UMULH`/`SMADDL`/`UMADDL` | Not tested | Covered by guest programs |
| `MSUB` | Not tested | Covered by guest programs |
| `BIC`/`ORN`/`EON` | Not tested | Covered by guest programs |
| `RET` to custom register | Only default `RET` (X30) | Covered by guest programs |
| `B.cond` for all 16 conditions | Only EQ/NE/MI/VS tested | Covered by guest programs |
| `LDXR`/`STXR` exclusive semantics | Action-only test, no exclusivity | `lse2-fault.c` |
| `LDAR`/`STLR` acquire/release | Not tested | Covered by guest programs |
| `CAS`/`CASP` atomics | Not tested | `lse2-fault.c` |
| `LDUR`/`STUR` unscaled | Not tested | `memory.c` |
| Floating-point (`FADD`, `FMOV`, etc.) | Returns `Unhandled` — not emitted | `fcvt.c`, `float_madds.c` |
| SIMD/NEON | Not emitted | `sha1.c`, `sha512.c`, `test-aes.c` |
| SVE/SVE2 | Not emitted | `sve-ioctls.c`, `sve-str.c` |
| PAC (pointer authentication) | Not emitted | `pauth-1..5.c` |
| BTI (branch target identification) | Not emitted | `bti-1..3.c` |
| MTE (memory tagging) | Not emitted | `mte-1..8.c` |
| SME (scalable matrix) | Not emitted | `sme-*.c` |

### A.4 Layer-by-Layer Gap Analysis

#### Layer 1 — IR / Interpreter Unit Tests

| Aspect | QEMU | HELM | Gap |
|--------|------|------|-----|
| Per-opcode isolated tests | ✗ None | ✅ 12 (ir.rs) + 15 (interp.rs) | HELM is stronger |
| Interpreter correctness | ✗ Implicit via TCI running guests | ✅ 15 explicit tests | HELM is stronger |
| Edge cases (overflow, zero, MAX) | ✗ Implicit | ⚠️ `div_by_zero` only | **Gap:** add wrapping overflow, shift-by-64, sext-32-to-64 |
| TCI assertions (`CONFIG_DEBUG_TCG`) | ✅ Runtime asserts in TCI | ✗ No runtime invariant checks | **Gap:** no debug assertions in HELM interp |

#### Layer 2 — Decode Tree Parser

| Aspect | QEMU | HELM | Gap |
|--------|------|------|-----|
| Success cases | 8 `succ_*.decode` | ✅ 74 tests in helm-decode | HELM is stronger |
| Error cases | 38 `err_*.decode` | ✅ Covered in helm-decode | HELM is stronger |
| A64 instruction lookup | N/A (Python generator) | ✅ Full a64.decode integration | HELM is stronger |

#### Layer 3 — Plugin Tests

| Aspect | QEMU | HELM | Gap |
|--------|------|------|-----|
| Plugin API tests | 10 C plugins | ✅ 81 tests in helm-plugin | HELM is stronger (unit tests) |
| Plugin + guest pairing | Every test rotated with plugins | ✗ No guest-with-plugin tests | **Gap:** no E2E plugin-instrumented runs |
| Plugin output validation | `check-plugin-output.sh` regex | ✗ No output validation | **Gap:** no plugin output golden checks |
| Plugin kernel boot test | `test_tcg_plugins.py` boots kernel+plugin | ✗ Not possible yet (no FS mode) | **Gap:** blocked on FS mode |

#### Layer 4 — User-Mode Guest Tests

| Aspect | QEMU | HELM | Gap |
|--------|------|------|-----|
| Statically linked binary tests | 36 AArch64 + 23 multiarch C tests | ✗ 0 guest binary tests in TCG | **Critical gap** |
| Reference comparison (`sha1`, `sha512`) | `.ref` files + diff | ✗ No golden reference testing | **Critical gap** |
| Float conversion correctness | `fcvt.c` + `fcvt.ref` | ✗ FP not implemented in TCG | **Gap** (blocked on FP emitter) |
| Backend comparison (TCI vs JIT) | Implicit via `make check-tcg` under TCI | ✅ 35 parity tests (interp vs threaded) | HELM is stronger (explicit) |
| Instruction-level cross-check vs reference | ✗ None | ✅ 73 tests in e2e_a64.rs | HELM is stronger |
| Cross-compiled binary execution | `qemu-aarch64 ./test` | ⚠️ 3 tests in helm-engine (fish binary, interpretive only) | **Gap:** no TCG backend binary run |
| Timeout / hung test protection | 120s `TIMEOUT` | ✗ No timeout on tests | **Gap** |

#### Layer 5 — System-Mode Guest Tests

| Aspect | QEMU | HELM | Gap |
|--------|------|------|-----|
| Bare-metal boot harness | `boot.S` + `minilib` + `kernel.ld` | ✗ None | **Gap** (blocked on FS mode) |
| Timer test (`vtimer.c`) | ✅ Tests cntvoff/cntv_cval/cntv_ctl | ✗ None | **Gap** |
| MMU / address translation test | ✅ `asid2.c`, page-table setup | ✗ None | **Gap** |
| Cross-page memory access test | ✅ `memory.c` | ✗ None | **Gap** |
| Interrupt test | ✅ `interrupt.c` | ✗ None | **Gap** |
| System register from EL1 | ✅ `pauth-3.c`, MTE setup | ⚠️ Tested at op level (system.rs), not via guest code | Partial |

#### Layer 6 — GDB Stub Tests

| Aspect | QEMU | HELM | Gap |
|--------|------|------|-----|
| GDB attach + register read | ✅ `test-sve.py`, `test-mte.py` | ✗ No gdbstub | **Gap** (not yet on roadmap) |
| Single-step correctness | ✅ Tested via gdbstub scripts | ✗ None | **Gap** |

#### Layer 7 — Full Kernel Boot

| Aspect | QEMU | HELM | Gap |
|--------|------|------|-----|
| Linux kernel boot to console | ✅ Standard CI target | ✗ None | **Gap** (blocked on FS mode) |
| Boot with plugin instrumentation | ✅ `test_tcg_plugins.py` | ✗ None | **Gap** |
| Docker cross-compilation CI | ✅ `tests/docker/test-tcg` | ✗ None | **Gap** |

### A.5 Quantified Summary

| Category | QEMU Assets | HELM Tests | Coverage |
|----------|-------------|------------|----------|
| TCG IR opcode unit tests | 0 | 27 | **HELM > QEMU** |
| System instruction unit tests | 0 (implicit) | 40 | **HELM > QEMU** |
| Backend parity (TCI↔JIT / interp↔threaded) | implicit | 35 | **HELM > QEMU** |
| A64 emitter action tests | 0 (implicit) | 35 | **HELM > QEMU** |
| E2E instruction-vs-reference tests | 0 | 73 | **HELM > QEMU** |
| Decode parser tests | 46 decode files | 74 | **HELM > QEMU** |
| User-mode guest binary tests (C programs) | 59 | **0** | **QEMU >> HELM** |
| Golden reference file tests (.ref diffing) | 5+ | **0** | **QEMU >> HELM** |
| System-mode bare-metal tests | 9 | **0** | **QEMU >> HELM** |
| Plugin + guest pairing tests | ~59 rotated | **0** | **QEMU >> HELM** |
| GDB stub tests | 10+ scripts | **0** | **QEMU >> HELM** |
| Kernel boot test | 1+ | **0** | **QEMU >> HELM** |
| FP/SIMD instruction coverage | via `fcvt.c`, `sha1.c`, etc. | **0** (Unhandled) | **QEMU >> HELM** |
| Atomic/exclusive instruction tests | `lse2-fault.c` | action-only (LDXR/STXR) | **QEMU > HELM** |
| Cross-page/unaligned access tests | `memory.c` | **0** | **QEMU >> HELM** |

### A.6 Critical Gaps (Priority Order)

1. **No guest binary tests.** HELM has zero tests that compile and run a C program through the TCG backend. This is the single largest gap. QEMU's entire TCG correctness confidence comes from running real programs. The `sha1.c` and `sha512.c` tests exercise dozens of instruction interactions that unit tests cannot cover.

2. **No golden reference testing.** QEMU's `.ref` file diffing catches subtle floating-point and flag-setting bugs that pass/fail unit tests miss. HELM should adopt this pattern as soon as SE binary execution via TCG is functional.

3. **No FP/SIMD in TCG emitter.** The A64 emitter returns `Unhandled` for all floating-point and SIMD instructions. This blocks using most of QEMU's guest tests (`sha1.c`, `sha512.c`, `fcvt.c`, `test-aes.c`). Without FP, the TCG backend can only run integer-only workloads.

4. **No cross-page / unaligned memory tests.** QEMU's `memory.c` system test catches memory subsystem bugs by reading/writing across page boundaries at every access size. HELM has load/store round-trip tests but none that exercise page crossing.

5. **No exclusive monitor tests.** LDXR/STXR are tested only at the `TranslateAction` level (emitter produces ops). There is no test verifying that the exclusive monitor correctly fails STXR after an intervening store from another address.

6. **No FP/SIMD in TCG emitter.** Repeated for emphasis: this blocks 20+ QEMU guest tests and all real-world workloads compiled with `-O2` or higher (compilers auto-vectorise).

7. **No plugin-instrumented execution.** HELM has strong plugin unit tests but never runs a real workload with a plugin attached through the TCG path. QEMU catches subtle instrumentation bugs by pairing every guest test with a randomly selected plugin.

8. **No FS-mode / bare-metal tests.** Blocked on FS mode implementation. Once available, porting QEMU's `vtimer.c`, `memory.c`, and `interrupt.c` should be the first action.

### A.7 Strengths Where HELM Exceeds QEMU

1. **Explicit IR unit tests** (27 tests) — QEMU has none; it relies entirely on integration testing.

2. **Instruction-level reference comparison** (73 e2e_a64 tests) — each AArch64 instruction is executed through both TCG and the interpretive `Aarch64Cpu`, with register-by-register comparison. QEMU has no equivalent.

3. **Explicit backend parity tests** (35 parity tests) — HELM's interp↔threaded comparison is QEMU's TCI↔JIT concept, but implemented as explicit assertions rather than implicit "run the same suite twice."

4. **System instruction unit tests** (40 tests) — HELM tests MRS/MSR, SVC/ERET, WFI, DC ZVA, TLBI, AT, barriers, and exception generation at the op level with expected-value assertions. QEMU validates these only transitively through kernel boot.

5. **Decode parser tests** (74 tests) — more than QEMU's 46 decode files, with additional structural assertions.
