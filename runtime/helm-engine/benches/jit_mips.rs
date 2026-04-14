//! JIT MIPS benchmark — Phase 1-F gate.
#![allow(missing_docs)]
//!
//! Measures AArch64 SE interpreter/JIT instruction throughput (MIPS) across
//! three modes: interpreter, dynasm, tiered.
//!
//! # Usage
//!
//! ```
//! cargo bench --package helm-engine --bench jit_mips --features jit-dynasm
//! ```
//!
//! # Workload
//!
//! A self-contained AArch64 loop binary (no stack, no syscalls):
//!   MOVZ X1, #100000   ; outer count
//!   MOVZ X2, #100      ; inner count
//!   inner: SUB X2, X2, #1; CBNZ X2, inner
//!   SUB X1, X1, #1; CBNZ X1, outer
//!   BRK  #0
//!
//! Total instructions ≈ 2 * 100000 * 100 = 20M per run (benchmark reports
//! per-quantum time and criterion derives throughput).

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use helm_engine::{ExecMode, HelmEngine, Isa, StopReason};
use helm_timing::VirtualTiming;

/// Synthetic tight-loop binary, loaded at LOAD_ADDR.
///
/// AArch64 (LE, no stack needed):
///   MOVZ X1, #1000     ; outer_count = 1000
///   outer:
///     MOVZ X2, #10000  ; inner_count = 10000
///   inner:
///     SUB  X2, X2, #1  ; X2--
///     CBNZ X2, inner   ; if X2 != 0 → inner
///     SUB  X1, X1, #1  ; X1--
///     CBNZ X1, outer   ; if X1 != 0 → outer
///   BRK  #0             ; terminate
///
/// Total instructions: ~2 * 1000 * 10000 + 2 * 1000 + 3 ≈ 20M.
const LOOP_BINARY: &[u8] = &[
    // MOVZ X1, #1000 = #0x3E8
    0x01, 0x7d, 0x80, 0xd2, // movz x1, #0x3e8
    // outer: MOVZ X2, #10000 = #0x2710
    0x82, 0xe2, 0x84, 0xd2, // movz x2, #0x2710
    // inner: SUB X2, X2, #1
    0x42, 0x04, 0x00, 0xd1, // sub x2, x2, #1
    // CBNZ X2, inner (back 1 insn = -4 bytes)
    0xe2, 0xff, 0xff, 0xb5, // cbnz x2, -4
    // SUB X1, X1, #1
    0x21, 0x04, 0x00, 0xd1, // sub x1, x1, #1
    // CBNZ X1, outer (back 4 insns = -16 bytes)
    0x81, 0xff, 0xff, 0xb5, // cbnz x1, -16
    // BRK #0 (EXIT_EXCEPTION)
    0x00, 0x00, 0x20, 0xd4, // brk #0
];

const LOAD_ADDR: u64 = 0x4000_0000;
const MEM_SIZE: usize = 16 * 1024 * 1024; // 16 MiB
/// Approximate total instructions for throughput measurement.
const INSN_APPROX: u64 = 20_002_003;

fn make_engine(jit_mode: &str) -> HelmEngine<VirtualTiming> {
    let mut engine = HelmEngine::<VirtualTiming>::new(
        Isa::AArch64,
        ExecMode::Syscall,
        VirtualTiming::default(),
        LOAD_ADDR,
        MEM_SIZE,
    );
    engine.memory.load_bytes(LOAD_ADDR, LOOP_BINARY);
    engine.set_pc(LOAD_ADDR);
    if jit_mode != "interp" {
        engine.set_jit(true);
    }
    engine
}

fn run_to_exit(engine: &mut HelmEngine<VirtualTiming>, use_jit: bool) {
    loop {
        let reason = if use_jit {
            engine.run_jit(INSN_APPROX)
        } else {
            engine.run(INSN_APPROX)
        };
        match reason {
            StopReason::Exception(_) | StopReason::Exit { .. } | StopReason::Unsupported | StopReason::Breakpoint => break,
            StopReason::Quantum => {}
        }
    }
}

fn bench_jit_modes(c: &mut Criterion) {
    let mut group = c.benchmark_group("aarch64-se-mips");
    group.throughput(Throughput::Elements(INSN_APPROX));

    for (mode, use_jit) in &[("interp", false), ("dynasm", true), ("tiered", true)] {
        let mode = *mode;
        let use_jit = *use_jit;
        group.bench_with_input(BenchmarkId::from_parameter(mode), &mode, |b, _| {
            b.iter(|| {
                let mut engine = make_engine(mode);
                run_to_exit(&mut engine, use_jit);
                criterion::black_box(engine.insns_retired)
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_jit_modes);
criterion_main!(benches);
