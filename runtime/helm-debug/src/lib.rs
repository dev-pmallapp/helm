#![allow(missing_docs)]

//! `helm-debug` — debug infrastructure for helm-ng.
//!
//! - GDB RSP server (TCP-based)
//! - Checkpoint manager (binary serialization)
//! - Watchpoint engine (address-range monitoring)
//! - Breakpoint engine (PC-match monitoring)
//! - Inspection API (on-demand state dump)

pub use helm_diag::{sim_info, sim_stub, sim_warn};

pub mod breakpoint;
pub mod checkpoint;
pub mod gdb;
pub mod inspect;
pub mod watchpoint;

#[cfg(feature = "instrumentation")]
pub use breakpoint::attach_breakpoint_engine;
pub use breakpoint::{BreakAction, BreakResult, Breakpoint, BreakpointEngine};
pub use checkpoint::{
    CheckpointHeader, CheckpointManager, DebugIntentCheckpoint, CHECKPOINT_VERSION,
};
pub use gdb::{GdbTarget, RspServer, StopReason};
pub use inspect::{Inspectable, InspectionResult};
#[cfg(feature = "instrumentation")]
pub use watchpoint::attach_watchpoint_engine;
pub use watchpoint::{WatchAction, WatchKind, WatchResult, Watchpoint, WatchpointEngine};

// ── Versioned debug protocol ────────────────────────────────────────────────

/// Versioned debug protocol identifier for handshaking.
pub struct HelmProtocol {
    pub major: u32,
    pub minor: u32,
}

impl HelmProtocol {
    /// Current protocol version.
    pub const CURRENT: Self = Self { major: 1, minor: 0 };

    /// Check compatibility (major must match).
    pub fn is_compatible(&self, remote_major: u32) -> bool {
        self.major == remote_major
    }
}

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum DebugError {
    #[error("not yet implemented")]
    NotImplemented,
    #[error("checkpoint data corrupt or version mismatch")]
    CorruptCheckpoint,
    #[error("GDB RSP error: {msg}")]
    Rsp { msg: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
