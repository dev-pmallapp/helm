//! Simulation event bus — allows crates to emit and observe events for
//! statistics collection and instrumentation hooks.

use crate::types::{Addr, Cycle};

/// Events emitted during simulation.
#[derive(Debug, Clone)]
pub enum SimEvent {
    /// An instruction was committed.
    InsnCommit { pc: Addr, cycle: Cycle },
    /// A branch was resolved.
    BranchResolved {
        pc: Addr,
        predicted: bool,
        taken: bool,
        cycle: Cycle,
    },
    /// A cache access occurred.
    CacheAccess {
        level: u8,
        hit: bool,
        addr: Addr,
        cycle: Cycle,
    },
    /// Pipeline flush triggered.
    PipelineFlush { cycle: Cycle, reason: String },
    /// Syscall was emulated.
    SyscallEmulated { number: u64, cycle: Cycle },
}

/// Trait for components that observe simulation events.
pub trait EventObserver: Send + Sync {
    fn on_event(&mut self, event: &SimEvent);
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
