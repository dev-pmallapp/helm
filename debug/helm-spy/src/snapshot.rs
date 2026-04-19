//! Snapshot schema owned by the collection/session layer.
//!
//! `helm-spy` owns the data model for point-in-time observation snapshots.
//! Delivery/formatting crates such as `helm-report` should consume and
//! re-export these types rather than defining their own parallel schema.

/// Immutable point-in-time copy of observation session state.
///
/// Created on the cold path. All atomic fields from the live session are
/// copied as plain integers, making the snapshot safe to format from any
/// thread without ordering concerns.
#[derive(Clone, Debug)]
pub struct HelmSpySnapshot {
    pub insn_count: u64,
    pub insn_mix: Vec<(String, u64)>,
    pub hot_pcs: Vec<(u64, u64)>,
    pub branch_heatmap: Vec<(u64, u64)>,
    pub cache_l1d: Option<CacheSnapshot>,
    pub branch_pred: Option<BranchPredSnapshot>,
    pub user_stage2_insn_abort: Option<UserStage2InsnAbortSnapshot>,
    pub fault_history: Option<Vec<CpuFaultEvent>>,
    pub tick_count: u64,
    pub snapshot_ns: u64,
}

/// Immutable snapshot of L1 data cache state.
#[derive(Clone, Debug)]
pub struct CacheSnapshot {
    pub name: String,
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
}

/// Immutable snapshot of branch predictor state.
#[derive(Clone, Debug)]
pub struct BranchPredSnapshot {
    pub name: String,
    pub kind: String,
    pub predictions: u64,
    pub mispredictions: u64,
    pub miss_rate: f64,
}

/// Immutable snapshot of low-VA EL1 user-style stage-2 instruction abort stats.
#[derive(Clone, Debug)]
pub struct UserStage2InsnAbortSnapshot {
    pub events: u64,
    pub repeats: u64,
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
        self.insn_mix.iter().map(|(_, count)| count).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> HelmSpySnapshot {
        HelmSpySnapshot {
            insn_count: 120,
            insn_mix: vec![("IntAlu".to_string(), 70), ("Load".to_string(), 50)],
            hot_pcs: vec![(0x1000, 10)],
            branch_heatmap: vec![(0x1004, 3)],
            cache_l1d: None,
            branch_pred: None,
            user_stage2_insn_abort: None,
            fault_history: None,
            tick_count: 40,
            snapshot_ns: 123,
        }
    }

    #[test]
    fn snapshot_reports_ipc_and_mix_total() {
        let snap = sample_snapshot();
        assert_eq!(snap.ipc(), 3.0);
        assert_eq!(snap.insn_mix_total(), 120);
    }

    #[test]
    fn snapshot_ipc_is_zero_when_ticks_are_zero() {
        let mut snap = sample_snapshot();
        snap.tick_count = 0;
        assert_eq!(snap.ipc(), 0.0);
    }
}
