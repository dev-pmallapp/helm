//! Execution backend — selects between interpretive and TCG modes.
//!
//! The backend is orthogonal to the simulation mode (SE vs FS).
//! Both modes use the same backend interface; what differs is how
//! syscalls and memory are handled by the outer runner.

use std::collections::HashMap;

use helm_core::types::Addr;
use helm_jit::block::TcgBlock;
use helm_jit::interp::TcgInterp;

/// Selects how guest instructions are executed.
///
/// ```text
/// ExecBackend    ×    Simulation mode
/// ─────────────       ────────────────
/// Interpretive        SE (syscall emulation)
/// Tcg                 FS (full system) — future
/// ```
pub enum ExecBackend {
    /// Fetch-decode-execute one instruction at a time via
    /// [`Aarch64Cpu::step()`].  Simple, no translation overhead,
    /// re-decodes every instruction on every visit.
    Interpretive,

    /// Translate basic blocks into [`TcgOp`](helm_jit::TcgOp) sequences,
    /// cache them, and re-execute from cache on subsequent visits.
    /// Amortizes decode cost over hot loops.
    Tcg {
        cache: HashMap<Addr, TcgBlock>,
        interp: TcgInterp,
    },
}

impl ExecBackend {
    /// Create an interpretive backend.
    pub fn interpretive() -> Self {
        Self::Interpretive
    }

    /// Create a TCG backend with an empty translation cache.
    pub fn tcg() -> Self {
        Self::Tcg {
            cache: HashMap::new(),
            interp: TcgInterp::new(),
        }
    }
}

impl Default for ExecBackend {
    fn default() -> Self {
        Self::Interpretive
    }
}
