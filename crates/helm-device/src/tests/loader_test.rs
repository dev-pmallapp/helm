use crate::loader::*;

#[test]
fn new_loader_has_no_devices() {
    let loader = DynamicDeviceLoader::new();
    assert!(loader.available_devices().is_empty());
}

#[test]
fn register_and_list_device() {
    let mut loader = DynamicDeviceLoader::new();
    loader.register("test-dev", |_cfg| None);
    assert!(loader.has_device("test-dev"));
    assert!(loader.available_devices().contains(&"test-dev"));
}

#[test]
fn has_device_false_for_unknown() {
    let loader = DynamicDeviceLoader::new();
    assert!(!loader.has_device("nonexistent"));
}

#[test]
fn create_device_unknown_type_fails() {
    let loader = DynamicDeviceLoader::new();
    let result = loader.create_device("nonexistent", &serde_json::Value::Null);
    assert!(result.is_err());
}

#[test]
fn create_device_factory_returns_none_fails() {
    let mut loader = DynamicDeviceLoader::new();
    loader.register("null-dev", |_cfg| None);
    let result = loader.create_device("null-dev", &serde_json::Value::Null);
    assert!(result.is_err());
}

#[test]
fn register_arm_builtins_populates_devices() {
    let mut loader = DynamicDeviceLoader::new();
    loader.register_arm_builtins();
    assert!(loader.has_device("pl011"));
    assert!(loader.has_device("sp804"));
    assert!(loader.has_device("gic"));
    assert!(loader.has_device("pl031"));
    assert!(loader.has_device("pl061"));
}

#[test]
fn create_builtin_pl011_succeeds() {
    let mut loader = DynamicDeviceLoader::new();
    loader.register_arm_builtins();
    let dev = loader.create_device("pl011", &serde_json::Value::Null);
    assert!(dev.is_ok());
}

#[test]
fn create_builtin_gic_with_config() {
    let mut loader = DynamicDeviceLoader::new();
    loader.register_arm_builtins();
    let cfg = serde_json::json!({"num_irqs": 128});
    let dev = loader.create_device("gic", &cfg);
    assert!(dev.is_ok());
}

#[test]
fn default_search_paths_not_empty() {
    let loader = DynamicDeviceLoader::new();
    assert!(!loader.search_paths.is_empty());
}

#[test]
fn device_load_error_display() {
    let err = DeviceLoadError::LibraryOpen("test.so".into());
    let msg = format!("{err}");
    assert!(msg.contains("test.so"));

    let err = DeviceLoadError::SymbolNotFound("sym".into());
    assert!(format!("{err}").contains("sym"));

    let err = DeviceLoadError::VersionMismatch {
        name: "dev".into(), expected: 1, found: 2,
    };
    assert!(format!("{err}").contains("mismatch"));

    let err = DeviceLoadError::NullVTable;
    assert!(!format!("{err}").is_empty());
}

#[test]
fn device_api_version_is_one() {
    assert_eq!(DEVICE_API_VERSION, 1);
}
