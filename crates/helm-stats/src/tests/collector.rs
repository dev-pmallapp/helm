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
