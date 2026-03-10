use crate::event::*;

struct CountingObserver {
    count: u64,
}

impl EventObserver for CountingObserver {
    fn on_event(&mut self, _event: &SimEvent) {
        self.count += 1;
    }
}

#[test]
fn observer_receives_events() {
    let mut obs = CountingObserver { count: 0 };
    let event = SimEvent::InsnCommit {
        pc: 0x1000,
        cycle: 1,
    };
    obs.on_event(&event);
    obs.on_event(&event);
    assert_eq!(obs.count, 2);
}

#[test]
fn all_sim_event_variants_are_clonable() {
    let events = vec![
        SimEvent::InsnCommit {
            pc: 0x1000,
            cycle: 1,
        },
        SimEvent::BranchResolved {
            pc: 0x1000,
            predicted: true,
            taken: false,
            cycle: 2,
        },
        SimEvent::CacheAccess {
            level: 1,
            hit: true,
            addr: 0x2000,
            cycle: 3,
        },
        SimEvent::PipelineFlush {
            cycle: 4,
            reason: "mispred".into(),
        },
        SimEvent::SyscallEmulated {
            number: 64,
            cycle: 5,
        },
    ];
    for e in &events {
        let cloned = e.clone();
        let _ = cloned; // no panic
    }
}

#[test]
fn pipeline_flush_carries_reason() {
    let e = SimEvent::PipelineFlush {
        cycle: 10,
        reason: "branch_mispredict".into(),
    };
    if let SimEvent::PipelineFlush { reason, .. } = e {
        assert_eq!(reason, "branch_mispredict");
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn syscall_emulated_carries_number() {
    let e = SimEvent::SyscallEmulated {
        number: 64,
        cycle: 7,
    };
    if let SimEvent::SyscallEmulated { number, .. } = e {
        assert_eq!(number, 64);
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn branch_resolved_mismatch_detected() {
    // predicted=true (taken), actual=false (not taken) → misprediction
    let e = SimEvent::BranchResolved {
        pc: 0x100,
        predicted: true,
        taken: false,
        cycle: 1,
    };
    if let SimEvent::BranchResolved {
        predicted, taken, ..
    } = e
    {
        assert_ne!(predicted, taken);
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn observer_called_for_each_distinct_event() {
    let mut obs = CountingObserver { count: 0 };
    let events = vec![
        SimEvent::InsnCommit {
            pc: 0x1000,
            cycle: 1,
        },
        SimEvent::BranchResolved {
            pc: 0x1000,
            predicted: true,
            taken: true,
            cycle: 2,
        },
        SimEvent::CacheAccess {
            level: 1,
            hit: false,
            addr: 0x2000,
            cycle: 3,
        },
        SimEvent::PipelineFlush {
            cycle: 4,
            reason: "test".into(),
        },
        SimEvent::SyscallEmulated {
            number: 93,
            cycle: 5,
        },
    ];
    for e in &events {
        obs.on_event(e);
    }
    assert_eq!(obs.count, 5);
}
