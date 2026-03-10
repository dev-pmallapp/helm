//! Plugin discovery and registration (no dynamic loading yet —
//! that requires `libloading` which is a future addition).

use super::component::ComponentInfo;
use std::collections::HashMap;

/// Registry of component types contributed by plugins (or built-in).
pub struct ComponentRegistry {
    components: HashMap<&'static str, ComponentInfo>,
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self {
            components: HashMap::new(),
        }
    }

    /// Register a component type.
    pub fn register(&mut self, info: ComponentInfo) {
        self.components.insert(info.type_name, info);
    }

    /// Instantiate a component by type name.
    pub fn create(&self, type_name: &str) -> Option<Box<dyn super::component::HelmComponent>> {
        self.components.get(type_name).map(|info| (info.factory)())
    }

    /// List all registered type names.
    pub fn list(&self) -> Vec<&'static str> {
        self.components.keys().copied().collect()
    }

    /// List types implementing a specific interface.
    pub fn types_with_interface(&self, interface: &str) -> Vec<&'static str> {
        self.components
            .values()
            .filter(|info| info.interfaces.contains(&interface))
            .map(|info| info.type_name)
            .collect()
    }
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        Self::new()
    }
}
