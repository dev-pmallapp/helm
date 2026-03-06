use crate::collector::*;
use helm_core::event::EventObserver;
use helm_core::event::SimEvent;

#[test]
fn empty_results_have_zero_ipc() {
    let r = SimResults::default();
    assert_eq!(r.ipc(), 0.0);
    assert_eq!(r.branch_mpki(), 0.0);
}

#[test]
fn ipc_calculated_correctly() {
    let r = SimResults {
        cycles: 100,
        instructions_committed: 200,
        ..Default::default()
    };
    assert!((r.ipc() - 2.0).abs() < f64::EPSILON);
}

#[test]
fn collector_counts_commits() {
    let mut collector = StatsCollector::new();
    collector.on_event(&SimEvent::InsnCommit {
        pc: 0x100,
        cycle: 1,
    });
    collector.on_event(&SimEvent::InsnCommit {
        pc: 0x104,
        cycle: 2,
    });
    assert_eq!(collector.results.instructions_committed, 2);
    assert_eq!(collector.results.cycles, 2);
}

#[test]
fn collector_counts_mispredictions() {
    let mut collector = StatsCollector::new();
    collector.on_event(&SimEvent::BranchResolved {
        pc: 0x100,
        predicted: true,
        taken: false,
        cycle: 1,
    });
    assert_eq!(collector.results.branches, 1);
    assert_eq!(collector.results.branch_mispredictions, 1);
}

#[test]
fn collector_tracks_cache_hits_misses() {
    let mut collector = StatsCollector::new();
    collector.on_event(&SimEvent::CacheAccess {
        level: 1,
        hit: true,
        addr: 0,
        cycle: 1,
    });
    collector.on_event(&SimEvent::CacheAccess {
        level: 1,
        hit: false,
        addr: 0,
        cycle: 2,
    });
    let (hits, misses) = collector.results.cache_accesses[&1];
    assert_eq!(hits, 1);
    assert_eq!(misses, 1);
}

#[test]
fn results_serialize_to_json() {
    let r = SimResults {
        cycles: 10,
        instructions_committed: 20,
        ..Default::default()
    };
    let json = r.to_json();
    assert!(json.contains("\"cycles\": 10"));
}

#[test]
fn branch_mpki_calculated_correctly() {
    // 5 mispredictions per 1000 instructions = 5.0 MPKI
    let r = SimResults {
        instructions_committed: 1000,
        branch_mispredictions: 5,
        ..Default::default()
    };
    assert!((r.branch_mpki() - 5.0).abs() < f64::EPSILON);
}

#[test]
fn branch_mpki_zero_when_no_instructions() {
    let r = SimResults {
        instructions_committed: 0,
        branch_mispredictions: 10,
        ..Default::default()
    };
    assert_eq!(r.branch_mpki(), 0.0);
}

#[test]
fn pipeline_flush_event_does_not_panic() {
    let mut collector = StatsCollector::new();
    collector.on_event(&SimEvent::PipelineFlush {
        cycle: 5,
        reason: "mispred".into(),
    });
    // No crash; flush events are not tracked in results
    assert_eq!(collector.results.instructions_committed, 0);
}

#[test]
fn syscall_emulated_event_does_not_panic() {
    let mut collector = StatsCollector::new();
    collector.on_event(&SimEvent::SyscallEmulated { number: 64, cycle: 10 });
    assert_eq!(collector.results.branches, 0);
}

#[test]
fn multiple_cache_levels_tracked_independently() {
    let mut collector = StatsCollector::new();
    collector.on_event(&SimEvent::CacheAccess { level: 1, hit: true, addr: 0, cycle: 1 });
    collector.on_event(&SimEvent::CacheAccess { level: 2, hit: false, addr: 0, cycle: 2 });
    let (l1_hits, l1_misses) = collector.results.cache_accesses[&1];
    let (l2_hits, l2_misses) = collector.results.cache_accesses[&2];
    assert_eq!(l1_hits, 1);
    assert_eq!(l1_misses, 0);
    assert_eq!(l2_hits, 0);
    assert_eq!(l2_misses, 1);
}

#[test]
fn stats_collector_default_constructs() {
    let c = StatsCollector::default();
    assert_eq!(c.results.cycles, 0);
    assert_eq!(c.results.instructions_committed, 0);
}

#[test]
fn ipc_when_cycles_equals_instructions() {
    let r = SimResults {
        cycles: 50,
        instructions_committed: 50,
        ..Default::default()
    };
    assert!((r.ipc() - 1.0).abs() < f64::EPSILON);
}

#[test]
fn correct_branch_prediction_not_counted_as_misprediction() {
    let mut collector = StatsCollector::new();
    collector.on_event(&SimEvent::BranchResolved {
        pc: 0x100,
        predicted: true,
        taken: true, // predicted correctly
        cycle: 1,
    });
    assert_eq!(collector.results.branches, 1);
    assert_eq!(collector.results.branch_mispredictions, 0);
}
