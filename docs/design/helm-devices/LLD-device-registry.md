# helm-devices — LLD: Device Registry

> Low-level design for `DeviceRegistry`, `DeviceDescriptor`, `ParamSchema`, `DeviceParams`, DLD loading, ABI versioning, and Python class injection.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-device-trait.md`](./LLD-device-trait.md) · [`ARCHITECTURE.md`](../../ARCHITECTURE.md)

---

## Table of Contents

1. [Purpose](#1-purpose)
2. [ParamSchema and DeviceParams](#2-paramschema-and-deviceparams)
3. [DeviceDescriptor](#3-devicedescriptor)
4. [DeviceRegistry](#4-deviceregistry)
5. [Self-Registration for Built-in Devices](#5-self-registration-for-built-in-devices)
6. [DLD Loading Protocol](#6-dld-loading-protocol)
7. [ABI Version Check](#7-abi-version-check)
8. [Python Class Injection](#8-python-class-injection)
9. [DldError Enum](#9-dlderror-enum)
10. [Full DLD Example (.so)](#10-full-dld-example-so)
11. [Registry Lookup and Device Creation](#11-registry-lookup-and-device-creation)

---

## 1. Purpose

The `DeviceRegistry` enables runtime device type lookup and instantiation by name. It serves two client groups:

**Python configuration layer.** When a Python script writes `helm_ng.Uart16550(clock_hz=1_843_200)`, the Python class is backed by a `DeviceDescriptor` in the registry. The registry's factory function instantiates the Rust device struct from the Python-supplied parameters.

**DLD system.** External `.so` files export a C-ABI function `helm_device_register` that is called when the DLD is loaded. The DLD registers one or more descriptors. The Python class definition is embedded in the DLD binary and injected into the `helm_ng` Python module namespace at load time.

The registry does not contain any device implementations. It contains type records (descriptors) and factory closures. Concrete device code lives in the DLD or in the main binary's built-in registration.

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
    /// Returns the validated/defaulted params, or a `DldError` on failure.
    ///
    /// This is **assignment-time validation** (Q1.6): checks type and
    /// presence. Called when Python assigns parameters (before `elaborate()`).
    pub fn validate(&self, mut params: DeviceParams) -> Result<DeviceParams, DldError> {
        for field in &self.fields {
            if !params.contains(field.name) {
                if field.required {
                    return Err(DldError::MissingParam(field.name));
                }
                params.insert(field.name, field.default.clone());
            }
        }
        Ok(params)
    }

    pub fn fields(&self) -> &[ParamField] { &self.fields }
}
```

### Dual-Phase Validation (Q1.6)

Parameter validation happens in two distinct phases:

**Phase 1 — Assignment-time (Python layer, before `elaborate()`):** `ParamSchema::validate()` is called whenever Python assigns a parameter. This catches type errors (wrong `ParamType`) and missing required fields immediately, not at simulation start. Python integration raises `TypeError` or `ValueError` at attribute assignment time.

**Phase 2 — Realize-time (device factory, inside `elaborate()`):** The device factory function performs semantic validation that requires knowing all parameter values together. For example, a device may require that `fifo_depth` is a power of two, or that `clock_hz` divides evenly by `baud_rate`. This logic lives in the factory closure (or a `realize()` method), not in `ParamSchema`, because it requires cross-field reasoning.

