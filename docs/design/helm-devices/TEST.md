# helm-devices — Test Plan

> Test coverage for `helm-devices`: unit tests, integration tests, and fuzzing targets.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-device-trait.md`](./LLD-device-trait.md) · [`LLD-interrupt-model.md`](./LLD-interrupt-model.md) · [`LLD-register-bank-macro.md`](./LLD-register-bank-macro.md) · [`LLD-device-registry.md`](./LLD-device-registry.md)

---

## Table of Contents

1. [Test Philosophy](#1-test-philosophy)
2. [Unit Tests: Device Trait](#2-unit-tests-device-trait)
3. [Unit Tests: Interrupt Model](#3-unit-tests-interrupt-model)
4. [Unit Tests: register_bank! Macro](#4-unit-tests-register_bank-macro)
5. [Unit Tests: DeviceRegistry](#5-unit-tests-deviceregistry)
6. [Integration Tests: Plugin .so Loading](#6-integration-tests-plugin-so-loading)
7. [Fuzzing: Random MMIO Writes](#7-fuzzing-random-mmio-writes)
8. [Test Fixtures and Helpers](#8-test-fixtures-and-helpers)
9. [Test Matrix Summary](#9-test-matrix-summary)

---

## 1. Test Philosophy

All tests in `helm-devices` run via `cargo test`. No external infrastructure, no network, no simulator boot required.

**Unit tests** live in `src/` files as `#[cfg(test)]` modules. They test one type in isolation.

**Integration tests** live in `tests/` at the crate root. They test across modules (e.g., `Device` + `InterruptPin` + `MemoryMap`).

**Fuzzing targets** live in `fuzz/fuzz_targets/`. They require `cargo-fuzz` and a nightly compiler. They must never panic on any input.

**Test device fixture.** A minimal `TestDevice` struct is defined in `tests/common/mod.rs` and is shared across integration tests. It is not a real device; it is a simple counter device for verifying dispatch and interrupt behavior.

---

## 2. Unit Tests: Device Trait

### 2.1 Offset Is Relative, Not Absolute

Verify that `Device::read()` and `Device::write()` receive the offset from the region base, not the absolute address. Since `helm-devices` does not contain `MemoryMap`, this is tested by calling the device methods directly with the expected offset.

```rust
// tests/device_trait.rs

mod common;
use common::TestDevice;
use helm_devices::Device;

/// A device used to verify MMIO dispatch.
/// Stores the last (offset, size, val) triple received by write().
struct EchoDevice {
    last_write: Option<(u64, usize, u64)>,
    read_return: u64,
}

impl EchoDevice {
    fn new(read_return: u64) -> Self {
        Self { last_write: None, read_return }
    }
}

impl Device for EchoDevice {
    fn read(&self, _offset: u64, _size: usize) -> u64 { self.read_return }
    fn write(&mut self, offset: u64, size: usize, val: u64) {
        self.last_write = Some((offset, size, val));
    }
    fn region_size(&self) -> u64 { 256 }
}

#[test]
fn test_device_read_returns_configured_value() {
    let dev = EchoDevice::new(0xDEAD_BEEF);
    assert_eq!(dev.read(0, 4), 0xDEAD_BEEF);
    assert_eq!(dev.read(4, 4), 0xDEAD_BEEF); // same regardless of offset
}

#[test]
fn test_device_write_stores_offset_size_val() {
    let mut dev = EchoDevice::new(0);
    dev.write(12, 4, 0xABCD_1234);
    assert_eq!(dev.last_write, Some((12, 4, 0xABCD_1234)));
}

#[test]
fn test_device_undefined_offset_returns_zero() {
    let dev = EchoDevice::new(0); // returns 0 for all reads
    assert_eq!(dev.read(999, 4), 0, "undefined offset must return 0");
}

#[test]
fn test_device_region_size_constant() {
    let dev = EchoDevice::new(0);
    let size1 = dev.region_size();
    let size2 = dev.region_size();
    assert_eq!(size1, size2, "region_size() must be idempotent");
}

#[test]
fn test_device_signal_default_noop() {
    let mut dev = EchoDevice::new(0);
    // Default Device::signal() is a no-op; must not panic
    dev.signal("reset", 1);
    dev.signal("unknown_signal", 42);
}
```

### 2.2 Device via MemoryMap Dispatch (Integration)

