# Plugin System Consolidation Proposal

## Current State Analysis

### Crate Structure

**helm-plugin-api** (Stable API - 366 lines)
- `component.rs` - HelmComponent trait, ComponentInfo
- `loader.rs` - ComponentRegistry for component factories
- `dynamic.rs` - Dynamic plugin loading (dlopen-based)
- `lib.rs` - Public exports, PluginMetadata, API versioning

**helm-plugins** (Builtin Plugins - ~1500+ lines)
- `plugin.rs` - HelmPlugin trait, PluginArgs
- `registry.rs` - PluginRegistry (callback storage/dispatch)
- `scoreboard.rs` - Per-vCPU lock-free data structure
- `callback.rs`, `info.rs` - Callback infrastructure
- `bridge.rs` - Adapts HelmPlugin → HelmComponent
- `trace/` - 5 builtin trace plugins (insn_count, execlog, hotblocks, howvec, syscall_trace)
- `memory/` - 1 builtin memory plugin (cache_sim)
- `debug/` - Debug plugins (future)

### Key Issues

1. **Two Plugin Systems**
   - `HelmComponent` - Object-oriented, lifecycle-based (realize, reset, tick)
   - `HelmPlugin` - Callback-based, instrumentation-focused (install callbacks)
   - Bridge layer adds complexity and indirection

2. **Dependency Coupling**
   - helm-plugins depends on helm-plugin-api
   - External plugin authors only need helm-plugin-api
   - But builtin plugins use both systems

3. **Unclear Separation**
   - Not obvious which types belong to stable API vs internal implementation
   - PluginRegistry, Scoreboard, callback infrastructure mixed with builtins

## Consolidation Strategies

### Option 1: Single Crate with Module-Based Separation

**Structure:**
```
helm-plugin/
├─ Cargo.toml
└─ src/
   ├─ lib.rs                    # Public exports, feature flags
   │
   ├─ api/                      # STABLE PUBLIC API
   │  ├─ mod.rs                 # Re-exports for external use
   │  ├─ component.rs           # HelmComponent trait
   │  ├─ plugin.rs              # HelmPlugin trait
   │  ├─ metadata.rs            # PluginMetadata, versioning
   │  ├─ loader.rs              # ComponentRegistry
   │  └─ dynamic.rs             # Dynamic loading (feature-gated)
   │
   ├─ runtime/                  # PLUGIN RUNTIME (public but internal)
   │  ├─ mod.rs
   │  ├─ registry.rs            # PluginRegistry (callback dispatch)
   │  ├─ scoreboard.rs          # Scoreboard helper
   │  ├─ callback.rs            # Callback types
   │  ├─ info.rs                # InsnInfo, MemInfo, etc.
   │  └─ bridge.rs              # HelmPlugin → HelmComponent adapter
   │
   └─ builtins/                 # BUILTIN PLUGINS (feature-gated)
      ├─ mod.rs
      ├─ trace/
      │  ├─ mod.rs
      │  ├─ insn_count.rs
      │  ├─ execlog.rs
      │  ├─ hotblocks.rs
      │  ├─ howvec.rs
      │  └─ syscall_trace.rs
      ├─ memory/
      │  ├─ mod.rs
      │  └─ cache_sim.rs
      └─ debug/
         └─ mod.rs
```

**Cargo.toml:**
```toml
[package]
name = "helm-plugin"
version = "0.1.0"
edition = "2021"

[features]
default = ["builtins"]
builtins = []         # Include built-in plugins
dynamic = ["libc"]    # Enable dynamic plugin loading

[dependencies]
helm-core = { workspace = true }
helm-object = { workspace = true }
helm-device = { workspace = true }
helm-timing = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
log = { workspace = true }

[target.'cfg(unix)'.dependencies]
libc = { version = "0.2", optional = true }
```

**lib.rs:**
```rust
//! # helm-plugin
//!
//! Unified plugin system for HELM simulator.
//!
//! ## For Plugin Authors
//!
//! Use the `api` module to implement custom plugins:
//!
//! ```rust
//! use helm_plugin::api::*;
//!
//! pub struct MyPlugin;
//!
//! impl HelmComponent for MyPlugin {
//!     fn component_type(&self) -> &'static str { "custom.my-plugin" }
//!     // ...
//! }
//! ```
//!
//! ## For Simulator Integrators
//!
//! Use the `runtime` module to manage plugins at runtime:
//!
//! ```rust
//! use helm_plugin::runtime::*;
//!
//! let mut registry = PluginRegistry::new();
//! // Install plugins and fire callbacks
//! ```

// ===== STABLE PUBLIC API =====
pub mod api {
    //! Stable plugin API for external plugin authors.
    //!
    //! This module contains the core traits and types that plugin
    //! authors depend on. Breaking changes to this API require a
    //! major version bump.

