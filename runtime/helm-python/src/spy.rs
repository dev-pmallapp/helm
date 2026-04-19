#![allow(missing_docs)]

use crate::system::HelmSystem;
use helm_engine::HelmSim;
use pyo3::prelude::*;
#[cfg(feature = "instrumentation")]
use std::sync::Arc;

// ── Shared builder ───────────────────────────────────────────────────────────

/// Build a HelmSpy from cache/predictor config and wire it to probes.
///
/// Shared between `HelmSpy::new()` (standalone) and `HelmSystem::spy()` (deprecated compat).
pub(crate) fn build_spy_session(
    sim: &mut HelmSim,
    cache_l1d_size: Option<usize>,
    cache_l1d_ways: usize,
    cache_l1d_line: usize,
    predictor: Option<&str>,
    predictor_bits: u8,
    predictor_table_bits: Option<u8>,
    start_insn: Option<u64>,
    end_insn: Option<u64>,
    system_ref: Option<Py<HelmSystem>>,
) -> PyResult<HelmSpy> {
    use helm_spy::analysis::branch_pred::{BranchPredictor, PredictorKind};
    use helm_spy::session::HelmSpy as InnerHelmSpy;
    #[cfg(feature = "instrumentation")]
    use helm_spy::window::Window;

    let mut session = InnerHelmSpy::new();

    if let Some(size) = cache_l1d_size {
        session = session.with_cache_l1d(size, cache_l1d_ways, cache_l1d_line);
    }

    if let Some(kind_str) = predictor {
        let kind = match kind_str {
            "bimodal" | "BiModal" => PredictorKind::BiModal {
                bits: predictor_bits,
            },
            "gshare" | "GShare" => PredictorKind::GShare {
                hist_bits: predictor_bits,
                table_bits: predictor_table_bits.unwrap_or(predictor_bits),
            },
            other => {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "unknown predictor kind {:?}: expected \"bimodal\" or \"gshare\"",
                    other
                )))
            }
        };
        session = session.with_branch_predictor(BranchPredictor::new(kind));
    }

    if let (Some(start), Some(end)) = (start_insn, end_insn) {
        if end <= start {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "end_insn ({end}) must be greater than start_insn ({start})"
            )));
        }
    }

    #[cfg(feature = "instrumentation")]
    match (start_insn, end_insn) {
        (Some(start), Some(end)) => {
            let window = Arc::new(Window::new(start, end));
            session.subscribe_in_window(sim.probes_mut(), window);
        }
        (None, None) => session.subscribe(sim.probes_mut()),
        _ => {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "start_insn and end_insn must either both be set or both be omitted",
            ))
        }
    }
    #[cfg(not(feature = "instrumentation"))]
    if start_insn.is_some() || end_insn.is_some() {
        return Err(pyo3::exceptions::PyRuntimeError::new_err(
            "instruction-window observation requires the 'instrumentation' feature",
        ));
    }
    let _ = sim; // used only when instrumentation wiring is enabled

    Ok(HelmSpy {
        session,
        system_ref,
    })
}

// ── HelmSpy ─────────────────────────────────────────────────────────────

/// Standalone observation session — attaches to a HelmSystem's probes.
#[pyclass(name = "HelmSpy")]
pub struct HelmSpy {
    pub(crate) session: helm_spy::session::HelmSpy,
    pub(crate) system_ref: Option<Py<HelmSystem>>,
}

