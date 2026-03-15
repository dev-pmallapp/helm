# helm-devices — LLD: Device Registry

> Low-level design for `DeviceRegistry`, `DeviceDescriptor`, `ParamSchema`, `DeviceParams`, plugin loading, ABI versioning, and Python class injection.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-device-trait.md`](./LLD-device-trait.md) · [`ARCHITECTURE.md`](../../ARCHITECTURE.md)

---

## Table of Contents

1. [Purpose](#1-purpose)
2. [ParamSchema and DeviceParams](#2-paramschema-and-deviceparams)
3. [DeviceDescriptor](#3-devicedescriptor)
4. [DeviceRegistry](#4-deviceregistry)
5. [Self-Registration for Built-in Devices](#5-self-registration-for-built-in-devices)
6. [Plugin Loading Protocol](#6-plugin-loading-protocol)
7. [ABI Version Check](#7-abi-version-check)
8. [Python Class Injection](#8-python-class-injection)
9. [PluginError Enum](#9-pluginerror-enum)
10. [Full Plugin Example (.so)](#10-full-plugin-example-so)
11. [Registry Lookup and Device Creation](#11-registry-lookup-and-device-creation)

---

## 1. Purpose

The `DeviceRegistry` enables runtime device type lookup and instantiation by name. It serves two client groups:

**Python configuration layer.** When a Python script writes `helm_ng.Uart16550(clock_hz=1_843_200)`, the Python class is backed by a `DeviceDescriptor` in the registry. The registry's factory function instantiates the Rust device struct from the Python-supplied parameters.

**Plugin system.** External `.so` files export a C-ABI function `helm_device_register` that is called when the plugin is loaded. The plugin registers one or more descriptors. The Python class definition is embedded in the plugin binary and injected into the `helm_ng` Python module namespace at load time.

The registry does not contain any device implementations. It contains type records (descriptors) and factory closures. Concrete device code lives in the plugin or in the main binary's built-in registration.

---

## 2. ParamSchema and DeviceParams

### ParamType

```rust
/// The type of a device parameter field.
#[derive(Debug, Clone)]
pub enum ParamType {
    /// Signed 64-bit integer.
    Int,
    /// Boolean.
    Bool,
    /// Memory size, parsed from strings like "32KiB", "4MiB", or plain integer bytes.
    MemorySize,
    /// UTF-8 string.
    String,
    /// One of a fixed set of named values.
    Enum(&'static [&'static str]),
}
```

### ParamValue

```rust
/// A concrete parameter value.
#[derive(Debug, Clone)]
pub enum ParamValue {
    Int(i64),
    Bool(bool),
    MemorySize(u64),  // always stored in bytes
    String(std::string::String),
    Enum(u32),        // index into ParamType::Enum variants
}
```

### ParamField

```rust
/// Description of one parameter field in a device's configuration.
#[derive(Debug, Clone)]
pub struct ParamField {
    /// Parameter name — used as the key in `DeviceParams`.
    pub name: &'static str,
    /// Type and valid values.
    pub kind: ParamType,
    /// Default value. Applied if the parameter is absent from `DeviceParams`.
    pub default: ParamValue,
    /// Whether this parameter is required. If `true` and absent with no default
    /// that makes sense, `DeviceRegistry::create()` returns `MissingParam`.
    pub required: bool,
    /// Human-readable description for Python help() output.
    pub description: &'static str,
}
```

### ParamSchema

```rust
/// The complete parameter schema for a device type.
///
/// Declares every parameter the device accepts. Used by:
/// - Python: to validate attribute assignments before `elaborate()`
/// - DeviceRegistry: to apply defaults and validate before calling the factory
/// - Python help(): to display parameter documentation
#[derive(Debug, Clone)]
pub struct ParamSchema {
    fields: Vec<ParamField>,
}

impl ParamSchema {
    pub fn new() -> Self {
        Self { fields: Vec::new() }
    }

    /// Add a required integer parameter.
    pub fn int(mut self, name: &'static str, description: &'static str) -> Self {
        self.fields.push(ParamField {
            name, description,
            kind: ParamType::Int,
            default: ParamValue::Int(0),
            required: true,
        });
        self
    }

