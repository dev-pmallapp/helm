#![allow(missing_docs)]
// helm-report -- delivery layer for the Instrumentation-v2 redesign.
//
// Receives already-collected analysis data (in the form of an
// HelmSpySnapshot) and delivers it to one or more configured
// destinations in a configured format.
//
// This crate has exactly one job: format bytes, write bytes. It contains
// no analysis logic, no probe subscriptions, and no hot-path code.

pub mod error;
pub mod format;
pub mod report;
pub mod schedule;
pub mod sink;
pub mod snapshot;

pub use error::SinkError;
pub use format::{CsvFormatter, GemstatsFormatter, JsonFormatter, ReportFormatter, TextFormatter};
pub use report::Report;
pub use schedule::{ReportSchedule, ReportTrigger};
pub use sink::{
    sink_from_uri, AsyncFileSink, BinaryTraceSink, FileSink, NullSink, PythonSink, Sink,
    StderrSink, TcpSink, TraceFileHeader, HELM_TRACE_MAGIC, HELM_TRACE_VERSION,
};
pub use snapshot::{BranchPredSnapshot, CacheSnapshot, CpuFaultEvent, HelmSpySnapshot};

/// Shared test infrastructure used across all test modules.
#[cfg(test)]
pub(crate) mod tests {
    use crate::snapshot::*;

    /// Construct a minimal HelmSpySnapshot for tests.
    pub fn test_snapshot() -> HelmSpySnapshot {
        HelmSpySnapshot {
            insn_count: 10_000_000,
            insn_mix: vec![
                ("IntAlu".to_owned(), 5_000_000),
                ("Load".to_owned(), 2_000_000),
                ("Store".to_owned(), 1_000_000),
                ("Branch".to_owned(), 1_500_000),
                ("SIMD".to_owned(), 500_000),
            ],
            hot_pcs: vec![
                (0xffff_8000_1001_2a4c, 234_812),
                (0xffff_8000_1001_2abc, 198_234),
            ],
            branch_heatmap: vec![(0xffff_8000_1001_2a4c, 100_000)],
            cache_l1d: Some(CacheSnapshot {
                name: "l1d".to_owned(),
                hits: 9_823_441,
                misses: 176_559,
                hit_rate: 0.982_153,
            }),
            branch_pred: Some(BranchPredSnapshot {
                name: "bimodal".to_owned(),
                kind: "BiModal".to_owned(),
                predictions: 1_500_000,
                mispredictions: 105_000,
                miss_rate: 0.07,
            }),
            fault_history: None,
            tick_count: 8_130_081,
            snapshot_ns: 1_710_849_600_000_000_000,
        }
    }
}