    pub use crate::api_impl::component::{ComponentInfo, HelmComponent};
    pub use crate::api_impl::plugin::{HelmPlugin, PluginArgs};
    pub use crate::api_impl::metadata::{PluginMetadata, PLUGIN_API_VERSION};
    pub use crate::api_impl::loader::ComponentRegistry;

    #[cfg(all(unix, feature = "dynamic"))]
    pub use crate::api_impl::dynamic::{
        DynamicPluginLoader, DynLoadError, HelmPluginVTable, ENTRY_SYMBOL,
    };

    // Re-export common types from helm-core
    pub use helm_core::types::{Addr, Cycle, Word};
    pub use helm_core::{HelmError, HelmResult};
    pub use helm_device::{DeviceAccess, MemoryMappedDevice};
    pub use helm_object::{HelmObject, Property, PropertyType, PropertyValue};
    pub use helm_timing::TimingModel;
}

// ===== PLUGIN RUNTIME =====
pub mod runtime {
    //! Plugin runtime and callback infrastructure.
    //!
    //! This module provides the infrastructure for managing plugins
    //! at runtime, including callback registration and dispatch.

    pub use crate::runtime_impl::registry::PluginRegistry;
    pub use crate::runtime_impl::scoreboard::Scoreboard;
    pub use crate::runtime_impl::callback::*;
    pub use crate::runtime_impl::info::*;
    pub use crate::runtime_impl::bridge::PluginComponentAdapter;

    #[cfg(feature = "builtins")]
    pub use crate::runtime_impl::bridge::register_builtins;
}

// ===== BUILTIN PLUGINS =====
#[cfg(feature = "builtins")]
pub mod builtins {
    //! Built-in plugins shipped with HELM.

    pub mod trace {
        pub use crate::builtins_impl::trace::*;
    }

    pub mod memory {
        pub use crate::builtins_impl::memory::*;
    }

    pub mod debug {
        pub use crate::builtins_impl::debug::*;
    }
}

// ===== INTERNAL MODULES =====
// These are not re-exported but used by public API
mod api_impl {
    pub mod component;
    pub mod plugin;
    pub mod metadata;
    pub mod loader;
    
    #[cfg(all(unix, feature = "dynamic"))]
    pub mod dynamic;
}

mod runtime_impl {
    pub mod registry;
    pub mod scoreboard;
    pub mod callback;
    pub mod info;
    pub mod bridge;
}

#[cfg(feature = "builtins")]
mod builtins_impl {
    pub mod trace;
    pub mod memory;
    pub mod debug;
}

// ===== CONVENIENCE RE-EXPORTS =====
// For backwards compatibility and convenience
pub use api::{ComponentInfo, HelmComponent, HelmPlugin, PluginArgs};
pub use runtime::PluginRegistry;

#[cfg(feature = "builtins")]
pub use runtime::register_builtins;
```

**Benefits:**
1. ✅ Single crate reduces maintenance overhead
2. ✅ Clear module boundaries (api, runtime, builtins)
3. ✅ Feature flags allow users to opt-out of builtins
4. ✅ Easier to document and understand
5. ✅ No circular dependencies
6. ✅ External plugins only need to depend on one crate
7. ✅ Clear API stability boundary via `api` module

**Drawbacks:**
1. ⚠️ Larger crate size (but still manageable ~2000 lines)
2. ⚠️ Must be careful about API surface area
3. ⚠️ Need discipline to keep api/ modules stable

---

### Option 2: Keep Separate but Reorganize

**Structure:**
```
helm-plugin-api/          # Stable external API only
├─ component.rs           # HelmComponent trait
├─ loader.rs              # ComponentRegistry
└─ dynamic.rs             # Dynamic loading

helm-plugin/              # Runtime + Builtins (depends on -api)
├─ api/                   # Re-export of helm-plugin-api
├─ runtime/               # Plugin runtime
│  ├─ plugin.rs           # HelmPlugin trait
│  ├─ registry.rs         # PluginRegistry
│  ├─ scoreboard.rs
│  ├─ callback.rs
│  ├─ info.rs
│  └─ bridge.rs
└─ builtins/              # Built-in implementations
   ├─ trace/
   ├─ memory/
   └─ debug/
```

**Benefits:**
1. ✅ Minimal helm-plugin-api for external plugins
2. ✅ Clear separation of stable API from implementation
3. ✅ Can version independently

**Drawbacks:**
1. ❌ Still two crates to maintain
2. ❌ Dependency chain: plugin-api ← plugin ← users
3. ❌ More complex to understand
4. ❌ Where does HelmPlugin trait belong?

---

### Option 3: Three-Layer Architecture

**Structure:**
```
helm-plugin-api/          # Pure trait definitions
├─ component.rs
├─ plugin.rs
└─ types.rs

