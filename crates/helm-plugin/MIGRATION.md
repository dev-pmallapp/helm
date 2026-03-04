# Migration from helm-plugin-api and helm-plugins

This document provides a comprehensive migration guide for moving from the old two-crate plugin system to the new unified `helm-plugin` crate.

## Quick Reference

### Import Changes

**Old imports:**
```rust
use helm_plugin_api::*;
use helm_plugin_api::loader::ComponentRegistry;
use helm_plugin_api::dynamic::DynamicPluginLoader;

use helm_plugins::PluginRegistry;
use helm_plugins::bridge::{register_builtins, PluginComponentAdapter};
use helm_plugins::info::{InsnInfo, MemInfo, SyscallInfo};
```

**New imports:**
```rust
use helm_plugin::api::*;
use helm_plugin::api::ComponentRegistry;
use helm_plugin::api::DynamicPluginLoader;

use helm_plugin::runtime::PluginRegistry;
use helm_plugin::runtime::{register_builtins, PluginComponentAdapter};
use helm_plugin::runtime::{InsnInfo, MemInfo, SyscallInfo};
```

Or more concisely using convenience re-exports:
```rust
use helm_plugin::api::*;
use helm_plugin::{PluginRegistry, register_builtins};
use helm_plugin::runtime::{InsnInfo, MemInfo, SyscallInfo};
```

### Dependency Changes

**Old Cargo.toml:**
```toml
[dependencies]
helm-plugin-api = "0.1"
helm-plugins = "0.1"
```

**New Cargo.toml:**
```toml
[dependencies]
helm-plugin = "0.2"
```

## Module Structure

The new `helm-plugin` crate is organized into three main modules:

### `api` - Stable Public API
Contains traits and types for plugin authors:
- `HelmComponent` trait
- `HelmPlugin` trait
- `ComponentRegistry`
- `PluginArgs`
- `PluginMetadata`
- Dynamic loading (Unix only, feature-gated)

### `runtime` - Plugin Runtime Infrastructure
Contains runtime support for plugin callbacks:
- `PluginRegistry` - callback registration and dispatch
- `Scoreboard` - per-vCPU lock-free data
- Callback types and info structs
- `PluginComponentAdapter` - bridges HelmPlugin to HelmComponent
- `register_builtins()` - registers built-in plugins

### `builtins` - Built-in Plugins
Contains implementations of common plugins:
- `trace/` - Execution tracing (insn_count, execlog, hotblocks, howvec, syscall_trace)
- `memory/` - Memory analysis (cache_sim)
- `debug/` - Debug utilities (future)

## Feature Flags

The `helm-plugin` crate supports the following features:

- `default = ["builtins"]` - Built-in plugins enabled by default
- `builtins` - Include built-in plugin implementations
- `dynamic` - Enable dynamic plugin loading (Unix only)

To use without built-ins:
```toml
helm-plugin = { version = "0.2", default-features = false }
```

To enable dynamic loading:
```toml
helm-plugin = { version = "0.2", features = ["dynamic"] }
```

## Code Examples

### External Plugin Author

**Before:**
```rust
// Cargo.toml
[dependencies]
helm-plugin-api = "0.1"

// src/lib.rs
use helm_plugin_api::*;

pub struct MyPlugin;
impl HelmComponent for MyPlugin {
    fn component_type(&self) -> &'static str { "custom.my" }
    // ...
}
```

**After:**
```rust
// Cargo.toml
[dependencies]
helm-plugin = "0.2"

// src/lib.rs
use helm_plugin::api::*;

pub struct MyPlugin;
impl HelmComponent for MyPlugin {
    fn component_type(&self) -> &'static str { "custom.my" }
    // ...
}
```

### Simulator Integration

**Before:**
```rust
use helm_plugin_api::loader::ComponentRegistry;
use helm_plugins::bridge::register_builtins;
use helm_plugins::PluginRegistry;

let mut comp_reg = ComponentRegistry::new();
register_builtins(&mut comp_reg);

let mut plugin_reg = PluginRegistry::new();
```

**After:**
```rust
use helm_plugin::api::ComponentRegistry;
use helm_plugin::{PluginRegistry, register_builtins};

let mut comp_reg = ComponentRegistry::new();
register_builtins(&mut comp_reg);

let mut plugin_reg = PluginRegistry::new();
```

### Using Plugin Callbacks

**Before:**
```rust
use helm_plugins::info::{InsnInfo, SyscallInfo};
use helm_plugins::PluginRegistry;

fn handle_insn(reg: &PluginRegistry, info: &InsnInfo) {
    reg.fire_insn_exec(0, info);
}
```

**After:**
```rust
use helm_plugin::runtime::{InsnInfo, SyscallInfo};
use helm_plugin::PluginRegistry;

fn handle_insn(reg: &PluginRegistry, info: &InsnInfo) {
    reg.fire_insn_exec(0, info);
}
```

## Benefits

1. **Simpler dependencies** - One crate instead of two
2. **Clearer organization** - Explicit module boundaries (api, runtime, builtins)
3. **Better discoverability** - All plugin functionality in one place
4. **Feature flags** - Opt-out of builtins if not needed
5. **Same stability guarantees** - API module maintains SemVer compatibility
6. **No code duplication** - Shared types live in appropriate modules

## Backward Compatibility

The old `helm-plugin-api` and `helm-plugins` crates remain in the workspace for now but are marked as deprecated. They will be removed in a future release.

If you need to maintain compatibility with both old and new versions temporarily, you can use conditional compilation:

```rust
#[cfg(feature = "new-plugin-api")]
use helm_plugin::api::*;

#[cfg(not(feature = "new-plugin-api"))]
use helm_plugin_api::*;
```

## Questions?

See [README.md](README.md) for more details or consult the [PLUGIN_CONSOLIDATION_PROPOSAL.md](../../PLUGIN_CONSOLIDATION_PROPOSAL.md) for the design rationale.
