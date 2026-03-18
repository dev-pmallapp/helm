# helm-ng -- LLD: API Versioning Strategy

> Low-level design for independent versioning of all 8 API surfaces.
> Cross-references: [`HLD.md`](./HLD.md) - [`helm-devices/LLD-device-registry.md`](./helm-devices/LLD-device-registry.md) - [`helm-debug/LLD-checkpoint.md`](./helm-debug/LLD-checkpoint.md) - [`DESIGN-QUESTIONS.md`](./DESIGN-QUESTIONS.md) (Q68, Q86--Q88)

---

## Table of Contents

1. [Versioning Philosophy](#1-versioning-philosophy)
2. [Surface Inventory Table](#2-surface-inventory-table)
3. [Per-Surface Design](#3-per-surface-design)
   - [3.1 Device Plugin ABI](#31-surface-1-device-plugin-abi-helm-devices)
   - [3.2 Instrument Plugin ABI](#32-surface-2-instrument-plugin-abi-helm-plugin)
   - [3.3 Python API](#33-surface-3-python-api-helm_ng-module)
   - [3.4 SimObject / Object Model](#34-surface-4-simobject--object-model)
   - [3.5 Checkpoint Format](#35-surface-5-checkpoint-format)
   - [3.6 HelmEventBus Event Types](#36-surface-6-helmeventbus-event-types)
   - [3.7 Debug Protocol](#37-surface-7-debug-protocol-helm-debug)
   - [3.8 register_bank! / ParamSchema](#38-surface-8-register_bank-macro-output--paramschema)
4. [Cross-Surface Coupling Rules](#4-cross-surface-coupling-rules)
5. [Version Manifest](#5-version-manifest)
6. [Compatibility Matrix](#6-compatibility-matrix)
7. [Practical Extension Patterns](#7-practical-extension-patterns)
8. [Migration Tooling](#8-migration-tooling)

---

## 1. Versioning Philosophy

### What "API Version" Means

An API version is a contract between a producer and a consumer. The version number is an assertion: "any consumer compiled or written against this contract will behave correctly when paired with this producer."

Different surface categories require different version semantics because their
failure modes differ:

| Category | Failure mode on mismatch | Version semantics |
|----------|--------------------------|-------------------|
| ABI (`.so` boundary) | Undefined behavior: corrupted vtables, wrong struct layouts, segfault | `major.minor` encoded as two `u32` fields; major mismatch = refuse to load |
| Protocol (GDB RSP, HelmProtocol) | Garbled packets, unrecognized commands | Feature negotiation at handshake; no static version check needed |
| Schema (checkpoint, ParamSchema) | Silent data corruption, missing fields | Monotonic `u32` version; forward-compatible with defaults for missing fields |
| Python API | `AttributeError`, `TypeError` at call site | Semantic version via `helm_ng.__version__`; `DeprecationWarning` for removals |

### Semantic Versioning Rules

For every surface, changes are classified as **major** (breaking), **minor** (additive, backward-compatible), or **patch** (non-observable fix):

**Major (breaking)** -- requires consumers to adapt:
- Removing a function, method, field, or enum variant
- Changing the signature of an existing function or callback
- Changing the layout of a `#[repr(C)]` struct
- Renaming a checkpoint field without providing a migration
- Removing a ParamSchema field without a deprecation cycle

**Minor (additive)** -- existing consumers continue to work unchanged:
- Adding a new function, method, or optional field
- Adding a new enum variant to a `#[non_exhaustive]` enum
- Adding a new callback type to a plugin ABI (if registered separately)
- Adding a new optional field to a `#[repr(C)]` struct (when guarded by a size field)
- Adding a new register to a `register_bank!` invocation (checkpoint deserialization uses defaults for missing fields)

**Patch (non-observable)** -- invisible to consumers:
- Bug fixes that do not change observable behavior
- Performance improvements
- Internal refactoring

### The Independence Principle

Each surface has an independent version. Surfaces only couple when one surface's change
mechanically forces another surface's change. Section 4 enumerates all such couplings
exhaustively. A change that affects only one surface bumps only that surface's version.

---

## 2. Surface Inventory Table

| # | Surface | Version Type | Current | Major Bump Trigger | Minor Bump Trigger | Checked By |
|---|---------|-------------|---------|--------------------|--------------------|------------|
| 1 | Device Plugin ABI | `u32` major + `u32` minor | 1.0 | `DeviceDescriptor` layout change; `Device` trait signature change; `DeviceParams`/`ParamValue` variant change | New optional `DeviceDescriptor` field (guarded by `struct_size`); new `PluginError` variant | Host at `dlopen` time |
| 2 | Instrument Plugin ABI | `u32` major + `u32` minor | 1.0 | Callback signature change; `PluginRegistry` layout change | New callback type added to registry; new `PluginArgs` field | Host at `dlopen` time |
| 3 | Python API (`helm_ng`) | semver string | 0.1.0 | Method removal; signature change; class removal | New method; new class; new optional parameter | User at import time via `__version__`; `DeprecationWarning` at call site |
| 4 | SimObject / Object Model | `u32` | 1 | Lifecycle method signature change; `ClassDescriptor` field change | New optional lifecycle hook with default impl | Host at `ClassRegistry::global()` init |
| 5 | Checkpoint Format | `u32` | 1 | `CheckpointHeader` field removal/rename; `ObjectBlob` format change; attribute encoding change | New optional `CheckpointHeader` field (serde default) | `CheckpointManager::restore()` |
| 6 | HelmEventBus Event Types | `u32` | 1 | Existing variant discriminant reassignment; variant field removal | New `#[non_exhaustive]` variant added | Subscriber at compile time (Rust) or runtime (Python) |
| 7a | GDB RSP | External (GDB-defined) | N/A | N/A (externally defined) | N/A | `qSupported` packet negotiation |
| 7b | HelmProtocol | `u32` major + `u32` minor | 1.0 | Command removal; response schema change | New command; new optional response field | Handshake at connection time |
| 8 | `register_bank!` / ParamSchema | Tied to surfaces 1 + 5 | N/A | See coupling rules (Section 4) | See coupling rules (Section 4) | Indirectly via Device ABI + Checkpoint version |

---

## 3. Per-Surface Design

### 3.1 Surface 1: Device Plugin ABI (`helm-devices`)

**Status quo.** A single `HELM_DEVICES_ABI_VERSION: u32 = 1` exported symbol. Exact-match check. No minor version. Any change, even additive, requires a full ABI bump and plugin recompile.

**Upgraded design.** Split into major + minor with a `struct_size` guard.

#### Version Carriers

```rust
// framework/helm-devices/src/abi.rs

/// Major ABI version. Incremented on breaking layout or signature changes.
/// Plugin with major != host major is refused.
pub const HELM_DEVICE_ABI_MAJOR: u32 = 1;

/// Minor ABI version. Incremented on additive changes (new optional fields,
/// new function pointers in the vtable). Plugin minor <= host minor is
/// compatible; plugin minor > host minor means the plugin uses features the
/// host does not understand -- refuse with a diagnostic.
pub const HELM_DEVICE_ABI_MINOR: u32 = 0;
```

#### C-ABI Descriptor with Size Guard

The fundamental problem with `#[repr(C)]` struct evolution is that adding a field
changes the struct size. The solution is a `struct_size` field at a fixed offset:

```rust
/// The C-ABI-stable descriptor that plugins export.
///
/// New fields are ALWAYS appended. The `struct_size` field at offset 0
/// tells the host how many bytes the plugin populated. Fields beyond
/// what the plugin knows about are zero-initialized by the host.
#[repr(C)]
pub struct DeviceDescriptorC {
    /// Must be `size_of::<DeviceDescriptorC>()` as the plugin sees it.
    /// The host reads this first and uses it to determine which fields
    /// are present.
    pub struct_size: u32,

    // -- Fields present since ABI 1.0 --
    pub abi_major: u32,
    pub abi_minor: u32,
    pub name: *const c_char,
    pub version: *const c_char,
    pub description: *const c_char,
    pub factory: extern "C" fn(*const DeviceParamsC) -> *mut c_void,
    pub param_schema: extern "C" fn() -> *const ParamSchemaC,
    pub python_class: *const c_char,

    // -- Fields added in ABI 1.1 (example) --
    // pub hot_reset: Option<extern "C" fn(*mut c_void)>,
}
```

#### Check Protocol

```
1. dlopen(path)
2. Load HELM_DEVICE_ABI_MAJOR symbol from plugin
3. If plugin_major != host_major:
     error: "plugin '{path}' ABI major {plugin_major} != host {host_major} -- recompile"
4. Load HELM_DEVICE_ABI_MINOR symbol from plugin
5. If plugin_minor > host_minor:
     error: "plugin '{path}' requires ABI {plugin_major}.{plugin_minor},
             host supports {host_major}.{host_minor} -- upgrade helm-ng"
6. If plugin_minor < host_minor:
     proceed silently (host has features the plugin does not use)
7. Load helm_device_register symbol
8. Call helm_device_register(&mut DeviceRegistrar)
```

#### Major Mismatch Behavior

```
error[E0001]: Device plugin ABI version mismatch
  --> /opt/helm/lib/libhelm_uart.so
  |
  = plugin ABI major: 2
  = host ABI major:   1
  = help: recompile the plugin against helm-devices 1.x
```

The host refuses to call `helm_device_register`. The `Library` handle is closed. The simulation does not start.

#### Minor Mismatch Behavior

- `plugin_minor <= host_minor`: proceed without warning. The host knows about all fields the plugin uses.
- `plugin_minor > host_minor`: refuse with an error. The plugin was compiled against a newer SDK that added fields the host does not understand.

#### Extension Without Major Bump

1. Append new fields to `DeviceDescriptorC`. Never insert, never reorder.
2. Increment `HELM_DEVICE_ABI_MINOR`.
3. The host reads `struct_size` and zero-initializes any bytes beyond what the plugin populated.
4. Code that accesses the new field checks `desc.struct_size >= offset_of!(DeviceDescriptorC, new_field) + size_of_val(&desc.new_field)` before reading it.

#### Deprecation Lifecycle

1. **Deprecated**: field/function marked `#[deprecated]` in the SDK. Plugin compiles with warning.
2. **Warning period**: one major version. The host still accepts the old field.
3. **Removed**: next major version. The field is removed from the struct. ABI major is incremented.

#### Plugin Exports

```rust
// Plugin .so must export these three symbols:
#[no_mangle]
pub static HELM_DEVICE_ABI_MAJOR: u32 = helm_devices::HELM_DEVICE_ABI_MAJOR;

#[no_mangle]
pub static HELM_DEVICE_ABI_MINOR: u32 = helm_devices::HELM_DEVICE_ABI_MINOR;

#[no_mangle]
pub extern "C" fn helm_device_register(r: &mut DeviceRegistrar) { ... }
```

---

### 3.2 Surface 2: Instrument Plugin ABI (`helm-plugin`)

**New surface.** Instrument plugins observe instruction execution, memory accesses, branches, syscalls, and faults without modifying simulation state.

#### Version Carriers

```rust
// framework/helm-plugin/src/abi.rs

pub const HELM_PLUGIN_ABI_MAJOR: u32 = 1;
pub const HELM_PLUGIN_ABI_MINOR: u32 = 0;
```

#### C-ABI Entry Point

```rust
/// Plugin registration entry point.
///
/// `registry` provides methods to subscribe to callback categories.
/// `args` contains plugin-specific CLI arguments.
#[repr(C)]
pub struct PluginRegistryC {
    pub struct_size: u32,
    pub abi_major: u32,
    pub abi_minor: u32,
    /// Register a callback for instruction execution events.
    pub subscribe_insn_exec: extern "C" fn(*mut c_void, InsnExecCb),
    /// Register a callback for memory access events.
    pub subscribe_mem_access: extern "C" fn(*mut c_void, MemAccessCb),
    /// Register a callback for branch events.
    pub subscribe_branch: extern "C" fn(*mut c_void, BranchCb),
    /// Register a callback for syscall events.
    pub subscribe_syscall: extern "C" fn(*mut c_void, SyscallCb),
    /// Register a callback for fault events.
    pub subscribe_fault: extern "C" fn(*mut c_void, FaultCb),
    // -- New callback types appended here in minor bumps --
}
```

The function-pointer-table pattern means new callback types are additive: append a
new `subscribe_*` function pointer, increment minor, and old plugins that never call
the new function pointer are unaffected.

#### Check Protocol

Identical to Device Plugin ABI (Section 3.1): major must match, plugin minor must not exceed host minor.

#### Callback Signature Stability

Each callback type has a fixed signature:

```rust
pub type InsnExecCb = extern "C" fn(pc: u64, opcode: u32, size: u8);
pub type MemAccessCb = extern "C" fn(pc: u64, addr: u64, size: u8, is_write: bool);
pub type BranchCb = extern "C" fn(pc: u64, target: u64, taken: bool);
pub type SyscallCb = extern "C" fn(nr: u64, args: *const u64, arg_count: u8);
pub type FaultCb = extern "C" fn(pc: u64, fault_type: u32, addr: u64);
```

Changing any callback signature is a major bump. Adding a new callback type is a minor bump.

#### Extension Without Major Bump

New callback types are registered via new function pointers appended to `PluginRegistryC`. The `struct_size` guard (same pattern as Device ABI) prevents old plugins from calling function pointers that do not exist.

---

### 3.3 Surface 3: Python API (`helm_ng` Module)

#### Version Carriers

```python
# python/helm_ng/__init__.py

__version__ = "0.1.0"

def version_manifest() -> dict:
    """Return the version of every API surface in the running helm-ng."""
    return _helm_ng.version_manifest()
```

The Python API version follows the overall `helm_ng` package version (semver). Surface-specific versions are exposed via `version_manifest()`.

#### Check Protocol

There is no automated check at import time beyond standard Python version comparison (`importlib.metadata.version("helm_ng")`). Instead, compatibility is enforced through:

1. **Deprecation warnings** at call sites of deprecated APIs.
2. **Version-gated documentation**: deprecated APIs are documented with "Deprecated since X.Y; use Z instead."
3. **Changelog enforcement**: every PR that changes the Python API must update `CHANGELOG.md`.

#### Deprecation Lifecycle

```python
import warnings

def old_method(self, ...):
    """Deprecated since 0.3.0. Use :meth:`new_method` instead."""
    warnings.warn(
        "old_method() is deprecated since helm_ng 0.3.0; use new_method() instead",
        DeprecationWarning,
        stacklevel=2,
    )
    return self.new_method(...)
```

Timeline:
1. **Version N**: `old_method` is deprecated. Calling it emits `DeprecationWarning`. Functionally identical to `new_method`.
2. **Version N+1**: `old_method` still exists. Warning is promoted to `FutureWarning` ("will be removed in N+2").
3. **Version N+2**: `old_method` is removed. Major version bump.

This provides a two-release deprecation window. Users running with `-W error::DeprecationWarning` catch issues early.

#### Extension Without Major Bump

- New Python classes and functions are additive (minor bump).
- New optional keyword arguments to existing functions are additive (minor bump), provided existing calls without the argument continue to work.
- New enum members in Python-side enums are additive (minor bump).

#### Major Mismatch Behavior

There is no cross-process version check for Python. If a user installs an incompatible version, they get standard Python exceptions (`AttributeError`, `TypeError`). The `version_manifest()` function helps users self-diagnose.

#### Compatibility Module

```python
# python/helm_ng/compat.py
#
# Aliases for renamed APIs during the deprecation window.
# Imported by __init__.py so both old and new names work.

from helm_ng.components import Simulation
# Example: Simulation was previously called Simulator
Simulator = Simulation  # Deprecated alias, removed in next major
```

---

### 3.4 Surface 4: SimObject / Object Model

#### Version Carrier

```rust
// framework/helm-devices/src/object_model.rs

/// SimObject trait version. Incremented when the lifecycle method set or
/// ClassDescriptor layout changes in a breaking way.
pub const OBJECT_MODEL_VERSION: u32 = 1;
```

#### Check Protocol

Checked at `ClassRegistry::global()` initialization time. Every `ClassDescriptor` submitted via `inventory::submit!` carries an `object_model_version: u32` field set by the `helm-devices` crate it was compiled against. On iteration:

```rust
for desc in inventory::iter::<ClassDescriptor> {
    if desc.object_model_version != OBJECT_MODEL_VERSION {
        panic!(
            "ClassDescriptor '{}' was compiled against object model v{}, \
             but the host requires v{}. Recompile the crate containing '{}'.",
            desc.name, desc.object_model_version, OBJECT_MODEL_VERSION, desc.name
        );
    }
    // ... register
}
```

This is a startup check, not a hot-loop check. It fires once at process initialization.

#### Extension Without Major Bump

New lifecycle methods are added with default implementations:

```rust
pub struct ClassDescriptor {
    // ... existing fields ...
    pub init: fn(&mut HelmObject),
    pub finalize: fn(&mut HelmObject, &mut World),
    pub all_finalized: fn(&mut HelmObject, &World),
    pub deinit: fn(&mut HelmObject),

    // Added in object_model v2 -- default is a no-op.
    // Old ClassDescriptors compiled against v1 have this field set to
    // the default fn pointer by zero-initialization + fixup in ClassRegistry.
    pub pre_reset: Option<fn(&mut HelmObject)>,
}
```

The `Option<fn(...)>` pattern allows `ClassDescriptor` instances compiled before the field existed to have `None` (zero-initialized). The host calls the function only if `Some`.

#### Major Mismatch Behavior

Panic at startup with a message naming the offending class and its compiled-against version. The simulation never reaches `World::instantiate()`.

---

### 3.5 Surface 5: Checkpoint Format

**Status quo.** `CheckpointHeader` contains `schema_version: u32 = 1` (Q86). Breaking changes increment it.

#### Version Carrier

```rust
// runtime/helm-debug/src/checkpoint/mod.rs

/// Monotonically increasing. Increment on every breaking checkpoint format
/// change. See Q86 in DESIGN-QUESTIONS.md.
pub const CHECKPOINT_SCHEMA_VERSION: u32 = 1;
```

#### Check Protocol

```rust
impl CheckpointManager {
    pub fn restore(path: &Path, builder: &WorldBuilder) -> Result<World, CheckpointError> {
        let header = Self::read_header(path)?;

        match header.schema_version.cmp(&CHECKPOINT_SCHEMA_VERSION) {
            Ordering::Equal => {
                // Exact match: load normally
                Self::restore_inner(path, builder)
            }
            Ordering::Less => {
                // Checkpoint is older: attempt migration
                Self::migrate_and_restore(path, builder, header.schema_version)
            }
            Ordering::Greater => {
                // Checkpoint is newer than this host: refuse
                Err(CheckpointError::IncompatibleVersion {
                    checkpoint: header.schema_version,
                    simulator: CHECKPOINT_SCHEMA_VERSION,
                    direction: "checkpoint is newer than simulator",
                })
            }
        }
    }
}
```

#### Forward Compatibility Rule

A checkpoint from version N must be loadable by version N+1 (with automatic migration). Version N+2 may reject version N checkpoints, providing a two-version forward-compatibility window. The migration path is: `helm migrate-checkpoint` transforms N to N+1 in place.

#### Serde Defaults for Additive Changes

When a new field is added to a device's checkpoint state, the serde `#[serde(default)]` attribute ensures old checkpoints (which lack the field) deserialize successfully with the default value. This is a minor change that does NOT increment `CHECKPOINT_SCHEMA_VERSION`:

```rust
#[derive(Serialize, Deserialize)]
pub struct Uart16550State {
    pub regs: Uart16550Regs,
    pub irq_asserted: bool,
    /// Added in helm-ng 0.3.0. Old checkpoints deserialize with default (false).
    #[serde(default)]
    pub dma_enabled: bool,
}
```

Only changes that break backward deserialization (field removal, field rename, type change) increment `CHECKPOINT_SCHEMA_VERSION`.

#### Major Mismatch Behavior

```
error[C0001]: Checkpoint version incompatible
  --> /data/checkpoints/sim_20260101.ckpt
  |
  = checkpoint schema_version: 3
  = simulator schema_version:  1
  = help: run `helm migrate-checkpoint /data/checkpoints/sim_20260101.ckpt`
          to upgrade the checkpoint, or use a newer helm-ng version
```

---

### 3.6 Surface 6: HelmEventBus Event Types

#### Version Carrier

```rust
// framework/helm-devices/src/bus/event_bus.rs

/// HelmEvent enum version. Incremented when a variant is removed or its
/// fields change. Adding a new variant to the #[non_exhaustive] enum
/// does NOT increment this -- it is a minor change.
pub const HELM_EVENT_VERSION: u32 = 1;
```

#### The Non-Exhaustive Pattern

```rust
/// Observable events fired on the HelmEventBus.
///
/// This enum is #[non_exhaustive] so that adding new variants is a
/// minor (non-breaking) change. Match arms must include a wildcard.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum HelmEvent {
    Exception     { cpu: ObjectRef, vector: u32, tval: u64, pc: u64 },
    CsrWrite      { cpu: ObjectRef, csr: u16, old: u64, new: u64 },
    ExternalIrq   { cpu: ObjectRef, irq_num: u32 },
    Breakpoint    { cpu: ObjectRef, addr: u64, bp_id: u32 },
    MagicInsn     { cpu: ObjectRef, pc: u64, value: u64 },
    SimulationStop{ reason: StopReason },
    MemWrite      { addr: u64, size: usize, val: u64, cycle: u64 },
    SyscallEnter  { nr: u64, args: [u64; 6] },
    SyscallReturn { nr: u64, ret: u64 },
    DeviceSignal  { device: ObjectRef, port: String, asserted: bool },
    Custom        { name: &'static str, data: Arc<dyn Any + Send + Sync> },
    // New variants appended here; discriminant assignment is implicit
    // and stable (Rust guarantees insertion-order discriminants for
    // fieldless enums; for data-carrying enums, discriminants are
    // compiler-assigned and not serialized).
}
```

#### Check Protocol

For Rust consumers: `#[non_exhaustive]` forces a wildcard arm in `match`. Adding a new variant does not break existing compiled code.

For Python consumers: the PyO3 wrapper maps `HelmEvent` variants to Python strings. Unknown variants are mapped to a catch-all `"Unknown"` string. The Python subscriber can check `event.kind` and ignore unknown kinds.

For instrument plugins (C ABI): events are delivered as a `u32` event_type discriminant + a `#[repr(C)]` event payload. Unknown `event_type` values are silently skipped by the plugin dispatch loop.

#### Extension Without Major Bump

Append new variants. Because `HelmEvent` is `#[non_exhaustive]`:
- Rust match arms require `_ =>` (compiler-enforced).
- Python dispatch uses `if/elif/.../else`.
- C-ABI dispatch uses a `switch` with `default: break`.

#### Major Mismatch Behavior

`HELM_EVENT_VERSION` is embedded in the `PluginRegistryC` struct. If a plugin was compiled against a different event version, the host refuses to deliver events rather than delivering events the plugin cannot parse.

---

### 3.7 Surface 7: Debug Protocol (`helm-debug`)

#### 7a. GDB RSP

The GDB Remote Serial Protocol is externally defined. helm-ng does not version it independently. Instead, compatibility is negotiated per-connection via the `qSupported` packet:

```
Client: $qSupported:swbreak+;hwbreak+#xx
Server: $PacketSize=4000;swbreak+;hwbreak+;qXfer:features:read+#xx
```

The server advertises the features it supports. The client adapts. This is the standard GDB protocol and requires no helm-ng-specific versioning.

**Target description** (`target.xml`) is served via `qXfer:features:read`. The XML describes the register layout for the current ISA. This is per-ISA, not per-helm-version, and follows the GDB target description DTD.

#### 7b. HelmProtocol

`HelmProtocol` is helm-ng's own introspection protocol (JSON-RPC over a Unix domain socket or TCP). It provides commands for querying simulation state, setting breakpoints, inspecting objects, and controlling execution.

#### Version Carriers

```rust
// runtime/helm-debug/src/protocol/mod.rs

pub const HELM_PROTOCOL_MAJOR: u32 = 1;
pub const HELM_PROTOCOL_MINOR: u32 = 0;
```

#### Check Protocol

Handshake at connection time:

```json
// Client sends:
{"jsonrpc": "2.0", "method": "helm.hello", "params": {"protocol_major": 1, "protocol_minor": 0}, "id": 1}

// Server responds:
{"jsonrpc": "2.0", "result": {"protocol_major": 1, "protocol_minor": 2, "helm_version": "0.3.0"}, "id": 1}
```

Rules:
- Major must match. If not, the server returns an error response and closes the connection.
- If `client_minor > server_minor`, the server warns that some commands may not be available.
- If `client_minor <= server_minor`, proceed normally.

#### Extension Without Major Bump

New JSON-RPC methods are additive (minor bump). New optional fields in existing responses are additive (minor bump). Clients must tolerate unknown fields in responses (standard JSON practice).

#### Major Mismatch Behavior

```json
{"jsonrpc": "2.0", "error": {"code": -32001, "message": "Protocol major version mismatch: client=2, server=1"}, "id": 1}
```

Connection is closed after this error.

---

### 3.8 Surface 8: `register_bank!` Macro Output / ParamSchema

`register_bank!` does not have its own independent version number. Its output affects two other versioned surfaces:

1. **Checkpoint format** (Surface 5): the serde layout of the generated struct is part of the checkpoint schema.
2. **Device Plugin ABI** (Surface 1): the generated `MmioHandler` impl is part of the device's ABI contract.

#### ParamSchema Versioning

`ParamSchema` is the list of parameters a device accepts. It is part of the Device Plugin ABI. Changes to `ParamSchema`:

| Change | Version Impact |
|--------|---------------|
| Add a new optional parameter with default | Minor (Device ABI minor bump). Old configs without the param still work. |
| Add a new required parameter | Major (Device ABI major bump). Old configs break. |
| Remove a parameter | Major. Old configs that supply it break. |
| Rename a parameter | Major. Old configs that supply the old name break. |
| Change a parameter's type | Major. Old configs that supply the old type break. |

#### ParamSchema Migration

When a parameter is renamed, the device provides a migration in its `ParamSchema`:

```rust
impl ParamSchema {
    /// Declare that `old_name` was renamed to `new_name`.
    /// If a config supplies `old_name`, it is silently mapped to `new_name`
    /// and a deprecation warning is logged.
    pub fn rename_param(
        mut self,
        old_name: &'static str,
        new_name: &'static str,
    ) -> Self {
        self.renames.push((old_name, new_name));
        self
    }
}
```

During validation, `ParamSchema::validate()` applies renames before checking for unknown parameters. This allows one release of backward compatibility.

#### register_bank! Checkpoint Evolution

When a device adds a new register to its `register_bank!` invocation:

```rust
register_bank! {
    pub struct Uart16550Regs {
        // ... existing registers ...
        reg DMA_CTRL @ 0x08 {  // NEW in v1.1
            field DMA_EN [0];
            field BURST  [3:1];
        }
    }
    device = Uart16550;
}
```

The generated serde impl must use `#[serde(default)]` on the new field so that old checkpoints (without `dma_ctrl`) deserialize with the default value (0). The macro generates this automatically for all register fields.

This is a minor change to the checkpoint schema -- `CHECKPOINT_SCHEMA_VERSION` is NOT incremented because deserialization is backward-compatible.

---

## 4. Cross-Surface Coupling Rules

Not all surfaces are independent. When a change to surface A mechanically forces a change to surface B, they are **coupled**. The following table is exhaustive:

| Surface A Change | Forces Change To | Reason |
|------------------|------------------|--------|
| `register_bank!` generated struct layout change (field add) | Checkpoint (Surface 5) minor | Serde output changes; `#[serde(default)]` maintains compatibility |
| `register_bank!` generated struct layout change (field remove/rename) | Checkpoint (Surface 5) major | Old checkpoints cannot deserialize |
| `ClassDescriptor` layout change | Device Plugin ABI (Surface 1) major | Plugin-compiled descriptors have wrong layout |
| SimObject lifecycle method signature change | Device Plugin ABI (Surface 1) major | Plugin-compiled lifecycle functions have wrong signature |
| SimObject lifecycle method signature change | Object Model (Surface 4) major | Same signature is used by built-in and plugin objects |
| New `HelmEvent` variant | Instrument Plugin ABI (Surface 2) minor | New event type for instrument callbacks |
| `HelmEvent` variant field change | Instrument Plugin ABI (Surface 2) major | Plugin callback receives wrong data |
| `DeviceDescriptor` field add (optional) | Python API (Surface 3) minor | Python device class generation may expose new param |
| `DeviceDescriptor` field remove | Python API (Surface 3) major | Python device classes lose a parameter |

### Coupling Diagram

```
register_bank! output -----> Checkpoint Format (5)
                       \
                        \--> Device Plugin ABI (1)

ClassDescriptor layout -----> Device Plugin ABI (1)
                        \
                         \--> Object Model (4)

HelmEvent variants ----------> Instrument Plugin ABI (2)

Device Plugin ABI (1) -------> Python API (3)  [device class injection]
```

### Non-Coupled Surfaces

The following pairs are explicitly NOT coupled:

- GDB RSP (7a) is externally defined and does not affect any other surface.
- HelmProtocol (7b) is independent of all other surfaces. It reads state but does not define struct layouts.
- Checkpoint Format (5) does not affect Device Plugin ABI (1). A device plugin does not need to know about checkpoint internals.

---

## 5. Version Manifest

### Rust Definition

```rust
/// Complete version manifest for helm-ng.
///
/// Embedded in:
/// - Every checkpoint file (in the CheckpointHeader)
/// - Every plugin at compile time (as a static symbol)
/// - The Python `helm_ng.version_manifest()` return value
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HelmVersionManifest {
    /// Overall helm-ng package version (semver).
    pub helm_version: String,

    /// Device Plugin ABI: major version.
    pub device_abi_major: u32,
    /// Device Plugin ABI: minor version.
    pub device_abi_minor: u32,

    /// Instrument Plugin ABI: major version.
    pub plugin_abi_major: u32,
    /// Instrument Plugin ABI: minor version.
    pub plugin_abi_minor: u32,

    /// SimObject / Object Model version.
    pub object_model: u32,

    /// Checkpoint format schema version.
    pub checkpoint_schema: u32,

    /// HelmEvent enum version.
    pub event_version: u32,

    /// HelmProtocol: major version.
    pub helm_protocol_major: u32,
    /// HelmProtocol: minor version.
    pub helm_protocol_minor: u32,
}

impl HelmVersionManifest {
    /// Build the manifest from the current compile-time constants.
    pub const fn current() -> Self {
        Self {
            helm_version: env!("CARGO_PKG_VERSION").to_string(),
            device_abi_major: HELM_DEVICE_ABI_MAJOR,
            device_abi_minor: HELM_DEVICE_ABI_MINOR,
            plugin_abi_major: HELM_PLUGIN_ABI_MAJOR,
            plugin_abi_minor: HELM_PLUGIN_ABI_MINOR,
            object_model: OBJECT_MODEL_VERSION,
            checkpoint_schema: CHECKPOINT_SCHEMA_VERSION,
            event_version: HELM_EVENT_VERSION,
            helm_protocol_major: HELM_PROTOCOL_MAJOR,
            helm_protocol_minor: HELM_PROTOCOL_MINOR,
        }
    }
}
```

### Checkpoint Embedding

The `CheckpointHeader` is extended to include the full manifest:

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct CheckpointHeader {
    /// Primary compatibility gate.
    pub schema_version: u32,
    /// Full version manifest at save time (informational + migration).
    pub manifest: HelmVersionManifest,
    /// ISA of the simulated system.
    pub isa: String,
    /// Execution mode at checkpoint time.
    pub mode: String,
    /// Unix timestamp when the checkpoint was written.
    pub created_at: u64,
    /// Number of ObjectBlob entries that follow.
    pub object_count: u32,
    /// Total simulated cycles at checkpoint time.
    pub cycle: u64,
    /// CRC32 of all object blobs.
    pub blob_checksum: u32,
}
```

### Plugin Embedding

Every device plugin embeds a manifest as a static symbol:

```rust
// Generated by the plugin SDK template
#[no_mangle]
pub static HELM_DEVICE_MANIFEST: HelmVersionManifest = HelmVersionManifest {
    helm_version: "0.2.0",     // SDK version plugin was compiled against
    device_abi_major: 1,
    device_abi_minor: 0,
    plugin_abi_major: 0,       // not applicable for device plugins
    plugin_abi_minor: 0,
    object_model: 1,
    checkpoint_schema: 1,
    event_version: 1,
    helm_protocol_major: 0,
    helm_protocol_minor: 0,
};
```

The host reads this symbol after loading the `.so` and logs it for diagnostics. The primary compatibility gate remains the `HELM_DEVICE_ABI_MAJOR` / `HELM_DEVICE_ABI_MINOR` symbols (fast integer comparison).

### Python Exposure

```python
>>> import helm_ng
>>> helm_ng.__version__
'0.2.0'
>>> helm_ng.version_manifest()
{
    'helm_version': '0.2.0',
    'device_abi': '1.0',
    'plugin_abi': '1.0',
    'object_model': 1,
    'checkpoint_schema': 1,
    'event_version': 1,
    'helm_protocol': '1.0',
}
```

---

## 6. Compatibility Matrix

### Device Plugin ABI

| Host ABI | Compatible Plugin ABI Range | Action |
|----------|----------------------------|--------|
| 1.0 | 1.0 | Load normally |
| 1.1 | 1.0, 1.1 | 1.0 plugins load (missing features ignored); 1.1 plugins load normally |
| 1.2 | 1.0, 1.1, 1.2 | All minor versions up to host minor are compatible |
| 2.0 | 2.0 | Only major-2 plugins; major-1 plugins refused |

**Policy**: The host supports only the current major ABI version. There is no multi-major-version support. When ABI major is bumped, plugin authors must recompile. A shim library pattern (Section 8) eases this transition.

### Instrument Plugin ABI

Same policy as Device Plugin ABI: current major only, any minor up to host minor.

### Checkpoint Format

| Simulator Schema | Loadable Checkpoint Schemas | Action |
|------------------|-----------------------------|--------|
| 1 | 1 | Load normally |
| 2 | 1, 2 | Schema 1 migrated automatically; schema 2 loaded normally |
| 3 | 2, 3 | Schema 1 rejected (two-version window); schema 2 migrated; schema 3 loaded |
| N | N-1, N | Two-version forward-compatibility window |

**Policy**: The simulator supports checkpoints from the current schema version and one previous version. Older checkpoints must first be migrated with `helm migrate-checkpoint`.

### HelmProtocol

| Server Major.Minor | Compatible Client Major.Minor | Action |
|---------------------|------------------------------|--------|
| 1.0 | 1.0 | Full compatibility |
| 1.2 | 1.0, 1.1, 1.2 | Older clients work (missing commands unavailable) |
| 2.0 | 2.x only | Major-1 clients rejected at handshake |

### Python API

Python follows standard semver. Users pin `helm_ng >= 0.2, < 1.0` in `requirements.txt`. Deprecated APIs survive for two minor versions.

---

## 7. Practical Extension Patterns

### 7.1 `#[repr(C)]` Struct Evolution (Device and Instrument Plugin ABI)

The `struct_size` pattern enables safe struct growth:

```rust
#[repr(C)]
pub struct DeviceDescriptorC {
    pub struct_size: u32,  // Always first. Set to size_of::<Self>().
    pub abi_major: u32,
    pub abi_minor: u32,
    pub name: *const c_char,
    // ... existing fields ...

    // === Added in ABI 1.1 ===
    pub hot_reset: Option<extern "C" fn(*mut c_void)>,
}

// Host-side: reading a field added in a newer minor version
fn read_hot_reset(desc: &DeviceDescriptorC) -> Option<extern "C" fn(*mut c_void)> {
    let field_end = memoffset::offset_of!(DeviceDescriptorC, hot_reset)
        + std::mem::size_of::<Option<extern "C" fn(*mut c_void)>>();
    if (desc.struct_size as usize) >= field_end {
        desc.hot_reset
    } else {
        None  // Plugin predates this field
    }
}
```

### 7.2 Function Pointer Table with Length (Instrument Plugin ABI)

```rust
#[repr(C)]
pub struct PluginRegistryC {
    pub struct_size: u32,
    pub abi_major: u32,
    pub abi_minor: u32,
    // Function pointers for subscribing to callback categories.
    // New callbacks appended; struct_size gates access.
    pub subscribe_insn_exec: extern "C" fn(*mut c_void, InsnExecCb),
    pub subscribe_mem_access: extern "C" fn(*mut c_void, MemAccessCb),
    // ... more subscribe functions ...
}
```

The plugin calls only the `subscribe_*` functions it knows about. It never accesses
function pointers beyond `struct_size`.

### 7.3 `#[non_exhaustive]` on Enums (HelmEventBus)

```rust
#[non_exhaustive]
pub enum HelmEvent {
    Exception { ... },
    // ...
}

// Consumer must write:
match event {
    HelmEvent::Exception { .. } => { /* handle */ }
    HelmEvent::MemWrite { .. } => { /* handle */ }
    _ => { /* unknown variant -- ignore or log */ }
}
```

The `#[non_exhaustive]` attribute is a compile-time guarantee that adding variants
is non-breaking for downstream match arms.

### 7.4 Optional/Defaulted Fields in Serde (Checkpoint)

```rust
#[derive(Serialize, Deserialize)]
pub struct MyDeviceState {
    pub existing_field: u32,

    #[serde(default)]
    pub new_field: u32,  // Old checkpoints deserialize with 0

    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,  // Old checkpoints get 115200
}

fn default_baud_rate() -> u32 { 115200 }
```

### 7.5 Python Deprecation Pattern

```python
import warnings

class Simulation:
    def run_for(self, n_instructions: int) -> StopReason:
        """Run the simulation for n_instructions.

        .. deprecated:: 0.3.0
            Use :meth:`run` with the ``limit`` parameter instead.
        """
        warnings.warn(
            "run_for() is deprecated since 0.3.0; use run(limit=n) instead",
            DeprecationWarning,
            stacklevel=2,
        )
        return self.run(limit=n_instructions)

    def run(self, *, limit: int | None = None) -> StopReason:
        """Run the simulation."""
        ...
```

### 7.6 Optional Lifecycle Hooks (Object Model)

```rust
pub struct ClassDescriptor {
    // Required (always present):
    pub alloc: fn() -> Box<dyn Any + Send>,
    pub init: fn(&mut HelmObject),
    pub finalize: fn(&mut HelmObject, &mut World),
    pub all_finalized: fn(&mut HelmObject, &World),
    pub deinit: fn(&mut HelmObject),

    // Optional (added later -- None for old code):
    pub pre_reset: Option<fn(&mut HelmObject)>,
    pub post_elaborate: Option<fn(&mut HelmObject, &World)>,
}
```

The host calls optional hooks only when `Some`. Code compiled before the hook existed
has `None` (zero-initialized in the `inventory::submit!` macro expansion, or explicitly
set to `None` by the developer).

### 7.7 ParamSchema Evolution

```rust
fn my_param_schema() -> ParamSchema {
    ParamSchema::new()
        .int_default("clock_hz", 1_843_200, "Clock frequency in Hz")
        .int_default("fifo_depth", 16, "FIFO depth")
        // Added in v1.1 -- old configs without this param get the default
        .int_default("dma_burst_len", 4, "DMA burst length (new in v1.1)")
        // Renamed in v1.2 -- old configs using "baud" are silently mapped
        .rename_param("baud", "baud_rate")
        .int_default("baud_rate", 115200, "Baud rate")
}
```

---

## 8. Migration Tooling

### 8.1 Checkpoint Migration CLI

```
helm migrate-checkpoint <file> [--to-version N] [--format cbor|json] [--in-place]
```

**Behavior:**
1. Read the checkpoint header to determine `schema_version`.
2. If `schema_version == target_version` (default: current `CHECKPOINT_SCHEMA_VERSION`), no-op with a message.
3. Apply migration functions sequentially: N -> N+1 -> N+2 -> ... -> target.
4. Write the migrated checkpoint to `<file>.migrated` (or in-place with `--in-place`).

**Migration functions** are registered in a static table:

```rust
// runtime/helm-debug/src/checkpoint/migrations.rs

type MigrationFn = fn(&mut CheckpointHeader, &mut [ObjectBlob]) -> Result<(), MigrationError>;

/// Registry of checkpoint migration functions.
/// migrations[i] transforms schema version i to version i+1.
static MIGRATIONS: &[MigrationFn] = &[
    migrate_v1_to_v2,
    // migrate_v2_to_v3, -- added when CHECKPOINT_SCHEMA_VERSION reaches 3
];

fn migrate_v1_to_v2(header: &mut CheckpointHeader, blobs: &mut [ObjectBlob]) -> Result<(), MigrationError> {
    // Example: a field was renamed from "irq_state" to "irq_asserted" in v2.
    for blob in blobs.iter_mut() {
        let mut attrs: HashMap<String, AttrValue> = ciborium::from_reader(&blob.attrs[..])?;
        if let Some(val) = attrs.remove("irq_state") {
            attrs.insert("irq_asserted".to_string(), val);
        }
        let mut buf = Vec::new();
        ciborium::into_writer(&attrs, &mut buf)?;
        blob.attrs = serde_bytes::ByteBuf::from(buf);
    }
    header.schema_version = 2;
    Ok(())
}
```

### 8.2 Plugin ABI Migration Shim

When the Device Plugin ABI major version is bumped, plugin authors must recompile. For the transition period, helm-ng provides a **shim library** that wraps old-ABI plugins:

```
libhelm_abi_shim_v1.so
```

This shim:
1. Exports `HELM_DEVICE_ABI_MAJOR = 2` (current).
2. Internally loads the old-ABI plugin via `dlopen`.
3. Reads the old-ABI `DeviceDescriptorC` (v1 layout) and translates it to the v2 layout, filling new fields with defaults.
4. Registers the translated descriptor with the host.

Usage:

```python
# Instead of loading the old plugin directly:
# helm_ng.load_plugin("/opt/plugins/old_uart.so")  # would fail: ABI mismatch

# Load via shim:
helm_ng.load_plugin_compat("/opt/plugins/old_uart.so", abi_version=1)
```

The shim is maintained for one major version. When ABI major reaches 3, the v1 shim is removed; a v2 shim is provided.

### 8.3 Python Compatibility Module

```python
# python/helm_ng/compat.py

"""
Compatibility aliases for renamed Python APIs.

This module is imported by __init__.py. Both old and new names work
during the deprecation window. Old names emit DeprecationWarning.
"""

import warnings as _warnings


def _deprecated_alias(old_name: str, new_obj, since: str, removed_in: str):
    """Create a deprecated alias that warns on first use."""
    def wrapper(*args, **kwargs):
        _warnings.warn(
            f"{old_name}() is deprecated since {since} and will be "
            f"removed in {removed_in}. Use {new_obj.__name__}() instead.",
            DeprecationWarning,
            stacklevel=2,
        )
        return new_obj(*args, **kwargs)
    wrapper.__name__ = old_name
    wrapper.__doc__ = f"Deprecated alias for {new_obj.__name__}. See DeprecationWarning."
    return wrapper


# Example aliases:
# Simulator = _deprecated_alias("Simulator", Simulation, "0.3.0", "0.5.0")
```

### 8.4 helm info Command

A diagnostic command that prints all version information:

```
$ helm info
helm-ng 0.3.0

API Surface Versions:
  Device Plugin ABI:      1.2
  Instrument Plugin ABI:  1.0
  Python API:             0.3.0
  Object Model:           1
  Checkpoint Schema:      2
  HelmEvent:              1
  GDB RSP:                (externally defined)
  HelmProtocol:           1.0

Loaded Plugins:
  /opt/helm/lib/libhelm_uart.so
    Device ABI: 1.0  (compatible)
    Manifest helm_version: 0.2.0

Checkpoint Compatibility:
  Can load schemas: 1, 2
  Current schema: 2
```

---

*For the device registry design, see [`helm-devices/LLD-device-registry.md`](./helm-devices/LLD-device-registry.md). For checkpoint format details, see [`helm-debug/LLD-checkpoint.md`](./helm-debug/LLD-checkpoint.md). For the design questions that motivated this document, see Q68, Q86--Q88 in [`DESIGN-QUESTIONS.md`](./DESIGN-QUESTIONS.md).*
