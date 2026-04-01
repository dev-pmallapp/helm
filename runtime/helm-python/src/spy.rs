#![allow(missing_docs)]

use crate::system::HelmSystem;
use helm_engine::HelmSim;
use pyo3::prelude::*;

// ── Shared builder ───────────────────────────────────────────────────────────

/// Build a HelmSpy from cache/predictor config and wire it to probes.
///
/// Shared between `HelmSpy::new()` (standalone) and `HelmSystem::spy()` (compat).
pub(crate) fn build_spy_session(
    sim: &mut HelmSim,
    cache_l1d_size: Option<usize>,
    cache_l1d_ways: usize,
    cache_l1d_line: usize,
    predictor: Option<&str>,
    predictor_bits: u8,
    predictor_table_bits: Option<u8>,
) -> PyResult<HelmSpy> {
    use helm_spy::analysis::branch_pred::{BranchPredictor, PredictorKind};
    use helm_spy::session::HelmSpy as InnerHelmSpy;

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

    #[cfg(debug_assertions)]
    session.subscribe(sim.probes_mut());
    let _ = sim; // used only in debug builds for probe subscription

    Ok(HelmSpy { session })
}

// ── HelmSpy ─────────────────────────────────────────────────────────────

/// Standalone observation session — attaches to a HelmSystem's probes.
#[pyclass(name = "HelmSpy")]
pub struct HelmSpy {
    pub(crate) session: helm_spy::session::HelmSpy,
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
    ))]
    fn new(
        system: &mut HelmSystem,
        cache_l1d_size: Option<usize>,
        cache_l1d_ways: usize,
        cache_l1d_line: usize,
        predictor: Option<&str>,
        predictor_bits: u8,
        predictor_table_bits: Option<u8>,
    ) -> PyResult<Self> {
        let sim = system.require_sim()?;
        build_spy_session(
            sim,
            cache_l1d_size,
            cache_l1d_ways,
            cache_l1d_line,
            predictor,
            predictor_bits,
            predictor_table_bits,
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
        #[allow(deprecated)]
        let d = PyDict::new_bound(py);
        let _ = d.set_item("insn_count", self.session.insn_count.value());
        let mix: Vec<(String, u64, f64)> = self
            .session
            .insn_mix
            .table()
            .into_iter()
            .map(|(n, c, f)| (n.to_string(), c, f))
            .collect();
        let _ = d.set_item("insn_mix", mix);
        let _ = d.set_item("hot_pcs", self.session.hot_pcs.top(20));
        let _ = d.set_item("branch_heatmap", self.session.branch_heatmap.top(20));
        if let Some(ref c) = self.session.cache_l1d {
            let _ = d.set_item("cache_hit_rate", c.hit_rate());
            let _ = d.set_item("cache_hits", c.hits());
            let _ = d.set_item("cache_misses", c.misses());
        }
        if let Some(ref p) = self.session.branch_pred {
            if let Ok(guard) = p.lock() {
                let _ = d.set_item("branch_miss_rate", guard.miss_rate());
                let _ = d.set_item("branch_mpki", guard.mpki(self.session.insn_count.value()));
            }
        }
        d.into()
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
}
