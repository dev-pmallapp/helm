//! `helm-probe` -- zero-cost typed probe points for helm-ng.
//!
//! # Build profile behaviour
//! - `--release` (`debug_assertions=false`): `Probe<T>` is zero-sized; all probe
//!   sites eliminated by the compiler.
//! - `cargo build` (dev): `Probe<T>` holds `Vec<Listener>`; `has_listeners()` is
//!   O(1) empty check; predicted-not-taken when no subscribers.
//! - `--features probe-full`: same as dev, plus richer event fields.

mod events;
mod macros; // probe!() re-exported by #[macro_export]
mod probe;

pub use events::{
    BranchEvent, BranchKind, CpuFaultEvent, CpuStepEvent, IrqEvent, MemAccessEvent, MmioEvent,
};
pub use probe::Probe;

/// CPU probe bundle. Add as `pub probes: CpuProbes` on `HelmEngine<T>`.
pub struct CpuProbes {
    pub pre_step: Probe<CpuStepEvent>,
    pub post_step: Probe<CpuStepEvent>,
    pub fault: Probe<CpuFaultEvent>,
    pub mem: Probe<MemAccessEvent>,
    pub branch: Probe<BranchEvent>,
}

impl Default for CpuProbes {
    fn default() -> Self {
        Self {
            pre_step: Probe::new(),
            post_step: Probe::new(),
            fault: Probe::new(),
            mem: Probe::new(),
            branch: Probe::new(),
        }
    }
}

/// GIC probe bundle. Add as `pub probes: GicProbes` on `GicState` (feature-gated).
pub struct GicProbes {
    pub irq_asserted: Probe<IrqEvent>,
    pub irq_deasserted: Probe<IrqEvent>,
    pub eoi: Probe<IrqEvent>,
}

impl Default for GicProbes {
    fn default() -> Self {
        Self {
            irq_asserted: Probe::new(),
            irq_deasserted: Probe::new(),
            eoi: Probe::new(),
        }
    }
}
