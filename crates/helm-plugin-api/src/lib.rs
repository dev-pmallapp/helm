//! # helm-plugin-api
//!
//! The public, stable API that plugin authors depend on.
//!
//! To build a HELM plugin (e.g. a custom device, branch predictor, or
//! cache model), create a Rust crate that depends on this API crate and
//! implements [`HelmComponent`].  See [`component`] for the full trait.
//!
//! # Quick Start
//!
//! ```ignore
//! use helm_plugin_api::*;
//!
//! pub struct MyUart { /* ... */ }
//!
//! impl HelmComponent for MyUart {
//!     fn component_type(&self) -> &'static str { "device.my-uart" }
//!     fn interfaces(&self) -> &[&str] { &["memory-mapped", "interrupt-source"] }
//!     fn reset(&mut self) -> HelmResult<()> { Ok(()) }
//! }
//! ```

pub mod component;
pub mod loader;

#[cfg(unix)]
pub mod dynamic;

// Re-export the key types plugin authors need.
pub use helm_core::types::{Addr, Cycle, Word};
pub use helm_core::{HelmError, HelmResult};
pub use helm_device::{DeviceAccess, MemoryMappedDevice};
pub use helm_object::{HelmObject, Property, PropertyType, PropertyValue};
pub use helm_timing::TimingModel;

pub use component::{ComponentInfo, HelmComponent};

/// Plugin ABI version.  Increment on breaking changes to this crate's
/// public API so the loader can reject incompatible plugins.
pub const PLUGIN_API_VERSION: u32 = 1;

/// Metadata embedded in every plugin shared library.
#[repr(C)]
pub struct PluginMetadata {
    pub api_version: u32,
    pub name: &'static str,
    pub version: &'static str,
    pub description: &'static str,
    pub author: &'static str,
}

#[cfg(test)]
mod tests;
