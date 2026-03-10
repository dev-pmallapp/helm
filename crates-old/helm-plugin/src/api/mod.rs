pub mod component;
pub mod loader;
pub mod metadata;
pub mod plugin;

#[cfg(all(unix, feature = "dynamic"))]
pub mod dynamic;

// Re-export key types at module root for convenience
pub use component::{ComponentInfo, HelmComponent};
pub use loader::ComponentRegistry;
pub use metadata::{PluginMetadata, PLUGIN_API_VERSION};
pub use plugin::{HelmPlugin, PluginArgs};

#[cfg(all(unix, feature = "dynamic"))]
pub use dynamic::{
    DynLoadError, DynamicPluginLoader, HelmPluginVTable, LoadedPluginInfo, ENTRY_SYMBOL,
};

// Re-export common types from helm-core
pub use helm_core::types::{Addr, Cycle, Word};
pub use helm_core::{HelmError, HelmResult};
pub use helm_device::{DeviceAccess, MemoryMappedDevice};
pub use helm_object::{HelmObject, Property, PropertyType, PropertyValue};
pub use helm_timing::TimingModel;
