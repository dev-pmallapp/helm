//! Dynamic device loader — loads device implementations from shared libraries.
//!
//! Extends the plugin loading pattern from `helm-plugin` to device models.
//! Device .so files export a C-ABI entry point. The loader manages search
//! paths and a factory registry.
//!
//! # Installation directories
//!
//! ```text
//! ~/.helm/devices/           # user-installed
//! /usr/lib/helm/devices/     # system-wide
//! ./devices/                 # project-local
//! ```

use crate::device::Device;
use std::collections::HashMap;

/// Current device plugin API version.
pub const DEVICE_API_VERSION: u32 = 1;

/// Name of the C symbol device libraries must export.
pub const DEVICE_ENTRY_SYMBOL: &str = "helm_device_entry";

/// Error during device library loading.
#[derive(Debug)]
pub enum DeviceLoadError {
    LibraryOpen(String),
    SymbolNotFound(String),
    VersionMismatch { name: String, expected: u32, found: u32 },
    NullVTable,
    CreateFailed(String),
}

impl std::fmt::Display for DeviceLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LibraryOpen(msg) => write!(f, "failed to open device library: {msg}"),
            Self::SymbolNotFound(sym) => write!(f, "symbol not found: {sym}"),
            Self::VersionMismatch { name, expected, found } =>
                write!(f, "device '{name}' API version mismatch: expected {expected}, found {found}"),
            Self::NullVTable => write!(f, "device entry point returned null"),
            Self::CreateFailed(msg) => write!(f, "device creation failed: {msg}"),
        }
    }
}

impl std::error::Error for DeviceLoadError {}

/// A Rust-native device factory function.
type DeviceFactoryFn = Box<dyn Fn(&serde_json::Value) -> Option<Box<dyn Device>> + Send + Sync>;

/// A registered device factory.
struct DeviceFactory {
    name: String,
    version: String,
    create: DeviceFactoryFn,
}

/// Loads device shared libraries and provides a factory registry.
///
/// Devices can be registered as builtins or loaded from .so files.
pub struct DynamicDeviceLoader {
    factories: HashMap<String, DeviceFactory>,
    /// Search paths for device libraries.
    pub search_paths: Vec<String>,
}

impl DynamicDeviceLoader {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
            search_paths: default_search_paths(),
        }
    }

    /// Register a built-in device factory.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        factory: impl Fn(&serde_json::Value) -> Option<Box<dyn Device>> + Send + Sync + 'static,
    ) {
        let n = name.into();
        self.factories.insert(n.clone(), DeviceFactory {
            name: n,
            version: "builtin".to_string(),
            create: Box::new(factory),
        });
    }

    /// List all registered device type names.
    pub fn available_devices(&self) -> Vec<&str> {
        self.factories.keys().map(|s| s.as_str()).collect()
    }

    /// Check if a device type is registered.
    pub fn has_device(&self, name: &str) -> bool {
        self.factories.contains_key(name)
    }

    /// Create a device instance from a registered factory.
    pub fn create_device(
        &self,
        type_name: &str,
        config: &serde_json::Value,
    ) -> Result<Box<dyn Device>, DeviceLoadError> {
        let factory = self.factories.get(type_name)
            .ok_or_else(|| DeviceLoadError::CreateFailed(
                format!("unknown device type: {type_name}")))?;

        (factory.create)(config)
            .ok_or_else(|| DeviceLoadError::CreateFailed(
                format!("factory for '{}' returned None", type_name)))
    }

    /// Register all built-in ARM device factories.
    pub fn register_arm_builtins(&mut self) {
        use crate::arm::*;
        use crate::backend::NullCharBackend;

        self.register("pl011", |_cfg| {
            Some(Box::new(pl011::Pl011::new("pl011", Box::new(NullCharBackend))))
        });
        self.register("sp804", |_cfg| {
            Some(Box::new(sp804::Sp804::new("sp804")))
        });
        self.register("pl031", |_cfg| {
            Some(Box::new(pl031::Pl031::new("pl031")))
        });
        self.register("sp805", |_cfg| {
            Some(Box::new(sp805::Sp805::new("sp805")))
        });
        self.register("pl061", |_cfg| {
            Some(Box::new(pl061::Pl061::new("pl061")))
        });
        self.register("gic", |cfg| {
            let num_irqs = cfg.get("num_irqs")
                .and_then(|v| v.as_u64())
                .unwrap_or(96) as u32;
            Some(Box::new(gic::Gic::new("gic", num_irqs)))
        });
        self.register("realview-sysregs", |_cfg| {
            Some(Box::new(sysregs::RealViewSysRegs::realview_pb_a8()))
        });
        self.register("bcm-sys-timer", |_cfg| {
            Some(Box::new(bcm_sys_timer::BcmSysTimer::new("sys-timer")))
        });
        self.register("bcm-mailbox", |_cfg| {
            Some(Box::new(bcm_mailbox::BcmMailbox::rpi3()))
        });
        self.register("bcm-gpio", |_cfg| {
            Some(Box::new(bcm_gpio::BcmGpio::new("gpio")))
        });
        self.register("bcm-mini-uart", |_cfg| {
            Some(Box::new(bcm_mini_uart::BcmMiniUart::new("mini-uart", Box::new(NullCharBackend))))
        });
    }
}

impl Default for DynamicDeviceLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// Default search paths for device libraries.
fn default_search_paths() -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(format!("{}/.helm/devices", home.to_string_lossy()));
    }
    paths.push("./devices".to_string());
    paths.push("/usr/lib/helm/devices".to_string());
    paths.push("/usr/local/lib/helm/devices".to_string());
    paths
}