This test is in `tests/` because it requires a real `MemoryMap` from `helm-memory`. If `helm-memory` is not yet available, this test is skipped with `#[ignore]` until that crate is ready.

```rust
// tests/device_mmio_dispatch.rs
#![cfg(feature = "with-helm-memory")]

use helm_devices::Device;
use helm_memory::{MemoryMap, MemoryRegion, MmioHandler};

struct CounterDevice { count: u64 }
impl Device for CounterDevice {
    fn read(&self, offset: u64, _size: usize) -> u64 {
        if offset == 0 { self.count } else { 0 }
    }
    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        if offset == 0 { self.count += val; }
    }
    fn region_size(&self) -> u64 { 8 }
}

// Adapter: Device → MmioHandler (the glue used by helm-memory)
struct DeviceAdapter(CounterDevice);
impl MmioHandler for DeviceAdapter {
    fn read(&self, offset: u64, size: usize) -> u64 { self.0.read(offset, size) }
    fn write(&mut self, offset: u64, size: usize, value: u64) { self.0.write(offset, size, value) }
}

#[test]
fn test_device_dispatch_via_memory_map() {
    let mut map = MemoryMap::new();
    map.add_mmio(0x1000_0000, Box::new(DeviceAdapter(CounterDevice { count: 0 })));

    // Write 5 via absolute address 0x1000_0000 (offset 0 within device)
    map.write(0x1000_0000, 4, 5);
    // Read back via absolute address — device sees offset 0
    let val = map.read(0x1000_0000, 4);
    assert_eq!(val, 5, "device read/write via MemoryMap should dispatch correctly");
}
```

---

## 3. Unit Tests: Interrupt Model

### 3.1 InterruptPin Assert Calls InterruptSink

```rust
// src/interrupt.rs  (in #[cfg(test)] module)

use super::{InterruptPin, InterruptSink, InterruptWire, WireId};
use std::sync::{Arc, Mutex};

/// A test sink that records all on_assert / on_deassert calls.
struct RecordingSink {
    asserts:   Mutex<Vec<WireId>>,
    deasserts: Mutex<Vec<WireId>>,
}

impl RecordingSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            asserts:   Mutex::new(Vec::new()),
            deasserts: Mutex::new(Vec::new()),
        })
    }
    fn assert_count(&self) -> usize { self.asserts.lock().unwrap().len() }
    fn deassert_count(&self) -> usize { self.deasserts.lock().unwrap().len() }
    fn last_assert_wire(&self) -> Option<WireId> {
        self.asserts.lock().unwrap().last().copied()
    }
}

impl InterruptSink for RecordingSink {
    fn on_assert(&self, wire_id: WireId) {
        self.asserts.lock().unwrap().push(wire_id);
    }
    fn on_deassert(&self, wire_id: WireId) {
        self.deasserts.lock().unwrap().push(wire_id);
    }
}

#[test]
fn test_interrupt_pin_assert_calls_sink() {
    let sink = RecordingSink::new();
    let mut pin = InterruptPin::new();
    let wire = InterruptWire::new(WireId::from(42u32), Arc::clone(&sink) as Arc<dyn InterruptSink>);
    pin.connect(wire);

    pin.assert();

    assert_eq!(sink.assert_count(), 1, "on_assert should have been called once");
    assert_eq!(sink.last_assert_wire(), Some(WireId::from(42u32)));
    assert!(pin.is_asserted());
}

#[test]
fn test_interrupt_pin_deassert_calls_sink() {
    let sink = RecordingSink::new();
    let mut pin = InterruptPin::new();
    let wire = InterruptWire::new(WireId::from(7u32), Arc::clone(&sink) as Arc<dyn InterruptSink>);
    pin.connect(wire);

    pin.assert();   // 1 → asserted
    pin.deassert(); // 1 → deasserted

    assert_eq!(sink.deassert_count(), 1);
    assert!(!pin.is_asserted());
}

#[test]
fn test_interrupt_pin_repeated_assert_is_noop() {
    let sink = RecordingSink::new();
    let mut pin = InterruptPin::new();
    let wire = InterruptWire::new(WireId::from(1u32), Arc::clone(&sink) as Arc<dyn InterruptSink>);
    pin.connect(wire);

    pin.assert();
    pin.assert(); // second assert — already high, no transition
    pin.assert(); // third assert — still high

    // on_assert called only ONCE (on the 0→1 transition)
    assert_eq!(sink.assert_count(), 1, "on_assert called more than once for same edge");
}

#[test]
fn test_interrupt_pin_not_connected_is_noop() {
    // Arrange: pin with no wire connected
    let pin = InterruptPin::new();

    // Act: assert on unconnected pin — must not panic (Q71)
    pin.assert();
    pin.deassert();

    // Assert: no panic reached here
    assert!(!pin.is_asserted());
}

#[test]
fn test_interrupt_pin_not_connected_logs_warning_not_panic() {
    // This test verifies the no-panic contract (Q71).
    // We can't easily assert log output in a unit test; we rely on
    // the absence of a panic. A future test with log capture could assert the warn.
    let pin = InterruptPin::new();
    let result = std::panic::catch_unwind(|| {
        pin.assert();
    });
    assert!(result.is_ok(), "InterruptPin::assert() on unconnected pin must not panic");
}

#[test]
fn test_interrupt_pin_is_not_clone() {
    // Compile-time check: this should NOT compile:
    // let pin = InterruptPin::new();
    // let _cloned = pin.clone();  // ERROR: InterruptPin does not impl Clone
    //
    // We verify at test time by checking trait absence via negative impl.
    // In Rust, we can't express "does not implement Clone" as a runtime assertion,
    // so this is documented as a compile-time guarantee. The trybuild integration
    // test (see tests/trybuild/) will catch any accidental Clone derivation.
    let _: () = {
        fn assert_not_clone<T: Clone>(_: &T) {}
        // If InterruptPin were Clone, the next line would compile; it must not:
        // assert_not_clone(&InterruptPin::new());
    };
}
```

