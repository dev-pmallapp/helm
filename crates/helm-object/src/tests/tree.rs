use crate::property::*;
use crate::tree::*;
use crate::{HelmObject, HelmResult};

struct StubObject(&'static str);

impl HelmObject for StubObject {
    fn type_name(&self) -> &'static str {
        self.0
    }
    fn properties(&self) -> Vec<Property> {
        vec![]
    }
    fn get_property(&self, _: &str) -> HelmResult<PropertyValue> {
        Err(helm_core::HelmError::Config("none".into()))
    }
    fn set_property(&mut self, _: &str, _: PropertyValue) -> HelmResult<()> {
        Err(helm_core::HelmError::Config("none".into()))
    }
}

#[test]
fn tree_resolve_root() {
    let tree = ObjectTree::new(Box::new(StubObject("platform")));
    let node = tree.resolve("/");
    assert!(node.is_some());
    assert_eq!(node.unwrap().object.type_name(), "platform");
}

#[test]
fn tree_add_and_resolve_child() {
    let mut tree = ObjectTree::new(Box::new(StubObject("platform")));
    tree.root
        .add_child("cores", Box::new(StubObject("core-container")));
    let node = tree.resolve("/cores");
    assert!(node.is_some());
    assert_eq!(node.unwrap().object.type_name(), "core-container");
}

#[test]
fn tree_nested_path() {
    let mut tree = ObjectTree::new(Box::new(StubObject("platform")));
    tree.root.add_child("cores", Box::new(StubObject("cores")));
    tree.resolve_mut("/cores")
        .unwrap()
        .add_child("core0", Box::new(StubObject("core.ooo")));
    let node = tree.resolve("/cores/core0");
    assert!(node.is_some());
    assert_eq!(node.unwrap().object.type_name(), "core.ooo");
}

#[test]
fn tree_missing_path_returns_none() {
    let tree = ObjectTree::new(Box::new(StubObject("platform")));
    assert!(tree.resolve("/nonexistent/path").is_none());
}
