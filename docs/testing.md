# helm-ng Developer Testing Guide

> Cross-references: [`docs/architecture/traits.md`](architecture/traits.md) | [`docs/api.md`](api.md)

---

## Table of Contents

1. [Testing Philosophy](#1-testing-philosophy)
2. [Test Infrastructure Setup](#2-test-infrastructure-setup)
3. [ISA Unit Tests: RISC-V](#3-isa-unit-tests-risc-v)
4. [ISA Unit Tests: ARM AArch64](#4-isa-unit-tests-arm-aarch64)
5. [Memory System Tests](#5-memory-system-tests)
6. [Integration Tests: Full Programs](#6-integration-tests-full-programs)
7. [Differential Testing Against QEMU](#7-differential-testing-against-qemu)
8. [Property-Based Testing with proptest](#8-property-based-testing-with-proptest)
9. [Benchmark Harness with criterion](#9-benchmark-harness-with-criterion)
10. [CI Pipeline](#10-ci-pipeline)

---

## 1. Testing Philosophy

### What "Correctness" Means for a Simulator

A simulator is not correct because its internal data structures look reasonable. It is correct because executing a sequence of instructions produces the same **architectural state** that real hardware — or a trusted reference — would produce. Architectural state is the externally observable snapshot of a hart: integer registers, floating-point registers, the program counter, and CSRs. Everything else — micro-op queues, cache hit/miss state, branch-predictor tables — is implementation detail. Tests inspect architectural state, never internal microarchitectural state.

This distinction matters for how you write tests. After executing an `ADD` instruction, you do not check that the ALU pipeline stage fired; you check that `rd` contains the expected sum and that `pc` advanced by exactly 4. If both are true, the instruction is correct by definition. If either is wrong, the implementation is wrong regardless of how plausible the internal logic looks.

### The Three Validation Layers

**Layer 1 — Unit (one instruction at a time).** Each instruction is tested in isolation: load known values into source registers, execute one step, assert destination registers and PC are exactly right. Unit tests catch encoding bugs, sign-extension errors, overflow behavior, and wrong opcode dispatch. They are fast (microseconds each), deterministic, and require no external tooling. Write a unit test for every instruction variant — not just the happy path. Test edge cases: shift by 0, shift by 31, add with overflow, load from address 0.

**Layer 2 — Integration (a complete program).** Run a compiled binary end-to-end through the simulator. This catches bugs that only appear when instructions interact: a store followed by a load, a branch whose target contains a load-use hazard, a syscall that modifies memory visible to subsequent instructions. Integration tests require either a cross-compiler (`riscv64-unknown-elf-gcc`, `aarch64-linux-gnu-gcc`) or a pre-compiled binary corpus. Mark them `#[ignore]` if they need external tools so that `cargo test` passes on machines without the full toolchain.

**Layer 3 — Differential (versus a reference).** Run the same binary through both helm-ng and a reference implementation (QEMU, Spike, or another trusted simulator). Compare exit codes, stdout, and if possible, final register state. Differential testing scales: once the harness exists, every binary in your test suite becomes a differential test automatically. It catches bugs that your unit tests cannot — cases where your implementation is internally consistent but disagrees with real hardware behavior.

### The Differential Testing Principle: QEMU as the Oracle

The ISA specs are the authoritative definition of correct behavior, but specs are ambiguous in corner cases, contain errata, and describe behavior that differs from what Linux binaries actually rely on. QEMU is a better oracle for SE-mode testing because:

- QEMU has been validated against real hardware across millions of user programs.
- QEMU's behavior in unspecified corners (misaligned access handling, certain CSR reads) matches what Linux programs expect, not just what the spec says.
- If helm-ng and QEMU agree, the result is almost certainly correct even if neither matches the spec's stated behavior for that corner case.

Use the spec to understand what *should* happen. Use QEMU to determine what *does* happen in practice and what running Linux software depends on. When they conflict, investigate — but default to matching QEMU for SE-mode correctness.

### Timing Accuracy vs Functional Correctness: Test Them Separately

A functionally correct simulator with wrong timing is still functionally correct. A timing-accurate simulator with wrong register state is broken. Never conflate the two. The testing strategy reflects this:

- **Functional tests** (`cargo test`) never assert anything about wall-clock time, cycle counts, or performance. They assert architectural state only.
- **Timing tests** (criterion benchmarks + FE-mode cycle-count comparisons) never assert architectural state. They assert throughput, latency, and cycle accuracy.

If you mix them — if a functional test fails because the simulator is slower than expected — you will get false negatives on correct implementations running on loaded CI machines. Keep the layers clean.

### When to Use Which Test Type

| Situation | Test type |
|-----------|-----------|
| Adding a new instruction | Unit test for every variant |
| Fixing a register-file bug | Unit test demonstrating the bug, then fix |
| Implementing a new syscall | Integration test with a small C program that calls it |
| Suspecting interaction between load/store and branch | Integration test |
| Validating a whole ISA extension | Differential test against QEMU + riscv-tests corpus |
| Ensuring decoder never panics | Property-based test |
| Measuring instruction throughput | Criterion benchmark |
| Detecting performance regressions | Nightly CI benchmark run |

---

## 2. Test Infrastructure Setup

### Cargo.toml Workspace

```toml
# Cargo.toml (workspace root)
[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.dev-dependencies]
proptest = "1"
criterion = { version = "0.5", features = ["html_reports"] }

[profile.bench]
opt-level = 3
lto = "thin"
codegen-units = 1
```

Each crate that has integration tests or benchmarks pulls from workspace dev-dependencies:

```toml
# crates/sim-core/Cargo.toml
[dev-dependencies]
proptest.workspace = true
criterion.workspace = true
tempfile = "3"
```

### Shared Test Helpers: `tests/common/mod.rs`

Place this module at the workspace level so all integration test files can use it via `mod common;`.

```rust
// tests/common/mod.rs

use helm_ng::memory::{MemoryMap, MemoryRegion};
use helm_ng::sim::{HelmEngine, Virtual};
use helm_ng::timing::Virtual;

/// Default memory layout for SE-mode unit tests.
///
/// 0x1000..0x2000 — text segment (executable)
/// 0x8000_0000..0x8001_0000 — data/stack segment
pub const TEXT_BASE: u64 = 0x1000;
pub const DATA_BASE: u64 = 0x8000_0000;
pub const TEXT_SIZE: u64 = 0x1000;
pub const DATA_SIZE: u64 = 0x1_0000;

/// Build a minimal RISC-V SE-mode kernel ready for unit testing.
///
/// The kernel starts with PC = TEXT_BASE and all registers zeroed.
/// Text and data regions are pre-allocated as RAM.
pub fn make_riscv_se_kernel() -> HelmEngine<Virtual> {
    let mut mem = MemoryMap::new();
    mem.add_region(
        TEXT_BASE,
        TEXT_SIZE,
        MemoryRegion::Ram {
            data: vec![0u8; TEXT_SIZE as usize],
            executable: true,
            writable: false,
        },
    );
    mem.add_region(
        DATA_BASE,
        DATA_SIZE,
        MemoryRegion::Ram {
            data: vec![0u8; DATA_SIZE as usize],
            executable: false,
            writable: true,
        },
    );
    let mut kernel = HelmEngine::new_riscv64(mem, Virtual::default());
    kernel.arch_state_mut().write_pc(TEXT_BASE);
    kernel
}

/// Build a minimal AArch64 SE-mode kernel with the same memory layout.
pub fn make_aarch64_se_kernel() -> HelmEngine<Virtual> {
    let mut mem = MemoryMap::new();
    mem.add_region(
        TEXT_BASE,
        TEXT_SIZE,
        MemoryRegion::Ram {
            data: vec![0u8; TEXT_SIZE as usize],
            executable: true,
            writable: false,
        },
    );
    mem.add_region(
        DATA_BASE,
        DATA_SIZE,
        MemoryRegion::Ram {
            data: vec![0u8; DATA_SIZE as usize],
            executable: false,
            writable: true,
        },
    );
    let mut kernel = HelmEngine::new_aarch64(mem, Virtual::default());
    kernel.arch_state_mut().write_pc(TEXT_BASE);
    kernel
}

/// Write a sequence of 32-bit little-endian instruction words into memory
/// starting at `base`. Panics if the write fails (indicates a setup error
/// in the test, not a simulator bug).
pub fn load_u32_instructions(mem: &mut MemoryMap, base: u64, insns: &[u32]) {
    for (i, &insn) in insns.iter().enumerate() {
        let addr = base + (i as u64) * 4;
        let bytes = insn.to_le_bytes();
        mem.write_bytes(addr, &bytes)
            .unwrap_or_else(|e| panic!("load_u32_instructions: write to {addr:#x} failed: {e:?}"));
    }
}

/// Run exactly `n` instructions, stopping early if an exception is raised.
/// Returns the exception if one occurred before `n` steps completed.
pub fn run_n_insns(
    kernel: &mut HelmEngine<Virtual>,
    n: u64,
) -> Option<helm_ng::sim::HartException> {
    for _ in 0..n {
        match kernel.step_once() {
            Ok(()) => {}
            Err(ex) => return Some(ex),
        }
    }
    None
}

/// Assert that integer register `reg` equals `expected`.
/// Panics with a descriptive message showing both values in hex.
#[track_caller]
pub fn assert_ireg(kernel: &HelmEngine<Virtual>, reg: usize, expected: u64) {
    let actual = kernel.arch_state().read_int(reg);
    assert_eq!(
        actual, expected,
        "x{reg}: expected {expected:#018x}, got {actual:#018x}"
    );
}

/// Assert that the PC equals `expected`.
#[track_caller]
pub fn assert_pc(kernel: &HelmEngine<Virtual>, expected: u64) {
    let actual = kernel.arch_state().read_pc();
    assert_eq!(
        actual, expected,
        "pc: expected {expected:#018x}, got {actual:#018x}"
    );
}
```

---

## 3. ISA Unit Tests: RISC-V

All RISC-V instruction encodings below follow the RV64I specification (Volume I, Unprivileged ISA). Encodings are given as hexadecimal literals with comments showing the field breakdown.

Create the test file at `crates/sim-core/tests/riscv_isa.rs`.

```rust
// crates/sim-core/tests/riscv_isa.rs

mod common;
use common::*;
use helm_ng::sim::HartException;

// ────────────────────────────────────────────────────────────────────────────
// ADD rd, rs1, rs2
//
// Encoding (R-type):
//   [31:25] funct7 = 0000000
//   [24:20] rs2
//   [19:15] rs1
//   [14:12] funct3 = 000
//   [11:7]  rd
//   [6:0]   opcode = 0110011 (OP)
//
// ADD x3, x1, x2 = 0x00208_1B3
//   funct7=0000000 rs2=00010 rs1=00001 funct3=000 rd=00011 opcode=0110011
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_add_basic() {
    let mut kernel = make_riscv_se_kernel();
    kernel.arch_state_mut().write_int(1, 5);
    kernel.arch_state_mut().write_int(2, 3);

    // ADD x3, x1, x2
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x0020_81B3]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    assert_ireg(&kernel, 3, 8);
    assert_pc(&kernel, TEXT_BASE + 4);
}

#[test]
fn test_add_overflow_wraps() {
    // RISC-V ADD is modular — overflow wraps silently, no trap.
    let mut kernel = make_riscv_se_kernel();
    kernel.arch_state_mut().write_int(1, u64::MAX);
    kernel.arch_state_mut().write_int(2, 1);

    // ADD x3, x1, x2
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x0020_81B3]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    assert_ireg(&kernel, 3, 0); // wraps to 0
    assert_pc(&kernel, TEXT_BASE + 4);
}

#[test]
fn test_add_rd_zero_is_discarded() {
    // Writing to x0 is a no-op; x0 always reads as 0.
    let mut kernel = make_riscv_se_kernel();
    kernel.arch_state_mut().write_int(1, 42);
    kernel.arch_state_mut().write_int(2, 7);

    // ADD x0, x1, x2 — rd=00000
    // 0x00208_033: funct7=0 rs2=2 rs1=1 funct3=0 rd=0 opcode=0110011
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x0020_8033]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    assert_ireg(&kernel, 0, 0); // x0 unchanged
    assert_pc(&kernel, TEXT_BASE + 4);
}

// ────────────────────────────────────────────────────────────────────────────
// ADDI rd, rs1, imm
//
// Encoding (I-type):
//   [31:20] imm[11:0]
//   [19:15] rs1
//   [14:12] funct3 = 000
//   [11:7]  rd
//   [6:0]   opcode = 0010011 (OP-IMM)
//
// ADDI x1, x0, 42 = 0x02A0_0093
//   imm=0x02A rs1=00000 funct3=000 rd=00001 opcode=0010011
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_addi_positive_immediate() {
    let mut kernel = make_riscv_se_kernel();
    // x0 = 0 always; ADDI x1, x0, 42

    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x02A0_0093]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    assert_ireg(&kernel, 1, 42);
    assert_pc(&kernel, TEXT_BASE + 4);
}

#[test]
fn test_addi_negative_immediate() {
    // ADDI with negative immediate: imm = -1 = 0xFFF in 12 bits.
    // ADDI x1, x0, -1 = 0xFFF0_0093
    let mut kernel = make_riscv_se_kernel();

    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0xFFF0_0093]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    // -1 sign-extended to 64 bits = 0xFFFF_FFFF_FFFF_FFFF
    assert_ireg(&kernel, 1, u64::MAX);
    assert_pc(&kernel, TEXT_BASE + 4);
}

#[test]
fn test_addi_sign_extension() {
    // ADDI x2, x0, -2048 (most negative 12-bit immediate)
    // imm[11:0] = 0x800, ADDI x2, x0, -2048 = 0x8000_0113
    let mut kernel = make_riscv_se_kernel();

    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x8000_0113]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    assert_ireg(&kernel, 2, (-2048i64) as u64);
    assert_pc(&kernel, TEXT_BASE + 4);
}

// ────────────────────────────────────────────────────────────────────────────
// LW rd, offset(rs1)  — Load Word (sign-extends to 64 bits in RV64)
//
// Encoding (I-type):
//   [31:20] imm[11:0]
//   [19:15] rs1
//   [14:12] funct3 = 010
//   [11:7]  rd
//   [6:0]   opcode = 0000011 (LOAD)
//
// LW x3, 0(x1) = 0x0000_A183
//   imm=0x000 rs1=00001 funct3=010 rd=00011 opcode=0000011
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_lw_basic() {
    let mut kernel = make_riscv_se_kernel();

    // Write 0xDEAD_BEEF at DATA_BASE
    kernel.memory_mut().write_bytes(DATA_BASE, &0xDEAD_BEEFu32.to_le_bytes()).unwrap();

    // x1 = DATA_BASE, then LW x3, 0(x1)
    kernel.arch_state_mut().write_int(1, DATA_BASE);
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x0000_A183]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    // LW sign-extends: 0xDEAD_BEEF has bit 31 set, so in RV64 it becomes
    // 0xFFFF_FFFF_DEAD_BEEF
    assert_ireg(&kernel, 3, 0xFFFF_FFFF_DEAD_BEEFu64);
    assert_pc(&kernel, TEXT_BASE + 4);
}

#[test]
fn test_lw_with_positive_offset() {
    let mut kernel = make_riscv_se_kernel();

    // Write 0x1234_5678 at DATA_BASE + 8
    kernel.memory_mut().write_bytes(DATA_BASE + 8, &0x1234_5678u32.to_le_bytes()).unwrap();

    // x1 = DATA_BASE, LW x2, 8(x1)
    // LW x2, 8(x1): imm=0x008 rs1=00001 funct3=010 rd=00010 opcode=0000011
    // = 0x0080_A103
    kernel.arch_state_mut().write_int(1, DATA_BASE);
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x0080_A103]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    // 0x1234_5678 has bit 31 clear, so sign-extension preserves the value
    assert_ireg(&kernel, 2, 0x1234_5678);
    assert_pc(&kernel, TEXT_BASE + 4);
}

// ────────────────────────────────────────────────────────────────────────────
// SW rs2, offset(rs1)  — Store Word
//
// Encoding (S-type):
//   [31:25] imm[11:5]
//   [24:20] rs2
//   [19:15] rs1
//   [14:12] funct3 = 010
//   [11:7]  imm[4:0]
//   [6:0]   opcode = 0100011 (STORE)
//
// SW x2, 0(x1) = 0x0020_A023
//   imm[11:5]=0000000 rs2=00010 rs1=00001 funct3=010 imm[4:0]=00000 opcode=0100011
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_sw_basic() {
    let mut kernel = make_riscv_se_kernel();

    kernel.arch_state_mut().write_int(1, DATA_BASE);
    kernel.arch_state_mut().write_int(2, 0xCAFE_BABE);

    // SW x2, 0(x1)
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x0020_A023]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    // Read back from memory directly
    let mut buf = [0u8; 4];
    kernel.memory().read_bytes(DATA_BASE, &mut buf).unwrap();
    assert_eq!(u32::from_le_bytes(buf), 0xCAFE_BABE);
    assert_pc(&kernel, TEXT_BASE + 4);
}

#[test]
fn test_sw_only_stores_low_32_bits() {
    let mut kernel = make_riscv_se_kernel();

    kernel.arch_state_mut().write_int(1, DATA_BASE);
    // High bits set in the 64-bit register value
    kernel.arch_state_mut().write_int(2, 0x1234_5678_ABCD_EF01);

    // SW x2, 0(x1) — must store only the low 32 bits
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x0020_A023]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    let mut buf = [0u8; 4];
    kernel.memory().read_bytes(DATA_BASE, &mut buf).unwrap();
    assert_eq!(u32::from_le_bytes(buf), 0xABCD_EF01);
}

// ────────────────────────────────────────────────────────────────────────────
// BEQ rs1, rs2, offset  — Branch if Equal
//
// Encoding (B-type):
//   [31]    imm[12]
//   [30:25] imm[10:5]
//   [24:20] rs2
//   [19:15] rs1
//   [14:12] funct3 = 000
//   [11:8]  imm[4:1]
//   [7]     imm[11]
//   [6:0]   opcode = 1100011 (BRANCH)
//
// BEQ x1, x2, +8 = 0x0020_8463
//   imm = 0x008 (+8 from PC), encoded as B-type
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_beq_taken() {
    let mut kernel = make_riscv_se_kernel();

    // x1 = x2 = 42 → branch is taken
    kernel.arch_state_mut().write_int(1, 42);
    kernel.arch_state_mut().write_int(2, 42);

    // BEQ x1, x2, +8 (branch offset = 8, target = TEXT_BASE + 8)
    // Encoding: imm=8 → imm[12]=0 imm[11]=0 imm[10:5]=000000 imm[4:1]=0100
    // 0x00208463
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x0020_8463]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    // Branch taken: PC = TEXT_BASE + 8
    assert_pc(&kernel, TEXT_BASE + 8);
}

#[test]
fn test_beq_not_taken() {
    let mut kernel = make_riscv_se_kernel();

    // x1 ≠ x2 → branch not taken, PC advances normally
    kernel.arch_state_mut().write_int(1, 42);
    kernel.arch_state_mut().write_int(2, 43);

    // BEQ x1, x2, +8
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x0020_8463]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    // Branch not taken: PC = TEXT_BASE + 4
    assert_pc(&kernel, TEXT_BASE + 4);
    // Registers unchanged
    assert_ireg(&kernel, 1, 42);
    assert_ireg(&kernel, 2, 43);
}

// ────────────────────────────────────────────────────────────────────────────
// JAL rd, offset  — Jump and Link
//
// Encoding (J-type):
//   [31]    imm[20]
//   [30:21] imm[10:1]
//   [20]    imm[11]
//   [19:12] imm[19:12]
//   [11:7]  rd
//   [6:0]   opcode = 1101111 (JAL)
//
// JAL x1, +8 = 0x008000EF
//   rd=00001, imm=8 (relative to current PC)
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_jal_jumps_and_links() {
    let mut kernel = make_riscv_se_kernel();

    // JAL x1, +8 — jump forward 8 bytes, save return address in x1
    // Encoding: 0x008000EF
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x0080_00EF]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    // rd = PC + 4 (return address = instruction after JAL)
    assert_ireg(&kernel, 1, TEXT_BASE + 4);
    // PC = TEXT_BASE + 8 (jump target)
    assert_pc(&kernel, TEXT_BASE + 8);
}

// ────────────────────────────────────────────────────────────────────────────
// ECALL  — Environment Call (syscall trap)
//
// Encoding: 0x00000073
// Raises HartException::Syscall with nr = a7, args from a0..a5
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_ecall_raises_syscall_exception() {
    let mut kernel = make_riscv_se_kernel();

    // Set up a syscall: nr=93 (exit) in x17(a7), arg0=0 in x10(a0)
    kernel.arch_state_mut().write_int(17, 93); // a7 = syscall number
    kernel.arch_state_mut().write_int(10, 0);  // a0 = exit code

    // ECALL = 0x00000073
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x0000_0073]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    let result = kernel.step_once();

    match result {
        Err(HartException::Syscall { nr, args }) => {
            assert_eq!(nr, 93, "expected syscall nr 93 (exit), got {nr}");
            assert_eq!(args[0], 0, "expected exit code 0, got {}", args[0]);
        }
        other => panic!("expected HartException::Syscall, got {other:?}"),
    }
}
```

---

## 4. ISA Unit Tests: ARM AArch64

AArch64 uses a fixed 32-bit instruction encoding. All instructions are little-endian on standard configurations. Create the test file at `crates/sim-core/tests/aarch64_isa.rs`.

```rust
// crates/sim-core/tests/aarch64_isa.rs

mod common;
use common::*;
use helm_ng::sim::HartException;

// ────────────────────────────────────────────────────────────────────────────
// ADD X3, X1, X2  — 64-bit register add
//
// Encoding:
//   [31]    sf = 1 (64-bit)
//   [30:29] op = 00 (ADD)
//   [28:24] 01011 (shifted register)
//   [23:22] shift = 00 (LSL)
//   [21]    0
//   [20:16] Rm = 00010 (X2)
//   [15:10] imm6 = 000000 (shift amount 0)
//   [9:5]   Rn = 00001 (X1)
//   [4:0]   Rd = 00011 (X3)
//
// ADD X3, X1, X2 = 0x8B020023
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_aarch64_add_x3_x1_x2() {
    let mut kernel = make_aarch64_se_kernel();
    kernel.arch_state_mut().write_int(1, 100);
    kernel.arch_state_mut().write_int(2, 200);

    // ADD X3, X1, X2
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x8B02_0023]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    assert_ireg(&kernel, 3, 300);
    assert_pc(&kernel, TEXT_BASE + 4);
}

#[test]
fn test_aarch64_add_64bit_width() {
    // Verify the 64-bit path: sum that exceeds 32-bit range
    let mut kernel = make_aarch64_se_kernel();
    kernel.arch_state_mut().write_int(1, 0xFFFF_FFFF);
    kernel.arch_state_mut().write_int(2, 1);

    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0x8B02_0023]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    assert_ireg(&kernel, 3, 0x1_0000_0000);
    assert_pc(&kernel, TEXT_BASE + 4);
}

// ────────────────────────────────────────────────────────────────────────────
// LDR X0, [X1, #8]  — Load Register (64-bit, unsigned offset)
//
// Encoding:
//   [31:30] size = 11 (64-bit)
//   [29:27] 111
//   [26]    V = 0 (integer register)
//   [25:24] 00
//   [23:22] opc = 01 (LOAD)
//   [21:10] imm12 = 000000000010 (offset/8 = 1, so offset = 8)
//   [9:5]   Rn = 00001 (X1)
//   [4:0]   Rt = 00000 (X0)
//
// LDR X0, [X1, #8] = 0xF9400420
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_aarch64_ldr_unsigned_offset() {
    let mut kernel = make_aarch64_se_kernel();

    // Write a known 64-bit value at DATA_BASE + 8
    let value: u64 = 0x0123_4567_89AB_CDEF;
    kernel.memory_mut().write_bytes(DATA_BASE + 8, &value.to_le_bytes()).unwrap();

    kernel.arch_state_mut().write_int(1, DATA_BASE);

    // LDR X0, [X1, #8]
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0xF940_0420]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    assert_ireg(&kernel, 0, 0x0123_4567_89AB_CDEF);
    assert_pc(&kernel, TEXT_BASE + 4);
}

// ────────────────────────────────────────────────────────────────────────────
// STR X0, [X1, #8]  — Store Register (64-bit, unsigned offset)
//
// Encoding (store variant, opc=00):
//   [31:30] size = 11 (64-bit)
//   [29:27] 111
//   [26]    V = 0
//   [25:24] 00
//   [23:22] opc = 00 (STORE)
//   [21:10] imm12 = 000000000010 (offset/8 = 1, so offset = 8)
//   [9:5]   Rn = 00001 (X1)
//   [4:0]   Rt = 00000 (X0)
//
// STR X0, [X1, #8] = 0xF9000420
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_aarch64_str_unsigned_offset() {
    let mut kernel = make_aarch64_se_kernel();

    kernel.arch_state_mut().write_int(0, 0xDEAD_BEEF_1234_5678);
    kernel.arch_state_mut().write_int(1, DATA_BASE);

    // STR X0, [X1, #8]
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0xF900_0420]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    let mut buf = [0u8; 8];
    kernel.memory().read_bytes(DATA_BASE + 8, &mut buf).unwrap();
    assert_eq!(u64::from_le_bytes(buf), 0xDEAD_BEEF_1234_5678);
    assert_pc(&kernel, TEXT_BASE + 4);
}

// ────────────────────────────────────────────────────────────────────────────
// CBZ X0, offset  — Compare and Branch if Zero
//
// Encoding:
//   [31]    sf = 1 (64-bit)
//   [30:25] 011010
//   [24]    op = 0 (CBZ, not CBNZ)
//   [23:5]  imm19 (signed offset in instructions, = offset/4)
//   [4:0]   Rt = 00000 (X0)
//
// CBZ X0, +8 (imm19 = 2): 0xB4000040
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_aarch64_cbz_taken_when_zero() {
    let mut kernel = make_aarch64_se_kernel();
    kernel.arch_state_mut().write_int(0, 0); // X0 = 0 → branch taken

    // CBZ X0, +8
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0xB400_0040]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    assert_pc(&kernel, TEXT_BASE + 8);
}

#[test]
fn test_aarch64_cbz_not_taken_when_nonzero() {
    let mut kernel = make_aarch64_se_kernel();
    kernel.arch_state_mut().write_int(0, 1); // X0 = 1 → branch not taken

    // CBZ X0, +8
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0xB400_0040]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    kernel.step_once().unwrap();

    assert_pc(&kernel, TEXT_BASE + 4);
}

// ────────────────────────────────────────────────────────────────────────────
// SVC #0  — Supervisor Call (syscall trap)
//
// Encoding: 0xD4000001
//   [31:21] 11010100 000
//   [20:5]  imm16 = 0x0000 (#0)
//   [4:2]   001
//   [1:0]   01
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_aarch64_svc_raises_syscall_exception() {
    let mut kernel = make_aarch64_se_kernel();

    // AArch64 Linux syscall convention: nr in X8, args in X0..X5
    kernel.arch_state_mut().write_int(8, 93);  // X8 = syscall nr (exit)
    kernel.arch_state_mut().write_int(0, 0);   // X0 = exit code

    // SVC #0
    load_u32_instructions(kernel.memory_mut(), TEXT_BASE, &[0xD400_0001]);
    kernel.arch_state_mut().write_pc(TEXT_BASE);

    let result = kernel.step_once();

    match result {
        Err(HartException::Syscall { nr, args }) => {
            assert_eq!(nr, 93, "expected syscall 93, got {nr}");
            assert_eq!(args[0], 0);
        }
        other => panic!("expected HartException::Syscall, got {other:?}"),
    }
}
```

---

## 5. Memory System Tests

Memory tests live at `crates/sim-core/tests/memory_system.rs`. These tests exercise `MemoryMap` directly, without a running kernel, to isolate memory bugs from execution bugs.

```rust
// crates/sim-core/tests/memory_system.rs

use helm_ng::memory::{MemFault, MemoryMap, MemoryRegion};

fn make_test_map() -> MemoryMap {
    let mut mem = MemoryMap::new();
    mem.add_region(
        0x8000_0000,
        0x1000,
        MemoryRegion::Ram {
            data: vec![0u8; 0x1000],
            executable: false,
            writable: true,
        },
    );
    mem
}

// ────────────────────────────────────────────────────────────────────────────
// Byte / halfword / word / doubleword read-write roundtrips
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_ram_byte_roundtrip() {
    let mut mem = make_test_map();
    mem.write(0x8000_0000, 1, 0xAB).unwrap();
    assert_eq!(mem.read(0x8000_0000, 1).unwrap(), 0xAB);
}

#[test]
fn test_ram_halfword_roundtrip() {
    let mut mem = make_test_map();
    mem.write(0x8000_0000, 2, 0xBEEF).unwrap();
    assert_eq!(mem.read(0x8000_0000, 2).unwrap(), 0xBEEF);
}

#[test]
fn test_ram_word_roundtrip() {
    let mut mem = make_test_map();
    mem.write(0x8000_0000, 4, 0xDEAD_BEEF).unwrap();
    assert_eq!(mem.read(0x8000_0000, 4).unwrap(), 0xDEAD_BEEF);
}

#[test]
fn test_ram_doubleword_roundtrip() {
    let mut mem = make_test_map();
    mem.write(0x8000_0000, 8, 0x0123_4567_89AB_CDEF).unwrap();
    assert_eq!(mem.read(0x8000_0000, 8).unwrap(), 0x0123_4567_89AB_CDEF);
}

#[test]
fn test_ram_partial_overwrite_preserves_neighbors() {
    let mut mem = make_test_map();
    // Write a known word, then overwrite one byte, verify the other bytes
    mem.write(0x8000_0000, 4, 0x1234_5678).unwrap();
    mem.write(0x8000_0001, 1, 0xAB).unwrap(); // overwrite byte 1
    // Byte order: little-endian → [0x78, 0x56, 0x34, 0x12] originally
    // After overwriting byte at offset 1: [0x78, 0xAB, 0x34, 0x12]
    assert_eq!(mem.read(0x8000_0000, 4).unwrap(), 0x1234_AB78);
}

// ────────────────────────────────────────────────────────────────────────────
// MMIO dispatch
// ────────────────────────────────────────────────────────────────────────────

use helm_ng::memory::{MmioHandler, MmioAccess};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct TestMmioHandler {
    last_write: Option<(u64, u64)>, // (offset, value)
    last_read: Option<u64>,         // offset
    read_return: u64,
}

impl MmioHandler for TestMmioHandler {
    fn read(&mut self, offset: u64) -> u64 {
        self.last_read = Some(offset);
        self.read_return
    }

    fn write(&mut self, offset: u64, value: u64) {
        self.last_write = Some((offset, value));
    }
}

#[test]
fn test_mmio_write_dispatched_to_handler() {
    let handler = Arc::new(Mutex::new(TestMmioHandler::default()));
    let mut mem = MemoryMap::new();
    mem.add_region(
        0xFFFF_0000,
        0x1000,
        MemoryRegion::Mmio {
            handler: handler.clone(),
        },
    );

    mem.write(0xFFFF_0008, 4, 0xDEAD_BEEF).unwrap();

    let h = handler.lock().unwrap();
    assert_eq!(
        h.last_write,
        Some((0x0008, 0xDEAD_BEEF)),
        "handler should receive offset=8, value=0xDEAD_BEEF"
    );
}

#[test]
fn test_mmio_read_dispatched_to_handler() {
    let handler = Arc::new(Mutex::new(TestMmioHandler {
        read_return: 0x42,
        ..Default::default()
    }));
    let mut mem = MemoryMap::new();
    mem.add_region(
        0xFFFF_0000,
        0x1000,
        MemoryRegion::Mmio {
            handler: handler.clone(),
        },
    );

    let val = mem.read(0xFFFF_0004, 4).unwrap();

    assert_eq!(val, 0x42);
    let h = handler.lock().unwrap();
    assert_eq!(h.last_read, Some(0x0004));
}

// ────────────────────────────────────────────────────────────────────────────
// Error conditions
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn test_unmapped_read_returns_fault() {
    let mem = make_test_map();
    // Address 0x0 is not mapped
    let result = mem.read(0x0000_0000, 4);
    assert!(
        matches!(result, Err(MemFault::UnmappedAddress { addr: 0x0000_0000 })),
        "expected UnmappedAddress, got {result:?}"
    );
}

#[test]
fn test_unmapped_write_returns_fault() {
    let mut mem = make_test_map();
    let result = mem.write(0x0000_0000, 4, 0xDEAD);
    assert!(matches!(result, Err(MemFault::UnmappedAddress { .. })));
}

#[test]
fn test_misaligned_halfword_returns_fault() {
    let mut mem = make_test_map();
    // Odd address for a 2-byte read — misaligned
    let result = mem.read(0x8000_0001, 2);
    assert!(
        matches!(result, Err(MemFault::Misaligned { addr: 0x8000_0001, size: 2 })),
        "expected Misaligned, got {result:?}"
    );
}

#[test]
fn test_misaligned_word_returns_fault() {
    let mut mem = make_test_map();
    let result = mem.read(0x8000_0002, 4); // 4-byte access at non-4-byte-aligned addr
    assert!(matches!(result, Err(MemFault::Misaligned { size: 4, .. })));
}

#[test]
fn test_misaligned_doubleword_returns_fault() {
    let mut mem = make_test_map();
    let result = mem.read(0x8000_0004, 8); // 8-byte access at non-8-byte-aligned addr
    assert!(matches!(result, Err(MemFault::Misaligned { size: 8, .. })));
}

#[test]
fn test_multiple_regions_dispatch_correctly() {
    let mut mem = MemoryMap::new();
    mem.add_region(
        0x1000,
        0x1000,
        MemoryRegion::Ram {
            data: vec![0xAA; 0x1000],
            executable: true,
            writable: false,
        },
    );
    mem.add_region(
        0x2000,
        0x1000,
        MemoryRegion::Ram {
            data: vec![0xBB; 0x1000],
            executable: false,
            writable: true,
        },
    );

    // Both regions are accessible; reads return data from the correct one
    assert_eq!(mem.read(0x1000, 1).unwrap(), 0xAA);
    assert_eq!(mem.read(0x2000, 1).unwrap(), 0xBB);

    // The gap between them is unmapped
    assert!(matches!(mem.read(0x1FFF, 1), Err(MemFault::UnmappedAddress { .. })));
}
```

---

## 6. Integration Tests: Full Programs

Integration tests run compiled ELF binaries through the full SE-mode execution path. They are kept in `tests/integration/` and marked `#[ignore]` when they require external tools.

```rust
// tests/integration/riscv_hello.rs

use helm_ng::loader::elf::load_riscv_elf;
use helm_ng::sim::{HartException, HelmEngine, Virtual};
use std::path::PathBuf;

/// Find a pre-compiled test binary in the project's test fixtures directory.
fn fixture(name: &str) -> PathBuf {
    let root = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(root).join("tests").join("fixtures").join(name)
}

#[test]
#[ignore = "requires riscv64-unknown-elf-gcc; run `make test-fixtures` first"]
fn test_hello_world_binary() {
    let binary = fixture("riscv64/hello_world");
    assert!(binary.exists(), "fixture not found: {}", binary.display());

    let (mem, entry_pc) = load_riscv_elf(&binary).expect("ELF load failed");
    let mut kernel = HelmEngine::new_riscv64(mem, Virtual::default());
    kernel.arch_state_mut().write_pc(entry_pc);

    // Capture stdout by installing a syscall hook that intercepts write(1, ...)
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    {
        let captured = captured.clone();
        kernel.set_syscall_hook(move |nr, args, mem| -> Option<u64> {
            // Linux RISC-V ABI: write = 64, args[0]=fd, args[1]=buf, args[2]=len
            if nr == 64 && args[0] == 1 {
                let mut buf = vec![0u8; args[2] as usize];
                mem.read_bytes(args[1], &mut buf).ok();
                captured.lock().unwrap().extend_from_slice(&buf);
                return Some(args[2]); // return byte count
            }
            None // fall through to default handler
        });
    }

    // Run until exit syscall or instruction limit
    let max_insns = 1_000_000u64;
    for _ in 0..max_insns {
        match kernel.step_once() {
            Ok(()) => {}
            Err(HartException::Syscall { nr: 93, args }) => {
                // exit(code)
                assert_eq!(args[0], 0, "program exited with non-zero code {}", args[0]);
                break;
            }
            Err(ex) => panic!("unexpected exception: {ex:?}"),
        }
    }

    let output = String::from_utf8(captured.lock().unwrap().clone())
        .expect("program output was not valid UTF-8");
    assert_eq!(output, "Hello, world!\n");
}
```

### riscv-tests Integration

The [riscv-tests](https://github.com/riscv-software-src/riscv-tests) suite is the canonical ISA validation corpus. Each test binary signals pass or fail by writing to a memory-mapped `tohost` address.

```rust
// tests/integration/riscv_tests.rs

use helm_ng::loader::elf::load_riscv_elf;
use helm_ng::memory::MemoryMap;
use helm_ng::sim::{HartException, HelmEngine, Virtual};
use std::path::{Path, PathBuf};

/// Address of the `tohost` symbol in riscv-tests binaries.
/// Writing 1 to this address signals test pass.
/// Writing any other value signals failure with that value as the test case id.
const TOHOST_ADDR: u64 = 0x8000_1000;

fn run_riscv_test(binary: &Path) -> Result<(), String> {
    let (mem, entry_pc) = load_riscv_elf(binary)
        .map_err(|e| format!("ELF load failed for {}: {e}", binary.display()))?;

    let mut kernel = HelmEngine::new_riscv64(mem, Virtual::default());
    kernel.arch_state_mut().write_pc(entry_pc);

    // Poll tohost after every instruction
    for _ in 0..10_000_000u64 {
        match kernel.step_once() {
            Ok(()) => {}
            Err(ex) => return Err(format!("unexpected exception: {ex:?}")),
        }

        // Check tohost
        if let Ok(val) = kernel.memory().read(TOHOST_ADDR, 8) {
            if val != 0 {
                return if val == 1 {
                    Ok(()) // pass
                } else {
                    Err(format!("test failed with tohost value {val:#x} (test case {})", val >> 1))
                };
            }
        }
    }

    Err("test timed out after 10M instructions".to_string())
}

macro_rules! riscv_test {
    ($name:ident, $binary:expr) => {
        #[test]
        #[ignore = "requires riscv-tests binaries; set RISCV_TESTS_DIR env var"]
        fn $name() {
            let dir = std::env::var("RISCV_TESTS_DIR")
                .unwrap_or_else(|_| "tests/fixtures/riscv-tests".to_string());
            let binary = PathBuf::from(&dir).join($binary);
            run_riscv_test(&binary).expect("riscv-test failed");
        }
    };
}

// RV64I integer tests
riscv_test!(rv64ui_add,    "rv64ui-p-add");
riscv_test!(rv64ui_addi,   "rv64ui-p-addi");
riscv_test!(rv64ui_and,    "rv64ui-p-and");
riscv_test!(rv64ui_andi,   "rv64ui-p-andi");
riscv_test!(rv64ui_auipc,  "rv64ui-p-auipc");
riscv_test!(rv64ui_beq,    "rv64ui-p-beq");
riscv_test!(rv64ui_bge,    "rv64ui-p-bge");
riscv_test!(rv64ui_bgeu,   "rv64ui-p-bgeu");
riscv_test!(rv64ui_blt,    "rv64ui-p-blt");
riscv_test!(rv64ui_bltu,   "rv64ui-p-bltu");
riscv_test!(rv64ui_bne,    "rv64ui-p-bne");
riscv_test!(rv64ui_jal,    "rv64ui-p-jal");
riscv_test!(rv64ui_jalr,   "rv64ui-p-jalr");
riscv_test!(rv64ui_lb,     "rv64ui-p-lb");
riscv_test!(rv64ui_lbu,    "rv64ui-p-lbu");
riscv_test!(rv64ui_lh,     "rv64ui-p-lh");
riscv_test!(rv64ui_lhu,    "rv64ui-p-lhu");
riscv_test!(rv64ui_lui,    "rv64ui-p-lui");
riscv_test!(rv64ui_lw,     "rv64ui-p-lw");
riscv_test!(rv64ui_lwu,    "rv64ui-p-lwu");
riscv_test!(rv64ui_ld,     "rv64ui-p-ld");
riscv_test!(rv64ui_or,     "rv64ui-p-or");
riscv_test!(rv64ui_ori,    "rv64ui-p-ori");
riscv_test!(rv64ui_sb,     "rv64ui-p-sb");
riscv_test!(rv64ui_sh,     "rv64ui-p-sh");
riscv_test!(rv64ui_sw,     "rv64ui-p-sw");
riscv_test!(rv64ui_sd,     "rv64ui-p-sd");
riscv_test!(rv64ui_sll,    "rv64ui-p-sll");
riscv_test!(rv64ui_slli,   "rv64ui-p-slli");
riscv_test!(rv64ui_slt,    "rv64ui-p-slt");
riscv_test!(rv64ui_slti,   "rv64ui-p-slti");
riscv_test!(rv64ui_sltiu,  "rv64ui-p-sltiu");
riscv_test!(rv64ui_sltu,   "rv64ui-p-sltu");
riscv_test!(rv64ui_sra,    "rv64ui-p-sra");
riscv_test!(rv64ui_srai,   "rv64ui-p-srai");
riscv_test!(rv64ui_srl,    "rv64ui-p-srl");
riscv_test!(rv64ui_srli,   "rv64ui-p-srli");
riscv_test!(rv64ui_sub,    "rv64ui-p-sub");
riscv_test!(rv64ui_xor,    "rv64ui-p-xor");
riscv_test!(rv64ui_xori,   "rv64ui-p-xori");

// Run all riscv-tests in a directory programmatically
#[test]
#[ignore = "requires riscv-tests binaries; set RISCV_TESTS_DIR env var"]
fn run_all_rv64ui() {
    let dir = std::env::var("RISCV_TESTS_DIR")
        .unwrap_or_else(|_| "tests/fixtures/riscv-tests".to_string());

    let entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {dir}: {e}"))
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("rv64ui-p-")
        })
        .collect();

    assert!(!entries.is_empty(), "no rv64ui-p-* files found in {dir}");

    let mut failures = Vec::new();
    for entry in entries {
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if let Err(e) = run_riscv_test(&path) {
            failures.push(format!("{name}: {e}"));
        }
    }

    if !failures.is_empty() {
        panic!("riscv-tests failures:\n{}", failures.join("\n"));
    }
}
```

---

## 7. Differential Testing Against QEMU

Differential testing is the highest-confidence validation method. It requires QEMU to be installed (`qemu-riscv64` for SE-mode RISC-V binaries, `qemu-aarch64` for AArch64).

### Shell Script: `scripts/diff_test.sh`

```bash
#!/usr/bin/env bash
# scripts/diff_test.sh — Differential test a directory of binaries
#
# Usage:
#   scripts/diff_test.sh tests/binaries/riscv64/
#   scripts/diff_test.sh tests/binaries/aarch64/
#
# Each binary is run through both helm-ng and QEMU.
# Exit codes and stdout are compared. Mismatches are reported.
#
# Dependencies: qemu-riscv64 (or qemu-aarch64), helm-ng in PATH or target/

set -euo pipefail

BINARY_DIR="${1:?usage: diff_test.sh <binary-dir>}"
HELMNG="${HELMNG:-./target/release/helm-ng}"
PASS=0
FAIL=0
ERRORS=()

for binary in "$BINARY_DIR"/*; do
    [[ -x "$binary" ]] || continue
    name="$(basename "$binary")"

    # Run with QEMU (reference)
    qemu_out=$(qemu-riscv64 "$binary" 2>/dev/null || true)
    qemu_exit=$?

    # Run with helm-ng
    helmng_out=$("$HELMNG" se "$binary" 2>/dev/null || true)
    helmng_exit=$?

    if [[ "$qemu_exit" == "$helmng_exit" && "$qemu_out" == "$helmng_out" ]]; then
        PASS=$(( PASS + 1 ))
    else
        FAIL=$(( FAIL + 1 ))
        ERRORS+=("MISMATCH: $name")
        if [[ "$qemu_exit" != "$helmng_exit" ]]; then
            ERRORS+=("  exit: qemu=$qemu_exit helm-ng=$helmng_exit")
        fi
        if [[ "$qemu_out" != "$helmng_out" ]]; then
            ERRORS+=("  stdout differs (qemu=$(echo "$qemu_out" | head -3 | tr '\n' '|'))")
        fi
    fi
done

echo "Differential test: $PASS passed, $FAIL failed"
for e in "${ERRORS[@]}"; do echo "$e"; done
[[ $FAIL -eq 0 ]]
```

### Rust Differential Test Framework: `tests/differential/mod.rs`

```rust
// tests/differential/mod.rs

use std::path::{Path, PathBuf};
use std::process::Command;

/// The result of running a binary in either helm-ng or QEMU.
#[derive(Debug, PartialEq)]
pub struct TestResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Run a binary through helm-ng in SE mode.
/// Returns None if helm-ng binary is not found (skip the test).
pub fn run_with_helmng(binary: &Path) -> Option<TestResult> {
    let helmng = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("release")
        .join("helm-ng");

    if !helmng.exists() {
        eprintln!("helm-ng release binary not found; run `cargo build --release` first");
        return None;
    }

    let output = Command::new(&helmng)
        .args(["se", binary.to_str().unwrap()])
        .output()
        .expect("failed to spawn helm-ng");

    Some(TestResult {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// Run a binary through QEMU user-mode emulation.
/// Returns None if qemu-riscv64 is not in PATH (skip the test).
pub fn run_with_qemu(binary: &Path) -> Option<TestResult> {
    // Detect ISA from binary path convention: .../riscv64/... or .../aarch64/...
    let path_str = binary.to_string_lossy();
    let qemu_bin = if path_str.contains("riscv64") {
        "qemu-riscv64"
    } else if path_str.contains("aarch64") {
        "qemu-aarch64"
    } else {
        "qemu-riscv64" // default
    };

    if which::which(qemu_bin).is_err() {
        eprintln!("{qemu_bin} not found in PATH; skipping differential test");
        return None;
    }

    let output = Command::new(qemu_bin)
        .arg(binary)
        .output()
        .expect("failed to spawn QEMU");

    Some(TestResult {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// Assert that helm-ng and QEMU produced equivalent results.
/// Panics with a diff if they disagree.
#[track_caller]
pub fn assert_equivalent(hn: &TestResult, qemu: &TestResult, binary_name: &str) {
    let mut mismatches = Vec::new();

    if hn.exit_code != qemu.exit_code {
        mismatches.push(format!(
            "exit code: helm-ng={} qemu={}",
            hn.exit_code, qemu.exit_code
        ));
    }

    if hn.stdout != qemu.stdout {
        let hn_str = String::from_utf8_lossy(&hn.stdout);
        let qemu_str = String::from_utf8_lossy(&qemu.stdout);
        mismatches.push(format!(
            "stdout differs:\n  helm-ng: {:?}\n  qemu:    {:?}",
            hn_str.chars().take(200).collect::<String>(),
            qemu_str.chars().take(200).collect::<String>(),
        ));
    }

    if !mismatches.is_empty() {
        panic!(
            "Differential mismatch for {}:\n{}",
            binary_name,
            mismatches.join("\n")
        );
    }
}
```

```rust
// tests/differential/riscv_binaries.rs
//
// Differential tests for the test binary corpus.
// Each test runs a binary through both helm-ng and QEMU, then compares results.

mod common;
use common::*;

fn fixture_dir() -> std::path::PathBuf {
    let root = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(root).join("tests").join("binaries").join("riscv64")
}

macro_rules! diff_test {
    ($test_name:ident, $binary:expr) => {
        #[test]
        #[ignore = "requires qemu-riscv64 and pre-compiled binary corpus"]
        fn $test_name() {
            let binary = fixture_dir().join($binary);
            let Some(hn) = run_with_helmng(&binary) else { return };
            let Some(qemu) = run_with_qemu(&binary) else { return };
            assert_equivalent(&hn, &qemu, $binary);
        }
    };
}

diff_test!(diff_hello_world,  "hello_world");
diff_test!(diff_fib,          "fibonacci");
diff_test!(diff_sort,         "sort_1000");
diff_test!(diff_string_ops,   "string_ops");
diff_test!(diff_malloc_free,  "malloc_free");
```

---

## 8. Property-Based Testing with proptest

Property-based tests find bugs in corners you did not think to test. They live alongside unit tests in `crates/sim-core/tests/properties.rs`.

```rust
// crates/sim-core/tests/properties.rs

use helm_ng::isa::riscv::decode as riscv_decode;
use helm_ng::memory::{MemFault, MemoryMap, MemoryRegion};
use proptest::prelude::*;

// ────────────────────────────────────────────────────────────────────────────
// Decoder must never panic on any 32-bit input
//
// The decoder may return Err(IllegalInstruction), but it must never panic,
// unwind, or overflow the stack. This test exercises every possible 32-bit
// encoding including undefined, reserved, and malformed instructions.
// ────────────────────────────────────────────────────────────────────────────

proptest! {
    #[test]
    fn riscv_decoder_no_panic(raw in 0u32..) {
        // Must not panic. May return Ok or Err — we don't care which.
        let _ = riscv_decode(raw);
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Memory: reads after writes return the same value (for aligned accesses)
// ────────────────────────────────────────────────────────────────────────────

fn size_mask(size: usize) -> u64 {
    match size {
        1 => 0xFF,
        2 => 0xFFFF,
        4 => 0xFFFF_FFFF,
        8 => u64::MAX,
        _ => unreachable!(),
    }
}

proptest! {
    #[test]
    fn memory_read_write_roundtrip(
        // Constrain addr to the mapped region (0x8000_0000..0x8001_0000)
        // leaving room for 8-byte accesses at the top
        addr in 0x8000_0000u64..0x8000_FFF8u64,
        val  in 0u64..,
        size in proptest::sample::select(vec![1usize, 2, 4, 8])
    ) {
        let mut mem = MemoryMap::new();
        mem.add_region(
            0x8000_0000,
            0x1_0000,
            MemoryRegion::Ram {
                data: vec![0u8; 0x1_0000],
                executable: false,
                writable: true,
            },
        );

        // Align the address to the access size
        let aligned = addr & !(size as u64 - 1);
        let masked_val = val & size_mask(size);

        mem.write(aligned, size, masked_val).unwrap();
        let read_back = mem.read(aligned, size).unwrap();

        prop_assert_eq!(
            read_back,
            masked_val,
            "addr={aligned:#018x} size={size} val={masked_val:#018x}"
        );
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Memory: write then read at a different size should return consistent bytes
// ────────────────────────────────────────────────────────────────────────────

proptest! {
    #[test]
    fn memory_byte_consistency(
        base in 0x8000_0000u64..0x8000_FFF0u64,
        val in 0u64..
    ) {
        let mut mem = MemoryMap::new();
        mem.add_region(
            0x8000_0000,
            0x1_0000,
            MemoryRegion::Ram {
                data: vec![0u8; 0x1_0000],
                executable: false,
                writable: true,
            },
        );

        let aligned = base & !7u64; // align to 8 bytes
        let full = val;
        mem.write(aligned, 8, full).unwrap();

        // The low 4 bytes read as a word should match low 32 bits
        let low_word = mem.read(aligned, 4).unwrap();
        prop_assert_eq!(low_word, full & 0xFFFF_FFFF);

        // The high 4 bytes should match bits [63:32]
        let high_word = mem.read(aligned + 4, 4).unwrap();
        prop_assert_eq!(high_word, (full >> 32) & 0xFFFF_FFFF);
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Instruction encoding: any instruction that decodes successfully
// should re-encode to the same bits (encode(decode(x)) == x)
// ────────────────────────────────────────────────────────────────────────────

proptest! {
    #[test]
    fn riscv_decode_encode_roundtrip(raw in 0u32..) {
        use helm_ng::isa::riscv::encode as riscv_encode;
        if let Ok(insn) = riscv_decode(raw) {
            let re_encoded = riscv_encode(&insn);
            prop_assert_eq!(
                re_encoded,
                raw,
                "decode then re-encode changed bits: {raw:#010x} → {re_encoded:#010x}"
            );
        }
        // If decode returns Err, that's fine — we skip re-encoding
    }
}
```

---

## 9. Benchmark Harness with criterion

Benchmarks live in `crates/sim-core/benches/`. They measure execution throughput and decode throughput — not functional correctness.

```rust
// crates/sim-core/benches/instruction_throughput.rs

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use helm_ng::memory::{MemoryMap, MemoryRegion};
use helm_ng::sim::{HelmEngine, Virtual};

const TEXT_BASE: u64 = 0x1000;
const DATA_BASE: u64 = 0x8000_0000;

fn make_kernel_with_program(insns: &[u32]) -> HelmEngine<Virtual> {
    let mut mem = MemoryMap::new();
    let text_size = (insns.len() * 4).max(0x1000) as u64;
    mem.add_region(
        TEXT_BASE,
        text_size,
        MemoryRegion::Ram {
            data: vec![0u8; text_size as usize],
            executable: true,
            writable: false,
        },
    );
    mem.add_region(
        DATA_BASE,
        0x1_0000,
        MemoryRegion::Ram {
            data: vec![0u8; 0x1_0000],
            executable: false,
            writable: true,
        },
    );

    for (i, &insn) in insns.iter().enumerate() {
        let addr = TEXT_BASE + (i as u64) * 4;
        mem.write_bytes(addr, &insn.to_le_bytes()).unwrap();
    }

    let mut kernel = HelmEngine::new_riscv64(mem, Virtual::default());
    kernel.arch_state_mut().write_pc(TEXT_BASE);
    kernel
}

/// Build a tight integer loop that runs N iterations.
/// Assembly equivalent:
///   li   x1, N        # loop counter
///   li   x2, 0        # accumulator
/// loop:
///   addi x2, x2, 1
///   addi x1, x1, -1
///   bne  x1, x0, loop # branch back if x1 != 0
fn integer_loop_program(n: u64) -> Vec<u32> {
    // li x1, N → ADDI x1, x0, N (for small N fitting in 12 bits)
    // For large N, use LUI + ADDI. Here we use a simple small-N variant
    // and set n ≤ 2047 for single-instruction load.
    assert!(n <= 2047, "use LUI+ADDI for large loop counts");
    let li_x1: u32 = ((n as u32) << 20) | (1 << 7) | 0x13; // ADDI x1, x0, n
    let li_x2: u32 = 0x0000_0113; // ADDI x2, x0, 0
    let addi_x2: u32 = 0x0011_0113; // ADDI x2, x2, 1
    let addi_x1: u32 = 0xFFF0_8093; // ADDI x1, x1, -1
    // BNE x1, x0, -8 (branch back 2 instructions = -8 bytes)
    // B-type: imm=-8 → imm[12]=1 imm[11]=1 imm[10:5]=111111 imm[4:1]=1000
    // 0xFE009CE3
    let bne_loop: u32 = 0xFE00_9CE3;
    vec![li_x1, li_x2, addi_x2, addi_x1, bne_loop]
}

/// Benchmark: raw instruction throughput on a tight integer loop.
/// Reports instructions/second for the SE-mode RISC-V interpreter.
fn bench_riscv_integer_loop(c: &mut Criterion) {
    let mut group = c.benchmark_group("riscv_integer_loop");

    for &n_insns in &[1_000u64, 10_000, 100_000, 1_000_000] {
        group.throughput(Throughput::Elements(n_insns));
        group.bench_with_input(
            BenchmarkId::from_parameter(n_insns),
            &n_insns,
            |b, &n| {
                // Loop body is 3 instructions (addi, addi, bne)
                // We want n total instruction executions
                let loop_iters = n / 3;
                let program = integer_loop_program(loop_iters.min(2047));

                b.iter(|| {
                    let mut kernel = make_kernel_with_program(&program);
                    kernel.run(n);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: memory throughput — sequential word stores into a data region.
fn bench_memory_store_throughput(c: &mut Criterion) {
    // SW x2, 0(x1); ADDI x1, x1, 4; BNE x1, x3, -8
    // x1 = DATA_BASE (start), x3 = DATA_BASE + N*4 (end), x2 = value
    let program: Vec<u32> = vec![
        0x0020_A023, // SW x2, 0(x1)
        0x0040_8093, // ADDI x1, x1, 4
        0xFE30_9CE3, // BNE x1, x3, -8
    ];

    c.bench_function("memory_store_throughput_64KB", |b| {
        b.iter(|| {
            let mut kernel = make_kernel_with_program(&program);
            kernel.arch_state_mut().write_int(1, DATA_BASE);
            kernel.arch_state_mut().write_int(2, 0xDEAD_BEEF);
            kernel.arch_state_mut().write_int(3, DATA_BASE + 0x1_0000);
            // 64KB / 4 bytes = 16384 stores, each costs 3 instructions
            kernel.run(16384 * 3);
        });
    });
}

/// Benchmark: instruction decode throughput.
/// Decodes 1M random-ish but structurally valid instruction words.
fn bench_riscv_decode_throughput(c: &mut Criterion) {
    use helm_ng::isa::riscv::decode;

    // Pre-generate a fixed set of valid-ish instruction words
    let insns: Vec<u32> = (0u32..1_000_000)
        .map(|i| {
            // Vary the bits in a structured way to exercise different opcodes
            let opcode = (i % 20) as u32 * 4; // rough opcode spread
            (i << 12) | opcode | 0x3
        })
        .collect();

    c.bench_function("riscv_decode_1M", |b| {
        b.iter(|| {
            for &raw in &insns {
                let _ = std::hint::black_box(decode(raw));
            }
        });
    });
}

criterion_group!(
    benches,
    bench_riscv_integer_loop,
    bench_memory_store_throughput,
    bench_riscv_decode_throughput
);
criterion_main!(benches);
```

Add the benchmark entry to `crates/sim-core/Cargo.toml`:

```toml
[[bench]]
name = "instruction_throughput"
harness = false
```

### What to Benchmark and Why

| Benchmark | Metric | Regression threshold |
|-----------|--------|----------------------|
| `riscv_integer_loop_1M` | Instructions/second | >5% slowdown blocks merge |
| `memory_store_throughput_64KB` | Bytes/second | >10% slowdown investigated |
| `riscv_decode_1M` | Decode calls/second | >5% slowdown blocks merge |
| `aarch64_integer_loop_1M` | Instructions/second | >5% slowdown blocks merge |

Run benchmarks locally before any performance-sensitive commit:

```bash
cargo bench --workspace 2>&1 | tee bench_results.txt

# Compare against a saved baseline
cargo bench --workspace -- --baseline main
```

---

## 10. CI Pipeline

The CI pipeline has three tiers: fast (every PR), integration (every PR, slower), and nightly. The fast tier must complete in under 2 minutes so developers get immediate feedback.

```yaml
# .github/workflows/ci.yml

name: CI

on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1

jobs:
  # ─── FAST (every PR, target <2 min) ────────────────────────────────────────
  fast:
    name: Fast checks
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy

      - name: Cache cargo registry
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry/index/
            ~/.cargo/registry/cache/
            ~/.cargo/git/db/
            target/
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}

      - name: Check formatting
        run: cargo fmt --all -- --check

      - name: Clippy (deny warnings)
        run: cargo clippy --workspace --all-targets -- -D warnings

      - name: Unit tests (lib + inline tests only)
        run: cargo test --workspace --lib

  # ─── INTEGRATION (every PR, target <10 min) ────────────────────────────────
  integration:
    name: Integration tests
    runs-on: ubuntu-latest
    needs: fast
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable

      - name: Cache cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry/index/
            ~/.cargo/registry/cache/
            ~/.cargo/git/db/
            target/
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}

      - name: Integration tests (no external tools required)
        run: cargo test --workspace --test '*'

      - name: Property-based tests
        run: cargo test --workspace --test properties -- --test-threads=4

  # ─── FULL (every PR, external tool dependent, skipped on fail) ─────────────
  full:
    name: Full tests (with external tools)
    runs-on: ubuntu-latest
    needs: integration
    continue-on-error: true  # do not block merge if toolchain unavailable
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable

      - name: Install RISC-V cross compiler
        run: |
          sudo apt-get update -q
          sudo apt-get install -y gcc-riscv64-unknown-elf gcc-aarch64-linux-gnu

      - name: Install QEMU user mode
        run: |
          sudo apt-get install -y qemu-user

      - name: Build test fixtures
        run: make test-fixtures

      - name: Tests requiring external tools
        run: |
          RISCV_TESTS_DIR=tests/fixtures/riscv-tests \
          cargo test --workspace -- --include-ignored --test-threads=8

      - name: Differential tests
        run: bash scripts/diff_test.sh tests/binaries/riscv64/

  # ─── NIGHTLY (nightly schedule, target <60 min) ────────────────────────────
  nightly:
    name: Nightly (benchmarks + full differential)
    runs-on: ubuntu-latest
    if: github.event_name == 'schedule' || github.event_name == 'workflow_dispatch'
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable

      - name: Cache cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry/index/
            ~/.cargo/registry/cache/
            ~/.cargo/git/db/
            target/
          key: ${{ runner.os }}-nightly-cargo-${{ hashFiles('**/Cargo.lock') }}

      - name: Install QEMU user mode
        run: sudo apt-get update -q && sudo apt-get install -y qemu-user

      - name: Build release binary
        run: cargo build --release --workspace

      - name: Criterion benchmarks
        run: |
          cargo bench --workspace 2>&1 | tee bench_results.txt
          # Upload results as artifact for trend tracking

      - name: Upload benchmark results
        uses: actions/upload-artifact@v4
        with:
          name: bench-results-${{ github.sha }}
          path: |
            bench_results.txt
            target/criterion/

      - name: Full differential test suite
        run: |
          bash scripts/diff_test.sh tests/binaries/riscv64/
          bash scripts/diff_test.sh tests/binaries/aarch64/

on:
  schedule:
    - cron: '0 3 * * *'  # 3am UTC nightly
  workflow_dispatch:      # also allow manual trigger
```

### Running Tests Locally

```bash
# Fast checks only (same as CI fast tier)
cargo fmt --all -- --check && cargo clippy --workspace -- -D warnings && cargo test --workspace --lib

# All unit + integration tests (no external tools)
cargo test --workspace

# Include tests that need cross-compilers / QEMU
RISCV_TESTS_DIR=~/riscv-tests cargo test --workspace -- --include-ignored

# Run a specific test by name
cargo test --workspace test_add_basic

# Run benchmarks and compare to a saved baseline
cargo bench --workspace -- --save-baseline main
# ... make changes ...
cargo bench --workspace -- --baseline main

# Run differential tests manually
bash scripts/diff_test.sh tests/binaries/riscv64/
```

### Environment Variables

| Variable | Purpose | Default |
|----------|---------|---------|
| `RISCV_TESTS_DIR` | Path to riscv-tests ELF directory | `tests/fixtures/riscv-tests` |
| `HELMNG` | Path to helm-ng binary for diff tests | `./target/release/helm-ng` |
| `RUST_BACKTRACE` | Enable backtraces on panic in tests | `1` |
| `PROPTEST_CASES` | Number of cases per proptest property | `256` (default) |

---

*Cross-references: [`docs/architecture/traits.md`](architecture/traits.md) for `TimingModel` and ISA trait definitions; [`docs/api.md`](api.md) for the full `HelmEngine`, `ArchState`, `MemoryMap`, and `HartException` API surface.*