helm-plugin-runtime/      # Runtime infrastructure
├─ registry.rs
├─ loader.rs
├─ scoreboard.rs
└─ dynamic.rs

helm-plugin-builtins/     # Builtin implementations
└─ (trace, memory, debug)
```

**Benefits:**
1. ✅ Maximum separation of concerns
2. ✅ Users can pick and choose dependencies

**Drawbacks:**
1. ❌ Three crates to maintain
2. ❌ Overly complex for the current scale
3. ❌ Dependency management complexity

---

## Recommended Approach: Option 1 (Single Crate)

### Migration Path

1. **Phase 1: Create new helm-plugin crate**
   - Copy all code from helm-plugin-api and helm-plugins
   - Organize into api/, runtime/, builtins/ modules
   - Set up feature flags
   - Add comprehensive tests

2. **Phase 2: Update dependent crates**
   - Update helm-cli: `helm-plugin-api` → `helm-plugin::api`
   - Update helm-engine: similar changes
   - Update helm-python: similar changes

3. **Phase 3: Deprecate old crates**
   - Mark helm-plugin-api as deprecated
   - Mark helm-plugins as deprecated
   - Point to helm-plugin in README

4. **Phase 4: Remove old crates** (after grace period)

### Usage Examples

**External plugin author:**
```rust
// Cargo.toml
[dependencies]
helm-plugin = "0.1"

// src/lib.rs
use helm_plugin::api::*;

pub struct MyPlugin;

impl HelmComponent for MyPlugin {
    fn component_type(&self) -> &'static str { "custom.my-plugin" }
    // ...
}
```

**Simulator integrator:**
```rust
// Cargo.toml
[dependencies]
helm-plugin = { version = "0.1", features = ["builtins"] }

// src/main.rs
use helm_plugin::api::ComponentRegistry;
use helm_plugin::runtime::PluginRegistry;
use helm_plugin::builtins::trace::InsnCount;

fn main() {
    let mut comp_registry = ComponentRegistry::new();
    helm_plugin::register_builtins(&mut comp_registry);
    
    let mut plugin_registry = PluginRegistry::new();
    // ...
}
```

**Dynamic plugin loader:**
```rust
// Cargo.toml
[dependencies]
helm-plugin = { version = "0.1", features = ["dynamic"] }

// src/main.rs
use helm_plugin::api::{ComponentRegistry, DynamicPluginLoader};

fn main() {
    let mut loader = DynamicPluginLoader::new();
    let mut registry = ComponentRegistry::new();
    
    unsafe {
        loader.load("./my_plugin.so", &mut registry)?;
    }
}
```

---

## API Stability Guarantees

### Stable API (SemVer guarantees)
- `api::HelmComponent` trait
- `api::HelmPlugin` trait
- `api::ComponentInfo` struct
- `api::PluginMetadata` struct
- `api::ComponentRegistry` (interface only)
- `api::PluginArgs` struct
- `api::PLUGIN_API_VERSION` constant

### Public but evolving
- `runtime::PluginRegistry` - callback infrastructure
- `runtime::Scoreboard` - helper utilities
- `runtime::*Info` structs - introspection types

### Internal (can change freely)
- Builtin plugin implementations
- Bridge adapter implementation
- Dynamic loading internals

---

## File Size Comparison

**Current:**
- helm-plugin-api: ~600 lines (4 files)
- helm-plugins: ~1800 lines (17 files)
- **Total: ~2400 lines, 2 crates**

**Proposed:**
- helm-plugin: ~2400 lines (same code, organized differently)
- **Total: ~2400 lines, 1 crate**

No increase in code size, just better organization.

---

## Open Questions

1. **Naming:** Should it be `helm-plugin` or `helm-plugins`?
   - Recommendation: `helm-plugin` (singular, like other Rust crates)

2. **HelmPlugin trait location:** api or runtime?
   - Recommendation: api (it's part of stable interface)

3. **Feature flag granularity:** Should each builtin be a separate feature?
   - Recommendation: Start simple (just `builtins`), can add later

4. **Dynamic loading:** Always available or feature-gated?
   - Recommendation: Feature-gated (requires libc, Unix-only)

5. **Backward compatibility:** Support old imports?
   - Recommendation: Add re-exports for 1-2 versions, then remove

---

## Conclusion

**Recommended: Consolidate into single `helm-plugin` crate with clear module boundaries.**

This approach:
- Reduces maintenance burden (1 crate instead of 2)
- Maintains clear API boundaries through modules
- Provides flexibility through feature flags
- Simplifies documentation and understanding
- Keeps same stability guarantees
- Minimal migration effort for users

The key insight is that the separation of concerns comes from **module organization**, not crate boundaries. For a codebase of this size (~2400 lines), a single well-organized crate is more maintainable than multiple smaller crates.
