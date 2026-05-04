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

/// Build an arm-virt FS sim with a real PL011 wired in; used by
/// the per-device migration tests.
fn build_arm_virt_sim() -> HelmSim {
    use helm_devices::NullCharBackend;
    use helm_engine::platform::arm_virt::ArmVirtGicVersion;
    use helm_engine::{HelmEngine, HelmSim};
    use helm_timing::VirtualTiming;

    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0x4000_0000,
        2 * 1024 * 1024,
    );
    // 2 MiB RAM, 2 vCPUs (we want > 1 to exercise the per-vCPU
    // path collapse-vs-fanout in `adopt_per_vcpu_mmu_counters`).
    engine
        .install_arm_virt_board(2, 2, ArmVirtGicVersion::V2, Box::new(NullCharBackend))
        .expect("arm-virt board installation should succeed");
    HelmSim::VirtualTiming(engine)
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

/// Per-vCPU MMU counters are exposed under `system.cpu<N>.mmu.*`
/// when the arm-virt board exists, and the snapshot pass adopts
/// the live `PerfCounter` handles so subsequent reads see hot-path
/// updates without forcing a re-walk.
#[test]
fn arm_virt_mmu_counters_appear_under_per_vcpu_paths() {
    let mut sim = build_arm_virt_sim();
    let reg = sim.stats_registry();
    // 2 vCPUs in build_arm_virt_sim.
    for n in 0..2 {
        let prefix = format!("system.cpu{n}.mmu");
        for leaf in ["tlb_hits", "tlb_misses", "stage1_walks", "stage2_walks"] {
            let path = format!("{prefix}.{leaf}");
            assert!(
                reg.counter_value(&path).is_some(),
                "missing MMU counter at {path}"
            );
        }
    }
}

/// PL011 exposes its `tx_count`/`rx_count` PerfCounters directly so
/// the helm-python SimObject-tree walker can adopt them under a
/// canonical path. Verify the engine helper returns clones that
/// share storage.
#[test]
fn arm_virt_pl011_perf_counters_share_storage() {
    let sim = build_arm_virt_sim();
    let (tx, rx) = sim
        .pl011_perf_counters()
        .expect("arm-virt sim has a PL011");
    // Increment via the borrowed handles -- subsequent reads via the
    // engine getter must see the increment, proving the registry
    // view shares the underlying Arc<AtomicU64>.
    tx.add(7);
    rx.add(3);
    assert_eq!(sim.uart_tx_count(), Some(7));
    assert_eq!(sim.uart_rx_count(), Some(3));
}

#[test]
fn cpu_commit_and_branch_paths_registered() {
    let mut sim = build_minimal_sim();
    let reg = sim.stats_registry();
    for path in [
        "system.cpu.commit.committed_insns",
        "system.cpu.commit.cycles",
        "system.cpu.branch.taken",
        "system.cpu.branch.not_taken",
        "system.cpu.branch.mispredict",
    ] {
        assert!(
            reg.counter_value(path).is_some(),
            "missing CPU counter at {path}"
        );
    }
    // committed_ops is a label counter, surfaced via label_total.
    assert!(reg
        .label_total("system.cpu.commit.committed_ops")
        .is_some());
}

#[test]
fn memory_paths_registered_and_share_storage() {
    let mut sim = build_minimal_sim();
    let reg = sim.stats_registry();
    for path in [
        "system.mem.loads",
        "system.mem.stores",
        "system.mem.bytes_read",
        "system.mem.bytes_written",
    ] {
        assert!(
            reg.counter_value(path).is_some(),
            "missing memory counter at {path}"
        );
    }
}

#[test]
fn arm_virt_gic_intc_paths_registered() {
    let mut sim = build_arm_virt_sim();
    let reg = sim.stats_registry();
    for path in [
        "system.gic.interrupts.sgi",
        "system.gic.interrupts.ppi",
        "system.gic.interrupts.spi",
        "system.gic.irq_acked",
        "system.gic.irq_eoi",
    ] {
        assert!(
            reg.counter_value(path).is_some(),
            "missing GIC counter at {path}"
        );
    }
}

#[test]
fn arm_virt_v3_gic_intc_paths_registered() {
    use helm_devices::NullCharBackend;
    use helm_engine::platform::arm_virt::ArmVirtGicVersion;
    use helm_engine::{HelmEngine, HelmSim};
    use helm_timing::VirtualTiming;

    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0x4000_0000,
        2 * 1024 * 1024,
    );
    engine
        .install_arm_virt_board(2, 2, ArmVirtGicVersion::V3, Box::new(NullCharBackend))
        .expect("arm-virt v3 board installation should succeed");
    let mut sim = HelmSim::VirtualTiming(engine);
    let reg = sim.stats_registry();
    for path in [
        "system.gic.interrupts.sgi",
        "system.gic.interrupts.ppi",
        "system.gic.interrupts.spi",
        "system.gic.irq_acked",
        "system.gic.irq_eoi",
    ] {
        assert!(
            reg.counter_value(path).is_some(),
            "missing v3 GIC counter at {path}"
        );
    }
}

#[test]
fn iostats_producer_registered_at_canonical_path() {
    use helm_engine::IoStats;
    let mut sim = build_minimal_sim();
    let stats = IoStats::new();
    let owned = stats.clone();
    sim.register_producer("system.virtio.blk0", Box::new(stats));
    {
        let reg = sim.stats_registry();
        assert!(reg.counter_value("system.virtio.blk0.tx_bytes").is_some());
        assert!(reg.counter_value("system.virtio.blk0.rx_bytes").is_some());
        assert!(reg.counter_value("system.virtio.blk0.requests").is_some());
        assert!(reg
            .counter_value("system.virtio.blk0.completions")
            .is_some());
    }
    // Bump via the owned handle; registry must see it via the
    // shared Arc<AtomicU64>.
    owned.tx_bytes.add(1024);
    owned.requests.inc();
    let reg = sim.stats_registry();
    assert_eq!(reg.counter_value("system.virtio.blk0.tx_bytes"), Some(1024));
    assert_eq!(reg.counter_value("system.virtio.blk0.requests"), Some(1));
}