    /// Add an optional integer parameter with a default value.
    pub fn int_default(mut self, name: &'static str, default: i64, description: &'static str) -> Self {
        self.fields.push(ParamField {
            name, description,
            kind: ParamType::Int,
            default: ParamValue::Int(default),
            required: false,
        });
        self
    }

    /// Add an optional boolean parameter.
    pub fn bool_default(mut self, name: &'static str, default: bool, description: &'static str) -> Self {
        self.fields.push(ParamField {
            name, description,
            kind: ParamType::Bool,
            default: ParamValue::Bool(default),
            required: false,
        });
        self
    }

    /// Add an optional memory size parameter.
    pub fn memory_size(mut self, name: &'static str, default_bytes: u64, description: &'static str) -> Self {
        self.fields.push(ParamField {
            name, description,
            kind: ParamType::MemorySize,
            default: ParamValue::MemorySize(default_bytes),
            required: false,
        });
        self
    }

    /// Add an enum parameter.
    pub fn enum_param(
        mut self,
        name: &'static str,
        variants: &'static [&'static str],
        default_idx: u32,
        description: &'static str,
    ) -> Self {
        self.fields.push(ParamField {
            name, description,
            kind: ParamType::Enum(variants),
            default: ParamValue::Enum(default_idx),
            required: false,
        });
        self
    }

    /// Validate a `DeviceParams` map against this schema.
    /// Applies defaults for missing optional fields.
    /// Returns the validated/defaulted params, or a `PluginError` on failure.
    pub fn validate(&self, mut params: DeviceParams) -> Result<DeviceParams, PluginError> {
        for field in &self.fields {
            if !params.contains(field.name) {
                if field.required {
                    return Err(PluginError::MissingParam(field.name));
                }
                params.insert(field.name, field.default.clone());
            }
        }
        Ok(params)
    }

    pub fn fields(&self) -> &[ParamField] { &self.fields }
}
```

### DeviceParams

```rust
/// A concrete set of parameter values for one device instantiation.
///
/// Created by the Python config layer from keyword arguments.
/// Validated against `ParamSchema` before being passed to the device factory.
#[derive(Debug, Default, Clone)]
pub struct DeviceParams {
    values: std::collections::HashMap<String, ParamValue>,
}

impl DeviceParams {
    pub fn new() -> Self { Self::default() }

    pub fn insert(&mut self, name: &str, val: ParamValue) {
        self.values.insert(name.to_string(), val);
    }

    pub fn contains(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    /// Get an integer parameter by name. Returns `Err` if absent or wrong type.
    pub fn get_int(&self, name: &str) -> Result<i64, PluginError> {
        match self.values.get(name) {
            Some(ParamValue::Int(v)) => Ok(*v),
            Some(_) => Err(PluginError::WrongParamType(name.to_string())),
            None    => Err(PluginError::MissingParam(name)),
        }
    }

    /// Get a boolean parameter. Returns `Err` if absent or wrong type.
    pub fn get_bool(&self, name: &str) -> Result<bool, PluginError> {
        match self.values.get(name) {
            Some(ParamValue::Bool(v)) => Ok(*v),
            Some(_) => Err(PluginError::WrongParamType(name.to_string())),
            None    => Err(PluginError::MissingParam(name)),
        }
    }

    /// Get a memory size in bytes. Returns `Err` if absent or wrong type.
    pub fn get_memory_size(&self, name: &str) -> Result<u64, PluginError> {
        match self.values.get(name) {
            Some(ParamValue::MemorySize(v)) => Ok(*v),
            Some(_) => Err(PluginError::WrongParamType(name.to_string())),
            None    => Err(PluginError::MissingParam(name)),
        }
    }

    /// Get a string parameter. Returns `Err` if absent or wrong type.
    pub fn get_str(&self, name: &str) -> Result<&str, PluginError> {
        match self.values.get(name) {
            Some(ParamValue::String(s)) => Ok(s.as_str()),
            Some(_) => Err(PluginError::WrongParamType(name.to_string())),
            None    => Err(PluginError::MissingParam(name)),
        }
    }

    /// Parse and insert a memory size from a string like "32KiB", "4MiB", or "8192".
    pub fn parse_memory_size(s: &str) -> Result<u64, PluginError> {
        // Supports: "N", "NKiB", "NMiB", "NGiB", "NKB", "NMB", "NGB"
        // Binary SI: KiB=1024, MiB=1024^2, GiB=1024^3
        let s = s.trim();
        let (num_part, mult) = if let Some(n) = s.strip_suffix("GiB") {
            (n, 1u64 << 30)
        } else if let Some(n) = s.strip_suffix("MiB") {
            (n, 1u64 << 20)
        } else if let Some(n) = s.strip_suffix("KiB") {
            (n, 1u64 << 10)
        } else if let Some(n) = s.strip_suffix("GB") {
            (n, 1_000_000_000u64)
        } else if let Some(n) = s.strip_suffix("MB") {
            (n, 1_000_000u64)
        } else if let Some(n) = s.strip_suffix("KB") {
            (n, 1_000u64)
        } else {
            (s, 1u64)
        };
        let n: u64 = num_part.trim().parse()
            .map_err(|_| PluginError::InvalidParamValue(format!("not a valid memory size: {s}")))?;
        Ok(n * mult)
    }
}
```

---

## 3. DeviceDescriptor

```rust
/// A complete runtime record for one device type.
///
/// Registered once per device type, either via `inventory::submit!`
/// (built-in devices) or via the plugin's `helm_device_register` call
/// (external .so devices).
pub struct DeviceDescriptor {
    /// Unique device type name — used as the key in `DeviceRegistry`.
    /// Convention: snake_case, e.g., "uart16550", "plic_riscv", "virtio_disk".
    pub name: &'static str,

