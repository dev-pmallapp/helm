# helm-plugin

Unified plugin system for the HELM simulator, consolidating the previously separate `helm-plugin-api` and `helm-plugins` crates.

## Overview

This crate provides:

- **Stable Plugin API** (`api` module) - Core traits and types for building plugins
- **Plugin Runtime** (`runtime` module) - Callback infrastructure and registry
- **Built-in Plugins** (`builtins` module) - Common trace, memory, and debug plugins

## For Plugin Authors

Create custom plugins by implementing the `HelmComponent` or `HelmPlugin` traits:

```rust
use helm_plugin::api::*;

pub struct MyCustomPlugin {
    // plugin state
}

impl HelmComponent for MyCustomPlugin {
    fn component_type(&self) -> &'static str {
        "custom.my-plugin"
    }
    
    fn interfaces(&self) -> &[&str] {
        &["custom"]
    }
    
    fn reset(&mut self) -> HelmResult<()> {
        // reset plugin state
        Ok(())
    }
}
```

## For Simulator Integrators

Use the runtime to manage plugins:

```rust
use helm_plugin::api::ComponentRegistry;
use helm_plugin::{PluginRegistry, register_builtins};

fn main() {
    // Register built-in plugins
    let mut comp_registry = ComponentRegistry::new();
    register_builtins(&mut comp_registry);
    
    // Create runtime registry for callbacks
    let mut plugin_registry = PluginRegistry::new();
    
    // Enable specific plugins and wire up callbacks...
}
```

## Module Organization

```
helm-plugin/
├─ api/           # Stable public API (versioned)
│  ├─ HelmComponent trait
│  ├─ HelmPlugin trait
│  ├─ ComponentRegistry
│  └─ Dynamic plugin loading (feature-gated)
│
├─ runtime/       # Plugin runtime infrastructure
│  ├─ PluginRegistry (callback dispatch)
│  ├─ Scoreboard (per-vCPU data)
│  ├─ Callback types
│  └─ Bridge adapter
│
└─ builtins/      # Built-in plugins (feature-gated)
   ├─ trace/      # Execution tracing (insn_count, execlog, etc.)
   ├─ memory/     # Memory analysis (cache_sim)
   └─ debug/      # Debug utilities
```

## Features

- `default` - Includes built-in plugins
- `builtins` - Enable built-in plugin implementations
- `dynamic` - Enable dynamic plugin loading (Unix only)

## Migration from Old Crates

This crate replaces both `helm-plugin-api` and `helm-plugins`. See [../helm-plugin-api/DEPRECATED.md](../helm-plugin-api/DEPRECATED.md) for migration guide.

**Old:**
```rust
use helm_plugin_api::*;
use helm_plugins::{PluginRegistry, register_builtins};
```

**New:**
```rust
use helm_plugin::api::*;
use helm_plugin::{PluginRegistry, register_builtins};
```

## Built-in Plugins

When the `builtins` feature is enabled (default), the following plugins are available:

### Trace Plugins
- `plugin.trace.insn-count` - Instruction counter (per-vCPU)
- `plugin.trace.execlog` - Execution trace logger
- `plugin.trace.hotblocks` - Basic block profiler
- `plugin.trace.howvec` - Instruction class histogram
- `plugin.trace.syscall-trace` - Syscall entry/return logger

### Memory Plugins
- `plugin.memory.cache` - Set-associative cache simulation

## API Stability

The `api` module provides a stable, versioned interface with semantic versioning guarantees. Breaking changes will result in a major version bump.

The `runtime` and `builtins` modules are public but may evolve more rapidly.

## See Also

- [PLUGIN_CONSOLIDATION_PROPOSAL.md](../../PLUGIN_CONSOLIDATION_PROPOSAL.md) - Design rationale
- [docs/plugin-trace-system.md](../../docs/plugin-trace-system.md) - Plugin system documentation