```rust
// Assignment-time: ParamSchema::validate() — type + presence only
// Realize-time: factory checks semantic constraints
factory: |params: DeviceParams| -> Result<Box<dyn Device>, DldError> {
    let clock_hz   = params.get_int("clock_hz")? as u32;
    let fifo_depth = params.get_int("fifo_depth")? as usize;
    // Semantic check at realize-time:
    if !matches!(fifo_depth, 1 | 16 | 32 | 64) {
        return Err(DldError::InvalidParamValue(
            format!("fifo_depth must be 1, 16, 32, or 64; got {fifo_depth}")
        ));
    }
    Ok(Box::new(Uart16550::new(clock_hz, fifo_depth)))
},
```
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
    pub fn get_int(&self, name: &str) -> Result<i64, DldError> {
        match self.values.get(name) {
            Some(ParamValue::Int(v)) => Ok(*v),
            Some(_) => Err(DldError::WrongParamType(name.to_string())),
            None    => Err(DldError::MissingParam(name)),
        }
    }

    /// Get a boolean parameter. Returns `Err` if absent or wrong type.
    pub fn get_bool(&self, name: &str) -> Result<bool, DldError> {
        match self.values.get(name) {
            Some(ParamValue::Bool(v)) => Ok(*v),
            Some(_) => Err(DldError::WrongParamType(name.to_string())),
            None    => Err(DldError::MissingParam(name)),
        }
    }

    /// Get a memory size in bytes. Returns `Err` if absent or wrong type.
    pub fn get_memory_size(&self, name: &str) -> Result<u64, DldError> {
        match self.values.get(name) {
            Some(ParamValue::MemorySize(v)) => Ok(*v),
            Some(_) => Err(DldError::WrongParamType(name.to_string())),
            None    => Err(DldError::MissingParam(name)),
        }
    }

    /// Get a string parameter. Returns `Err` if absent or wrong type.
    pub fn get_str(&self, name: &str) -> Result<&str, DldError> {
        match self.values.get(name) {
            Some(ParamValue::String(s)) => Ok(s.as_str()),
            Some(_) => Err(DldError::WrongParamType(name.to_string())),
            None    => Err(DldError::MissingParam(name)),
        }
    }

    /// Parse and insert a memory size from a string like "32KiB", "4MiB", or "8192".
    pub fn parse_memory_size(s: &str) -> Result<u64, DldError> {
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
            .map_err(|_| DldError::InvalidParamValue(format!("not a valid memory size: {s}")))?;
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
/// (built-in devices) or via the DLD's `helm_device_register` call
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
    /// May return `Err(DldError::DeviceCreate)` if OS resource allocation fails.
    pub factory: fn(DeviceParams) -> Result<Box<dyn Device>, DldError>,

    /// Return the parameter schema for this device type.
    ///
    /// Called once at registration time to auto-generate the Python class,
    /// and on demand for validation. Must return the same schema every call.
    ///
    /// `ParamSchema` is the authoritative source of truth for all Python
    /// parameter definitions (Q3.5). The Python class is generated from it
    /// automatically — no hand-written `python_class` string is needed.
    pub param_schema: fn() -> ParamSchema,

    /// Optional extra Python class body to append after the auto-generated class.
    ///
    /// Use only for devices that need Python-side methods or properties beyond
    /// the auto-generated parameter attributes. `None` for the vast majority
    /// of devices. Must NOT re-declare any parameters already in `param_schema`.
    pub python_class_extra: Option<&'static str>,

    /// Alternative names by which this device type can be looked up.
    ///
    /// All aliases resolve to the same `DeviceDescriptor`. Useful for
    /// backward compatibility when renaming a device type (Q3.7).
    /// Convention: list deprecated names, not abbreviations.
    /// Example: `&["uart_16550", "uart16550_legacy"]`
    pub aliases: &'static [&'static str],

    /// Host capabilities this device requires to function correctly.
    ///
    /// Checked by `DeviceRegistry::load_dld()` and
    /// `DeviceRegistry::create()`. If the host cannot satisfy a required
    /// capability, the DLD fails to load with `DldError::CapabilityMissing`
    /// (Q3.8). Built-in devices with no special requirements set `&[]`.
    pub required_capabilities: &'static [HostCapability],
}
```

---

## 4. DeviceRegistry

```rust
/// Runtime registry of device type descriptors.
///
/// Singleton-like: the `helm_ng` Python module holds one `DeviceRegistry`.
/// Built-in devices self-register via `inventory::submit!`.
/// DLD devices register via `helm_device_register()` called at load time.
pub struct DeviceRegistry {
    /// Maps device type name to descriptor.
    devices: std::collections::HashMap<&'static str, DeviceDescriptor>,
    /// Loaded DLD library handles (kept alive to prevent dlclose).
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

    /// Register a device type. Called by DLDs via `helm_device_register`.
    ///
    /// Returns `Err(DldError::NameConflict)` if a device with the same
    /// name is already registered.
    pub fn register(&mut self, desc: DeviceDescriptor) -> Result<(), DldError> {
        if self.devices.contains_key(desc.name) {
            return Err(DldError::NameConflict(desc.name.to_string()));
        }
        self.devices.insert(desc.name, desc);
        Ok(())
    }

