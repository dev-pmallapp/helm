use crate::component::{ComponentInfo, HelmComponent};
use crate::loader::ComponentRegistry;
use helm_core::HelmResult;

struct DummyComp;
impl HelmComponent for DummyComp {
    fn component_type(&self) -> &'static str {
        "test.dummy"
    }
    fn interfaces(&self) -> &[&str] {
        &["test-iface"]
    }
    fn reset(&mut self) -> HelmResult<()> {
        Ok(())
    }
}

#[test]
fn register_and_create() {
    let mut reg = ComponentRegistry::new();
    reg.register(ComponentInfo {
        type_name: "test.dummy",
        description: "A test component",
        interfaces: &["test-iface"],
        factory: Box::new(|| Box::new(DummyComp)),
    });
    let comp = reg.create("test.dummy");
    assert!(comp.is_some());
    assert_eq!(comp.unwrap().component_type(), "test.dummy");
}

#[test]
fn missing_type_returns_none() {
    let reg = ComponentRegistry::new();
    assert!(reg.create("nope").is_none());
}

#[test]
fn filter_by_interface() {
    let mut reg = ComponentRegistry::new();
    reg.register(ComponentInfo {
        type_name: "a",
        description: "",
        interfaces: &["mmio"],
        factory: Box::new(|| Box::new(DummyComp)),
    });
    reg.register(ComponentInfo {
        type_name: "b",
        description: "",
        interfaces: &["timer"],
        factory: Box::new(|| Box::new(DummyComp)),
    });
    let mmio = reg.types_with_interface("mmio");
    assert_eq!(mmio, vec!["a"]);
}