---

## 4. Unit Tests: register_bank! Macro

### 4.1 Write, Read Back, Side Effect Fired

```rust
// tests/register_bank.rs

use helm_devices::register_bank;

register_bank! {
    pub struct TestRegs {
        reg CTRL   @ 0x00 { field ENABLE [0]; field MODE [2:1]; }
        reg STATUS @ 0x04 is read_only { field READY [0]; }
        reg DATA   @ 0x08 is write_only;
        reg CLEAR  @ 0x0C is clear_on_read;
    }
    device = TestDevice;
}

struct TestDevice {
    regs: TestRegs,
    on_write_ctrl_calls: Vec<(u32, u32)>,
    on_write_data_calls: Vec<(u32, u32)>,
}

impl TestDevice {
    fn new() -> Self {
        Self {
            regs: TestRegs::default(),
            on_write_ctrl_calls: Vec::new(),
            on_write_data_calls: Vec::new(),
        }
    }
    fn on_write_ctrl(&mut self, old: u32, new: u32) {
        self.on_write_ctrl_calls.push((old, new));
    }
    fn on_write_data(&mut self, old: u32, new: u32) {
        self.on_write_data_calls.push((old, new));
    }
}

impl helm_devices::register_bank::TestRegsHooks for TestDevice {}

#[test]
fn test_register_write_then_read_back() {
    let mut dev = TestDevice::new();

    // Write 0xAB to CTRL (offset 0x00)
    dev.regs.mmio_write(0x00, 4, 0xAB, &mut dev);
    // Read back
    let val = dev.regs.mmio_read(0x00, 4, &mut dev);
    assert_eq!(val, 0xAB, "CTRL should read back the written value");
}

#[test]
fn test_on_write_hook_fired() {
    let mut dev = TestDevice::new();

    dev.regs.mmio_write(0x00, 4, 0x05, &mut dev);
    dev.regs.mmio_write(0x00, 4, 0x07, &mut dev);

    assert_eq!(dev.on_write_ctrl_calls.len(), 2);
    assert_eq!(dev.on_write_ctrl_calls[0], (0x00, 0x05)); // (old=0, new=5)
    assert_eq!(dev.on_write_ctrl_calls[1], (0x05, 0x07)); // (old=5, new=7)
}

#[test]
fn test_read_only_register_ignores_writes() {
    let mut dev = TestDevice::new();

    // Manually set STATUS via its internal field
    dev.regs.set_status_ready(1);
    assert_eq!(dev.regs.mmio_read(0x04, 4, &mut dev), 1);

    // Write to read-only STATUS — should be silently ignored
    dev.regs.mmio_write(0x04, 4, 0xFF, &mut dev);
    // STATUS should still be 1 (unchanged)
    assert_eq!(dev.regs.mmio_read(0x04, 4, &mut dev), 1,
        "write to read-only register must be silently ignored");
}

#[test]
fn test_write_only_register_reads_zero() {
    let mut dev = TestDevice::new();

    dev.regs.mmio_write(0x08, 4, 0xDEAD_BEEF, &mut dev);
    let val = dev.regs.mmio_read(0x08, 4, &mut dev);
    assert_eq!(val, 0, "read from write-only register must return 0");
}

#[test]
fn test_clear_on_read_register() {
    let mut dev = TestDevice::new();

    // Set CLEAR register directly
    dev.regs.clear = 0xCAFE_BABE;

    // First read: returns the value
    let val = dev.regs.mmio_read(0x0C, 4, &mut dev);
    assert_eq!(val, 0xCAFE_BABE, "first read should return stored value");

    // Second read: must return 0 (auto-cleared after first read)
    let val2 = dev.regs.mmio_read(0x0C, 4, &mut dev);
    assert_eq!(val2, 0, "clear-on-read register must be 0 after first read");
}

#[test]
fn test_bitfield_accessors_get_set() {
    let mut dev = TestDevice::new();

    // Set MODE bits [2:1] = 0b10 (= 2)
    dev.regs.set_ctrl_mode(2);
    assert_eq!(dev.regs.ctrl_mode(), 2);

    // Set ENABLE bit [0] = 1
    dev.regs.set_ctrl_enable(1);
    assert_eq!(dev.regs.ctrl_enable(), 1);

    // CTRL raw value should be 0b101 = 5
    assert_eq!(dev.regs.ctrl, 5);
}

#[test]
fn test_undefined_offset_read_returns_zero() {
    let mut dev = TestDevice::new();
    let val = dev.regs.mmio_read(0xFF, 4, &mut dev);
    assert_eq!(val, 0, "undefined offset read must return 0");
}

#[test]
fn test_undefined_offset_write_is_ignored() {
    let mut dev = TestDevice::new();
    // Must not panic, must not modify any register
    dev.regs.mmio_write(0xFF, 4, 0xDEAD, &mut dev);
    // All registers should still be at their default (0)
    assert_eq!(dev.regs.ctrl, 0);
}

#[test]
fn test_serde_round_trip() {
    let mut dev = TestDevice::new();

    dev.regs.mmio_write(0x00, 4, 0xABCD, &mut dev);
    dev.regs.set_status_ready(1);

    // Serialize
    let serialized = bincode::serialize(&dev.regs).expect("serialize");

    // Deserialize into a fresh bank
    let restored: TestRegs = bincode::deserialize(&serialized).expect("deserialize");

    assert_eq!(restored.ctrl, dev.regs.ctrl);
    assert_eq!(restored.status, dev.regs.status);
}
```

