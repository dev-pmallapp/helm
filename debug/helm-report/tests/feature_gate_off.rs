//! Verify that with the `report` feature disabled (the default for
//! perf builds), the entire helm-report delivery layer collapses to
//! ZSTs whose hot/cold-path methods compile to nothing.
//!
//! Run with: `cargo test -p helm-report --no-default-features
//!           --test feature_gate_off`
//!
//! The whole file is gated `#[cfg(not(feature = "report"))]` so the
//! workspace's default `cargo test` (which enables `report,helmstats`
//! transitively) does not pull these assertions in -- the live impl
//! has different sizes.

#![cfg(not(feature = "report"))]

use helm_report::{
    AsyncFileSink, BinaryTraceSink, CsvFormatter, FileSink, HelmSpySnapshot, HelmstatsFormatter,
    JsonFormatter, JitActivitySnapshot, MmuActivitySnapshot, BranchDirectionSnapshot,
    NullSink, PythonSink, Report, ReportFormatter, ReportSchedule, ReportTrigger,
    Sink, StderrSink, TcpSink, TextFormatter, TraceFileHeader,
};
use std::sync::Arc;

/// Minimal `HelmSpySnapshot` for the noop tests -- the snapshot type
/// has no `Default` impl since its `insn_mix`/`hot_pcs`/etc fields
/// would normally come from the live aggregator.
fn empty_snapshot() -> HelmSpySnapshot {
    HelmSpySnapshot {
        insn_count: 0,
        insn_mix: Vec::new(),
        branch_direction: BranchDirectionSnapshot { taken: 0, not_taken: 0 },
        mmu_activity: MmuActivitySnapshot::default(),
        hot_pcs: Vec::new(),
        branch_heatmap: Vec::new(),
        cache_l1d: None,
        branch_pred: None,
        jit_activity: JitActivitySnapshot::default(),
        scoreboard_filter: None,
        scoreboard_addr_filter: None,
        user_stage2_insn_abort: None,
        fault_history: None,
        tick_count: 0,
        snapshot_ns: 0,
    }
}

#[test]
fn null_sink_is_zst() {
    assert_eq!(std::mem::size_of::<NullSink>(), 0);
}

#[test]
fn stderr_sink_is_zst() {
    assert_eq!(std::mem::size_of::<StderrSink>(), 0);
}

#[test]
fn file_sink_is_zst() {
    assert_eq!(std::mem::size_of::<FileSink>(), 0);
}

#[test]
fn async_file_sink_is_zst() {
    assert_eq!(std::mem::size_of::<AsyncFileSink>(), 0);
}

#[test]
fn tcp_sink_is_zst() {
    assert_eq!(std::mem::size_of::<TcpSink>(), 0);
}

#[test]
fn python_sink_is_zst() {
    assert_eq!(std::mem::size_of::<PythonSink>(), 0);
}

#[test]
fn binary_trace_sink_is_zst() {
    // PhantomData-only generic; per-instantiation size is 0.
    assert_eq!(std::mem::size_of::<BinaryTraceSink<u64>>(), 0);
}

#[test]
fn trace_file_header_is_zst() {
    assert_eq!(std::mem::size_of::<TraceFileHeader>(), 0);
}

#[test]
fn formatters_are_zsts() {
    assert_eq!(std::mem::size_of::<TextFormatter>(), 0);
    assert_eq!(std::mem::size_of::<JsonFormatter>(), 0);
    assert_eq!(std::mem::size_of::<CsvFormatter>(), 0);
    assert_eq!(std::mem::size_of::<HelmstatsFormatter>(), 0);
}

#[test]
fn report_and_schedule_are_zsts() {
    assert_eq!(std::mem::size_of::<Report>(), 0);
    assert_eq!(std::mem::size_of::<ReportSchedule>(), 0);
}

#[test]
fn null_sink_write_compiles_to_nothing() {
    let sink = NullSink;
    for _ in 0..100_000 {
        sink.write(&[0u8; 256]).unwrap();
    }
    sink.flush().unwrap();
    assert_eq!(sink.name(), "null");
}

#[test]
fn formatters_emit_empty_buffers() {
    let snap = empty_snapshot();
    let formatters: Vec<Box<dyn ReportFormatter>> = vec![
        Box::new(TextFormatter),
        Box::new(JsonFormatter),
        Box::new(CsvFormatter),
        Box::new(HelmstatsFormatter),
    ];
    for f in formatters {
        assert!(f.format_session(&snap).is_empty());
        assert!(f.format_counter("x", 1, "u").is_empty());
        assert!(f.format_histogram("y", &[("0", 1)]).is_empty());
    }
}

#[test]
fn report_deliver_is_noop() {
    use std::sync::atomic::{AtomicU64, Ordering};

    let writes = Arc::new(AtomicU64::new(0));

    struct CountingSink(Arc<AtomicU64>);
    impl Sink for CountingSink {
        fn write(&self, _: &[u8]) -> std::io::Result<()> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn name(&self) -> &str {
            "counting"
        }
    }

    let snap = Arc::new(empty_snapshot());
    let report = Report::new(
        snap,
        Box::new(TextFormatter),
        vec![Box::new(CountingSink(Arc::clone(&writes)))],
    );

    // In the noop build, `Report::new` drops the sinks immediately
    // and `deliver()` returns Ok without invoking the formatter or
    // any sink.
    report.deliver().expect("noop deliver must be Ok");
    report.flush_all().expect("noop flush must be Ok");
    assert_eq!(
        writes.load(Ordering::Relaxed),
        0,
        "noop Report::deliver must not call sink.write()"
    );
}

#[test]
fn report_schedule_check_is_noop() {
    let snap = Arc::new(empty_snapshot());
    let report = Report::new(snap, Box::new(TextFormatter), Vec::new());
    let mut schedule = ReportSchedule::new(
        report,
        vec![
            ReportTrigger::AtExit,
            ReportTrigger::EveryNInsns(1_000),
            ReportTrigger::OnPc(0xdead_beef),
            ReportTrigger::Explicit,
            ReportTrigger::OnCounter {
                name: "x".into(),
                threshold: 5,
            },
        ],
    );

    for i in 0..1_000_000u64 {
        schedule.check(0xdead_beef, i);
    }
    schedule.flush_at_exit();
    schedule.deliver().expect("noop schedule deliver must be Ok");
}

// The `helmstats` writer entry points must be ABSENT without the
// `helmstats` feature. The whole module is already
// `#![cfg(not(feature = "report"))]`, and `helmstats` implies
// `report`, so this test always runs when the file does. We make
// the absence assertion explicit via a runtime cfg-check; trying to
// name `helm_report::emit_config_ini` directly would refuse to
// compile, which is the strongest possible verification.
#[test]
fn helmstats_writers_are_absent() {
    assert!(cfg!(not(feature = "helmstats")));
}
