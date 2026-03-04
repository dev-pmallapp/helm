# DEPRECATED

**This crate has been merged into `helm-plugin`.**

As of version 0.2.0, `helm-plugin-api` and `helm-plugins` have been consolidated into a single `helm-plugin` crate for better maintainability and clearer organization.

## Migration Guide

### For External Plugin Authors

**Old:**
```toml
[dependencies]
helm-plugin-api = "0.1"
```

```rust
use helm_plugin_api::*;

pub struct MyPlugin;

impl HelmComponent for MyPlugin {
    fn component_type(&self) -> &'static str { "custom.my-plugin" }
    // ...
}
```

**New:**
```toml
[dependencies]
helm-plugin = "0.2"
```

```rust
use helm_plugin::api::*;

pub struct MyPlugin;

impl HelmComponent for MyPlugin {
    fn component_type(&self) -> &'static str { "custom.my-plugin" }
    // ...
}
```

### For Simulator Integrators

**Old:**
```toml
[dependencies]
helm-plugin-api = "0.1"
helm-plugins = "0.1"
```

```rust
use helm_plugin_api::loader::ComponentRegistry;
use helm_plugins::{PluginRegistry, register_builtins};
```

**New:**
```toml
[dependencies]
helm-plugin = "0.2"
```

```rust
use helm_plugin::api::ComponentRegistry;
use helm_plugin::{PluginRegistry, register_builtins};
// or more explicitly:
// use helm_plugin::runtime::PluginRegistry;
```

## Benefits of the Consolidation

1. **Single dependency** - Only need to depend on `helm-plugin` instead of two crates
2. **Clear module organization** - `api` for stable interfaces, `runtime` for infrastructure, `builtins` for implementations
3. **Better discoverability** - All plugin-related functionality in one place
4. **Easier to maintain** - No need to keep two crates in sync

## See Also

- [helm-plugin crate](../helm-plugin/)
- [PLUGIN_CONSOLIDATION_PROPOSAL.md](../../PLUGIN_CONSOLIDATION_PROPOSAL.md)
