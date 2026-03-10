//! Plugin metadata and versioning.

/// Plugin ABI version. Increment on breaking changes to this crate's
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