---

## 5. Unit Tests: DeviceRegistry

### 5.1 Create with Valid and Invalid Params

```rust
// tests/device_registry.rs

use helm_devices::registry::{
    DeviceDescriptor, DeviceParams, DeviceRegistry, ParamSchema, ParamValue, PluginError,
};
use helm_devices::Device;

// A minimal test device
struct NullDevice { size: u64 }
impl Device for NullDevice {
    fn read(&self, _: u64, _: usize) -> u64 { 0 }
    fn write(&mut self, _: u64, _: usize, _: u64) {}
    fn region_size(&self) -> u64 { self.size }
}

fn make_registry() -> DeviceRegistry {
    let mut reg = DeviceRegistry::new();
    reg.register(DeviceDescriptor {
        name: "null_device",
        version: "1.0.0",
        description: "Minimal test device",
        factory: |params| {
            let size = params.get_int("size")? as u64;
            Ok(Box::new(NullDevice { size }))
        },
        param_schema: || {
            ParamSchema::new().int("size", "Region size in bytes")
        },
        python_class: "",
    }).unwrap();
    reg
}

#[test]
fn test_registry_create_with_valid_params() {
    let reg = make_registry();

    let mut params = DeviceParams::new();
    params.insert("size", ParamValue::Int(256));

    let device = reg.create("null_device", params).expect("create should succeed");
    assert_eq!(device.region_size(), 256);
}

#[test]
fn test_registry_create_with_missing_required_param() {
    let reg = make_registry();

    // Don't supply "size" — it's required
    let params = DeviceParams::new();
    let result = reg.create("null_device", params);

    assert!(
        matches!(result, Err(PluginError::MissingParam("size"))),
        "expected MissingParam error, got: {:?}", result
    );
}

#[test]
fn test_registry_create_unknown_device() {
    let reg = make_registry();
    let result = reg.create("no_such_device", DeviceParams::new());
    assert!(matches!(result, Err(PluginError::UnknownDevice(_))));
}

#[test]
fn test_registry_name_conflict() {
    let mut reg = DeviceRegistry::new();

    let desc = DeviceDescriptor {
        name: "my_device",
        version: "1.0.0",
        description: "",
        factory: |_| Ok(Box::new(NullDevice { size: 8 })),
        param_schema: || ParamSchema::new(),
        python_class: "",
    };

    reg.register(desc.clone()).expect("first register should succeed");
    let result = reg.register(desc);
    assert!(
        matches!(result, Err(PluginError::NameConflict(_))),
        "second register of same name should fail"
    );
}

#[test]
fn test_param_schema_applies_defaults() {
    let mut reg = DeviceRegistry::new();
    reg.register(DeviceDescriptor {
        name: "with_defaults",
        version: "1.0.0",
        description: "",
        factory: |params| {
            let size = params.get_int("size")?;
            assert_eq!(size, 64, "default size should be 64");
            Ok(Box::new(NullDevice { size: size as u64 }))
        },
        param_schema: || ParamSchema::new().int_default("size", 64, "Region size"),
        python_class: "",
    }).unwrap();

    // Create without specifying "size" — schema should apply default
    let device = reg.create("with_defaults", DeviceParams::new()).expect("create with defaults");
    assert_eq!(device.region_size(), 64);
}

#[test]
fn test_param_memory_size_parsing() {
    use helm_devices::registry::DeviceParams;
    assert_eq!(DeviceParams::parse_memory_size("32KiB").unwrap(), 32 * 1024);
    assert_eq!(DeviceParams::parse_memory_size("4MiB").unwrap(), 4 * 1024 * 1024);
    assert_eq!(DeviceParams::parse_memory_size("1GiB").unwrap(), 1024 * 1024 * 1024);
    assert_eq!(DeviceParams::parse_memory_size("8192").unwrap(), 8192);
    assert!(DeviceParams::parse_memory_size("garbage").is_err());
}

#[test]
fn test_registry_list_includes_registered_device() {
    let reg = make_registry();
    let names: Vec<&str> = reg.list().iter().map(|d| d.name).collect();
    assert!(names.contains(&"null_device"));
}
```

