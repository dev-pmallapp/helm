//! Runtime type registry — maps type names to constructors.

use super::HelmObject;
use std::collections::HashMap;

/// Metadata for a registered component type.
pub struct TypeInfo {
    pub name: &'static str,
    pub parent: Option<&'static str>,
    pub description: &'static str,
    /// Interfaces this type implements (e.g. `["core", "timing-model"]`).
    pub interfaces: &'static [&'static str],
}

/// A boxed function that creates a new default instance of a component.
pub type ComponentFactory = Box<dyn Fn() -> Box<dyn HelmObject> + Send + Sync>;

struct RegisteredType {
    info: TypeInfo,
    factory: ComponentFactory,
}

/// Central registry that maps type names to factories.
pub struct TypeRegistry {
    types: HashMap<&'static str, RegisteredType>,
}

impl TypeRegistry {
    pub fn new() -> Self {
        Self {
            types: HashMap::new(),
        }
    }

    /// Register a component type with its factory.
    pub fn register(&mut self, info: TypeInfo, factory: ComponentFactory) {
        self.types
            .insert(info.name, RegisteredType { info, factory });
    }

    /// Instantiate a component by type name.
    pub fn create(&self, type_name: &str) -> Option<Box<dyn HelmObject>> {
        self.types.get(type_name).map(|rt| (rt.factory)())
    }

    /// List all registered type names.
    pub fn list_types(&self) -> Vec<&'static str> {
        self.types.keys().copied().collect()
    }

    /// Get info for a type.
    pub fn type_info(&self, name: &str) -> Option<&TypeInfo> {
        self.types.get(name).map(|rt| &rt.info)
    }

    /// List types that implement a given interface.
    pub fn types_with_interface(&self, interface: &str) -> Vec<&'static str> {
        self.types
            .values()
            .filter(|rt| rt.info.interfaces.contains(&interface))
            .map(|rt| rt.info.name)
            .collect()
    }
}

impl Default for TypeRegistry {
    fn default() -> Self {
        Self::new()
    }
}
