use crate::property::*;
use crate::registry::*;
use crate::{HelmObject, HelmResult};

struct DummyObject;

impl HelmObject for DummyObject {
    fn type_name(&self) -> &'static str {
        "test.dummy"
    }
    fn properties(&self) -> Vec<Property> {
        vec![]
    }
    fn get_property(&self, _name: &str) -> HelmResult<PropertyValue> {
        Err(helm_core::HelmError::Config("no properties".into()))
    }
    fn set_property(&mut self, _name: &str, _value: PropertyValue) -> HelmResult<()> {
        Err(helm_core::HelmError::Config("no properties".into()))
    }
}

#[test]
fn register_and_create() {
    let mut reg = TypeRegistry::new();
    reg.register(
        TypeInfo {
            name: "test.dummy",
            parent: None,
            description: "A dummy",
            interfaces: &["test"],
        },
        Box::new(|| Box::new(DummyObject)),
    );
    let obj = reg.create("test.dummy");
    assert!(obj.is_some());
    assert_eq!(obj.unwrap().type_name(), "test.dummy");
}

#[test]
fn unknown_type_returns_none() {
    let reg = TypeRegistry::new();
    assert!(reg.create("nonexistent").is_none());
}

#[test]
fn list_types_with_interface() {
    let mut reg = TypeRegistry::new();
    reg.register(
        TypeInfo {
            name: "a",
            parent: None,
            description: "",
            interfaces: &["core"],
        },
        Box::new(|| Box::new(DummyObject)),
    );
    reg.register(
        TypeInfo {
            name: "b",
            parent: None,
            description: "",
            interfaces: &["cache"],
        },
        Box::new(|| Box::new(DummyObject)),
    );
    let cores = reg.types_with_interface("core");
    assert_eq!(cores.len(), 1);
    assert!(cores.contains(&"a"));
}