#[pymethods]
impl HelmSpy {
    /// Create a new observation session attached to the given system.
    #[new]
    #[pyo3(signature = (
        system,
        *,
        cache_l1d_size=None,
        cache_l1d_ways=8,
        cache_l1d_line=64,
        predictor=None,
        predictor_bits=10,
        predictor_table_bits=None,
        start_insn=None,
        end_insn=None,
    ))]
    fn new(
        py: Python<'_>,
        system: Py<HelmSystem>,
        cache_l1d_size: Option<usize>,
        cache_l1d_ways: usize,
        cache_l1d_line: usize,
        predictor: Option<&str>,
        predictor_bits: u8,
        predictor_table_bits: Option<u8>,
        start_insn: Option<u64>,
        end_insn: Option<u64>,
    ) -> PyResult<Self> {
        let mut system_ref = system.borrow_mut(py);
        let sim = system_ref.require_sim()?;
        build_spy_session(
            sim,
            cache_l1d_size,
            cache_l1d_ways,
            cache_l1d_line,
            predictor,
            predictor_bits,
            predictor_table_bits,
            start_insn,
            end_insn,
            Some(system.clone_ref(py)),
        )
    }

    /// Detach from the system's probes. Metrics are frozen at detach time.
    fn detach(&mut self) -> PyResult<()> {
        // Currently a no-op — probes are statically subscribed.
        // Full detach will be implemented when probe unsubscribe is added.
        Ok(())
    }

    /// Total instructions retired since subscribe() was called.
    #[getter]
    fn insn_count(&self) -> u64 {
        self.session.insn_count.value()
    }

    /// Instruction mix table: list of (class_name, count, fraction) tuples.
    fn insn_mix(&self) -> Vec<(String, u64, f64)> {
        self.session
            .insn_mix
            .table()
            .into_iter()
            .map(|(name, count, frac)| (name.to_string(), count, frac))
            .collect()
    }

    /// Top-N hottest instruction PCs by execution count.
    #[pyo3(signature = (n=20))]
    fn hot_pcs(&self, n: usize) -> Vec<(u64, u64)> {
        self.session.hot_pcs.top(n)
    }

    /// Top-N most-executed branch source PCs.
    #[pyo3(signature = (n=20))]
    fn branch_heatmap(&self, n: usize) -> Vec<(u64, u64)> {
        self.session.branch_heatmap.top(n)
    }

    /// L1D cache hit rate [0.0, 1.0]. None if no cache model configured.
    #[getter]
    fn cache_hit_rate(&self) -> Option<f64> {
        self.session.cache_l1d.as_ref().map(|c| c.hit_rate())
    }

    /// L1D cache hits. None if no cache model configured.
    #[getter]
    fn cache_hits(&self) -> Option<u64> {
        self.session.cache_l1d.as_ref().map(|c| c.hits())
    }

    /// L1D cache misses. None if no cache model configured.
    #[getter]
    fn cache_misses(&self) -> Option<u64> {
        self.session.cache_l1d.as_ref().map(|c| c.misses())
    }

    /// Branch predictor miss rate [0.0, 1.0]. None if no predictor configured.
    #[getter]
    fn branch_miss_rate(&self) -> Option<f64> {
        self.session
            .branch_pred
            .as_ref()
            .and_then(|p| p.lock().ok().map(|g| g.miss_rate()))
    }

    /// Branch predictor mispredictions per 1000 instructions. None if no predictor.
    #[pyo3(signature = (insn_count=None))]
    fn branch_mpki(&self, insn_count: Option<u64>) -> Option<f64> {
        let n = insn_count.unwrap_or_else(|| self.session.insn_count.value());
        self.session
            .branch_pred
            .as_ref()
            .and_then(|p| p.lock().ok().map(|g| g.mpki(n)))
    }

    /// Snapshot of all current metrics as a Python dict.
    fn snapshot(&self, py: Python<'_>) -> pyo3::PyObject {
        use pyo3::types::PyDict;
        let snapshot = self.snapshot_for_output(py);
        #[allow(deprecated)]
        let d = PyDict::new_bound(py);
        let _ = d.set_item("insn_count", snapshot.insn_count);
        let _ = d.set_item("insn_mix", self.session.insn_mix.table());
        let _ = d.set_item("hot_pcs", snapshot.hot_pcs);
        let _ = d.set_item("branch_heatmap", snapshot.branch_heatmap);
        if let Some(ref c) = snapshot.cache_l1d {
            let _ = d.set_item("cache_hit_rate", c.hit_rate);
            let _ = d.set_item("cache_hits", c.hits);
            let _ = d.set_item("cache_misses", c.misses);
        }
        if let Some(ref p) = snapshot.branch_pred {
            let _ = d.set_item("branch_miss_rate", p.miss_rate);
            let mpki = if snapshot.insn_count == 0 {
                0.0
            } else {
                (p.mispredictions as f64 * 1000.0) / snapshot.insn_count as f64
            };
            let _ = d.set_item("branch_mpki", mpki);
        }
        if let Some(ref stats) = snapshot.user_stage2_insn_abort {
            let _ = d.set_item("user_stage2_insn_abort_events", stats.events);
            let _ = d.set_item("user_stage2_insn_abort_repeats", stats.repeats);
        }
        d.into()
    }

    /// Render the current snapshot using a named `helm-report` formatter.
    ///
    /// Supported formats: `text`, `json`, `csv`, `gemstats`.
    #[pyo3(signature = (format="text"))]
    fn render(&self, py: Python<'_>, format: &str) -> PyResult<String> {
        use helm_report::{
            CsvFormatter, GemstatsFormatter, JsonFormatter, ReportFormatter, TextFormatter,
        };

        let snapshot = self.snapshot_for_output(py);
        let formatter: Box<dyn ReportFormatter> = match format {
            "text" => Box::new(TextFormatter),
            "json" => Box::new(JsonFormatter),
            "csv" => Box::new(CsvFormatter),
            "gemstats" => Box::new(GemstatsFormatter),
            other => {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "unknown report format '{other}' (expected text, json, csv, or gemstats)"
                )))
            }
        };

        let bytes = formatter.format_session(&snapshot);
        String::from_utf8(bytes).map_err(|err| {
            pyo3::exceptions::PyRuntimeError::new_err(format!(
                "formatter '{format}' produced non-UTF-8 output: {err}"
            ))
        })
    }

    /// Deliver the current snapshot using a named formatter to a `helm-report` sink URI.
    ///
    /// Supported formats: `text`, `json`, `csv`, `gemstats`.
    /// Supported URIs follow `helm-report` conventions, e.g. `stderr:`,
    /// `null:`, `file:/abs/path`, `file+sync:/abs/path`, `tcp:host:port`.
    #[pyo3(signature = (uri, *, format="text"))]
    fn write_report(&self, py: Python<'_>, uri: &str, format: &str) -> PyResult<()> {
        use helm_report::{
            sink_from_uri, CsvFormatter, GemstatsFormatter, JsonFormatter, Report, ReportFormatter,
            TextFormatter,
        };
        use std::sync::Arc;

        let formatter: Box<dyn ReportFormatter> = match format {
            "text" => Box::new(TextFormatter),
            "json" => Box::new(JsonFormatter),
            "csv" => Box::new(CsvFormatter),
            "gemstats" => Box::new(GemstatsFormatter),
            other => {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "unknown report format '{other}' (expected text, json, csv, or gemstats)"
                )))
            }
        };

        let sink = sink_from_uri(uri).map_err(crate::errors::report_error)?;
        let report = Report::new(Arc::new(self.snapshot_for_output(py)), formatter, vec![sink]);
        report.deliver().map_err(crate::errors::report_error)
    }

    // ── track_*() API (v2) ─────────────────────────────────────────────────

    /// Enable instruction tracking. Activates insn_count, insn_mix, and hot_pcs.
    /// These are always-on by default; this method is a no-op but makes the
    /// intent explicit for the new observe().track_*() API pattern.
    fn track_insns(&self) -> PyResult<()> {
        // Already tracked by default via probe subscriptions
        Ok(())
    }

    /// Enable branch tracking. Activates branch_heatmap.
    fn track_branches(&self) -> PyResult<()> {
        // Already tracked by default via probe subscriptions
        Ok(())
    }

    /// Enable memory tracking with optional L1D cache configuration.
    #[pyo3(signature = (*, l1d_size=None, l1d_ways=8, l1d_line=64))]
    fn track_memory(
        &mut self,
        l1d_size: Option<usize>,
        l1d_ways: usize,
        l1d_line: usize,
    ) -> PyResult<()> {
        if let Some(size) = l1d_size {
            if self.session.cache_l1d.is_none() {
                self.session.cache_l1d = Some(std::sync::Arc::new(
                    helm_spy::analysis::CacheModel::new("L1D", size, l1d_ways, l1d_line),
                ));
            }
        }
        Ok(())
    }

    fn __repr__(&self) -> String {
        format!(
            "HelmSpy(insns={}, cache={}, pred={})",
            self.session.insn_count.value(),
            if self.session.cache_l1d.is_some() {
                "yes"
            } else {
                "no"
            },
            if self.session.branch_pred.is_some() {
                "yes"
            } else {
                "no"
            },
        )
    }

    fn __traverse__(&self, visit: pyo3::PyVisit<'_>) -> Result<(), pyo3::PyTraverseError> {
        if let Some(ref system) = self.system_ref {
            visit.call(system)?;
        }
        Ok(())
    }

    fn __clear__(&mut self) {
        self.system_ref = None;
    }
}

