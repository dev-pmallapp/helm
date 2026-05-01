//! Standalone helper that drives `StatsProducer::register_stats` for
//! a flat list of `(path, &producer)` entries. This is the foundation
//! for the QOM-style elaboration walker described in
//! `docs/research/gem5-stats-helm-adaptation.md` § 3.7.4. Full
//! SimObject-tree integration is intentionally **out of scope** for
//! this slice -- only the trait + standalone walker function landed
//! in S4.5. Future slices wire it through `HelmSim::instantiate()`
//! once the object tree exposes a child iterator.

use helm_stats::{StatsProducer, StatsRegistry, StatsScope};

/// Build a `StatsScope` per producer (rooted at the supplied dot-path
/// segment) and call `register_stats` on each. The walker neither
/// owns the producers nor mutates them; producers stash counter
/// handles via interior mutability if they need hot-path access.
pub fn walk_and_register<'a, S, I>(producers: I, registry: &mut StatsRegistry)
where
    S: StatsProducer + 'a,
    I: IntoIterator<Item = (&'a str, &'a S)>,
{
    for (path, producer) in producers {
        let mut scope = StatsScope::new(registry, path);
        producer.register_stats(&mut scope);
    }
}
