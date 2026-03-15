//! Named attribute registry — exposes all arch state for checkpointing and introspection.
//!
//! Every persistent field in a SimObject must be registered here. This is the
//! "no dark state" invariant from the design (SIMICS-inspired).

use std::collections::HashMap;

/// A typed attribute value.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    U64(u64),
    I64(i64),
    Bool(bool),
    Bytes(Vec<u8>),
    Str(String),
}

impl From<u64> for AttrValue {
    fn from(v: u64) -> Self { Self::U64(v) }
}
impl From<i64> for AttrValue {
    fn from(v: i64) -> Self { Self::I64(v) }
}
impl From<bool> for AttrValue {
    fn from(v: bool) -> Self { Self::Bool(v) }
}
impl From<Vec<u8>> for AttrValue {
    fn from(v: Vec<u8>) -> Self { Self::Bytes(v) }
}
impl From<String> for AttrValue {
    fn from(v: String) -> Self { Self::Str(v) }
}

/// Registry of named attributes for a single SimObject.
///
/// `ArchState::register_attrs()` populates this with all architectural registers.
/// `CheckpointManager` serialises/deserialises via this registry.
#[derive(Default)]
pub struct AttrRegistry {
    attrs: HashMap<String, AttrValue>,
}

impl AttrRegistry {
    pub fn new() -> Self { Self::default() }

    /// Register or overwrite an attribute.
    pub fn set(&mut self, name: impl Into<String>, val: impl Into<AttrValue>) {
        self.attrs.insert(name.into(), val.into());
    }

    /// Read an attribute by name.
    pub fn get(&self, name: &str) -> Option<&AttrValue> { self.attrs.get(name) }

    /// Iterate all attributes.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &AttrValue)> {
        self.attrs.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Number of registered attributes.
    pub fn len(&self) -> usize { self.attrs.len() }

    pub fn is_empty(&self) -> bool { self.attrs.is_empty() }
}
