//! Device registry and descriptor types.
//!
//! The [`DeviceRegistry`] enables runtime device-type lookup and instantiation
//! by name. It holds [`DeviceDescriptor`] records -- one per device type --
//! each carrying a factory function, parameter schema, and metadata.
//!
//! # Client groups
//!
//! - **Python configuration layer**: `helm_ng.Uart16550(clock_hz=1_843_200)` is
//!   backed by a descriptor in the registry. The factory function instantiates
//!   the Rust device struct from Python-supplied parameters.
//! - **Plugin system**: external `.so` files export a C-ABI function
//!   `helm_device_register` that registers one or more descriptors at load time.
//!
//! # Error handling
//!
//! All fallible operations in this module return [`DldError`] (Device-Loader
//! Error), which covers plugin loading, name conflicts, parameter validation,
//! and device construction failures.

use std::collections::HashMap;
use std::path::Path;

use super::device::Device;
use super::params::{DeviceParams, ParamSchema};

// ---- HostCapability -----------------------------------------------------

/// A capability the device requires from the host environment.
///
/// Checked by [`DeviceRegistry::create`] before calling the factory function.
/// If the host cannot satisfy a required capability,
/// [`DldError::CapabilityMissing`] is returned and no device is created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCapability {
    /// Device requires a KVM fd for hardware-accelerated virtualization.
    KvmAcceleration,
    /// Device requires a vhost-net fd for kernel-bypassed networking.
    VhostNet,
    /// Device requires a `/dev/vfio/N` fd for IOMMU pass-through.
    VfioPassthrough,
    /// Device requires USB host controller access.
    UsbHost,
    /// Custom capability name (for plugins that define their own requirements).
    Custom(&'static str),
}

// ---- DldError -----------------------------------------------------------

/// Errors from plugin loading, device creation, or parameter validation.
///
/// Named `DldError` (Device-Loader Error) to distinguish from general
/// simulation errors in `helm-core`.
#[derive(Debug, thiserror::Error)]
pub enum DldError {
    /// `dlopen()` failed -- usually wrong path or missing shared library dependencies.
    #[error("dlopen failed: {0}")]
    DlopenFailed(String),

    /// Plugin does not export `HELM_DEVICES_ABI_VERSION` symbol.
    #[error("plugin missing HELM_DEVICES_ABI_VERSION symbol -- not a valid helm-devices plugin")]
    MissingAbiSymbol,

    /// Plugin's ABI version does not match the host's version.
    #[error(
        "ABI version mismatch: host={expected}, plugin={found} -- recompile plugin against helm-devices {expected}"
    )]
    AbiVersionMismatch {
        /// Host-side ABI version.
        expected: u32,
        /// Plugin-side ABI version.
        found: u32,
    },

    /// Plugin does not export `helm_device_register` symbol.
    #[error("plugin missing helm_device_register symbol -- not a valid helm-devices plugin")]
    MissingRegisterSymbol,

    /// A device with the same name is already registered.
    #[error("device name conflict: '{0}' is already registered")]
    NameConflict(String),

    /// The Python class name extracted from the descriptor conflicts with an
    /// existing name in the `helm_ng` module namespace.
    #[error("Python class name conflict: '{0}' already exists in helm_ng namespace")]
    PythonNameConflict(String),

    /// Requested device type name is not in the registry.
    #[error("unknown device type: '{0}'")]
    UnknownDevice(String),

    /// A required parameter was not supplied and has no default.
    #[error("missing required parameter: '{0}'")]
    MissingParam(&'static str),

    /// A parameter was supplied with an incompatible type.
    #[error("wrong type for parameter '{0}'")]
    WrongParamType(String),

    /// A parameter value is out of range or otherwise invalid.
    #[error("invalid parameter value: {0}")]
    InvalidParamValue(String),

    /// Device construction failed after parameter validation.
    #[error("device construction failed: {0}")]
    DeviceCreate(String),

    /// Device requires a host capability the current host cannot satisfy.
    #[error("missing host capability: {0:?}")]
    CapabilityMissing(HostCapability),

    /// Feature not yet implemented (Phase 1+ placeholder).
    #[error("not implemented")]
    NotImplemented,
}

// ---- DeviceDescriptor ---------------------------------------------------