    /// Load a `.so` DLD, check ABI version, call `helm_device_register`,
    /// and inject the Python class string.
    ///
    /// On success, the DLD's Library handle is stored in `_libs` to prevent
    /// the dynamic linker from unloading it.
    pub fn load_dld(&mut self, path: &std::path::Path) -> Result<(), DldError> {
        // 1. dlopen the .so
        let lib = unsafe { libloading::Library::new(path) }
            .map_err(|e| DldError::DlopenFailed(e.to_string()))?;

        // 2. ABI version check (see §7)
        {
            let abi_sym: libloading::Symbol<*const u32> = unsafe {
                lib.get(b"HELM_DEVICES_ABI_VERSION\0")
                    .map_err(|_| DldError::MissingAbiSymbol)?
            };
            let dld_abi = unsafe { **abi_sym };
            if dld_abi != HELM_DEVICES_ABI_VERSION {
                return Err(DldError::AbiVersionMismatch {
                    expected: HELM_DEVICES_ABI_VERSION,
                    found: dld_abi,
                });
            }
        }

        // 3. Call helm_device_register
        {
            type RegisterFn = extern "C" fn(*mut DeviceRegistry);
            let register: libloading::Symbol<RegisterFn> = unsafe {
                lib.get(b"helm_device_register\0")
                    .map_err(|_| DldError::MissingRegisterSymbol)?
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
    ) -> Result<Box<dyn Device>, DldError> {
        let desc = self.devices.get(name)
            .ok_or_else(|| DldError::UnknownDevice(name.to_string()))?;
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
use helm_devices::registry::{BuiltinDevice, DeviceDescriptor, DeviceParams, ParamSchema, DldError};
use helm_devices::Device;

inventory::submit! {
    BuiltinDevice(DeviceDescriptor {
        name: "plic_riscv",
        version: "1.0.0",
        description: "RISC-V Platform-Level Interrupt Controller",
        factory: |params: DeviceParams| -> Result<Box<dyn Device>, DldError> {
            let num_sources = params.get_int("num_sources")? as u32;
            let num_contexts = params.get_int("num_contexts").unwrap_or(Ok(2))? as u32;
            Ok(Box::new(Plic::new(num_sources, num_contexts)))
        },
        param_schema: || {
            // ParamSchema is authoritative; Python class auto-generated from it (Q3.5)
            ParamSchema::new()
                .int("num_sources", "Number of interrupt sources (1–1023)")
                .int_default("num_contexts", 2, "Number of hart contexts (default 2)")
        },
        python_class_extra: None,  // auto-generated (Q3.5)
        aliases: &[],
        required_capabilities: &[],
    })
}
```

The `inventory::collect!` + `inventory::submit!` pattern uses linker magic (`.init_array` sections) to run the submit closures before `main()`. This is safe and well-tested on Linux, macOS, and Windows.

---

## 6. DLD Loading Protocol

### The C-ABI Entry Point

Every DLD `.so` exports exactly this symbol:

```rust
// In the DLD crate (crate-type = ["cdylib"])

/// Helm-ng ABI version — checked before calling helm_device_register.
/// Must equal `HELM_DEVICES_ABI_VERSION` from the helm-devices crate.
#[no_mangle]
pub static HELM_DEVICES_ABI_VERSION: u32 = 1;

/// Entry point called by `DeviceRegistry::load_dld()`.
///
/// Register all device types exported by this DLD.
/// May call `registry.register()` multiple times (Q69 — multiple devices per .so).
/// Must not panic. On error, log and return — partial registration is acceptable
/// (the registry will contain whatever was registered before the error).
#[no_mangle]
pub extern "C" fn helm_device_register(registry: *mut helm_devices::DeviceRegistry) {
    // Safety: the caller (DeviceRegistry::load_dld) holds a &mut DeviceRegistry
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

A single `.so` may register multiple device types. The DLD calls `r.register()` once per device type. There is no limit on the number of registrations per DLD.

**Naming convention for multi-device DLDs:** The DLD file name should reflect the package (e.g., `libhelm_serial.so`), and the individual device names are `uart16550`, `spi_controller`, etc.

### Load Sequence

```
1. dlopen(path)                → libloading::Library::new(path)
2. Load HELM_DEVICES_ABI_VERSION symbol
3. Compare to host HELM_DEVICES_ABI_VERSION
4. If mismatch → DldError::AbiVersionMismatch, return
5. Load helm_device_register symbol
6. If missing  → DldError::MissingRegisterSymbol, return
7. Call helm_device_register(&mut registry)
8. Inside the call: r.register() for each device type
   Each register() checks for name conflicts → DldError::NameConflict
   Each register() injects python_class string (if non-empty)
9. Keep Library alive in registry._libs
```

---

## 7. ABI Version Check

The `HELM_DEVICES_ABI_VERSION` constant is a `u32` exported from every DLD. The host's `DeviceRegistry` checks it before calling `helm_device_register`.

**When to bump the ABI version:**

| Change | Bump? |
|--------|-------|
| Add a new optional method to `Device` trait | No (default impl preserves compatibility) |
| Change `Device::read()` or `Device::write()` signature | Yes |
| Change `DeviceDescriptor` struct layout | Yes |
| Change `DeviceParams` / `ParamValue` enum variants | Yes |
| Add a new `DldError` variant | No (unknown variants are safe)  |
| Change `HELM_DEVICES_ABI_VERSION` constant definition | N/A (that IS the version) |

The version is a single `u32`. There is no minor/patch split at the ABI level — any breaking change bumps the integer. Non-breaking additions do not require a bump.

**Embedding the ABI version in the DLD:**

The DLD must use the `HELM_DEVICES_ABI_VERSION` constant from the `helm-devices` crate it was compiled against. The static export ensures the value is fixed at DLD compile time:

```rust
// This will not compile if helm-devices is not a dependency of the DLD crate
#[no_mangle]
pub static HELM_DEVICES_ABI_VERSION: u32 = helm_devices::HELM_DEVICES_ABI_VERSION;
```

---

## 8. Python Class Auto-Generation

`ParamSchema` is the authoritative source for a device's Python class definition (Q3.5). The host's PyO3 layer auto-generates a Python class from the schema at registration time. No hand-written `python_class` string is needed.

**Auto-generation rules:**
- Class name: `name` field converted from `snake_case` to `CamelCase` (e.g., `"uart16550"` → `Uart16550`)
- Each `ParamField` in `param_schema()` becomes a class attribute with the correct `Param.*` type
- Required fields have no default; optional fields use their `ParamField::default` value
- Docstring: `description` field from `DeviceDescriptor`

**Injection timing:**
- Built-in devices: injected at `helm_ng` module import time (during `#[pymodule]` init)
- DLD devices: injected when `helm_ng.load_dld(path)` is called from Python

**`python_class_extra`** (used rarely): if a device needs Python-side helper methods or properties beyond the auto-generated attributes, set `python_class_extra: Some("...")` with the extra class body lines. The auto-generated class is emitted first; the extra string is appended into the same class body.

**Name conflict handling (Q67):**

Before generating/injecting the class, the loader checks whether the class name already exists in `helm_ng`'s `__dict__`. If it does, `DldError::PythonNameConflict` is returned and the DLD is not loaded.

**After injection, Python can write:**

```python
import helm_ng
uart = helm_ng.Uart16550(clock_hz=3_686_400, fifo_depth=64)
```

**Python API for DLD loading:**

```python
# Load a DLD from a .so file
helm_ng.load_dld("/opt/helm/lib/libhelm_serial.so")

# The Uart16550 and SpiController classes are now available:
uart = helm_ng.Uart16550(clock_hz=1_843_200)
spi  = helm_ng.SpiController(freq_hz=10_000_000)
```

### HostCapability Type (Q3.8)

```rust
/// A capability the device requires from the host environment.
///
/// Checked by `DeviceRegistry::create()` before calling the factory function.
/// If the host cannot satisfy a required capability, `DldError::CapabilityMissing`
/// is returned and no device is created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCapability {
    /// Device requires a KVM fd for hardware-accelerated virtualization.
    KvmAcceleration,
    /// Device requires a vhost-net fd for kernel-bypassed networking.
    VhostNet,
    /// Device requires a /dev/vfio/N fd for IOMMU pass-through.
    VfioPassthrough,
    /// Device requires USB host controller access.
    UsbHost,
    /// Custom capability name (for DLDs that define their own requirements).
    Custom(&'static str),
}
```

### Checkpoint Migration Export (Q3.6)

When a device's checkpoint format changes between versions, the DLD exports an additional C-ABI function so the host can migrate saved state without losing existing checkpoints:

```rust
/// Called by CheckpointManager when restoring a checkpoint from an older
/// device version (old_version < current_version).
///
/// `data` is the serialized bytes from the old checkpoint.
/// Returns `Ok(Vec<u8>)` with the migrated bytes in the current format,
/// or `Err(String)` with a human-readable reason if migration is impossible.
///
/// Symbol name convention: `helm_{device_name}_migrate_checkpoint`
/// (e.g., `helm_uart16550_migrate_checkpoint`)
#[no_mangle]
pub extern "C" fn helm_uart16550_migrate_checkpoint(
    old_version: u32,
    data: *const u8,
    len: usize,
) -> helm_devices::CheckpointMigrateResult {
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    match old_version {
        0 => migrate_v0_to_v1(bytes),
        _ => helm_devices::CheckpointMigrateResult::Err(
            format!("no migration path from version {old_version}")
        ),
    }
}
```

The host looks up `helm_{name}_migrate_checkpoint` in the DLD's `.so` after ABI version check. If the symbol is absent, the host assumes no migration is needed (checkpoint format is unchanged).

---

## 9. DldError Enum

```rust
/// Errors from DLD loading, device creation, or parameter validation.
#[derive(Debug, thiserror::Error)]
pub enum DldError {
    /// `dlopen()` failed — usually wrong path or missing shared library dependencies.
    #[error("dlopen failed: {0}")]
    DlopenFailed(String),

    /// DLD does not export `HELM_DEVICES_ABI_VERSION` symbol.
    #[error("DLD missing HELM_DEVICES_ABI_VERSION symbol — not a valid helm-devices DLD")]
    MissingAbiSymbol,

    /// DLD's ABI version does not match the host's version.
    #[error("ABI version mismatch: host={expected}, DLD={found} — recompile DLD against helm-devices {expected}")]
    AbiVersionMismatch { expected: u32, found: u32 },

    /// DLD does not export `helm_device_register` symbol.
    #[error("DLD missing helm_device_register symbol — not a valid helm-devices DLD")]
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

    /// Device requires a host capability the current host cannot satisfy.
    #[error("missing host capability: {0:?}")]
    CapabilityMissing(HostCapability),
}

impl From<DeviceError> for DldError {
    fn from(e: DeviceError) -> Self {
        Self::DeviceCreate(e.to_string())
    }
}
```

---

## 10. Full DLD Example (.so)

A complete, minimal DLD crate for a UART 16550:

```toml
# examples/dld-uart/Cargo.toml
[package]
name = "helm-dld-uart"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
helm-devices = { path = "../../framework/helm-devices" }
log = "0.4"
```

```rust
// examples/dld-uart/src/lib.rs

use helm_devices::register_bank;
use helm_devices::{Device, DeviceDescriptor, DeviceParams, DeviceRegistry, DldError};
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
    fn read(&mut self, offset: u64, size: usize) -> u64 {
        self.regs.mmio_read(offset, size, self)
    }
    fn write(&mut self, offset: u64, size: usize, val: u64) {
        self.regs.mmio_write(offset, size, val, self);
    }
    fn region_size(&self) -> u64 { 8 }
}

// ── DLD ABI version export ────────────────────────────────────────────────────
#[no_mangle]
pub static HELM_DEVICES_ABI_VERSION: u32 = helm_devices::HELM_DEVICES_ABI_VERSION;

// ── Descriptor ───────────────────────────────────────────────────────────────
fn uart_descriptor() -> DeviceDescriptor {
    DeviceDescriptor {
        name: "uart16550",
        version: "1.0.0",
        description: "16550-compatible UART",
        factory: |params: DeviceParams| -> Result<Box<dyn Device>, DldError> {
            let clock_hz = params.get_int("clock_hz")? as u32;
            Ok(Box::new(Uart16550::new(clock_hz)))
        },
        param_schema: || {
            // ParamSchema is authoritative; Python class is auto-generated from it (Q3.5)
            helm_devices::params::ParamSchema::new()
                .int_default("clock_hz", 1_843_200, "Input clock frequency in Hz")
                .int_default("fifo_depth", 16, "FIFO depth (1, 16, 32, or 64)")
        },
        python_class_extra: None,  // auto-generated from param_schema (Q3.5)
        aliases: &["uart_16550"],  // backward-compat alias (Q3.7)
        required_capabilities: &[], // no special host requirements (Q3.8)
    }
}

// Optional: checkpoint migration export (Q3.6)
// If the device's checkpoint format changes between versions, export this symbol:
//
// #[no_mangle]
// pub extern "C" fn helm_uart16550_migrate_checkpoint(
//     version: u32,
//     data: *const u8,
//     len: usize,
// ) -> helm_devices::CheckpointMigrateResult {
//     // Deserialize old format (version N-1), return serialized new format
//     todo!()
// }

// ── DLD entry point ──────────────────────────────────────────────────────────
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

// Load an external DLD
registry.load_dld("/opt/helm/lib/libhelm_serial.so".as_ref())?;

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

# Load DLD (registers class in helm_ng namespace + device in registry)
helm_ng.load_dld("/opt/helm/lib/libhelm_serial.so")

# Instantiate using the injected Python class
uart = helm_ng.Uart16550(clock_hz=3_686_400)

# Or programmatically:
uart = helm_ng.DeviceRegistry.create("uart16550", clock_hz=3_686_400)
```
