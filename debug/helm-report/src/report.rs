// src/report.rs -- Report: pairs a snapshot with a formatter and sinks.
//
// Dual-impl per `docs/design/helm-report/HLD.md` § 13. With `report`
// off, `Report::new()` accepts and silently drops the formatter and
// sinks, and `deliver()` / `deliver_to()` / `flush_all()` are inlined
// `Ok(())` bodies (no formatting, no I/O).

#[cfg(feature = "report")]
pub use live::Report;
#[cfg(not(feature = "report"))]
pub use noop::Report;

#[cfg(feature = "report")]
mod live {

use crate::{error::SinkError, format::ReportFormatter, sink::Sink, snapshot::HelmSpySnapshot};
use std::sync::Arc;

/// Pairs an immutable session snapshot with a formatter and one or more sinks.
///
/// `deliver()` formats the snapshot exactly once, then writes the resulting
/// bytes to each sink in order. Errors from individual sinks are accumulated;
/// a failure from one sink does NOT prevent delivery to subsequent sinks.
pub struct Report {
    session: Arc<HelmSpySnapshot>,
    formatter: Box<dyn ReportFormatter>,
    sinks: Vec<Box<dyn Sink>>,
}

impl Report {
    pub fn new(
        session: Arc<HelmSpySnapshot>,
        formatter: Box<dyn ReportFormatter>,
        sinks: Vec<Box<dyn Sink>>,
    ) -> Self {
        Report {
            session,
            formatter,
            sinks,
        }
    }

    /// Format once; write to all sinks. Collects all errors.
    pub fn deliver(&self) -> Result<(), SinkError> {
        let data = self.formatter.format_session(&self.session);
        let mut errors = Vec::new();
        for sink in &self.sinks {
            if let Err(e) = sink.write(&data) {
                errors.push(e);
            }
        }
        self.flush_all_inner(&mut errors);
        if errors.is_empty() {
            Ok(())
        } else if errors.len() == 1 {
            Err(SinkError::Io(errors.remove(0)))
        } else {
            Err(SinkError::MultipleErrors(errors))
        }
    }

    /// Format once; write to a single additional sink (ad-hoc delivery).
    /// Does not affect the permanent sink list.
    pub fn deliver_to(&self, sink: &dyn Sink) -> Result<(), SinkError> {
        let data = self.formatter.format_session(&self.session);
        sink.write(&data).map_err(SinkError::Io)?;
        sink.flush().map_err(SinkError::Io)
    }

    /// Flush all registered sinks.
    pub fn flush_all(&self) -> Result<(), SinkError> {
        let mut errors = Vec::new();
        self.flush_all_inner(&mut errors);
        if errors.is_empty() {
            Ok(())
        } else if errors.len() == 1 {
            Err(SinkError::Io(errors.remove(0)))
        } else {
            Err(SinkError::MultipleErrors(errors))
        }
    }

    fn flush_all_inner(&self, errors: &mut Vec<std::io::Error>) {
        for sink in &self.sinks {
            if let Err(e) = sink.flush() {
                errors.push(e);
            }
        }
    }
}

}

#[cfg(not(feature = "report"))]
mod noop {
    use crate::{error::SinkError, format::ReportFormatter, sink::Sink, snapshot::HelmSpySnapshot};
    use std::sync::Arc;

    /// ZST shell. Holds no fields; the formatter and sinks are
    /// dropped at construction time. `deliver()` is an inlined
    /// `Ok(())`.
    pub struct Report;

    impl Report {
        #[inline(always)]
        pub fn new(
            _session: Arc<HelmSpySnapshot>,
            _formatter: Box<dyn ReportFormatter>,
            _sinks: Vec<Box<dyn Sink>>,
        ) -> Self {
            Report
        }

        #[inline(always)]
        pub fn deliver(&self) -> Result<(), SinkError> {
            Ok(())
        }

        #[inline(always)]
        pub fn deliver_to(&self, _sink: &dyn Sink) -> Result<(), SinkError> {
            Ok(())
        }

        #[inline(always)]
        pub fn flush_all(&self) -> Result<(), SinkError> {
            Ok(())
        }
    }
}

#[cfg(all(test, feature = "report"))]
mod tests {
    use super::*;
    use crate::format::TextFormatter;
    use std::sync::{Arc, Mutex};

    #[test]
    fn report_deliver_writes_to_single_sink() {
        let snap = Arc::new(crate::tests::test_snapshot());
        let sink = crate::sink::TestSink::new("inspect");
        let report = Report::new(
            Arc::clone(&snap),
            Box::new(TextFormatter::default()),
            vec![],
        );
        report.deliver_to(&sink).unwrap();
        let out = sink.contents_as_string();
        assert!(out.contains("sim_insns"), "deliver_to missing sim_insns");
    }

    #[test]
    fn report_deliver_fan_out_to_multiple_sinks() {
        let snap = Arc::new(crate::tests::test_snapshot());

        let captured1: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let captured2: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

        struct CaptureSink(Arc<Mutex<Vec<u8>>>);
        impl crate::sink::Sink for CaptureSink {
            fn write(&self, data: &[u8]) -> std::io::Result<()> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(())
            }
            fn name(&self) -> &str {
                "capture"
            }
        }

        let report = Report::new(
            snap,
            Box::new(TextFormatter::default()),
            vec![
                Box::new(CaptureSink(Arc::clone(&captured1))),
                Box::new(CaptureSink(Arc::clone(&captured2))),
            ],
        );

        report.deliver().unwrap();

        let c1 = String::from_utf8(captured1.lock().unwrap().clone()).unwrap();
        let c2 = String::from_utf8(captured2.lock().unwrap().clone()).unwrap();

        assert_eq!(c1, c2, "fan-out sinks received different content");
        assert!(c1.contains("sim_insns"), "content missing from sink 1");
        assert!(c2.contains("sim_insns"), "content missing from sink 2");
    }

    #[test]
    fn report_deliver_continues_on_sink_error() {
        use std::io;

        struct FailSink;
        impl crate::sink::Sink for FailSink {
            fn write(&self, _: &[u8]) -> io::Result<()> {
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "injected failure",
                ))
            }
            fn name(&self) -> &str {
                "fail"
            }
        }

        let captured = Arc::new(Mutex::new(Vec::<u8>::new()));
        struct CaptureSink2(Arc<Mutex<Vec<u8>>>);
        impl crate::sink::Sink for CaptureSink2 {
            fn write(&self, data: &[u8]) -> io::Result<()> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(())
            }
            fn name(&self) -> &str {
                "capture2"
            }
        }

        let snap = Arc::new(crate::tests::test_snapshot());
        let report = Report::new(
            snap,
            Box::new(TextFormatter::default()),
            vec![
                Box::new(FailSink),
                Box::new(CaptureSink2(Arc::clone(&captured))),
            ],
        );

        let result = report.deliver();
        assert!(result.is_err(), "expected error from FailSink");

        let content = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        assert!(
            content.contains("sim_insns"),
            "CaptureSink2 should have received content despite FailSink error"
        );
    }
}