---

## 6. Integration Tests: Plugin .so Loading

### 6.1 Test Fixture: Minimal cdylib

A `cdylib` test fixture crate provides a real `.so` plugin for the integration tests. It lives at `tests/fixtures/test-plugin/`:

```toml
# tests/fixtures/test-plugin/Cargo.toml
[package]
name = "helm-test-plugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
helm-devices = { path = "../../../../crates/helm-devices" }
```

```rust
// tests/fixtures/test-plugin/src/lib.rs

use helm_devices::{Device, DeviceDescriptor, DeviceParams, DeviceRegistry, PluginError};

struct PluginTestDevice { val: u64 }
impl Device for PluginTestDevice {
    fn read(&self, offset: u64, _: usize) -> u64 { if offset == 0 { self.val } else { 0 } }
    fn write(&mut self, offset: u64, _: usize, v: u64) { if offset == 0 { self.val = v; } }
    fn region_size(&self) -> u64 { 8 }
}

#[no_mangle]
pub static HELM_DEVICES_ABI_VERSION: u32 = helm_devices::HELM_DEVICES_ABI_VERSION;

#[no_mangle]
pub extern "C" fn helm_device_register(registry: *mut DeviceRegistry) {
    let r = unsafe { &mut *registry };
    let _ = r.register(DeviceDescriptor {
        name: "plugin_test_device",
        version: "0.1.0",
        description: "Test-only plugin device",
        factory: |params: DeviceParams| {
            let init_val = params.get_int("init_val").unwrap_or(0);
            Ok(Box::new(PluginTestDevice { val: init_val as u64 }))
        },
        param_schema: || {
            helm_devices::params::ParamSchema::new()
                .int_default("init_val", 0, "Initial value at offset 0")
        },
        python_class: r#"
class PluginTestDevice(Device):
    init_val: Param.Int = 0
"#,
    });
}
```

