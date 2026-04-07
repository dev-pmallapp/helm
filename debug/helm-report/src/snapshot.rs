//! Snapshot schema re-exported from `helm-spy`.
//!
//! `helm-report` formats and delivers collected data; it no longer owns the
//! snapshot data model.

pub use helm_spy::snapshot::{BranchPredSnapshot, CacheSnapshot, CpuFaultEvent, HelmSpySnapshot};
