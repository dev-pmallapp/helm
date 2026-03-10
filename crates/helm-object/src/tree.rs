//! Hierarchical composition tree for simulation objects.

use super::HelmObject;
use std::collections::HashMap;

/// A node in the composition tree, holding an object and its children.
pub struct ObjectNode {
    pub name: String,
    pub object: Box<dyn HelmObject>,
    pub children: HashMap<String, ObjectNode>,
}

impl ObjectNode {
    pub fn new(name: impl Into<String>, object: Box<dyn HelmObject>) -> Self {
        Self {
            name: name.into(),
            object,
            children: HashMap::new(),
        }
    }

    /// Add a child node.
    pub fn add_child(&mut self, name: impl Into<String>, object: Box<dyn HelmObject>) {
        let name = name.into();
        self.children
            .insert(name.clone(), ObjectNode::new(name, object));
    }

    /// Look up a descendant by slash-separated path (e.g. `"cores/core0/rob"`).
    pub fn get(&self, path: &str) -> Option<&ObjectNode> {
        let mut current = self;
        for segment in path.split('/').filter(|s| !s.is_empty()) {
            current = current.children.get(segment)?;
        }
        Some(current)
    }

    /// Mutable version of `get`.
    pub fn get_mut(&mut self, path: &str) -> Option<&mut ObjectNode> {
        let mut current = self;
        for segment in path.split('/').filter(|s| !s.is_empty()) {
            current = current.children.get_mut(segment)?;
        }
        Some(current)
    }

    /// List child names.
    pub fn child_names(&self) -> Vec<&str> {
        self.children.keys().map(String::as_str).collect()
    }
}

/// The root of the composition tree, representing the entire platform.
pub struct ObjectTree {
    pub root: ObjectNode,
}

impl ObjectTree {
    pub fn new(root_object: Box<dyn HelmObject>) -> Self {
        Self {
            root: ObjectNode::new("platform", root_object),
        }
    }

    /// Resolve an absolute path (e.g. `"/cores/core0"`).
    pub fn resolve(&self, path: &str) -> Option<&ObjectNode> {
        let path = path.strip_prefix('/').unwrap_or(path);
        if path.is_empty() {
            Some(&self.root)
        } else {
            self.root.get(path)
        }
    }

    pub fn resolve_mut(&mut self, path: &str) -> Option<&mut ObjectNode> {
        let path = path.strip_prefix('/').unwrap_or(path);
        if path.is_empty() {
            Some(&mut self.root)
        } else {
            self.root.get_mut(path)
        }
    }
}