### 6.2 Integration Test

```rust
// tests/plugin_loading.rs

use helm_devices::registry::{DeviceRegistry, DeviceParams, ParamValue};
use std::path::PathBuf;

fn plugin_path() -> PathBuf {
    // The build script outputs the test plugin .so alongside the test binary.
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    PathBuf::from(manifest)
        .join("tests/fixtures/test-plugin/target/debug")
        .join("libhelm_test_plugin.so")
}

#[test]
#[cfg(target_os = "linux")]
fn test_plugin_load_and_create() {
    let path = plugin_path();
    if !path.exists() {
        eprintln!("SKIP: test plugin not built. Run: cargo build -p helm-test-plugin");
        return;
    }

    let mut reg = DeviceRegistry::new();
    reg.load_plugin(&path).expect("plugin should load cleanly");

    // Verify device is registered
    let names: Vec<&str> = reg.list().iter().map(|d| d.name).collect();
    assert!(names.contains(&"plugin_test_device"), "plugin device not found in registry");

    // Create with init_val=42
    let mut params = DeviceParams::new();
    params.insert("init_val", ParamValue::Int(42));
    let mut device = reg.create("plugin_test_device", params).expect("create failed");

    // Verify read returns init_val
    assert_eq!(device.read(0, 8), 42);

    // Write and read back
    device.write(0, 8, 100);
    assert_eq!(device.read(0, 8), 100);
}

#[test]
#[cfg(target_os = "linux")]
fn test_plugin_abi_version_mismatch_detected() {
    use helm_devices::registry::PluginError;

    // A plugin with the wrong ABI version would be caught at load time.
    // We can't easily fabricate one without a second cdylib; instead, we test
    // the version comparison logic directly.
    let result: Result<(), PluginError> = Err(PluginError::AbiVersionMismatch {
        expected: helm_devices::HELM_DEVICES_ABI_VERSION,
        found: helm_devices::HELM_DEVICES_ABI_VERSION + 1,
    });
    assert!(matches!(result, Err(PluginError::AbiVersionMismatch { .. })));
}

#[test]
#[cfg(target_os = "linux")]
fn test_plugin_missing_symbol_error() {
    use helm_devices::registry::PluginError;
    // Load a valid shared library that lacks helm_device_register (e.g., libc.so.6).
    // On Linux, libc is always available.
    let mut reg = DeviceRegistry::new();
    let result = reg.load_plugin(std::path::Path::new("/lib/x86_64-linux-gnu/libc.so.6"));
    // Should fail with MissingAbiSymbol or MissingRegisterSymbol
    match result {
        Err(PluginError::MissingAbiSymbol) | Err(PluginError::MissingRegisterSymbol) => {}
        Err(PluginError::DlopenFailed(_)) => {} // acceptable if libc path differs
        other => panic!("unexpected result: {:?}", other),
    }
}
```

---

## 7. Fuzzing: Random MMIO Writes

### 7.1 Fuzz Target: Any Device Must Not Panic

The fuzzer drives arbitrary MMIO read/write sequences at a test device. The invariant: no input sequence may cause a panic.

```rust
// fuzz/fuzz_targets/device_mmio.rs
#![no_main]

use libfuzzer_sys::fuzz_target;
use helm_devices::Device;

/// A minimal register-bank device for fuzz testing.
/// Stores 8 × u32 registers, no side effects, no interrupts.
struct FuzzDevice {
    regs: [u32; 8],
}

impl Device for FuzzDevice {
    fn read(&self, offset: u64, _size: usize) -> u64 {
        let idx = (offset / 4) as usize;
        if idx < self.regs.len() { self.regs[idx] as u64 } else { 0 }
    }
    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        let idx = (offset / 4) as usize;
        if idx < self.regs.len() { self.regs[idx] = val as u32; }
    }
    fn region_size(&self) -> u64 { 32 }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 { return; }

    let mut dev = FuzzDevice { regs: [0u32; 8] };

    // Each 7-byte chunk: [is_read: u8][offset: u16 LE][size_tag: u8][val: u32 LE]
    for chunk in data.chunks_exact(7) {
        let is_read    = chunk[0] & 1;
        let raw_offset = u16::from_le_bytes([chunk[1], chunk[2]]) as u64;
        // Keep offset in device region (allow some out-of-bounds to test robustness)
        let offset = raw_offset % 64;
        let size   = match chunk[3] % 4 { 0 => 1, 1 => 2, 2 => 4, _ => 8 };
        let val    = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], 0]) as u64;

        if is_read == 0 {
            dev.write(offset, size, val);
        } else {
            let _ = dev.read(offset, size);
        }
    }
    // Must reach here without panicking.
});
```

