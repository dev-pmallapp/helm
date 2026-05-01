//! `PerfFormula` -- gem5-style lazy formula over registered counters
//! and histograms.
//!
//! A `PerfFormula` is an expression tree built from constants,
//! references to other registry entries, and the four arithmetic
//! operators. It is evaluated at dump time against a
//! `StatsRegistryRead` view.
//!
//! Dual-impl, feature-gated:
//!
//! - `--features=formulas` (off by default; implies `stats`):
//!   live enum + recursive `eval`.
//! - default build: ZST shell whose constructors all return `Self`
//!   and `eval` returns `0.0`. The registry's `formula()` registration
//!   path is also gated, so call sites compile in both modes but the
//!   release build pays nothing.
//!
//! See `docs/research/gem5-stats-helm-adaptation.md` § 3.1 / § 4
//! (Slice S5).

use crate::registry::StatsRegistryRead;

#[cfg(feature = "formulas")]
pub use live::PerfFormula;
#[cfg(not(feature = "formulas"))]
pub use noop::PerfFormula;

#[cfg(feature = "formulas")]
mod live {
    use super::StatsRegistryRead;

    /// Lazy expression tree over registry entries. Constructed at
    /// `register_stats` time, evaluated at dump time.
    #[derive(Clone, Debug)]
    pub enum PerfFormula {
        Const(f64),
        /// Absolute counter path (`system.cpu0.icache.hits`).
        Counter(String),
        /// Absolute histogram total (sum of all buckets).
        HistogramTotal(String),
        /// Absolute label-counter total (sum of all labels).
        LabelTotal(String),
        Add(Box<PerfFormula>, Box<PerfFormula>),
        Sub(Box<PerfFormula>, Box<PerfFormula>),
        Mul(Box<PerfFormula>, Box<PerfFormula>),
        /// Division. If the divisor evaluates to `0.0`, the result is
        /// `0.0` (gem5 convention -- avoids NaNs in `stats.txt`).
        Div(Box<PerfFormula>, Box<PerfFormula>),
    }

    impl PerfFormula {
        pub fn constant(v: f64) -> Self {
            Self::Const(v)
        }
        pub fn counter(path: impl Into<String>) -> Self {
            Self::Counter(path.into())
        }
        pub fn histogram_total(path: impl Into<String>) -> Self {
            Self::HistogramTotal(path.into())
        }
        pub fn label_total(path: impl Into<String>) -> Self {
            Self::LabelTotal(path.into())
        }
        pub fn add(a: Self, b: Self) -> Self {
            Self::Add(Box::new(a), Box::new(b))
        }
        pub fn sub(a: Self, b: Self) -> Self {
            Self::Sub(Box::new(a), Box::new(b))
        }
        pub fn mul(a: Self, b: Self) -> Self {
            Self::Mul(Box::new(a), Box::new(b))
        }
        pub fn div(a: Self, b: Self) -> Self {
            Self::Div(Box::new(a), Box::new(b))
        }

        /// Evaluate the formula against a registry view. Missing
        /// references resolve to `0`. Division by zero yields `0.0`.
        pub fn eval(&self, reg: &dyn StatsRegistryRead) -> f64 {
            match self {
                Self::Const(v) => *v,
                Self::Counter(p) => reg.counter_value(p).unwrap_or(0) as f64,
                Self::HistogramTotal(p) => reg.histogram_total(p).unwrap_or(0) as f64,
                Self::LabelTotal(p) => reg.label_total(p).unwrap_or(0) as f64,
                Self::Add(a, b) => a.eval(reg) + b.eval(reg),
                Self::Sub(a, b) => a.eval(reg) - b.eval(reg),
                Self::Mul(a, b) => a.eval(reg) * b.eval(reg),
                Self::Div(a, b) => {
                    let d = b.eval(reg);
                    if d == 0.0 {
                        0.0
                    } else {
                        a.eval(reg) / d
                    }
                }
            }
        }
    }
}

#[cfg(not(feature = "formulas"))]
mod noop {
    use super::StatsRegistryRead;

    /// ZST formula. Every constructor returns `Self`; `eval` returns
    /// `0.0`. The registry's `formula()` registration is also a no-op
    /// without the feature, so the formula tree never materialises.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct PerfFormula;

    impl PerfFormula {
        #[inline(always)]
        pub fn constant(_v: f64) -> Self {
            Self
        }
        #[inline(always)]
        pub fn counter(_p: impl Into<String>) -> Self {
            Self
        }
        #[inline(always)]
        pub fn histogram_total(_p: impl Into<String>) -> Self {
            Self
        }
        #[inline(always)]
        pub fn label_total(_p: impl Into<String>) -> Self {
            Self
        }
        #[inline(always)]
        pub fn add(_a: Self, _b: Self) -> Self {
            Self
        }
        #[inline(always)]
        pub fn sub(_a: Self, _b: Self) -> Self {
            Self
        }
        #[inline(always)]
        pub fn mul(_a: Self, _b: Self) -> Self {
            Self
        }
        #[inline(always)]
        pub fn div(_a: Self, _b: Self) -> Self {
            Self
        }
        #[inline(always)]
        pub fn eval(&self, _reg: &dyn StatsRegistryRead) -> f64 {
            0.0
        }
    }
}

#[cfg(all(test, feature = "formulas"))]
mod tests {
    use super::PerfFormula;
    use crate::StatsRegistry;

    #[test]
    fn const_eval_is_value() {
        let reg = StatsRegistry::new();
        assert_eq!(PerfFormula::constant(42.0).eval(&reg), 42.0);
    }

    #[test]
    fn counter_eval_reads_registry() {
        let mut reg = StatsRegistry::new();
        reg.counter("system.cpu0.icache.hits", "").add(7);
        let f = PerfFormula::counter("system.cpu0.icache.hits");
        assert_eq!(f.eval(&reg), 7.0);
    }

    #[test]
    fn missing_counter_reads_zero() {
        let reg = StatsRegistry::new();
        let f = PerfFormula::counter("nope");
        assert_eq!(f.eval(&reg), 0.0);
    }

    #[test]
    fn hit_rate_formula() {
        let mut reg = StatsRegistry::new();
        reg.counter("c.hits", "").add(3);
        reg.counter("c.misses", "").add(1);
        let f = PerfFormula::div(
            PerfFormula::counter("c.hits"),
            PerfFormula::add(
                PerfFormula::counter("c.hits"),
                PerfFormula::counter("c.misses"),
            ),
        );
        assert_eq!(f.eval(&reg), 0.75);
    }

    #[test]
    fn divide_by_zero_yields_zero() {
        let reg = StatsRegistry::new();
        let f = PerfFormula::div(PerfFormula::constant(5.0), PerfFormula::constant(0.0));
        assert_eq!(f.eval(&reg), 0.0);
    }

    #[test]
    fn histogram_and_label_total_paths() {
        let mut reg = StatsRegistry::new();
        let h = reg.histogram("h", "", &[2, 4]);
        h.record(1);
        h.record(3);
        h.record(5);
        let l = reg.label_counter("lbl", "");
        l.bump_static("a");
        l.bump_static("a");
        l.bump_static("b");
        assert_eq!(PerfFormula::histogram_total("h").eval(&reg), 3.0);
        assert_eq!(PerfFormula::label_total("lbl").eval(&reg), 3.0);
    }
}
