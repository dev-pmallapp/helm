//! Snapshot schema re-exported from `helm-spy`.
//!
//! `helm-report` formats and delivers collected data; it no longer owns the
//! snapshot data model.

pub use helm_spy::snapshot::{
    AddrRangeFilterSnapshot, BranchDirectionSnapshot, BranchPredSnapshot, CacheSnapshot,
    CpuFaultEvent, HelmSpySnapshot, JitActivitySnapshot, MmuActivitySnapshot,
    PcRangeFilterSnapshot, UserStage2InsnAbortSnapshot,
};
