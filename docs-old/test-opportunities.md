# Test Opportunities

Crate-by-crate audit of existing test coverage and concrete gaps.
Each section lists what **is** tested, what **is not**, and specific test
ideas. Crates are ordered roughly by severity of the gap.

---

## helm-cli (0 tests)

**Source files:** `bin/helm.rs`, `bin/helm_arm.rs`, `bin/helm_system_arm.rs`
**Test files:** none

No tests exist. These are binary entry-points, but the CLI argument
parsing can still be unit-tested.

| Opportunity | Notes |
|---|---|
| Clap `Cli` struct parsing | Verify default values, required args, and `trailing_var_arg` for each binary. |
| `--max-insns` default | Assert the default is `100_000_000`. |
| ISA / mode enum round-trip | Parse `"arm64"`, `"se"`, etc. through `ValueEnum` and back. |
| Error on missing `--binary` | `helm` requires `--binary`; confirm a helpful error. |
| `.py` vs binary dispatch | `helm_arm` and `helm_system_arm` branch on `.py` extension — test the detection logic. |

---

## helm-isa (17 tests — no ARM coverage)

**Source files:** `arm/aarch64/decode.rs` (212 lines), `arm/aarch64/exec.rs`
(2916 lines), `arm/regs.rs` (175 lines), `arm/sysreg.rs`, `riscv/mod.rs`,
`x86/mod.rs`, `frontend.rs`
**Test files:** `frontend.rs`, `riscv.rs`, `x86.rs` — **no `arm.rs`**

The x86 and RISC-V stubs each have 6 tests and the `IsaFrontend` trait has
5 tests. The entire ARM backend — by far the largest module — is untested
at the unit level.

| Opportunity | Notes |
|---|---|
| `Aarch64Cpu` register accessors | `xn`, `set_xn`, `xn_sp`, `set_xn_sp`, `wn`, `set_wn`, `current_sp`, `set_current_sp` — straightforward value round-trips. |
| NZCV flag helpers | `set_nzcv` / `n()` / `z()` / `c()` / `v()` on `Aarch64Regs`. |
| `Aarch64Decoder::decode_insn` | Feed known instruction encodings (NOP, ADD-imm, B, etc.) and assert opcode + operand fields. |
| `Aarch64Cpu::step` | Requires an `AddressSpace` but can be tested with a small mapped region containing known instructions. |
| `set_se_mode` | Verify SE-mode flag toggle. |
| `ArmFrontend` via `IsaFrontend` trait | Like the existing x86/riscv tests but for ARM. |
| `Aarch64Regs` `Default` impl | Confirm zero-initialisation of GPRs, PC, SP. |
| `sysreg.rs` | System register read/write, EL0–EL3 state. |

---

## helm-memory (40 tests — no `mmu.rs` tests)

**Source files:** `address_space.rs`, `cache.rs`, `coherence.rs`, `mmu.rs`,
`tlb.rs`
**Test files:** `address_space.rs`, `cache.rs`, `coherence.rs`,
`subsystem.rs`, `tlb.rs` — **no `mmu.rs`**

`mmu.rs` is ~400 lines implementing the ARMv8 page-table walker
(`Granule`, `Pte`, `Permissions`, `walk`, `translate`,
`TranslationConfig::parse`). None of it is directly tested.

| Opportunity | Notes |
|---|---|
| `Granule` size/page_shift/bits_per_level | Each variant (4K / 16K / 64K) has deterministic answers. |
| `Pte` bit-field accessors | `is_valid`, `is_table`, `is_block`, `oa`, `ap`, `af`, `pxn`, `uxn`, `attr_indx`, `dbm` — construct PTE u64 values and assert. |
| `Permissions::from_pte` + `check` | Build PTEs with different AP / PXN / UXN bits; verify `check(el, is_write, is_fetch)`. |
| `TranslationConfig::parse` | Feed known TCR_EL1 values, assert `t0sz`, `t1sz`, `granule0`, `granule1`. |
| `select_ttbr` | Boundary test: VA in upper half → TTBR1, lower half → TTBR0. |
| `walk` happy path | Set up a minimal page table in a byte buffer and verify `WalkResult`. |
| `walk` faults | Invalid PTE at each level → correct `TranslationFault` variant + level. |
| `TranslationFault::to_fsc` / `level` | Enumerate variants and check FSC encoding. |
| `translate` integration | End-to-end VA → PA through walk + permission check. |

