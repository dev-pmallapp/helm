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

#[test]
fn tree_resolve_empty_path_gives_root() {
    let tree = ObjectTree::new(Box::new(StubObject("platform")));
    let node = tree.resolve("");
    assert!(node.is_some());
    assert_eq!(node.unwrap().object.type_name(), "platform");
}

#[test]
fn tree_resolve_mut_allows_child_addition() {
    let mut tree = ObjectTree::new(Box::new(StubObject("platform")));
    tree.root.add_child("mem", Box::new(StubObject("memory")));
    let mem_node = tree.resolve_mut("/mem").unwrap();
    mem_node.add_child("l1", Box::new(StubObject("cache.l1")));
    let l1 = tree.resolve("/mem/l1");
    assert!(l1.is_some());
    assert_eq!(l1.unwrap().object.type_name(), "cache.l1");
}

#[test]
fn child_names_empty_for_leaf_node() {
    let tree = ObjectTree::new(Box::new(StubObject("platform")));
    let names = tree.root.child_names();
    assert!(names.is_empty());
}

#[test]
fn child_names_lists_all_direct_children() {
    let mut tree = ObjectTree::new(Box::new(StubObject("platform")));
    tree.root.add_child("cores", Box::new(StubObject("cores")));
    tree.root.add_child("memory", Box::new(StubObject("memory")));
    let mut names = tree.root.child_names();
    names.sort();
    assert_eq!(names, vec!["cores", "memory"]);
}

#[test]
fn object_node_name_field() {
    let node = ObjectNode::new("my_node", Box::new(StubObject("t")));
    assert_eq!(node.name, "my_node");
}