    /// Semantic version of this device implementation.
    /// Used for diagnostic output; not for ABI compatibility (use ABI_VERSION for that).
    pub version: &'static str,

    /// One-line human-readable description.
    pub description: &'static str,

    /// Factory function: given validated `DeviceParams`, construct and return the device.
    ///
    /// Must not panic on valid params (schema-validated before this call).
    /// May return `Err(PluginError::DeviceCreate)` if OS resource allocation fails.
    pub factory: fn(DeviceParams) -> Result<Box<dyn Device>, PluginError>,

    /// Return the parameter schema for this device type.
    ///
    /// Called once at registration time to populate the Python class,
    /// and on demand for validation. Must return the same schema every call.
    pub param_schema: fn() -> ParamSchema,

    /// Python class definition string, injected into `helm_ng` module namespace
    /// at plugin load time (or at process start for built-in devices).
    ///
    /// Must define a class with the same name as `name` (CamelCase convention).
    /// Must declare all parameters in `param_schema()` as class attributes.
    /// Must NOT declare `base_addr` or `irq` — those are system-level.
    ///
    /// Set to `""` for devices that are not exposed to Python.
    pub python_class: &'static str,
}
```

---

## 4. DeviceRegistry

```rust
/// Runtime registry of device type descriptors.
///
/// Singleton-like: the `helm_ng` Python module holds one `DeviceRegistry`.
/// Built-in devices self-register via `inventory::submit!`.
/// Plugin devices register via `helm_device_register()` called at load time.
pub struct DeviceRegistry {
    /// Maps device type name to descriptor.
    devices: std::collections::HashMap<&'static str, DeviceDescriptor>,
    /// Loaded plugin library handles (kept alive to prevent dlclose).
    _libs: Vec<libloading::Library>,
}

impl DeviceRegistry {
    /// Create an empty registry. Built-in devices are added via
    /// `DeviceRegistry::collect_builtins()` which iterates `inventory`.
    pub fn new() -> Self {
        let mut reg = Self {
            devices: std::collections::HashMap::new(),
            _libs: Vec::new(),
        };
        reg.collect_builtins();
        reg
    }

    /// Register a device type. Called by plugins via `helm_device_register`.
    ///
    /// Returns `Err(PluginError::NameConflict)` if a device with the same
    /// name is already registered.
    pub fn register(&mut self, desc: DeviceDescriptor) -> Result<(), PluginError> {
        if self.devices.contains_key(desc.name) {
            return Err(PluginError::NameConflict(desc.name.to_string()));
        }
        self.devices.insert(desc.name, desc);
        Ok(())
    }