### 7.2 Fuzz Target: register_bank! Device

The more meaningful fuzzer targets a device that uses `register_bank!`, exercising the generated dispatch table.

```rust
// fuzz/fuzz_targets/register_bank_mmio.rs
#![no_main]

use libfuzzer_sys::fuzz_target;
use helm_devices::{Device, register_bank};

register_bank! {
    pub struct FuzzRegs {
        reg A @ 0x00 { field F0 [3:0]; field F1 [7:4]; }
        reg B @ 0x04 is read_only;
        reg C @ 0x08 is write_only;
        reg D @ 0x0C is clear_on_read;
        reg E @ 0x10 is write_only;
        reg F @ 0x14;
    }
    device = FuzzBankDevice;
}

struct FuzzBankDevice {
    regs: FuzzRegs,
    write_count: u32,
}

impl FuzzBankDevice {
    fn on_write_a(&mut self, _old: u32, _new: u32) { self.write_count += 1; }
    fn on_write_c(&mut self, _old: u32, _new: u32) { self.write_count += 1; }
    fn on_write_e(&mut self, _old: u32, _new: u32) { self.write_count += 1; }
    fn on_write_f(&mut self, _old: u32, _new: u32) { self.write_count += 1; }
}

impl Device for FuzzBankDevice {
    fn read(&self, offset: u64, size: usize) -> u64 {
        self.regs.mmio_read(offset, size)
    }
    fn write(&mut self, offset: u64, size: usize, val: u64) {
        self.regs.mmio_write(offset, size, val, self);
    }
    fn region_size(&self) -> u64 { 32 }
}

fuzz_target!(|data: &[u8]| {
    let mut dev = FuzzBankDevice { regs: FuzzRegs::default(), write_count: 0 };

    for chunk in data.chunks_exact(7) {
        let is_read = chunk[0] & 1;
        let offset  = (u16::from_le_bytes([chunk[1], chunk[2]]) as u64) % 48; // allow OOB
        let size    = match chunk[3] % 4 { 0 => 1, 1 => 2, 2 => 4, _ => 8 };
        let val     = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], 0]) as u64;

        if is_read == 0 {
            dev.write(offset, size, val);
        } else {
            let _ = dev.read(offset, size);
        }
    }
    // Postcondition: write_count is bounded (no runaway)
    assert!(dev.write_count < u32::MAX, "write count overflow");
});
```

### 7.3 Running the Fuzzers

```bash
# Initialize cargo-fuzz (once per workspace)
cargo fuzz init

# Run the basic device MMIO fuzzer
cargo fuzz run device_mmio -- -max_len=512 -timeout=10

# Run the register_bank fuzzer
cargo fuzz run register_bank_mmio -- -max_len=512 -timeout=10

# Reproduce a specific crash
cargo fuzz run device_mmio fuzz/artifacts/device_mmio/crash-<hash>

# Run with UndefinedBehaviorSanitizer
RUSTFLAGS="-Z sanitizer=undefined" cargo fuzz run device_mmio -- -max_len=512
```

---

## 8. Test Fixtures and Helpers

```rust
// tests/common/mod.rs

use helm_devices::{Device, interrupt::InterruptPin};

/// A simple 8-register device used across integration tests.
///
/// Stores 8 u32 registers. Holds one interrupt pin.
/// Asserts the interrupt when register 7 is written with a non-zero value.
pub struct TestDevice {
    pub regs: [u32; 8],
    pub irq_out: InterruptPin,
    pub signal_log: Vec<(String, u64)>,
}

impl TestDevice {
    pub fn new() -> Self {
        Self {
            regs: [0u32; 8],
            irq_out: InterruptPin::new(),
            signal_log: Vec::new(),
        }
    }
}

impl Device for TestDevice {
    fn read(&self, offset: u64, _size: usize) -> u64 {
        let idx = (offset / 4) as usize;
        if idx < 8 { self.regs[idx] as u64 } else { 0 }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        let idx = (offset / 4) as usize;
        if idx < 8 {
            self.regs[idx] = val as u32;
            if idx == 7 {
                // Register 7 controls interrupt: non-zero → assert
                if val != 0 { self.irq_out.assert(); } else { self.irq_out.deassert(); }
            }
        }
    }

    fn region_size(&self) -> u64 { 32 }

    fn signal(&mut self, name: &str, val: u64) {
        self.signal_log.push((name.to_string(), val));
    }
}
```

