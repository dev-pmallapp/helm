//! Engine-level integration test for `HelmEngine::stats_registry()`.
//!
//! Verifies that:
//!
//! 1. The engine produces a populated registry with the expected
//!    canonical paths (`system.cpu.cycles`, `system.cpu.insns_retired`,
//!    `system.cpu.jit.*`).
//! 2. JIT counter handles registered through `JitPerfStats::register_stats`
//!    share their backing storage with the registry view -- bumping
//!    `engine.jit_perf_stats().blocks_compiled` is reflected at
//!    `system.cpu.jit.blocks_compiled` in the registry.
//!
//! `helm-engine`'s dev-dep override forces `helm-stats/stats` on, so
//! the assertions are meaningful in this test binary even though the
//! crate's own `stats` feature is off.

use helm_engine::{
    build_simulator_from_request, ExecMode, HelmSim, Isa, SimulatorBuildRequest, StatsRegistryRead,
    TimingChoice,
};

fn build_minimal_sim() -> HelmSim {
    build_simulator_from_request(SimulatorBuildRequest::new(
        Isa::AArch64,
        ExecMode::Functional,
        TimingChoice::VirtualTiming { ipc: 1.0 },
        0x4000_0000,
        1 << 20,
    ))
}

#[test]
fn stats_registry_exposes_engine_and_jit_paths() {
    let mut sim = build_minimal_sim();
    let reg = sim.stats_registry();

    // Engine-level scalar gauges, refreshed on every call.
    assert!(reg.counter_value("system.cpu.cycles").is_some());
    assert!(reg.counter_value("system.cpu.insns_retired").is_some());

    // JIT producer registered every PerfCounter field.
    let jit_paths = [
        "system.cpu.jit.blocks_compiled",
        "system.cpu.jit.compiled_guest_insns",
        "system.cpu.jit.blocks_executed",
        "system.cpu.jit.fallback_count",
        "system.cpu.jit.fallback_insns",
        "system.cpu.jit.unsupported_block_starts",
    ];
    for path in jit_paths {
        assert!(
            reg.counter_value(path).is_some(),
            "missing JIT counter at {path}"
        );
    }

    // Label counters expand under their own paths.
    assert!(reg
        .label_total("system.cpu.jit.unsupported_opcodes")
        .is_some());
    assert!(reg
        .label_total("system.cpu.jit.reject_reasons")
        .is_some());
}

#[test]
fn jit_counter_increments_visible_via_registry() {
    let mut sim = build_minimal_sim();
    {
        // Touch the registry so producers register.
        let _ = sim.stats_registry();
    }
    // Increment via the JitPerfStats handle the engine surfaces.
    let stats = sim.jit_perf_stats();
    stats.blocks_compiled.add(5);
    stats.fallback_count.inc();
    stats.unsupported_opcodes.bump_static("ldp_unsupported_size");

    // Re-borrow the registry; it must reflect those increments.
    let reg = sim.stats_registry();
    assert_eq!(
        reg.counter_value("system.cpu.jit.blocks_compiled"),
        Some(5)
    );
    assert_eq!(reg.counter_value("system.cpu.jit.fallback_count"), Some(1));
    let snap = reg
        .label_snapshot("system.cpu.jit.unsupported_opcodes")
        .expect("opcode label counter present");
    let total: u64 = snap.iter().map(|(_, v)| *v).sum();
    assert_eq!(total, 1);
}

#[test]
fn dump_text_block_includes_jit_paths() {
    let mut sim = build_minimal_sim();
    sim.jit_perf_stats().blocks_compiled.add(3);
    let reg = sim.stats_registry();
    let text = reg.dump_text();
    assert!(text.contains("system.cpu.jit.blocks_compiled"));
    assert!(text.contains("system.cpu.cycles"));
    assert!(text.contains("Begin Simulation Statistics"));
}

/// Caller-registered producer at an arbitrary canonical path (the
/// QOM-style scope walker pattern). The producer's counters must be
/// visible at `<path>.<leaf>` in the registry view.
#[test]
fn register_producer_walks_at_canonical_path() {
    use helm_engine::{StatsProducer, StatsScope};

    struct FakePl011;
    impl StatsProducer for FakePl011 {
        fn register_stats(&self, scope: &mut StatsScope<'_>) {
            let tx = scope.counter("tx_bytes", "PL011 bytes transmitted");
            let rx = scope.counter("rx_bytes", "PL011 bytes received");
            tx.add(11);
            rx.add(5);
        }
    }

    let mut sim = build_minimal_sim();
    sim.register_producer("system.peripheral.uart0", Box::new(FakePl011));
    let reg = sim.stats_registry();
    assert_eq!(
        reg.counter_value("system.peripheral.uart0.tx_bytes"),
        Some(11)
    );
    assert_eq!(
        reg.counter_value("system.peripheral.uart0.rx_bytes"),
        Some(5)
    );
}

/// Re-borrowing the registry must not double-register a durable
/// producer (counters are interior-mutable, double-walk would
/// corrupt the value).
#[test]
fn second_stats_registry_borrow_does_not_re_register() {
    use helm_engine::{StatsProducer, StatsScope};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct CountingProducer(Arc<AtomicUsize>);
    impl StatsProducer for CountingProducer {
        fn register_stats(&self, _scope: &mut StatsScope<'_>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let mut sim = build_minimal_sim();
    sim.register_producer(
        "system.test.counting",
        Box::new(CountingProducer(Arc::clone(&calls))),
    );
    let _ = sim.stats_registry();
    let _ = sim.stats_registry();
    let _ = sim.stats_registry();
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "durable producers must register exactly once"
    );
}