/// A complete runtime record for one device type.
///
/// Registered once per device type, either via built-in registration or
/// via a plugin's `helm_device_register` call (external `.so` devices).
pub struct DeviceDescriptor {
    /// Unique device type name -- used as the key in [`DeviceRegistry`].
    ///
    /// Convention: `snake_case`, e.g. `"uart16550"`, `"plic_riscv"`, `"virtio_disk"`.
    pub name: &'static str,

    /// Semantic version of this device implementation.
    ///
    /// Used for diagnostic output; not for ABI compatibility (use
    /// `HELM_DEVICES_ABI_VERSION` for that).
    pub version: &'static str,

    /// One-line human-readable description.
    pub description: &'static str,

    /// Factory function: given validated [`DeviceParams`], construct and return
    /// the device.
    ///
    /// Must not panic on valid params (schema-validated before this call).
    /// May return `Err(DldError::DeviceCreate)` if OS resource allocation fails.
    pub factory: fn(DeviceParams) -> Result<Box<dyn Device>, DldError>,

    /// Return the parameter schema for this device type.
    ///
    /// Called once at registration time to auto-generate the Python class,
    /// and on demand for validation. Must return the same schema every call.
    ///
    /// `ParamSchema` is the authoritative source of truth for all Python
    /// parameter definitions.
    pub param_schema: fn() -> ParamSchema,

    /// Optional extra Python class body to append after the auto-generated class.
    ///
    /// Use only for devices that need Python-side methods or properties beyond
    /// the auto-generated parameter attributes. `None` for the vast majority
    /// of devices. Must NOT re-declare any parameters already in `param_schema`.
    pub python_class_extra: Option<&'static str>,

    /// Alternative names by which this device type can be looked up.
    ///
    /// All aliases resolve to the same [`DeviceDescriptor`]. Useful for
    /// backward compatibility when renaming a device type.
    ///
    /// Convention: list deprecated names, not abbreviations.
    /// Example: `&["uart_16550", "uart16550_legacy"]`
    pub aliases: &'static [&'static str],

    /// Host capabilities this device requires to function correctly.
    ///
    /// Checked by [`DeviceRegistry::create`]. If the host cannot satisfy a
    /// required capability, `DldError::CapabilityMissing` is returned and no
    /// device is created. Built-in devices with no special requirements use `&[]`.
    pub required_capabilities: &'static [HostCapability],
}

impl std::fmt::Debug for DeviceDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceDescriptor")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("description", &self.description)
            .field("aliases", &self.aliases)
            .field("required_capabilities", &self.required_capabilities)
            .finish_non_exhaustive()
    }
}

// ---- DeviceRegistry -----------------------------------------------------

/// The current ABI version. Bump this integer whenever the [`DeviceRegistry`]
/// or [`DeviceDescriptor`] types have a breaking change.
pub const HELM_DEVICES_ABI_VERSION: u32 = 1;

/// Runtime registry of device type descriptors.
///
/// The `helm_ng` Python module holds one `DeviceRegistry`. Built-in devices
/// are registered via [`DeviceRegistry::register`]. Plugin devices register
/// via `helm_device_register()` called at `.so` load time.
pub struct DeviceRegistry {
    /// Maps device type name to descriptor.
    devices: HashMap<&'static str, DeviceDescriptor>,
    /// Loaded plugin library handles (kept alive to prevent `dlclose`).
    ///
    /// Placeholder: will hold `libloading::Library` values once the
    /// plugin-loading system is implemented in Phase 1.
    _libs: Vec<()>,
}

impl DeviceRegistry {
    /// Create an empty registry.
    ///
    /// No built-in devices are collected automatically; call [`register`]
    /// to add device types.
    ///
    /// [`register`]: DeviceRegistry::register
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
            _libs: Vec::new(),
        }
    }

    /// Register a device type.
    ///
    /// Called by plugins via `helm_device_register`, or by the host binary
    /// for built-in devices.
    ///
    /// Returns `Err(DldError::NameConflict)` if a device with the same name
    /// is already registered.
    pub fn register(&mut self, desc: DeviceDescriptor) -> Result<(), DldError> {
        if self.devices.contains_key(desc.name) {
            return Err(DldError::NameConflict(desc.name.to_owned()));
        }
        self.devices.insert(desc.name, desc);
        Ok(())
    }

    /// Load a device plugin from a shared library (`.so` / `.dylib`).
    ///
    /// **Not yet implemented** -- returns [`DldError::NotImplemented`].
    /// Full plugin loading (dlopen, ABI check, `helm_device_register` call)
    /// will be implemented in Phase 1.
    pub fn load_dld(&mut self, _path: &Path) -> Result<(), DldError> {
        Err(DldError::NotImplemented)
    }

    /// Instantiate a device by type name with the given parameters.
    ///
    /// Validates params against the device's schema (applies defaults for
    /// missing optional params). Returns `Err` if the name is not registered,
    /// params are invalid, or construction fails.
    pub fn create(
        &self,
        name: &str,
        params: DeviceParams,
    ) -> Result<Box<dyn Device>, DldError> {
        let desc = self
            .devices
            .get(name)
            .ok_or_else(|| DldError::UnknownDevice(name.to_owned()))?;
        let schema = (desc.param_schema)();
        let validated = schema.validate(params)?;
        (desc.factory)(validated)
    }

    /// Return the parameter schema for a device type, or `None` if not registered.
    pub fn param_schema(&self, name: &str) -> Option<ParamSchema> {
        self.devices.get(name).map(|d| (d.param_schema)())
    }

    /// List all registered device descriptors.
    pub fn list(&self) -> Vec<&DeviceDescriptor> {
        self.devices.values().collect()
    }
}