---

## helm-engine (63 tests — no loader or SE-subsystem unit tests)

**Source files:** `loader/elf64.rs` (280 lines), `loader/arm64_image.rs`
(213 lines), `se/classify.rs` (149 lines), `se/thread.rs` (229 lines),
`se/backend.rs`
**Test files:** `core_sim.rs`, `e2e_aarch64.rs`, `plugins.rs`, `sim.rs`,
`timing.rs` — **no `loader.rs`, `classify.rs`, `thread.rs`, `backend.rs`**

| Opportunity | Notes |
|---|---|
| `parse_arm64_header` | Feed a valid 64-byte ARM64 Image header and verify parsed fields. Also test the error path for invalid magic. |
| `classify_a64` | Pure function mapping a u32 instruction word to an `InsnClass`. Cover the major encoding groups (data-processing, branch, load/store, FP/SIMD). |
| `Scheduler` (thread) | `new`, `spawn`, `current_tid`, `live_count`, `any_runnable`, `block_current`, `exit_current`, `try_switch`. These are CPU-independent and highly testable. |
| `SeBackend` constructors | `interpretive()` vs `tcg()` return correct variant. |
| `load_elf` | Would need a tiny static ELF fixture; can at least test the error path for non-ELF input. |

---

## helm-device (242 tests — gaps in scheduler, loader, device, proto/)

**Source files (untested):** `scheduler.rs` (132 lines), `loader.rs`
(175 lines), `device.rs` (176 lines), `proto/amba.rs`, `proto/axi.rs`,
`proto/pci.rs`, `proto/i2c.rs`, `proto/spi.rs`, `proto/usb.rs`

The device crate has strong test coverage for bus, IRQ, MMIO, FDT, VirtIO,
and ARM device models. The following modules have no tests:

| Opportunity | Notes |
|---|---|
| `DeviceScheduler` | `new`, `add`, `step`, `run_until`, `num_threads`, `global_time`. Create a mock `TickableDevice` and verify scheduling order. |
| `DynamicDeviceLoader` | `register`, `available_devices`, `has_device`, `create_device`, `register_arm_builtins`. |
| `LegacyWrapper` | `new`, `inner`, `inner_mut` — wrap a test `MemoryMappedDevice`. |
| `AhbBus` / `ApbBus` | `attach`, `read`/`write` dispatch to the correct device, out-of-range access error. |
| `AxiBus` | Same as AHB — attach + read/write. |
| `PciBus` / `PciDevice` | Config-space read/write, BAR mapping, bridge latency. |
| `I2cBus` / `SpiBus` / `UsbBus` | Basic attach + transaction routing. |

---

## helm-translate (10 tests — no `block.rs` tests)

**Source files:** `block.rs`, `cache.rs`, `translator.rs`
**Test files:** `cache.rs` (8 tests), `translator.rs` (2 tests)

| Opportunity | Notes |
|---|---|
| `TranslatedBlock` construction | Verify field accessors (start_pc, ops, length). |
| Translator cache invalidation | Translate → invalidate → re-translate, confirm new translation is used. |
| Translator with different ISAs | If the translator is ISA-generic, test with stubs for ARM vs RISC-V frontends. |
| Cache eviction under pressure | Insert many blocks, verify LRU or similar eviction policy. |

---

## helm-tcg (35 tests — no `a64_emitter.rs` or `block.rs` tests)

**Source files:** `a64_emitter.rs`, `block.rs`, `context.rs`, `interp.rs`,
`ir.rs`
**Test files:** `context.rs` (8), `interp.rs` (15), `ir.rs` (12) — **no
`a64_emitter.rs` or `block.rs`**

