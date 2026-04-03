//! Object model class descriptor for SimObject versioning.
//!
//! Each SimObject subclass registers a `ClassDescriptor` with a version
//! number. The version is checked at `ClassRegistry::global()` init time
//! and stored in checkpoint headers for migration detection.

/// Describes a SimObject class for the object model registry.
#[derive(Debug, Clone)]
pub struct ClassDescriptor {
    /// Unique type name for this class (e.g., "uart16550", "gicv2").
    pub type_name: &'static str,

    /// Version of this class implementation.
    ///
    /// Bump when the checkpoint-visible state (registered attributes)
    /// changes in a way that requires migration. Minor additions with
    /// `#[serde(default)]` do not require a bump.
    pub version: u32,

    /// One-line human-readable description.
    pub description: &'static str,

    /// Parent class name, if any (for hierarchy tracking).
    pub parent: Option<&'static str>,
}

impl ClassDescriptor {
    /// Create a new class descriptor.
    pub const fn new(type_name: &'static str, version: u32, description: &'static str) -> Self {
        Self {
            type_name,
            version,
            description,
            parent: None,
        }
    }

}
