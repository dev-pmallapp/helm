//! Smoke test for `helm_engine::stats_walker::walk_and_register`.
//!
//! `helm-engine`'s dev-dependency override forces `helm-stats`'s
//! `stats` feature on during `cargo test -p helm-engine`, so the
//! registry behaves live and the assertions are meaningful. The
//! producer/scope/registry types are unconditionally exported by
//! `helm-stats` (ZST without the feature, live with it), so the test
//! file itself does not need a `cfg` gate -- it asserts on values
//! that only the live impl can produce.
//!
//! NB: gating on `#[cfg(feature = "stats")]` against `helm-engine`'s
//! own `stats` feature would silently skip the test, since the
//! dev-dep override does not propagate to helm-engine's feature flag.
//! See the slice deviation note in
//! `docs/research/gem5-stats-helm-adaptation.md` § 4 (Slice S4.5).

use helm_engine::stats_walker::walk_and_register;
use helm_stats::{StatsProducer, StatsRegistry, StatsScope};

struct FakeIcache;

impl StatsProducer for FakeIcache {
    fn register_stats(&self, scope: &mut StatsScope<'_>) {
        let hits = scope.counter("hits", "L1I cache hits");
        let misses = scope.counter("misses", "L1I cache misses");
        hits.add(3);
        misses.add(1);
    }
}

#[test]
fn walk_and_register_invokes_producer_at_canonical_path() {
    let mut registry = StatsRegistry::new();
    let icache = FakeIcache;

    walk_and_register([("system.cpu0.icache", &icache)], &mut registry);

    assert_eq!(
        registry.counter("system.cpu0.icache.hits", "").get(),
        3,
        "walker should register and route increments to system.cpu0.icache.hits"
    );
    assert_eq!(
        registry.counter("system.cpu0.icache.misses", "").get(),
        1,
        "walker should register and route increments to system.cpu0.icache.misses"
    );
}
