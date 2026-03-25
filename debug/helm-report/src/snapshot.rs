// src/snapshot.rs -- HelmSpySnapshot and supporting types.
//
// These types are defined here because helm-spy does not yet exist.
// When helm-spy is implemented, these definitions will move there and
// helm-report will re-export them via the helm-spy dependency.

/// Immutable point-in-time copy of observation session state.
///
/// Created on the cold path. All atomic fields from the live session are
/// copied as plain integers, making the snapshot safe to format from any
/// thread without ordering concerns.
#[derive(Clone, Debug)]
pub struct HelmSpySnapshot {
    // Instruction mix
    pub insn_count: u64,
    pub insn_mix: Vec<(String, u64)>, // (class_name, count), order stable

    // Hot PC heatmap (top-N PCs by visit count)
    pub hot_pcs: Vec<(u64, u64)>, // (pc, count), sorted descending by count

    // Branch heatmap (top-N branch sites)
    pub branch_heatmap: Vec<(u64, u64)>, // (pc, count), sorted descending by count

    // Optional subsystems
    pub cache_l1d: Option<CacheSnapshot>,
    pub branch_pred: Option<BranchPredSnapshot>,
    pub fault_history: Option<Vec<CpuFaultEvent>>,

    // Timing
    pub tick_count: u64,
    pub snapshot_ns: u64, // UNIX nanoseconds (wall clock) at snapshot time
}

/// Immutable snapshot of L1 data cache state.
#[derive(Clone, Debug)]
pub struct CacheSnapshot {
    pub name: String,
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64, // hits / (hits + misses)
}

/// Immutable snapshot of branch predictor state.
#[derive(Clone, Debug)]
pub struct BranchPredSnapshot {
    pub name: String,
    pub kind: String, // "BiModal" | "GShare" | "Perfect"
    pub predictions: u64,
    pub mispredictions: u64,
    pub miss_rate: f64, // mispredictions / predictions
}

/// A single CPU fault event from the fault history.
#[derive(Clone, Debug)]
pub struct CpuFaultEvent {
    pub insn_count: u64,
    pub pc: u64,
    pub fault_code: u32,
    pub description: String,
}

impl HelmSpySnapshot {
    /// Compute IPC from the snapshot fields. Returns 0.0 if tick_count == 0.
    pub fn ipc(&self) -> f64 {
        if self.tick_count == 0 {
            0.0
        } else {
            self.insn_count as f64 / self.tick_count as f64
        }
    }

    /// Total instruction count across all mix classes. Should equal insn_count.
    pub fn insn_mix_total(&self) -> u64 {
        self.insn_mix.iter().map(|(_, c)| c).sum()
    }
}