    /// Load a `.so` plugin, check ABI version, call `helm_device_register`,
    /// and inject the Python class string.
    ///
    /// On success, the plugin's Library handle is stored in `_libs` to prevent
    /// the dynamic linker from unloading it.
    pub fn load_plugin(&mut self, path: &std::path::Path) -> Result<(), PluginError> {
        // 1. dlopen the .so
        let lib = unsafe { libloading::Library::new(path) }
            .map_err(|e| PluginError::DlopenFailed(e.to_string()))?;

        // 2. ABI version check (see §7)
        {
            let abi_sym: libloading::Symbol<*const u32> = unsafe {
                lib.get(b"HELM_DEVICES_ABI_VERSION\0")
                    .map_err(|_| PluginError::MissingAbiSymbol)?
            };
            let plugin_abi = unsafe { **abi_sym };
            if plugin_abi != HELM_DEVICES_ABI_VERSION {
                return Err(PluginError::AbiVersionMismatch {
                    expected: HELM_DEVICES_ABI_VERSION,
                    found: plugin_abi,
                });
            }
        }

        // 3. Call helm_device_register
        {
            type RegisterFn = extern "C" fn(*mut DeviceRegistry);
            let register: libloading::Symbol<RegisterFn> = unsafe {
                lib.get(b"helm_device_register\0")
                    .map_err(|_| PluginError::MissingRegisterSymbol)?
            };
            unsafe { register(self as *mut _) };
        }

        // 4. Python class injection (see §8)
        // Handled after register() calls, inside register() itself via
        // the python_class field on each DeviceDescriptor.

        // 5. Keep library alive
        self._libs.push(lib);
        Ok(())
    }