impl Default for DeviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for DeviceRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceRegistry")
            .field("device_count", &self.devices.len())
            .field("device_names", &self.devices.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::params::{DeviceParams, ParamSchema, ParamValue};

    /// Minimal device for testing the registry.
    struct DummyDevice {
        value: i64,
    }

    impl Device for DummyDevice {
        fn read(&mut self, _offset: u64, _size: usize) -> u64 {
            self.value as u64
        }
        fn write(&mut self, _offset: u64, _size: usize, _val: u64) {}
        fn region_size(&self) -> u64 {
            16
        }
    }

    fn dummy_descriptor() -> DeviceDescriptor {
        DeviceDescriptor {
            name: "dummy",
            version: "0.1.0",
            description: "A dummy test device",
            factory: |params| {
                let value = params.get_int("value")?;
                Ok(Box::new(DummyDevice { value }))
            },
            param_schema: || {
                ParamSchema::new()
                    .int("value", "the value to store")
            },
            python_class_extra: None,
            aliases: &[],
            required_capabilities: &[],
        }
    }

    #[test]
    fn register_and_create() {
        let mut reg = DeviceRegistry::new();
        reg.register(dummy_descriptor()).unwrap();

        let mut params = DeviceParams::new();
        params.insert("value", ParamValue::Int(99));
        let mut dev = reg.create("dummy", params).unwrap();
        assert_eq!(dev.read(0, 4), 99);
    }

    #[test]
    fn register_name_conflict() {
        let mut reg = DeviceRegistry::new();
        reg.register(dummy_descriptor()).unwrap();
        let result = reg.register(dummy_descriptor());
        assert!(matches!(result, Err(DldError::NameConflict(_))));
    }

    #[test]
    fn create_unknown_device() {
        let reg = DeviceRegistry::new();
        let params = DeviceParams::new();
        let result = reg.create("nonexistent", params);
        assert!(matches!(result, Err(DldError::UnknownDevice(_))));
    }

    #[test]
    fn create_missing_required_param() {
        let mut reg = DeviceRegistry::new();
        reg.register(dummy_descriptor()).unwrap();

        // "value" is required but not supplied
        let params = DeviceParams::new();
        let result = reg.create("dummy", params);
        assert!(matches!(result, Err(DldError::MissingParam(_))));
    }

    #[test]
    fn param_schema_lookup() {
        let mut reg = DeviceRegistry::new();
        reg.register(dummy_descriptor()).unwrap();

        let schema = reg.param_schema("dummy").unwrap();
        assert_eq!(schema.fields().len(), 1);
        assert_eq!(schema.fields()[0].name, "value");
    }

    #[test]
    fn param_schema_unknown() {
        let reg = DeviceRegistry::new();
        assert!(reg.param_schema("nonexistent").is_none());
    }

    #[test]
    fn list_devices() {
        let mut reg = DeviceRegistry::new();
        reg.register(dummy_descriptor()).unwrap();

        let list = reg.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "dummy");
    }

    #[test]
    fn load_dld_not_implemented() {
        let mut reg = DeviceRegistry::new();
        let result = reg.load_dld(Path::new("/fake/path.so"));
        assert!(matches!(result, Err(DldError::NotImplemented)));
    }

    #[test]
    fn empty_registry() {
        let reg = DeviceRegistry::new();
        assert!(reg.list().is_empty());
    }
}
