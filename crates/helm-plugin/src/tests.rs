//! Tests for the helm-plugin crate.

#[test]
fn test_plugin_api_available() {
    // Just ensure the API types are accessible
    use crate::api::{HelmComponent, HelmPlugin, PluginArgs, ComponentRegistry};
    let _args = PluginArgs::new();
    let _registry = ComponentRegistry::new();
}

#[cfg(feature = "builtins")]
#[test]
fn test_builtins_available() {
    // Ensure builtin plugins are accessible with feature flag
    use crate::builtins::trace::InsnCount;
    use crate::builtins::memory::CacheSim;
    let _insn_count = InsnCount::new();
    let _cache = CacheSim::new();
}
