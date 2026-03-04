use helm_plugin_api::component::HelmComponent;
use helm_plugin_api::loader::ComponentRegistry;

use crate::bridge::{register_builtins, PluginComponentAdapter};
use crate::info::InsnInfo;
use crate::plugin::PluginArgs;
use crate::registry::PluginRegistry;

#[test]
fn register_builtins_populates_registry() {
    let mut reg = ComponentRegistry::new();
    register_builtins(&mut reg);

    let types = reg.list();
    assert!(types.contains(&"plugin.trace.insn-count"));
    assert!(types.contains(&"plugin.trace.execlog"));
    assert!(types.contains(&"plugin.trace.hotblocks"));
    assert!(types.contains(&"plugin.trace.howvec"));
    assert!(types.contains(&"plugin.trace.syscall-trace"));
    assert!(types.contains(&"plugin.memory.cache"));
}

#[test]
fn builtin_count_matches() {
    let mut reg = ComponentRegistry::new();
    register_builtins(&mut reg);
    assert_eq!(reg.list().len(), 6);
}

#[test]
fn create_returns_valid_component() {
    let mut reg = ComponentRegistry::new();
    register_builtins(&mut reg);

    let comp = reg.create("plugin.trace.insn-count");
    assert!(comp.is_some());

    let comp = comp.unwrap();
    assert_eq!(comp.component_type(), "plugin.trace.insn-count");
    assert!(comp.interfaces().contains(&"trace"));
}

#[test]
fn create_unknown_returns_none() {
    let mut reg = ComponentRegistry::new();
    register_builtins(&mut reg);
    assert!(reg.create("plugin.nonexistent").is_none());
}

#[test]
fn filter_by_trace_interface() {
    let mut reg = ComponentRegistry::new();
    register_builtins(&mut reg);

    let trace_plugins = reg.types_with_interface("trace");
    assert!(trace_plugins.contains(&"plugin.trace.insn-count"));
    assert!(trace_plugins.contains(&"plugin.trace.execlog"));
    assert!(!trace_plugins.contains(&"plugin.memory.cache"));
}

#[test]
fn filter_by_profiling_interface() {
    let mut reg = ComponentRegistry::new();
    register_builtins(&mut reg);

    let prof = reg.types_with_interface("profiling");
    assert!(prof.contains(&"plugin.trace.hotblocks"));
    assert!(prof.contains(&"plugin.trace.howvec"));
    assert!(prof.contains(&"plugin.memory.cache"));
    assert!(!prof.contains(&"plugin.trace.execlog"));
}

#[test]
fn adapter_reset_clears_installed_flag() {
    let plugin = Box::new(crate::trace::InsnCount::new());
    let mut adapter = PluginComponentAdapter::new(plugin, "plugin.trace.insn-count", &["trace"]);

    // Install into a throwaway registry
    let mut preg = PluginRegistry::new();
    adapter.install(&mut preg, &PluginArgs::new());

    // Reset should allow re-install
    adapter.reset().unwrap();

    let mut preg2 = PluginRegistry::new();
    adapter.install(&mut preg2, &PluginArgs::new());
    assert!(preg2.has_insn_callbacks());
}

#[test]
fn adapter_install_is_idempotent() {
    let plugin = Box::new(crate::trace::InsnCount::new());
    let mut adapter = PluginComponentAdapter::new(plugin, "plugin.trace.insn-count", &["trace"]);

    let mut preg = PluginRegistry::new();
    adapter.install(&mut preg, &PluginArgs::new());
    let count_before = preg.insn_exec.len();

    // Second install should be a no-op
    adapter.install(&mut preg, &PluginArgs::new());
    assert_eq!(preg.insn_exec.len(), count_before);
}

#[test]
fn adapter_callbacks_fire_through_component() {
    let plugin = Box::new(crate::trace::InsnCount::new());
    let mut adapter = PluginComponentAdapter::new(plugin, "plugin.trace.insn-count", &["trace"]);

    let mut preg = PluginRegistry::new();
    adapter.install(&mut preg, &PluginArgs::new());

    let insn = InsnInfo {
        vaddr: 0x1000,
        bytes: vec![0; 4],
        size: 4,
        mnemonic: "ADD".to_string(),
        symbol: None,
    };
    preg.fire_insn_exec(0, &insn);
    preg.fire_insn_exec(0, &insn);

    assert!(preg.has_insn_callbacks());
}

#[test]
fn component_lifecycle_methods_succeed() {
    let mut reg = ComponentRegistry::new();
    register_builtins(&mut reg);

    for type_name in reg.list() {
        let mut comp = reg.create(type_name).unwrap();
        assert!(comp.realize().is_ok(), "realize failed for {type_name}");
        assert!(comp.reset().is_ok(), "reset failed for {type_name}");
        assert!(comp.tick(100).is_ok(), "tick failed for {type_name}");
    }
}