| Opportunity | Notes |
|---|---|
| `A64TcgEmitter::translate_insn` | Feed known AArch64 encodings (NOP `0xd503201f`, ADD-imm, B, LDR, STR) and verify emitted TCG ops. |
| `TranslateAction` variants | Confirm `Continue` vs `EndBlock` for branch vs arithmetic instructions. |
| `TcgBlock` construction | Verify `new`, fields (start_pc, ops). |
| Emitter end-to-end | `new` → multiple `translate_insn` calls → `finish`, verify the resulting `TcgBlock`. |

---

## helm-plugin (81 tests — tests not in `src/tests/` directory)

**Note:** Tests live in a single `src/tests.rs` file rather than the
standard `src/tests/` directory. Coverage of the API, runtime, and
callback layers is solid.

| Opportunity | Notes |
|---|---|
| Builtin `ExecLog` | `new`, `lines()` after firing insn callbacks. |
| Builtin `HotBlocks` | `new`, `top(n)` returns sorted by frequency. |
| Builtin `HowVec` | Basic construction and callback wiring. |
| Builtin `SyscallTrace` | `new`, `entries()` after firing syscall callbacks. |
| Builtin `CacheSim` deeper | Current test just constructs; verify `l1d_hit_rate` after simulated accesses. |
| `DynamicPluginLoader` | `new`, `count`, `loaded_plugins` — at least test the no-plugin baseline. |
| Migrate to `src/tests/` directory | Aligns with the project convention (one test file per source module). |

---

## helm-llvm (167 tests — no `parser.rs` or `memory.rs` tests)

**Source files (untested):** `parser.rs` (594 lines), `memory.rs`
(225 lines)
**Test files:** all other modules are well-covered

| Opportunity | Notes |
|---|---|
| `LLVMParser::parse` | Parse minimal LLVM-IR strings (single `define`, empty function, function with arithmetic). |
| Parser error paths | Malformed IR → descriptive error. |
| `SimpleMemory` | `new`, `with_latency`, read/write round-trip, out-of-bounds error. |
| `HybridMemory` | Construction and basic read/write. |
| Memory latency model | Confirm configured load/store latencies are returned. |

---

## helm-syscall (31 tests — missing `generic.rs` and `freebsd/` tests)

**Source files (partially tested):** `os/linux/generic.rs`,
`os/freebsd/mod.rs`
**Test files:** `aarch64.rs`, `fd_table.rs`, `handler.rs`, `table.rs`

| Opportunity | Notes |
|---|---|
| `SyscallHandler::handle` (generic) | Test dispatch for different `IsaKind` values. |
| `SyscallHandler::new` | Verify ISA stored correctly. |
| FreeBSD stub | Even if unimplemented, a smoke test asserting the module compiles and stubs return expected errors. |
| `set_brk` / `set_tid` | Round-trip the configured values. |
| `try_sched_action` | Test clone / exit / futex action variants. |

---

## helm-pipeline (29 tests — fully covered by file, light on edge cases)

All source modules have corresponding test files. Coverage gaps are
functional rather than structural.

| Opportunity | Notes |
|---|---|
| `Scheduler::is_full` boundary | Insert exactly `capacity` entries, verify `is_full` transitions. |
| ROB wrap-around | Fill, commit half, fill again — test circular buffer correctness. |
| `RenameUnit` with many arch regs | Stress the free-list with repeated rename/free cycles. |
| Branch predictor update | Currently only `predict` is tested; add tests for training / update if the API supports it. |
| `Pipeline::new` | Verify sub-component capacities match `CoreConfig`. |

---

## helm-timing (38 tests — fully covered)

All five source modules have test files with good coverage.

| Opportunity | Notes |
|---|---|
| `TemporalDecoupler` with many cores | Current tests use 1–2 cores; test with 4+. |
| `EventQueue` priority inversion | Schedule events out of order, verify FIFO within same timestamp. |
| `SamplingController` multi-cycle warmup | Advance through warmup → measurement → cooldown → done across a realistic instruction count. |

