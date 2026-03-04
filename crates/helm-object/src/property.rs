//! Typed property descriptors and values.

use serde::{Deserialize, Serialize};

/// Metadata for a single property exposed by a [`HelmObject`](super::HelmObject).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Property {
    pub name: String,
    pub type_info: PropertyType,
    pub description: String,
    pub read_only: bool,
}

/// The type of a property value, with optional constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PropertyType {
    Bool,
    Int {
        min: Option<i64>,
        max: Option<i64>,
    },
    UInt {
        min: Option<u64>,
        max: Option<u64>,
    },
    Float {
        min: Option<f64>,
        max: Option<f64>,
    },
    Str,
    Enum {
        variants: Vec<String>,
    },
    /// Reference to a child object by type name.
    Object {
        type_name: String,
    },
}

/// A concrete property value at runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PropertyValue {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(String),
}

impl PropertyValue {
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::UInt(v) => Some(*v),
            Self::Int(v) if *v >= 0 => Some(*v as u64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Float(v) => Some(*v),
            Self::UInt(v) => Some(*v as f64),
            Self::Int(v) => Some(*v as f64),
            _ => None,
        }
    }
}