    /// Instantiate a device by type name with the given parameters.
    ///
    /// Validates params against schema (applies defaults for missing optional params).
    /// Returns `Err` if the name is not registered, params are invalid, or construction fails.
    pub fn create(
        &self,
        name: &str,
        params: DeviceParams,
    ) -> Result<Box<dyn Device>, PluginError> {
        let desc = self.devices.get(name)
            .ok_or_else(|| PluginError::UnknownDevice(name.to_string()))?;
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

    /// Iterate `inventory` and register all built-in devices.
    fn collect_builtins(&mut self) {
        for desc in inventory::iter::<BuiltinDevice> {
            // Ignore errors: built-in names must be unique by construction.
            let _ = self.register(desc.0.clone());
        }
    }
}

/// The current ABI version. Bump this integer whenever the `DeviceRegistry`
/// or `DeviceDescriptor` types have a breaking change.
pub const HELM_DEVICES_ABI_VERSION: u32 = 1;
```

---

## 5. Self-Registration for Built-in Devices

Built-in device types (those compiled into the main binary, not loaded from `.so`) use the `inventory` crate to self-register without requiring a central list.

```rust
// In helm-devices/src/registry.rs:
use inventory;

/// Wrapper for inventory submission of built-in device descriptors.
pub struct BuiltinDevice(pub DeviceDescriptor);
inventory::collect!(BuiltinDevice);
```

A built-in device (e.g., in a `helm-devices-riscv-virt` crate) registers itself:

```rust
// In helm-devices-riscv-virt/src/plic.rs:
use helm_devices::registry::{BuiltinDevice, DeviceDescriptor, DeviceParams, ParamSchema, PluginError};
use helm_devices::Device;

inventory::submit! {
    BuiltinDevice(DeviceDescriptor {
        name: "plic_riscv",
        version: "1.0.0",
        description: "RISC-V Platform-Level Interrupt Controller",
        factory: |params: DeviceParams| -> Result<Box<dyn Device>, PluginError> {
            let num_sources = params.get_int("num_sources")? as u32;
            let num_contexts = params.get_int("num_contexts").unwrap_or(Ok(2))? as u32;
            Ok(Box::new(Plic::new(num_sources, num_contexts)))
        },
        param_schema: || {
            ParamSchema::new()
                .int("num_sources", "Number of interrupt sources (1–1023)")
                .int_default("num_contexts", 2, "Number of hart contexts (default 2)")
        },
        python_class: r#"
class PlicRiscv(Device):
    """RISC-V Platform-Level Interrupt Controller."""
    num_sources:  Param.Int = 64
    num_contexts: Param.Int = 2
"#,
    })
}
```

The `inventory::collect!` + `inventory::submit!` pattern uses linker magic (`.init_array` sections) to run the submit closures before `main()`. This is safe and well-tested on Linux, macOS, and Windows.

---

## 6. Plugin Loading Protocol

### The C-ABI Entry Point

Every plugin `.so` exports exactly this symbol:

```rust
// In the plugin crate (crate-type = ["cdylib"])

/// Helm-ng ABI version — checked before calling helm_device_register.
/// Must equal `HELM_DEVICES_ABI_VERSION` from the helm-devices crate.
#[no_mangle]
pub static HELM_DEVICES_ABI_VERSION: u32 = 1;

/// Entry point called by `DeviceRegistry::load_plugin()`.
///
/// Register all device types exported by this plugin.
/// May call `registry.register()` multiple times (Q69 — multiple devices per .so).
/// Must not panic. On error, log and return — partial registration is acceptable
/// (the registry will contain whatever was registered before the error).
#[no_mangle]
pub extern "C" fn helm_device_register(registry: *mut helm_devices::DeviceRegistry) {
    // Safety: the caller (DeviceRegistry::load_plugin) holds a &mut DeviceRegistry
    // and passes a valid non-null pointer.
    let r = unsafe { &mut *registry };

    if let Err(e) = r.register(MY_UART_DESCRIPTOR) {
        log::error!("helm_device_register: failed to register uart16550: {e}");
    }
    if let Err(e) = r.register(MY_SPI_DESCRIPTOR) {
        log::error!("helm_device_register: failed to register spi_controller: {e}");
    }
}
```

### Multiple Devices Per .so (Q69)

A single `.so` may register multiple device types. The plugin calls `r.register()` once per device type. There is no limit on the number of registrations per plugin.

**Naming convention for multi-device plugins:** The plugin file name should reflect the package (e.g., `libhelm_serial.so`), and the individual device names are `uart16550`, `spi_controller`, etc.

### Load Sequence

```
1. dlopen(path)                → libloading::Library::new(path)
2. Load HELM_DEVICES_ABI_VERSION symbol
3. Compare to host HELM_DEVICES_ABI_VERSION
4. If mismatch → PluginError::AbiVersionMismatch, return
5. Load helm_device_register symbol
6. If missing → PluginError::MissingRegisterSymbol, return
7. Call helm_device_register(&mut registry)
8. Inside the call: r.register() for each device type
   Each register() checks for name conflicts → PluginError::NameConflict
   Each register() injects python_class string (if non-empty)
9. Keep Library alive in registry._libs
```

---

## 7. ABI Version Check

The `HELM_DEVICES_ABI_VERSION` constant is a `u32` exported from every plugin. The host's `DeviceRegistry` checks it before calling `helm_device_register`.

**When to bump the ABI version:**

| Change | Bump? |
|--------|-------|
| Add a new optional method to `Device` trait | No (default impl preserves compatibility) |
| Change `Device::read()` or `Device::write()` signature | Yes |
| Change `DeviceDescriptor` struct layout | Yes |
| Change `DeviceParams` / `ParamValue` enum variants | Yes |
| Add a new `PluginError` variant | No (unknown variants are safe) |
| Change `HELM_DEVICES_ABI_VERSION` constant definition | N/A (that IS the version) |

The version is a single `u32`. There is no minor/patch split at the ABI level — any breaking change bumps the integer. Non-breaking additions do not require a bump.

**Embedding the ABI version in the plugin:**

The plugin must use the `HELM_DEVICES_ABI_VERSION` constant from the `helm-devices` crate it was compiled against. The static export ensures the value is fixed at plugin compile time:

```rust
// This will not compile if helm-devices is not a dependency of the plugin crate
#[no_mangle]
pub static HELM_DEVICES_ABI_VERSION: u32 = helm_devices::HELM_DEVICES_ABI_VERSION;
```

---

## 8. Python Class Injection

When a device is registered (either via `inventory::submit!` at startup or via `helm_device_register` at plugin load time), its `python_class` string is `exec()`'d into the `helm_ng` Python module namespace.

**Injection timing:**
- Built-in devices: injected at `helm_ng` module import time (during `#[pymodule]` init)
- Plugin devices: injected when `helm_ng.load_plugin(path)` is called from Python

**Name conflict handling (Q67):**

Before `exec()`'ing the class string, the loader checks whether the class name already exists in `helm_ng`'s `__dict__`. If it does, `PluginError::PythonNameConflict` is returned and the plugin is not loaded. The class name is extracted from the Python class string by the loader using a simple regex before `exec()`:

```rust
fn extract_class_name(python_class: &str) -> Option<&str> {
    // Matches "class Foo" or "class Foo(Bar):"
    python_class.lines()
        .find_map(|line| {
            let line = line.trim();
            if line.starts_with("class ") {
                line[6..].split(|c: char| !c.is_alphanumeric() && c != '_').next()
            } else {
                None
            }
        })
}
```

**Python class string requirements:**

The embedded Python class must:
1. Declare all parameters from `param_schema()` as class attributes with `Param.*` types
2. NOT declare `base_addr` or `irq` (those are system-level, not device-level)
3. Inherit from `helm_ng.Device` (or `Device` if already in scope)
4. Use the same CamelCase class name as the device type's snake_case `name` converted

**Example Python class string for UART 16550:**

```rust
pub static UART16550_PYTHON_CLASS: &str = r#"
class Uart16550(Device):
    """16550-compatible UART.

    Parameters
    ----------
    clock_hz : int
        Input clock frequency in Hz (default 1,843,200 Hz).
    fifo_depth : int
        FIFO depth: 1 (no FIFO), 16, 32, or 64 (default 16).
    """
    clock_hz:   Param.Int = 1_843_200
    fifo_depth: Param.Int = 16
"#;
```

After injection, Python can write:

```python
import helm_ng
uart = helm_ng.Uart16550(clock_hz=3_686_400, fifo_depth=64)
```

**Python API for plugin loading:**

```python
# Load a plugin from a .so file
helm_ng.load_plugin("/opt/helm/lib/libhelm_serial.so")

# The Uart16550 and SpiController classes are now available:
uart = helm_ng.Uart16550(clock_hz=1_843_200)
spi  = helm_ng.SpiController(freq_hz=10_000_000)
```

---

## 9. PluginError Enum

```rust
/// Errors from plugin loading, device creation, or parameter validation.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// `dlopen()` failed — usually wrong path or missing shared library dependencies.
    #[error("dlopen failed: {0}")]
    DlopenFailed(String),

    /// Plugin does not export `HELM_DEVICES_ABI_VERSION` symbol.
    #[error("plugin missing HELM_DEVICES_ABI_VERSION symbol — not a valid helm-devices plugin")]
    MissingAbiSymbol,

    /// Plugin's ABI version does not match the host's version.
    #[error("ABI version mismatch: host={expected}, plugin={found} — recompile plugin against helm-devices {expected}")]
    AbiVersionMismatch { expected: u32, found: u32 },

    /// Plugin does not export `helm_device_register` symbol.
    #[error("plugin missing helm_device_register symbol — not a valid helm-devices plugin")]
    MissingRegisterSymbol,

    /// A device with the same name is already registered.
    #[error("device name conflict: '{0}' is already registered")]
    NameConflict(String),

    /// The Python class name extracted from `python_class` conflicts with
    /// an existing name in the `helm_ng` module namespace.
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
}

impl From<DeviceError> for PluginError {
    fn from(e: DeviceError) -> Self {
        PluginError::DeviceCreate(e.to_string())
    }
}
```

---

## 10. Full Plugin Example (.so)

A complete, minimal plugin crate for a UART 16550:

```toml
# examples/plugin-uart/Cargo.toml
[package]
name = "helm-plugin-uart"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
helm-devices = { path = "../../crates/helm-devices" }
log = "0.4"
```

```rust
// examples/plugin-uart/src/lib.rs

use helm_devices::register_bank;
use helm_devices::{Device, DeviceDescriptor, DeviceParams, DeviceRegistry, PluginError};
use helm_devices::interrupt::InterruptPin;

// ── Register bank ────────────────────────────────────────────────────────────
register_bank! {
    pub struct Uart16550Regs {
        reg RBR @ 0x00 is read_only;
        reg THR @ 0x00 is write_only;
        reg IER @ 0x01 { field ERBFI [0]; field ETBEI [1]; }
        reg LSR @ 0x05 is read_only { field DR [0]; field THRE [5]; }
        reg SCR @ 0x07;
    }
    device = Uart16550;
}

// ── Device struct ─────────────────────────────────────────────────────────────
pub struct Uart16550 {
    pub irq_out: InterruptPin,
    clock_hz: u32,
    regs: Uart16550Regs,
    rx_buf: std::collections::VecDeque<u8>,
}

impl Uart16550 {
    pub fn new(clock_hz: u32) -> Self {
        Self {
            irq_out: InterruptPin::new(),
            clock_hz,
            regs: Uart16550Regs::default(),
            rx_buf: std::collections::VecDeque::with_capacity(16),
        }
    }

    fn on_write_thr(&mut self, _old: u32, val: u32) {
        // Transmit side effect: in loopback mode, push to rx_buf
        if self.regs.mcr_loop() != 0 {
            self.rx_buf.push_back(val as u8);
            self.regs.set_lsr_dr(1);
        }
        // After write, THRE=1 (holding register now empty — we ignore timing)
        self.regs.set_lsr_thre(1);
        self.update_irq();
    }

    fn update_irq(&mut self) {
        let rda = self.regs.ier_erbfi() != 0 && self.regs.lsr_dr() != 0;
        let thre = self.regs.ier_etbei() != 0 && self.regs.lsr_thre() != 0;
        if rda || thre { self.irq_out.assert(); } else { self.irq_out.deassert(); }
    }
}

impl Device for Uart16550 {
    fn read(&self, offset: u64, size: usize) -> u64 {
        self.regs.mmio_read(offset, size)
    }
    fn write(&mut self, offset: u64, size: usize, val: u64) {
        self.regs.mmio_write(offset, size, val, self);
    }
    fn region_size(&self) -> u64 { 8 }
}

// ── Plugin ABI version export ─────────────────────────────────────────────────
#[no_mangle]
pub static HELM_DEVICES_ABI_VERSION: u32 = helm_devices::HELM_DEVICES_ABI_VERSION;

// ── Embedded Python class ─────────────────────────────────────────────────────
static PYTHON_CLASS: &str = r#"
class Uart16550(Device):
    """16550-compatible UART."""
    clock_hz:   Param.Int = 1_843_200
    fifo_depth: Param.Int = 16
"#;

// ── Descriptor ───────────────────────────────────────────────────────────────
fn uart_descriptor() -> DeviceDescriptor {
    DeviceDescriptor {
        name: "uart16550",
        version: "1.0.0",
        description: "16550-compatible UART",
        factory: |params: DeviceParams| -> Result<Box<dyn Device>, PluginError> {
            let clock_hz = params.get_int("clock_hz")? as u32;
            Ok(Box::new(Uart16550::new(clock_hz)))
        },
        param_schema: || {
            helm_devices::params::ParamSchema::new()
                .int_default("clock_hz", 1_843_200, "Input clock frequency in Hz")
                .int_default("fifo_depth", 16, "FIFO depth (1, 16, 32, or 64)")
        },
        python_class: PYTHON_CLASS,
    }
}

// ── Plugin entry point ────────────────────────────────────────────────────────
#[no_mangle]
pub extern "C" fn helm_device_register(registry: *mut DeviceRegistry) {
    let r = unsafe { &mut *registry };
    if let Err(e) = r.register(uart_descriptor()) {
        log::error!("helm_device_register: {e}");
    }
}
```

---

## 11. Registry Lookup and Device Creation

```rust
// Typical usage in World or System::elaborate():

let mut registry = DeviceRegistry::new(); // collects built-ins

// Load an external plugin
registry.load_plugin("/opt/helm/lib/libhelm_serial.so".as_ref())?;

// Create a device by name with parameters
let mut params = DeviceParams::new();
params.insert("clock_hz", ParamValue::Int(3_686_400));
let uart: Box<dyn Device> = registry.create("uart16550", params)?;

// List all available device types
for desc in registry.list() {
    println!("{} v{}: {}", desc.name, desc.version, desc.description);
}

// Inspect a device's parameter schema
if let Some(schema) = registry.param_schema("uart16550") {
    for field in schema.fields() {
        println!("  {} ({:?}): {}", field.name, field.kind, field.description);
    }
}
```

**Python-side usage:**

```python
import helm_ng

# Load plugin (registers class in helm_ng namespace + device in registry)
helm_ng.load_plugin("/opt/helm/lib/libhelm_serial.so")

# Instantiate using the injected Python class
uart = helm_ng.Uart16550(clock_hz=3_686_400)

# Or programmatically:
uart = helm_ng.DeviceRegistry.create("uart16550", clock_hz=3_686_400)
```
