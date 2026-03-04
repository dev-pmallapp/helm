//! Bridge between `HelmPlugin` (callback-based instrumentation) and
//! `HelmComponent` (the stable plugin-API component model).
//!
//! [`PluginComponentAdapter`] wraps any [`HelmPlugin`] so it can live
//! inside a [`ComponentRegistry`].  [`register_builtins`] populates a
//! registry with every built-in plugin that ships with `helm-plugins`.

use helm_core::HelmResult;

use crate::api::component::{ComponentInfo, HelmComponent};
use crate::api::loader::ComponentRegistry;
use crate::api::plugin::{HelmPlugin, PluginArgs};
use crate::runtime::registry::PluginRegistry;

use crate::builtins::memory::CacheSim;
use crate::builtins::trace::{ExecLog, HotBlocks, HowVec, InsnCount, SyscallTrace};

// ---------------------------------------------------------------------------
// Adapter: HelmPlugin -> HelmComponent
// ---------------------------------------------------------------------------

/// Wraps a callback-based [`HelmPlugin`] so it satisfies the
/// [`HelmComponent`] trait from `helm-plugin-api`.
pub struct PluginComponentAdapter {
    plugin: Box<dyn HelmPlugin>,
    component_type: &'static str,
    interfaces: &'static [&'static str],
    installed: bool,
}

impl PluginComponentAdapter {
    pub fn new(
        plugin: Box<dyn HelmPlugin>,
        component_type: &'static str,
        interfaces: &'static [&'static str],
    ) -> Self {
        Self {
            plugin,
            component_type,
            interfaces,
            installed: false,
        }
    }

    /// Install the wrapped plugin's callbacks into a [`PluginRegistry`].
    /// Call this once after `realize()` with the simulation's registry.
    pub fn install(&mut self, reg: &mut PluginRegistry, args: &PluginArgs) {
        if !self.installed {
            self.plugin.install(reg, args);
            self.installed = true;
        }
    }

    /// Delegate to the wrapped plugin's `atexit()`.
    pub fn atexit(&mut self) {
        self.plugin.atexit();
    }
}

impl HelmComponent for PluginComponentAdapter {
    fn component_type(&self) -> &'static str {
        self.component_type
    }

    fn interfaces(&self) -> &[&str] {
        self.interfaces
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.installed = false;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Built-in registration
// ---------------------------------------------------------------------------

type BuiltinEntry = (
    &'static str,
    &'static str,
    &'static [&'static str],
    fn() -> Box<dyn HelmPlugin>,
);

/// Register every built-in plugin as a component in the registry.
pub fn register_builtins(registry: &mut ComponentRegistry) {
    let builtins: Vec<BuiltinEntry> = vec![
        (
            "plugin.trace.insn-count",
            "Instruction counter (per-vCPU scoreboard)",
            &["trace"],
            || Box::new(InsnCount::new()),
        ),
        (
            "plugin.trace.execlog",
            "Execution trace logger",
            &["trace"],
            || Box::new(ExecLog::new()),
        ),
        (
            "plugin.trace.hotblocks",
            "Hot-block profiler",
            &["trace", "profiling"],
            || Box::new(HotBlocks::new()),
        ),
        (
            "plugin.trace.howvec",
            "Instruction-class histogram",
            &["trace", "profiling"],
            || Box::new(HowVec::new()),
        ),
        (
            "plugin.trace.syscall-trace",
            "Syscall entry/return logger",
            &["trace", "syscall"],
            || Box::new(SyscallTrace::new()),
        ),
        (
            "plugin.memory.cache",
            "Set-associative cache simulation",
            &["memory", "profiling"],
            || Box::new(CacheSim::new()),
        ),
    ];

    for (type_name, description, interfaces, factory) in builtins {
        let ifaces: &'static [&'static str] = Box::leak(interfaces.to_vec().into_boxed_slice());
        let comp_type: &'static str = type_name;
        registry.register(ComponentInfo {
            type_name,
            description,
            interfaces: ifaces,
            factory: Box::new(move || {
                Box::new(PluginComponentAdapter::new(factory(), comp_type, ifaces))
            }),
        });
    }
}