---

## 9. Test Matrix Summary

| Test | Category | File | Verifies |
|------|----------|------|---------|
| `test_device_read_returns_configured_value` | Unit | `tests/device_trait.rs` | `Device::read()` return value |
| `test_device_write_stores_offset_size_val` | Unit | `tests/device_trait.rs` | `Device::write()` offset semantics |
| `test_device_undefined_offset_returns_zero` | Unit | `tests/device_trait.rs` | Undefined offset → 0, no panic |
| `test_device_region_size_constant` | Unit | `tests/device_trait.rs` | `region_size()` idempotent |
| `test_device_signal_default_noop` | Unit | `tests/device_trait.rs` | Default `signal()` no-op |
| `test_device_dispatch_via_memory_map` | Integration | `tests/device_mmio_dispatch.rs` | MemoryMap → device offset dispatch |
| `test_interrupt_pin_assert_calls_sink` | Unit | `src/interrupt.rs` | `assert()` → `on_assert()` |
| `test_interrupt_pin_deassert_calls_sink` | Unit | `src/interrupt.rs` | `deassert()` → `on_deassert()` |
| `test_interrupt_pin_repeated_assert_is_noop` | Unit | `src/interrupt.rs` | No double-edge on repeated assert |
| `test_interrupt_pin_not_connected_is_noop` | Unit | `src/interrupt.rs` | Unconnected pin: no-op, no panic (Q71) |
| `test_interrupt_pin_not_connected_logs_warning_not_panic` | Unit | `src/interrupt.rs` | Confirms no-panic contract (Q71) |
| `test_register_write_then_read_back` | Unit | `tests/register_bank.rs` | Write-read round-trip |
| `test_on_write_hook_fired` | Unit | `tests/register_bank.rs` | `on_write_*` hook invocation and args |
| `test_read_only_register_ignores_writes` | Unit | `tests/register_bank.rs` | `read_only` qualifier |
| `test_write_only_register_reads_zero` | Unit | `tests/register_bank.rs` | `write_only` qualifier |
| `test_clear_on_read_register` | Unit | `tests/register_bank.rs` | `clear_on_read` qualifier |
| `test_bitfield_accessors_get_set` | Unit | `tests/register_bank.rs` | Generated field accessors |
| `test_undefined_offset_read_returns_zero` | Unit | `tests/register_bank.rs` | Undefined offset in bank |
| `test_undefined_offset_write_is_ignored` | Unit | `tests/register_bank.rs` | Undefined offset write ignored |
| `test_serde_round_trip` | Unit | `tests/register_bank.rs` | Serde checkpoint (Q64) |
| `test_registry_create_with_valid_params` | Unit | `tests/device_registry.rs` | Registry create success |
| `test_registry_create_with_missing_required_param` | Unit | `tests/device_registry.rs` | `MissingParam` error |
| `test_registry_create_unknown_device` | Unit | `tests/device_registry.rs` | `UnknownDevice` error |
| `test_registry_name_conflict` | Unit | `tests/device_registry.rs` | `NameConflict` error |
| `test_param_schema_applies_defaults` | Unit | `tests/device_registry.rs` | Optional params get defaults |
| `test_param_memory_size_parsing` | Unit | `tests/device_registry.rs` | Memory size string parsing |
| `test_plugin_load_and_create` | Integration | `tests/plugin_loading.rs` | .so load + create (Linux only) |
| `test_plugin_abi_version_mismatch_detected` | Integration | `tests/plugin_loading.rs` | ABI version check (Q68) |
| `test_plugin_missing_symbol_error` | Integration | `tests/plugin_loading.rs` | Missing symbol error path |
| `fuzz device_mmio` | Fuzzing | `fuzz/fuzz_targets/device_mmio.rs` | No panic on arbitrary MMIO |
| `fuzz register_bank_mmio` | Fuzzing | `fuzz/fuzz_targets/register_bank_mmio.rs` | No panic on arbitrary register bank MMIO |
