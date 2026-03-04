use crate::component::HelmComponent;
use helm_core::HelmResult;

struct TestDevice;

impl HelmComponent for TestDevice {
    fn component_type(&self) -> &'static str {
        "device.test"
    }
    fn interfaces(&self) -> &[&str] {
        &["memory-mapped", "test"]
    }
    fn reset(&mut self) -> HelmResult<()> {
        Ok(())
    }
}

#[test]
fn component_has_type_and_interfaces() {
    let d = TestDevice;
    assert_eq!(d.component_type(), "device.test");
    assert!(d.interfaces().contains(&"memory-mapped"));
}

#[test]
fn default_lifecycle_methods_succeed() {
    let mut d = TestDevice;
    assert!(d.realize().is_ok());
    assert!(d.reset().is_ok());
    assert!(d.tick(100).is_ok());
}
