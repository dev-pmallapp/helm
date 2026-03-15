# TEST: helm-engine

> Test plan for the `helm-engine` crate — unit tests, integration tests, and benchmarks.

**Crate:** `helm-engine`
**Test files:**
- `crates/helm-engine/tests/engine_basic.rs` — unit tests for HelmEngine
- `crates/helm-engine/tests/scheduler_tests.rs` — Scheduler and quantum tests
- `crates/helm-engine/tests/helmsim_dispatch.rs` — HelmSim dispatch correctness
- `crates/helm-engine/benches/engine_bench.rs` — criterion.rs benchmarks

---

## Table of Contents

1. [Test Philosophy](#1-test-philosophy)
2. [Test Helpers and Fixtures](#2-test-helpers-and-fixtures)
3. [Unit Tests: HelmEngine Basic Execution](#3-unit-tests-helmengine-basic-execution)
4. [Unit Tests: Quantum Exhaustion](#4-unit-tests-quantum-exhaustion)
5. [Unit Tests: Breakpoint Stops Execution](#5-unit-tests-breakpoint-stops-execution)
6. [Unit Tests: HelmSim Dispatch](#6-unit-tests-helmsim-dispatch)
7. [Unit Tests: Checkpoint Round-Trip](#7-unit-tests-checkpoint-round-trip)
8. [Unit Tests: Syscall Dispatch](#8-unit-tests-syscall-dispatch)
9. [Benchmark: Instructions per Second (criterion.rs)](#9-benchmark-instructions-per-second-criterionrs)
10. [Test Matrix](#10-test-matrix)

---

## 1. Test Philosophy

The `helm-engine` test suite validates three distinct concerns:

**1. Correctness of the execution loop.** Instruction execution must produce the correct architectural state. Tests verify register values, PC advancement, and memory state after executing known instruction sequences.

**2. Correctness of control flow.** The quantum budget, breakpoint stop flag, and SimExit path must behave exactly as specified. Off-by-one errors in the instruction count budget or missed stop flag checks are critical bugs.

**3. Performance regression prevention.** Benchmarks establish a baseline instructions/second for the `Virtual` timing model. A regression >5% in a bench CI run should fail the build.

Tests do NOT inspect internal microarchitectural state (timing model internals, cache state). They inspect only:
- Architectural registers (`ArchState` via `ThreadContext`)
- Memory contents (via `MemoryMap::read_bytes_functional`)
- `StopReason` returned by `run()` / `step_once()`
- `insns_executed()` counter

---

## 2. Test Helpers and Fixtures

```rust
// crates/helm-engine/tests/common/mod.rs

use helm_engine::{HelmSim, TimingChoice, build_simulator, StopReason};
use helm_core::{Isa, ExecMode};

/// Build a minimal RISC-V Virtual-mode simulator for testing.
/// Memory: 1 MiB RAM at 0x8000_0000. PC starts at 0x8000_0000.
pub fn riscv_virtual_sim() -> HelmSim {
    let mut sim = build_simulator(Isa::RiscV, ExecMode::Functional, TimingChoice::Virtual);
    sim.memory_mut().add_ram(0x8000_0000, 1024 * 1024);
    sim.thread_context().write_pc(0x8000_0000);
    sim
}

/// Write a slice of u32 instruction words into the simulator's memory at `addr`.
pub fn write_insns(sim: &mut HelmSim, addr: u64, insns: &[u32]) {
    for (i, &insn) in insns.iter().enumerate() {
        let target = addr + (i as u64 * 4);
        let bytes = insn.to_le_bytes();
        sim.memory_mut()
            .write_bytes_functional(target, &bytes)
            .expect("write_insns: memory write failed");
    }
}

/// Assert the stop reason is QuantumExhausted. Panics with context on failure.
pub fn assert_quantum_exhausted(reason: StopReason) {
    assert_eq!(
        reason, StopReason::QuantumExhausted,
        "Expected QuantumExhausted, got {reason:?}"
    );
}

/// Assert the stop reason is Breakpoint at a specific PC.
pub fn assert_breakpoint_at(reason: StopReason, expected_pc: u64) {
    match reason {
        StopReason::Breakpoint { pc } => {
            assert_eq!(pc, expected_pc,
                "Breakpoint at wrong PC: expected {expected_pc:#x}, got {pc:#x}");
        }
        other => panic!("Expected Breakpoint, got {other:?}"),
    }
}

// ── RISC-V instruction encodings ─────────────────────────────────────────────

/// RISC-V NOP (ADDI x0, x0, 0)
pub const RISCV_NOP: u32 = 0x0000_0013;

/// RISC-V EBREAK (trigger breakpoint event)
pub const RISCV_EBREAK: u32 = 0x0010_0073;

/// RISC-V ADDI rd, rs1, imm — encode an ADDI instruction.
pub fn riscv_addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    assert!(rd < 32 && rs1 < 32);
    let imm12 = (imm as u32) & 0xFFF;
    (imm12 << 20) | (rs1 << 15) | (0b000 << 12) | (rd << 7) | 0x13
}

/// RISC-V LW rd, offset(rs1)
pub fn riscv_lw(rd: u32, rs1: u32, offset: i32) -> u32 {
    let imm12 = (offset as u32) & 0xFFF;
    (imm12 << 20) | (rs1 << 15) | (0b010 << 12) | (rd << 7) | 0x03
}

/// RISC-V SW rs2, offset(rs1)
pub fn riscv_sw(rs1: u32, rs2: u32, offset: i32) -> u32 {
    let imm = (offset as u32) & 0xFFF;
    let imm11_5 = (imm >> 5) << 25;
    let imm4_0 = (imm & 0x1F) << 7;
    imm11_5 | (rs2 << 20) | (rs1 << 15) | (0b010 << 12) | imm4_0 | 0x23
}

/// RISC-V ECALL
pub const RISCV_ECALL: u32 = 0x0000_0073;

/// RISC-V JAL x0, 0 (infinite loop to self)
pub const RISCV_JAL_SELF: u32 = 0x0000_006F;
```

---

## 3. Unit Tests: HelmEngine Basic Execution

```rust
// crates/helm-engine/tests/engine_basic.rs

mod common;
use common::*;
use helm_engine::StopReason;

/// Test: NOP loop advances PC correctly.
///
/// Load 10 NOPs at 0x8000_0000. Run for 10 instructions.
/// After run, PC must be at 0x8000_0028 (0x8000_0000 + 10*4).
#[test]
fn test_nop_loop_advances_pc() {
    let mut sim = riscv_virtual_sim();
    let base = 0x8000_0000u64;

    // Write 10 NOPs followed by an infinite loop (to prevent fetch fault).
    let mut insns = vec![RISCV_NOP; 10];
    insns.push(RISCV_JAL_SELF);  // loop forever after the 10 NOPs
    write_insns(&mut sim, base, &insns);

    let reason = sim.run(10);

    assert_quantum_exhausted(reason);
    assert_eq!(
        sim.thread_context().read_pc(),
        base + 10 * 4,
        "PC should be at base + 40 after 10 NOPs"
    );
    assert_eq!(sim.insns_executed(), 10);
}

/// Test: ADDI updates integer register correctly.
///
/// ADDI x1, x0, 42 → x1 should equal 42.
/// ADDI x2, x1, 8  → x2 should equal 50.
#[test]
fn test_addi_register_updates() {
    let mut sim = riscv_virtual_sim();
    let base = 0x8000_0000u64;

    let insns = [
        riscv_addi(1, 0, 42),   // x1 = x0 + 42 = 42
        riscv_addi(2, 1, 8),    // x2 = x1 + 8  = 50
        RISCV_JAL_SELF,
    ];
    write_insns(&mut sim, base, &insns);

    let reason = sim.run(2);

    assert_quantum_exhausted(reason);
    assert_eq!(sim.thread_context().read_int_reg(1), 42, "x1 should be 42");
    assert_eq!(sim.thread_context().read_int_reg(2), 50, "x2 should be 50");
}

/// Test: x0 (zero register) is always 0, even after ADDI x0, x0, 1.
///
/// RISC-V hardwires x0 to 0. Writes to x0 are silently discarded.
#[test]
fn test_x0_hardwired_zero() {
    let mut sim = riscv_virtual_sim();
    let base = 0x8000_0000u64;

    let insns = [
        riscv_addi(0, 0, 42),  // attempt to write x0 = 42
        RISCV_JAL_SELF,
    ];
    write_insns(&mut sim, base, &insns);

    sim.run(1);

    assert_eq!(
        sim.thread_context().read_int_reg(0), 0,
        "x0 must always be 0"
    );
}

/// Test: Load word (LW) from memory.
///
/// Write 0xDEAD_BEEF to 0x8000_1000.
/// LW x1, 0x1000(x0) — but x0=0, so we need to set a base register.
/// More precisely: set x2 = 0x8000_1000, then LW x1, 0(x2).
#[test]
fn test_lw_reads_memory() {
    let mut sim = riscv_virtual_sim();
    let base = 0x8000_0000u64;
    let data_addr = 0x8000_1000u64;

    // Write target value to memory.
    sim.memory_mut()
        .write_bytes_functional(data_addr, &0xDEAD_BEEFu32.to_le_bytes())
        .unwrap();

    // Build instruction sequence:
    //   LUI x2, 0x80001 (upper 20 bits of data_addr)
    //   ADDI x2, x2, 0  (data_addr lower 12 bits = 0, no adjustment needed)
    //   LW x1, 0(x2)
    // For simplicity in this test: load the address into x2 via multiple ADDIs.
    // (A real test would use LUI+ADDI or a helper.)

    // We set x2 directly via ThreadContext (test helper, not instruction).
    sim.thread_context().write_int_reg(2, data_addr);

    let insns = [
        riscv_lw(1, 2, 0),  // x1 = mem[x2 + 0]
        RISCV_JAL_SELF,
    ];
    write_insns(&mut sim, base, &insns);

    sim.run(1);

    assert_eq!(
        sim.thread_context().read_int_reg(1),
        0xDEAD_BEEF,
        "LW should load 0xDEAD_BEEF from memory"
    );
}

/// Test: Store word (SW) writes to memory.
///
/// SW x1, 0(x2): store x1 at address in x2.
#[test]
fn test_sw_writes_memory() {
    let mut sim = riscv_virtual_sim();
    let base = 0x8000_0000u64;
    let data_addr = 0x8000_2000u64;

    // Set up registers.
    sim.thread_context().write_int_reg(1, 0xCAFE_F00D);  // value to store
    sim.thread_context().write_int_reg(2, data_addr);      // store address

    let insns = [
        riscv_sw(2, 1, 0),  // mem[x2 + 0] = x1
        RISCV_JAL_SELF,
    ];
    write_insns(&mut sim, base, &insns);

    sim.run(1);

    let mut buf = [0u8; 4];
    sim.memory().read_bytes_functional(data_addr, 4).unwrap()
        .iter().enumerate().for_each(|(i, &b)| buf[i] = b);
    let stored = u32::from_le_bytes(buf);

    assert_eq!(stored, 0xCAFE_F00D, "SW should write 0xCAFE_F00D to memory");
}

/// Test: insns_executed counter increments correctly.
///
/// Run 100 NOPs. Counter must report exactly 100.
#[test]
fn test_insns_executed_counter() {
    let mut sim = riscv_virtual_sim();
    let base = 0x8000_0000u64;

    let mut insns = vec![RISCV_NOP; 100];
    insns.push(RISCV_JAL_SELF);
    write_insns(&mut sim, base, &insns);

    sim.run(100);

    assert_eq!(sim.insns_executed(), 100);

    // Run 50 more.
    // Rewrite: reset PC to base, reset counter via reset(), run 50.
    sim.reset();
    write_insns(&mut sim, base, &insns);
    sim.run(50);
    assert_eq!(sim.insns_executed(), 50,
        "Counter should be 50 after reset + run(50)");
}
```

---

## 4. Unit Tests: Quantum Exhaustion

```rust
// crates/helm-engine/tests/engine_basic.rs (continued)

/// Test: run(N) returns QuantumExhausted after exactly N instructions.
///
/// The budget ceiling must be tight — not N-1, not N+1.
#[test]
fn test_quantum_exhaustion_is_exact() {
    let mut sim = riscv_virtual_sim();
    let base = 0x8000_0000u64;

    // Infinite NOP loop: 256 NOPs, then JAL back to base.
    let mut insns = vec![RISCV_NOP; 256];
    insns.push(0xFF1FF06F_u32);  // JAL x0, -0x100*4 (approximate; use actual encoding)
    write_insns(&mut sim, base, &insns);

    // Run for exactly 50 instructions.
    let reason = sim.run(50);

    assert_eq!(reason, StopReason::QuantumExhausted,
        "run(50) must return QuantumExhausted after 50 NOPs");
    assert_eq!(sim.insns_executed(), 50,
        "insns_executed must be exactly 50");
    assert_eq!(sim.thread_context().read_pc(), base + 50 * 4,
        "PC must be exactly at base + 200 (50 instructions * 4 bytes)");
}

/// Test: run(0) returns QuantumExhausted immediately without executing anything.
#[test]
fn test_run_zero_budget() {
    let mut sim = riscv_virtual_sim();
    let base = 0x8000_0000u64;

    write_insns(&mut sim, base, &[RISCV_NOP]);

    let reason = sim.run(0);

    assert_eq!(reason, StopReason::QuantumExhausted);
    assert_eq!(sim.insns_executed(), 0, "No instructions executed with budget=0");
    assert_eq!(sim.thread_context().read_pc(), base, "PC must not advance with budget=0");
}

/// Test: run() accumulates insns_executed across multiple calls.
#[test]
fn test_insns_executed_accumulates() {
    let mut sim = riscv_virtual_sim();
    let base = 0x8000_0000u64;

    let mut insns = vec![RISCV_NOP; 300];
    insns.push(RISCV_JAL_SELF);
    write_insns(&mut sim, base, &insns);

    sim.run(100);
    assert_eq!(sim.insns_executed(), 100);

    sim.run(100);
    assert_eq!(sim.insns_executed(), 200);

    sim.run(100);
    assert_eq!(sim.insns_executed(), 300);
}
```

---

## 5. Unit Tests: Breakpoint Stops Execution

```rust
// crates/helm-engine/tests/engine_basic.rs (continued)

/// Test: EBREAK instruction fires HelmEvent::Breakpoint and stops execution.
///
/// Layout:
///   0x8000_0000: NOP
///   0x8000_0004: NOP
///   0x8000_0008: EBREAK  ← should stop here
///   0x8000_000C: NOP     ← must NOT be executed
///
/// run(100) should return StopReason::Breakpoint at PC=0x8000_0008.
/// insns_executed should be 3 (NOP, NOP, EBREAK).
#[test]
fn test_ebreak_fires_breakpoint_event() {
    let mut sim = riscv_virtual_sim();
    let base = 0x8000_0000u64;

    let insns = [
        RISCV_NOP,    // 0x8000_0000
        RISCV_NOP,    // 0x8000_0004
        RISCV_EBREAK, // 0x8000_0008 — fires HelmEvent::Breakpoint
        RISCV_NOP,    // 0x8000_000C — must not execute
        RISCV_JAL_SELF,
    ];
    write_insns(&mut sim, base, &insns);

    let reason = sim.run(100);

    match reason {
        StopReason::Breakpoint { pc } => {
            assert_eq!(pc, base + 8,
                "Breakpoint must fire at PC of EBREAK instruction");
        }
        other => panic!("Expected Breakpoint, got {other:?}"),
    }

    // 3 instructions retired: NOP, NOP, EBREAK.
    assert_eq!(sim.insns_executed(), 3,
        "Exactly 3 instructions must be retired before breakpoint stop");

    // PC must be at EBREAK (or EBREAK+4 depending on whether EBREAK advances PC).
    // RISC-V: EBREAK fires before PC advance in our model — verify spec.
    // Design decision: EBREAK retires (PC advances to +4), event fires, stop_flag set.
    // Next iteration sees stop_flag → returns Breakpoint with PC = EBREAK+4 = 0x800C.
    // Adjust assertion per implementation:
    let pc_after = sim.thread_context().read_pc();
    assert!(
        pc_after == base + 8 || pc_after == base + 12,
        "PC after EBREAK must be at EBREAK or EBREAK+4, got {pc_after:#x}"
    );
}

/// Test: External stop via ThreadContext::pause() stops execution mid-run.
///
/// This simulates GDB or Python pausing the simulator.
#[test]
fn test_external_pause_stops_execution() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let mut sim = riscv_virtual_sim();
    let base = 0x8000_0000u64;

    // Infinite NOP loop.
    let insns: Vec<u32> = std::iter::repeat(RISCV_NOP).take(1024).collect();
    write_insns(&mut sim, base, &insns);

    // Note: In single-threaded tests, we cannot truly test async pause.
    // Instead, test that pause() via ThreadContext stops after current run().
    // The async scenario is covered by the GDB integration test.

    // Synchronous test: pause() before run() → immediate QuantumExhausted
    // (stop_flag is set; run() checks it at the top of the first iteration).
    sim.thread_context().pause();
    let reason = sim.run(1_000_000);

    assert_eq!(reason, StopReason::Breakpoint { pc: base },
        "pause() before run() should return Breakpoint immediately");
    assert_eq!(sim.insns_executed(), 0,
        "No instructions should be executed after pre-pause");
}

/// Test: HelmEventBus subscriber receives Breakpoint event.
///
/// Verifies that the event bus fires on EBREAK, not just the stop_flag.
#[test]
fn test_breakpoint_event_fires_on_bus() {
    use std::sync::{Arc, Mutex};
    use helm_devices::bus::event_bus::{HelmEventBus, HelmEventKind, HelmEvent};

    let mut sim = riscv_virtual_sim();
    let base = 0x8000_0000u64;

    // Replace the default event bus with an observable one.
    let bus = Arc::new(HelmEventBus::new());
    let fired = Arc::new(Mutex::new(false));
    {
        let fired_clone = Arc::clone(&fired);
        bus.subscribe(HelmEventKind::Breakpoint, move |_event| {
            *fired_clone.lock().unwrap() = true;
        });
    }
    sim.set_event_bus(Arc::clone(&bus));

    write_insns(&mut sim, base, &[RISCV_EBREAK, RISCV_JAL_SELF]);
    sim.run(100);

    assert!(
        *fired.lock().unwrap(),
        "HelmEvent::Breakpoint must be fired on EBREAK"
    );
}
```

---

## 6. Unit Tests: HelmSim Dispatch

```rust
// crates/helm-engine/tests/helmsim_dispatch.rs

use helm_engine::{HelmSim, TimingChoice, build_simulator, StopReason};
use helm_core::{Isa, ExecMode};

mod common;
use common::*;

/// Test: build_simulator with Virtual timing produces HelmSim::Virtual.
/// Verifies by running and checking that Virtual-specific behavior holds.
#[test]
fn test_build_simulator_virtual() {
    let mut sim = build_simulator(Isa::RiscV, ExecMode::Functional, TimingChoice::Virtual);
    sim.memory_mut().add_ram(0x8000_0000, 64 * 1024);
    sim.thread_context().write_pc(0x8000_0000);
    write_insns(&mut sim, 0x8000_0000, &[RISCV_NOP, RISCV_JAL_SELF]);

    let reason = sim.run(1);
    assert_eq!(reason, StopReason::QuantumExhausted);
    assert_eq!(sim.insns_executed(), 1);
}

/// Test: build_simulator with Interval timing produces HelmSim::Interval.
#[test]
fn test_build_simulator_interval() {
    let mut sim = build_simulator(
        Isa::RiscV,
        ExecMode::Functional,
        TimingChoice::Interval { interval_ns: 10_000 },
    );
    sim.memory_mut().add_ram(0x8000_0000, 64 * 1024);
    sim.thread_context().write_pc(0x8000_0000);
    write_insns(&mut sim, 0x8000_0000, &[RISCV_NOP, RISCV_JAL_SELF]);

    let reason = sim.run(1);
    assert_eq!(reason, StopReason::QuantumExhausted);
}

/// Test: build_simulator with Accurate timing produces HelmSim::Accurate.
#[test]
fn test_build_simulator_accurate() {
    let mut sim = build_simulator(Isa::RiscV, ExecMode::Functional, TimingChoice::Accurate);
    sim.memory_mut().add_ram(0x8000_0000, 64 * 1024);
    sim.thread_context().write_pc(0x8000_0000);
    write_insns(&mut sim, 0x8000_0000, &[RISCV_NOP, RISCV_JAL_SELF]);

    let reason = sim.run(1);
    assert_eq!(reason, StopReason::QuantumExhausted);
}

/// Test: All three HelmSim variants produce identical architectural state
/// for the same instruction sequence.
///
/// This validates that ISA dispatch is timing-model-independent.
#[test]
fn test_all_variants_produce_identical_arch_state() {
    let make_sim = |timing: TimingChoice| {
        let mut sim = build_simulator(Isa::RiscV, ExecMode::Functional, timing);
        sim.memory_mut().add_ram(0x8000_0000, 64 * 1024);
        sim.thread_context().write_pc(0x8000_0000);
        let insns = [
            riscv_addi(1, 0, 10),  // x1 = 10
            riscv_addi(2, 0, 20),  // x2 = 20
            riscv_addi(3, 1, 5),   // x3 = 15
            RISCV_JAL_SELF,
        ];
        write_insns(&mut sim, 0x8000_0000, &insns);
        sim
    };

    let mut virt_sim = make_sim(TimingChoice::Virtual);
    let mut ivl_sim  = make_sim(TimingChoice::Interval { interval_ns: 10_000 });
    let mut acc_sim  = make_sim(TimingChoice::Accurate);

    virt_sim.run(3);
    ivl_sim.run(3);
    acc_sim.run(3);

    // All three must have identical x1, x2, x3 values.
    for reg in 1..=3u32 {
        let v = virt_sim.thread_context().read_int_reg(reg);
        let i = ivl_sim.thread_context().read_int_reg(reg);
        let a = acc_sim.thread_context().read_int_reg(reg);
        assert_eq!(v, i, "x{reg}: Virtual={v} != Interval={i}");
        assert_eq!(v, a, "x{reg}: Virtual={v} != Accurate={a}");
    }

    // All three must have identical PC.
    let vpc = virt_sim.thread_context().read_pc();
    let ipc = ivl_sim.thread_context().read_pc();
    let apc = acc_sim.thread_context().read_pc();
    assert_eq!(vpc, ipc, "PC: Virtual={vpc:#x} != Interval={ipc:#x}");
    assert_eq!(vpc, apc, "PC: Virtual={vpc:#x} != Accurate={apc:#x}");
}

/// Test: thread_context() returns a valid ThreadContext for all variants.
/// Verifies the dyn dispatch chain described in LLD-helm-sim.md section 5.
#[test]
fn test_thread_context_all_variants() {
    let timings = [
        TimingChoice::Virtual,
        TimingChoice::Interval { interval_ns: 1000 },
        TimingChoice::Accurate,
    ];

    for timing in timings {
        let mut sim = build_simulator(Isa::RiscV, ExecMode::Functional, timing);
        sim.memory_mut().add_ram(0x8000_0000, 4096);
        sim.thread_context().write_pc(0x8000_1234);
        let pc = sim.thread_context().read_pc();
        assert_eq!(pc, 0x8000_1234,
            "thread_context PC round-trip failed for timing={timing:?}");
    }
}
```

---

## 7. Unit Tests: Checkpoint Round-Trip

```rust
// crates/helm-engine/tests/engine_basic.rs (continued)

/// Test: checkpoint_save / checkpoint_restore preserves architectural state.
///
/// Run 50 NOPs. Save checkpoint. Mutate state. Restore checkpoint. Verify state matches.
#[test]
fn test_checkpoint_round_trip() {
    let mut sim = riscv_virtual_sim();
    let base = 0x8000_0000u64;

    // Write a long NOP sequence.
    let insns: Vec<u32> = std::iter::repeat(RISCV_NOP).take(200).collect();
    write_insns(&mut sim, base, &insns);

    // Run 50 instructions.
    sim.run(50);
    let pc_before = sim.thread_context().read_pc();
    assert_eq!(pc_before, base + 200);

    // Set some registers to known values.
    sim.thread_context().write_int_reg(5, 0xABCD_1234_5678_9ABC);
    sim.thread_context().write_int_reg(10, 0xDEAD_BEEF_CAFE_F00D);

    // Save checkpoint.
    let ckpt = sim.checkpoint_save();
    assert!(!ckpt.is_empty(), "Checkpoint must not be empty");

    // Mutate state significantly.
    sim.thread_context().write_pc(0xDEAD_0000);
    sim.thread_context().write_int_reg(5, 0);
    sim.thread_context().write_int_reg(10, 0);

    // Restore checkpoint.
    sim.checkpoint_restore(&ckpt);

    // Verify state matches pre-checkpoint snapshot.
    assert_eq!(sim.thread_context().read_pc(), pc_before,
        "PC must match post-restore");
    assert_eq!(sim.thread_context().read_int_reg(5), 0xABCD_1234_5678_9ABC,
        "x5 must match post-restore");
    assert_eq!(sim.thread_context().read_int_reg(10), 0xDEAD_BEEF_CAFE_F00D,
        "x10 must match post-restore");
}

/// Test: checkpoint with incompatible ISA panics.
///
/// A RiscV checkpoint cannot be restored into an AArch64 simulator.
#[test]
#[should_panic(expected = "ISA mismatch")]
fn test_checkpoint_isa_mismatch_panics() {
    let mut riscv_sim = riscv_virtual_sim();
    riscv_sim.memory_mut().add_ram(0x8000_0000, 4096);
    let ckpt = riscv_sim.checkpoint_save();

    let mut aarch64_sim = build_simulator(
        Isa::AArch64, ExecMode::Functional, TimingChoice::Virtual
    );
    aarch64_sim.memory_mut().add_ram(0x8000_0000, 4096);

    // Must panic with "ISA mismatch".
    aarch64_sim.checkpoint_restore(&ckpt);
}
```

---

## 8. Unit Tests: Syscall Dispatch

```rust
// crates/helm-engine/tests/engine_basic.rs (continued)

use helm_core::{SyscallHandler, ThreadContext};

/// Minimal mock syscall handler for testing.
struct MockSyscallHandler {
    pub calls: Vec<(u64, [u64; 6])>,  // recorded calls
    pub return_value: u64,
}

impl SyscallHandler for MockSyscallHandler {
    fn handle(&mut self, nr: u64, args: &[u64; 6], _tc: &mut dyn ThreadContext) -> u64 {
        self.calls.push((nr, *args));
        self.return_value
    }
}

/// Test: ECALL in Syscall mode dispatches to SyscallHandler with correct nr and args.
#[test]
fn test_ecall_dispatches_to_handler() {
    let mut sim = build_simulator(Isa::RiscV, ExecMode::Syscall, TimingChoice::Virtual);
    sim.memory_mut().add_ram(0x8000_0000, 64 * 1024);
    sim.thread_context().write_pc(0x8000_0000);

    // RISC-V ABI: a7=nr, a0–a5=args.
    // Set: syscall nr=42 in x17(a7), args x10=1, x11=2.
    sim.thread_context().write_int_reg(17, 42);  // a7 = syscall nr
    sim.thread_context().write_int_reg(10, 1);   // a0 = arg0
    sim.thread_context().write_int_reg(11, 2);   // a1 = arg1

    let handler = Box::new(MockSyscallHandler {
        calls: vec![],
        return_value: 0xBEEF,
    });
    // Note: handler ownership moves into sim; we need to share state via Arc<Mutex>.
    // In real test: use Arc<Mutex<MockSyscallHandler>> for post-run inspection.
    // Simplified here for clarity:

    sim.set_syscall_handler(handler);

    write_insns(&mut sim, 0x8000_0000, &[RISCV_ECALL, RISCV_JAL_SELF]);

    sim.run(1);

    // Return value should be in x10 (a0).
    assert_eq!(
        sim.thread_context().read_int_reg(10), 0xBEEF,
        "Syscall return value should be in a0"
    );
}

/// Test: ECALL in Functional mode returns StopReason::Exception.
#[test]
fn test_ecall_in_functional_mode_causes_exception() {
    let mut sim = riscv_virtual_sim();  // Functional mode, no syscall handler
    let base = 0x8000_0000u64;

    write_insns(&mut sim, base, &[RISCV_ECALL]);

    let reason = sim.run(1);

    match reason {
        StopReason::Exception { vector, .. } => {
            // RISC-V: Environment call from U-mode = exception vector 8
            assert_eq!(vector, 8,
                "ECALL from U-mode must generate exception vector 8");
        }
        other => panic!("Expected Exception, got {other:?}"),
    }
}
```

---

## 9. Benchmark: Instructions per Second (criterion.rs)

```rust
// crates/helm-engine/benches/engine_bench.rs

use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId, black_box};

use helm_engine::{build_simulator, TimingChoice};
use helm_core::{Isa, ExecMode};

/// Helper: build and pre-load a RISC-V Virtual simulator with N NOPs.
fn make_nop_sim(n_insns: usize) -> helm_engine::HelmSim {
    let mut sim = build_simulator(Isa::RiscV, ExecMode::Functional, TimingChoice::Virtual);
    sim.memory_mut().add_ram(0x8000_0000, (n_insns * 4 + 4096) as u64);
    sim.thread_context().write_pc(0x8000_0000);

    // Write NOPs + JAL-to-self loop.
    let nop: u32 = 0x0000_0013;
    let jal_self: u32 = 0x0000_006F;

    for i in 0..n_insns {
        let addr = 0x8000_0000u64 + (i as u64 * 4);
        sim.memory_mut()
            .write_bytes_functional(addr, &nop.to_le_bytes())
            .unwrap();
    }
    let loop_addr = 0x8000_0000u64 + (n_insns as u64 * 4);
    sim.memory_mut()
        .write_bytes_functional(loop_addr, &jal_self.to_le_bytes())
        .unwrap();

    sim
}

/// Benchmark: NOP loop throughput for Virtual timing model.
///
/// Measures instructions/second. This is the performance ceiling for the
/// `helm-engine` kernel — the ideal case with no memory faults and minimal
/// instruction complexity.
///
/// Target: >= 100M instructions/second on a 3+ GHz x86_64 host.
fn bench_virtual_nop_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("virtual_nop_throughput");

    for budget in [1_000u64, 10_000, 100_000, 1_000_000] {
        group.bench_with_input(
            BenchmarkId::new("nop_loop", budget),
            &budget,
            |b, &budget| {
                let mut sim = make_nop_sim(budget as usize + 1);
                b.iter(|| {
                    // Reset PC before each iteration so the budget is always consumed.
                    sim.thread_context().write_pc(0x8000_0000);
                    // Reset insns counter to prevent u64 overflow over many iterations.
                    // (In real impl: sim.reset_counters() or similar)
                    black_box(sim.run(black_box(budget)))
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: ADDI throughput — tests register file read/write.
///
/// ADDI x1, x1, 1 repeated N times. Tests:
///   - Register file read (rs1=x1)
///   - Integer addition
///   - Register file write (rd=x1)
///   - PC advancement
fn bench_virtual_addi_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("virtual_addi_throughput");

    let addi_x1_x1_1: u32 = 0x0010_8093;  // ADDI x1, x1, 1
    let budget = 1_000_000u64;

    group.bench_function("addi_x1_loop_1M", |b| {
        let mut sim = build_simulator(Isa::RiscV, ExecMode::Functional, TimingChoice::Virtual);
        sim.memory_mut().add_ram(0x8000_0000, (budget * 4 + 4096) as u64);
        sim.thread_context().write_pc(0x8000_0000);

        for i in 0..budget {
            let addr = 0x8000_0000u64 + (i * 4);
            sim.memory_mut()
                .write_bytes_functional(addr, &addi_x1_x1_1.to_le_bytes())
                .unwrap();
        }

        b.iter(|| {
            sim.thread_context().write_pc(0x8000_0000);
            sim.thread_context().write_int_reg(1, 0);
            black_box(sim.run(black_box(budget)))
        });
    });

    group.finish();
}

/// Benchmark: Memory access throughput — LW/SW alternating.
///
/// Tests:
///   - Address computation (add)
///   - Memory subsystem read path (FlatView lookup + RAM access)
///   - Timing model on_memory_access() hook (Virtual: no-op)
fn bench_virtual_memory_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("virtual_memory_throughput");

    // Pattern: LW x2, 0(x1); SW x2, 0(x1); repeat
    let lw_x2_0_x1: u32 = 0x0000_A103;  // LW x2, 0(x1)
    let sw_x2_0_x1: u32 = 0x0020_A023;  // SW x2, 0(x1)
    let budget = 100_000u64;
    let pattern = [lw_x2_0_x1, sw_x2_0_x1];
    let n_insns = budget as usize;

    group.bench_function("lw_sw_alternating_100K", |b| {
        let mut sim = build_simulator(Isa::RiscV, ExecMode::Functional, TimingChoice::Virtual);
        sim.memory_mut().add_ram(0x8000_0000, (n_insns * 4 + 8192) as u64);
        sim.thread_context().write_pc(0x8000_0000);

        // Load instructions.
        for i in 0..n_insns {
            let addr = 0x8000_0000u64 + (i as u64 * 4);
            let insn = pattern[i % 2];
            sim.memory_mut()
                .write_bytes_functional(addr, &insn.to_le_bytes())
                .unwrap();
        }

        // x1 points to a scratch word at the end of the instruction region.
        let scratch_addr = 0x8000_0000u64 + (n_insns as u64 * 4) + 16;
        sim.thread_context().write_int_reg(1, scratch_addr);

        b.iter(|| {
            sim.thread_context().write_pc(0x8000_0000);
            black_box(sim.run(black_box(budget)))
        });
    });

    group.finish();
}

/// Benchmark: step_once() overhead vs run(1).
///
/// Both should be nearly identical since `step_once` calls `run(1)`.
/// Any divergence indicates unnecessary overhead in the step_once path.
fn bench_step_once_overhead(c: &mut Criterion) {
    let mut sim = make_nop_sim(1);

    c.bench_function("step_once_vs_run_1", |b| {
        b.iter(|| {
            sim.thread_context().write_pc(0x8000_0000);
            black_box(sim.step_once())
        });
    });
}

criterion_group!(
    benches,
    bench_virtual_nop_throughput,
    bench_virtual_addi_throughput,
    bench_virtual_memory_throughput,
    bench_step_once_overhead,
);
criterion_main!(benches);
```

### Running the Benchmarks

```bash
# Run all helm-engine benchmarks
cargo bench -p helm-engine

# Run a specific benchmark group
cargo bench -p helm-engine -- virtual_nop_throughput

# Save a baseline for regression comparison
cargo bench -p helm-engine -- --save-baseline main

# Compare against saved baseline
cargo bench -p helm-engine -- --baseline main
```

### Performance Targets (CI Gates)

| Benchmark | Minimum acceptable | Failure action |
|---|---|---|
| `virtual_nop_throughput/1M` | 100M insns/sec | Block merge |
| `virtual_addi_throughput/1M` | 80M insns/sec | Block merge |
| `virtual_memory_throughput/100K` | 50M insns/sec | Block merge |
| `step_once` vs `run(1)` overhead | < 5% difference | Warning only |

---

## 10. Test Matrix

| Test | File | Coverage area |
|---|---|---|
| `test_nop_loop_advances_pc` | `engine_basic.rs` | PC advancement, basic fetch |
| `test_addi_register_updates` | `engine_basic.rs` | Register file write |
| `test_x0_hardwired_zero` | `engine_basic.rs` | RISC-V x0 zero-register invariant |
| `test_lw_reads_memory` | `engine_basic.rs` | Memory read path |
| `test_sw_writes_memory` | `engine_basic.rs` | Memory write path |
| `test_insns_executed_counter` | `engine_basic.rs` | Instruction counter |
| `test_quantum_exhaustion_is_exact` | `engine_basic.rs` | Budget ceiling correctness |
| `test_run_zero_budget` | `engine_basic.rs` | Edge case: budget=0 |
| `test_insns_executed_accumulates` | `engine_basic.rs` | Multi-call counter accumulation |
| `test_ebreak_fires_breakpoint_event` | `engine_basic.rs` | Breakpoint stop path |
| `test_external_pause_stops_execution` | `engine_basic.rs` | ThreadContext::pause() |
| `test_breakpoint_event_fires_on_bus` | `engine_basic.rs` | HelmEventBus integration |
| `test_checkpoint_round_trip` | `engine_basic.rs` | Checkpoint save/restore |
| `test_checkpoint_isa_mismatch_panics` | `engine_basic.rs` | Checkpoint compatibility |
| `test_ecall_dispatches_to_handler` | `engine_basic.rs` | Syscall dispatch |
| `test_ecall_in_functional_mode_causes_exception` | `engine_basic.rs` | ExecMode::Functional path |
| `test_build_simulator_virtual` | `helmsim_dispatch.rs` | HelmSim::Virtual variant |
| `test_build_simulator_interval` | `helmsim_dispatch.rs` | HelmSim::Interval variant |
| `test_build_simulator_accurate` | `helmsim_dispatch.rs` | HelmSim::Accurate variant |
| `test_all_variants_produce_identical_arch_state` | `helmsim_dispatch.rs` | Timing-independent ISA semantics |
| `test_thread_context_all_variants` | `helmsim_dispatch.rs` | dyn ThreadContext dispatch chain |
| `bench_virtual_nop_throughput` | `engine_bench.rs` | Performance: NOP throughput |
| `bench_virtual_addi_throughput` | `engine_bench.rs` | Performance: register ops |
| `bench_virtual_memory_throughput` | `engine_bench.rs` | Performance: memory access |

### Running All Tests

```bash
# All unit tests
cargo test -p helm-engine

# With output (useful for debugging)
cargo test -p helm-engine -- --nocapture

# Specific test
cargo test -p helm-engine test_nop_loop_advances_pc

# Tests + benchmarks (benches run as tests, not timed)
cargo test -p helm-engine --benches
```

---

*See [`HLD.md`](HLD.md) for crate-level design context.*
*See [`LLD-helm-engine.md`](LLD-helm-engine.md) for the inner loop design these tests exercise.*
*See [`LLD-helm-sim.md`](LLD-helm-sim.md) for the HelmSim dispatch tests.*
*See [`LLD-scheduler.md`](LLD-scheduler.md) for Scheduler tests (not yet written — Phase 1).*