impl HelmSpy {
    fn snapshot_for_output(&self, py: Python<'_>) -> helm_spy::snapshot::HelmSpySnapshot {
        let mut snapshot = self.session.snapshot();
        if let Some(ref system_ref) = self.system_ref {
            let system = system_ref.borrow(py);
            Self::augment_snapshot_with_system_stats(&mut snapshot, &system);
        }
        snapshot
    }

    fn augment_snapshot_with_system_stats(
        snapshot: &mut helm_spy::snapshot::HelmSpySnapshot,
        system: &HelmSystem,
    ) {
        Self::set_user_stage2_abort_stats(
            snapshot,
            system
                .sim
                .as_ref()
                .and_then(|sim| sim.user_stage2_insn_abort_stats()),
        );
    }

    fn set_user_stage2_abort_stats(
        snapshot: &mut helm_spy::snapshot::HelmSpySnapshot,
        stats: Option<(u64, u64)>,
    ) {
        if let Some((events, repeats)) = stats {
            snapshot.user_stage2_insn_abort =
                Some(helm_spy::snapshot::UserStage2InsnAbortSnapshot { events, repeats });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HelmSpy;
    use helm_spy::session::HelmSpy as InnerHelmSpy;
    use pyo3::Python;

    #[test]
    fn render_text_contains_sim_insns() {
        let spy = HelmSpy {
            session: InnerHelmSpy::new(),
            system_ref: None,
        };
        spy.session.insn_count.add(42);

        Python::with_gil(|py| {
            let rendered = spy.render(py, "text").expect("render text");
            assert!(rendered.contains("sim_insns"));
            assert!(rendered.contains("42"));
        });
    }

    #[test]
    fn render_rejects_unknown_format() {
        let spy = HelmSpy {
            session: InnerHelmSpy::new(),
            system_ref: None,
        };
        Python::with_gil(|py| {
            assert!(spy.render(py, "bogus").is_err());
        });
    }

    #[test]
    fn set_user_stage2_abort_stats_adds_stage2_fields() {
        let mut snapshot = InnerHelmSpy::new().snapshot();

        HelmSpy::set_user_stage2_abort_stats(&mut snapshot, Some((3, 1)));

        let stats = snapshot
            .user_stage2_insn_abort
            .expect("stage2 counters should be populated");
        assert_eq!(stats.events, 3);
        assert_eq!(stats.repeats, 1);
    }
}
