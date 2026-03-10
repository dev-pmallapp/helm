//! # helm-object
//!
//! HELM Object Model (HOM), inspired by QEMU's QOM.
//! Provides a trait-based type system for simulation components with:
//! - Typed, introspectable properties
//! - A hierarchical composition tree
//! - Runtime type registration and lookup

pub mod property;
pub mod registry;
pub mod tree;

pub use property::{Property, PropertyType, PropertyValue};
pub use registry::{ComponentFactory, TypeInfo, TypeRegistry};
pub use tree::{ObjectNode, ObjectTree};

use helm_core::HelmResult;
use std::any::Any;

/// Core trait that every HELM simulation component implements.
///
/// This is the foundation of the object model.  Components expose their
/// configuration as [`Property`] values, enabling introspection, JSON
/// serialisation, and runtime modification from Python or HMP.
pub trait HelmObject: Any + Send + Sync {
    /// Fully-qualified type name (e.g. `"core.ooo"`, `"cache.l1d"`).
    fn type_name(&self) -> &'static str;

    /// Human-readable description.
    fn description(&self) -> &str {
        ""
    }

    /// List all properties exposed by this object.
    fn properties(&self) -> Vec<Property>;

    /// Read a property value by name.
    fn get_property(&self, name: &str) -> HelmResult<PropertyValue>;

    /// Write a property value by name (may fail for read-only props).
    fn set_property(&mut self, name: &str, value: PropertyValue) -> HelmResult<()>;

    /// Called once after all properties have been set, before simulation starts.
    fn realize(&mut self) -> HelmResult<()> {
        Ok(())
    }

    /// Tear down (inverse of realize).
    fn unrealize(&mut self) -> HelmResult<()> {
        Ok(())
    }

    /// Reset to initial state.
    fn reset(&mut self) -> HelmResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
