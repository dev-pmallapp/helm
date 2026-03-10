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