---

## helm-core (32 tests — fully covered)

All source modules (`config`, `error`, `event`, `ir`, `types`) have
test files.

| Opportunity | Notes |
|---|---|
| `CacheConfig` edge cases | Zero-size cache, line_size > cache size. |
| `MicroOp` clone / equality | Verify `Clone` and `PartialEq` if derived. |
| `SimEvent` full variant coverage | Some variants may only appear in one test — ensure every variant is exercised. |

---

## helm-object (29 tests — fully covered)

| Opportunity | Notes |
|---|---|
| `Property` with all `PropertyType` variants | Test that validation rejects wrong-type values. |
| `Tree` deep nesting | Resolve paths 3+ levels deep. |
| `Registry` duplicate registration | Confirm behavior when registering the same type name twice. |

---

## helm-stats (21 tests — fully covered)

| Opportunity | Notes |
|---|---|
| `Counter` thread safety | Increment from multiple threads (uses atomics). |
| `StatsCollector` snapshot consistency | Record events, snapshot, verify totals. |
| `SimResults::to_json` schema | Parse the JSON output and validate field names. |

---

## helm-systemc (74 tests — fully covered)

This crate has excellent test coverage relative to its size.

| Opportunity | Notes |
|---|---|
| Clock edge cases | 0 Hz frequency, very large frequencies, overflow. |
| TLM large payloads | Transactions with multi-KB data buffers. |

---

## helm-decode (74 tests — fully covered)

Comprehensive tests including QEMU compatibility and conformance.

| Opportunity | Notes |
|---|---|
| Malformed decode-tree input | Fuzz-like edge cases: empty input, only comments, duplicate patterns. |
| `generate_decoder` with overlapping patterns | Verify conflict detection. |

---

## helm-python (0 Rust-side tests)

**Source:** `lib.rs` (519 lines of PyO3 bindings)
**Test files:** none (Python-side tests live in `python/tests/`)

Rust-side unit tests are impractical for PyO3 `#[pyclass]` types without
a Python interpreter. Coverage comes from `make test-python`. If
additional Python-side tests are desired:

| Opportunity | Notes |
|---|---|
| Python config round-trip | Build a `PlatformConfig` in Python, pass to Rust, read back. |
| Error propagation | Trigger a Rust error and verify the Python exception type and message. |
| Plugin wiring from Python | Register a plugin by name, run a trivial simulation, check stats. |

---

## Summary — highest-impact gaps

| Priority | Crate | Gap | Estimated new tests |
|---|---|---|---|
| 🔴 High | `helm-isa` | Entire ARM backend (decode, exec, regs) untested | 20–30 |
| 🔴 High | `helm-memory` | `mmu.rs` (page-table walker, PTE, permissions) untested | 15–20 |
| 🟠 Medium | `helm-engine` | Loader (ELF, ARM64 Image) and SE subsystem (classify, thread scheduler) | 15–20 |
| 🟠 Medium | `helm-device` | `scheduler.rs`, `loader.rs`, `device.rs`, entire `proto/` | 15–20 |
| 🟠 Medium | `helm-llvm` | `parser.rs` (594 lines) and `memory.rs` (225 lines) | 10–15 |
| 🟡 Low | `helm-tcg` | `a64_emitter.rs`, `block.rs` | 8–12 |
| 🟡 Low | `helm-translate` | `block.rs`, translator edge cases | 5–8 |
| 🟡 Low | `helm-plugin` | Builtin plugins, restructure to `src/tests/` | 10–15 |
| 🟡 Low | `helm-cli` | CLI arg parsing smoke tests | 5–8 |
| ⚪ Minor | `helm-syscall` | `generic.rs`, FreeBSD stub | 5–8 |
| ⚪ Minor | `helm-pipeline` | Edge-case coverage for existing modules | 5–8 |
