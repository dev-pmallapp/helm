//! `StatsProducer` + `StatsScope` -- QOM-style hierarchical
//! registration for stats. A node implements `StatsProducer` once;
//! `StatsScope` is the path-prefixed view onto the registry that the
//! elaboration walker hands to it.
//!
//! Dual-impl, feature-gated:
//!
//! - `--features=stats`: live `StatsScope` carries `&mut StatsRegistry`
//!   and a dot-path prefix, concatenating leaves into full paths
//!   before delegating to the underlying registry methods.
//! - default build: `StatsScope` is a unit ZST (with a phantom
//!   lifetime parameter so the `<'a>` keeps compiling), every method
//!   is `#[inline(always)]` empty, and `register_stats` calls evaporate.
//!
//! See `docs/design/helm-stats/LLD-stats.md` § 4b.

#[cfg(feature = "stats")]
pub use live::StatsScope;
#[cfg(not(feature = "stats"))]
pub use noop::StatsScope;

/// Trait implemented by objects that publish stats. The elaboration
/// walker invokes `register_stats` once per object with a `StatsScope`
/// already prefixed to that object's canonical dot-path.
///
/// `&self` because counter handles are `Clone` (cheap Arc bumps when
/// `stats` is on, ZST copies otherwise) and the producer normally
/// stashes them through interior mutability or already owns them.
pub trait StatsProducer {
    fn register_stats(&self, scope: &mut StatsScope<'_>);
}

#[cfg(feature = "stats")]
mod live {
    use crate::{LabelCounter, PerfCounter, PerfHistogram, StatsRegistry};
    use std::sync::Arc;

    /// Path-prefixed view onto a `StatsRegistry`. Each call concatenates
    /// `leaf` onto the current prefix with `.` and delegates to the
    /// underlying registry method. An empty prefix (root scope) emits
    /// `leaf` verbatim, never a leading `.`.
    pub struct StatsScope<'a> {
        pub(crate) registry: &'a mut StatsRegistry,
        pub(crate) prefix: String,
    }

    impl<'a> StatsScope<'a> {
        /// Construct a new scope at `prefix`. Pass an empty string for
        /// the root scope.
        pub fn new(registry: &'a mut StatsRegistry, prefix: impl Into<String>) -> Self {
            Self {
                registry,
                prefix: prefix.into(),
            }
        }

        /// Current absolute prefix (no trailing `.`).
        pub fn prefix(&self) -> &str {
            &self.prefix
        }

        /// Register-or-fetch a counter at `prefix.leaf`.
        pub fn counter(&mut self, leaf: &str, desc: &str) -> PerfCounter {
            let path = join_path(&self.prefix, leaf);
            self.registry.counter(&path, desc)
        }

        /// Register-or-fetch a histogram at `prefix.leaf`.
        pub fn histogram(
            &mut self,
            leaf: &str,
            desc: &str,
            edges: &[u64],
        ) -> Arc<PerfHistogram> {
            let path = join_path(&self.prefix, leaf);
            self.registry.histogram(&path, desc, edges)
        }

        /// Register-or-fetch a label counter at `prefix.leaf`.
        pub fn label_counter(&mut self, leaf: &str, desc: &str) -> LabelCounter {
            let path = join_path(&self.prefix, leaf);
            self.registry.label_counter(&path, desc)
        }

        /// Open a child scope at `prefix.segment`.
        pub fn child(&mut self, segment: &str) -> StatsScope<'_> {
            let prefix = join_path(&self.prefix, segment);
            StatsScope {
                registry: &mut *self.registry,
                prefix,
            }
        }
    }

    fn join_path(prefix: &str, leaf: &str) -> String {
        if prefix.is_empty() {
            leaf.to_string()
        } else {
            let mut s = String::with_capacity(prefix.len() + 1 + leaf.len());
            s.push_str(prefix);
            s.push('.');
            s.push_str(leaf);
            s
        }
    }
}

#[cfg(not(feature = "stats"))]
mod noop {
    use crate::{LabelCounter, PerfCounter, PerfHistogram, StatsRegistry};
    use std::marker::PhantomData;
    use std::sync::Arc;

    /// ZST scope. Carries a phantom lifetime parameter so `StatsScope<'a>`
    /// is the same shape in both impls.
    pub struct StatsScope<'a> {
        _marker: PhantomData<&'a mut StatsRegistry>,
    }

    impl<'a> StatsScope<'a> {
        #[inline(always)]
        pub fn new(_registry: &'a mut StatsRegistry, _prefix: impl Into<String>) -> Self {
            Self {
                _marker: PhantomData,
            }
        }
        #[inline(always)]
        pub fn prefix(&self) -> &str {
            ""
        }
        #[inline(always)]
        pub fn counter(&mut self, _leaf: &str, _desc: &str) -> PerfCounter {
            PerfCounter::new()
        }
        #[inline(always)]
        pub fn histogram(
            &mut self,
            _leaf: &str,
            _desc: &str,
            _edges: &[u64],
        ) -> Arc<PerfHistogram> {
            PerfHistogram::new(Vec::new())
        }
        #[inline(always)]
        pub fn label_counter(&mut self, _leaf: &str, _desc: &str) -> LabelCounter {
            LabelCounter::new()
        }
        #[inline(always)]
        pub fn child(&mut self, _segment: &str) -> StatsScope<'_> {
            StatsScope {
                _marker: PhantomData,
            }
        }
    }
}

#[cfg(all(test, feature = "stats"))]
mod tests {
    use super::{StatsProducer, StatsScope};
    use crate::StatsRegistry;

    struct Icache;
    impl StatsProducer for Icache {
        fn register_stats(&self, scope: &mut StatsScope<'_>) {
            let hits = scope.counter("hits", "L1I cache hits");
            hits.inc();
        }
    }

    #[test]
    fn child_scopes_concatenate_dot_paths() {
        let mut reg = StatsRegistry::new();
        // Top-level scope with a non-empty prefix.
        let mut root = StatsScope {
            registry: &mut reg,
            prefix: "system".to_string(),
        };
        let counter = root
            .child("cpu0")
            .child("icache")
            .counter("hits", "L1I cache hits");
        counter.inc();

        // Re-fetching at the canonical path returns the same handle.
        let same = reg.counter("system.cpu0.icache.hits", "");
        assert_eq!(same.get(), 1);
    }

    #[test]
    fn root_scope_empty_prefix_does_not_emit_leading_dot() {
        let mut reg = StatsRegistry::new();
        let mut root = StatsScope::new(&mut reg, "");
        let c = root.counter("x", "x desc");
        c.inc();

        // The path must be "x", not ".x".
        assert_eq!(reg.counter("x", "").get(), 1);
        assert_eq!(reg.counter(".x", "").get(), 0);
    }

    #[test]
    fn child_of_root_concatenates_without_leading_dot() {
        let mut reg = StatsRegistry::new();
        let mut root = StatsScope::new(&mut reg, "");
        let mut cpu = root.child("cpu0");
        let c = cpu.counter("cycles", "cpu cycles");
        c.add(7);

        assert_eq!(reg.counter("cpu0.cycles", "").get(), 7);
    }

    #[test]
    fn producer_trait_is_object_safe_via_static_dispatch() {
        let mut reg = StatsRegistry::new();
        let mut scope = StatsScope::new(&mut reg, "system.cpu0.icache");
        let icache = Icache;
        icache.register_stats(&mut scope);
        assert_eq!(reg.counter("system.cpu0.icache.hits", "").get(), 1);
    }
}
