# cursor-platform-python -- Codebase Audit

Date: 2026-04-07

## Summary

`helm-platform` is a clean metadata crate that defines the `Platform` trait,
address constants, attachment slots, and build plans. It correctly avoids
depending on `helm-engine`. `helm-python` is the PyO3 boundary that
translates Python config objects into Rust `HelmSim` instances. The most
significant issues are thick coupling in `instantiate.rs` (which directly
imports all HW crate types for PCI/VirtIO device construction), the `Cpu`
class exposing OoO microarchitecture parameters that are not consumed by the
engine, hardcoded built-in platform defaults, and `PlatformError` using
manual `Display`/`Error` instead of `thiserror`.

---

## Design Issues

### D1. `instantiate.rs` does platform-specific PCI/VirtIO bring-up

**Severity: High**

The Python binding module directly imports and constructs PCI and VirtIO
device types:

```rust
// runtime/helm-python/src/instantiate.rs:3-15
use helm_engine::platform::arm_virt::install_arm_virt_pci_bar_device;
use helm_hw_pci::{build_pci_bar0_endpoint, build_pci_ram_bar_pair, Bdf, PciBus};
use helm_hw_virtio::blk::VirtioBlk;
use helm_hw_virtio::console::VirtioConsole;
use helm_hw_virtio::net::VirtioNet;
use helm_hw_virtio::pci::build_virtio_pci_rng_pair;
use helm_hw_virtio::proto::transport::VirtioMmioTransport;
use helm_hw_virtio::proto::virtqueue::RamBlockBackend;
use helm_hw_virtio::rng::VirtioRng;
```

The Python layer is doing platform-specific device construction, not just
config translation. This means:
- Adding any new PCI/VirtIO device type requires changing `helm-python`
- `helm-python` has direct compile-time dependencies on all HW crates
- The config-to-engine boundary is thick with device-specific logic

**Suggested fix:** Move device construction into `helm-engine` or
`helm-platform`. `instantiate.rs` should pass a `DiscoveredDeviceList` to
the engine and let the engine (or platform) handle concrete construction.

---

### D2. `Cpu` exposes OoO microarchitecture parameters not consumed by the engine

**Severity: Medium**

```rust
// runtime/helm-python/src/cpu.rs:8-23
pub struct Cpu {
    pub isa: String,
    pub model: String,
    pub width: u32,
    pub rob_size: u32,    // reorder buffer size
    pub iq_size: u32,     // issue queue size
    pub lq_size: u32,     // load queue size
    pub sq_size: u32,     // store queue size
}
```

These fields (`rob_size`, `iq_size`, `lq_size`, `sq_size`) are exposed to
Python users with defaults (128, 64, 32, 32) but there is no evidence they
are consumed by `IntervalTiming` or any other engine component. Users may
set these expecting behavioral impact and get none.

**Suggested fix:** Either wire these parameters into `IntervalTiming` /
`AccurateTiming` or remove them from the Python API. If they are forward-
looking for Phase 3, document them as "reserved for future use".

---

### D3. `maybe_realize_builtin_platform` hardcodes 1 vCPU + GICv3

**Severity: Medium**

The built-in platform realization path in `helm-engine` defaults to 1 vCPU
and GICv3. There is no way for the Python config to override the vCPU count
or GIC version when using the built-in platform path.

**Suggested fix:** Read vCPU count and GIC version from the `FrozenSimulatorConfig`
or `BuiltInFreezeDefaults` and pass them to `install_arm_virt_board`.

---

### D4. `helm-platform` cannot construct machines (by design)

**Severity: Low**

The platform crate defines topology and address constants but actual device
construction stays in `helm-engine` (because it depends on engine-internal
types like `HelmAddressSpace` and `FsState`). This is documented at
`runtime/helm-platform/src/aarch64/virt.rs:1-6`.

Each new platform would duplicate the bring-up pattern in `helm-engine`,
increasing coupling. Acceptable for one platform, problematic if more are
added.

---

### D5. `PlatformError` manual `Display` / `Error` implementation

**Severity: Low**

```rust
// runtime/helm-platform/src/lib.rs:44-71
#[derive(Debug)]
pub enum PlatformError {
    DeviceCreation(String),
    SlotFull { slot: String },
    ConfigFrozen,
    Other(String),
}
impl std::fmt::Display for PlatformError { ... }
impl std::error::Error for PlatformError {}
```

Inconsistent with other crates that use `thiserror`.

---

## Correctness Issues

### C1. `instantiate_system` relies on string matching for mode/timing

**Severity: Low**

```rust
// runtime/helm-python/src/system.rs:18-26
pub(crate) fn parse_mode(s: &str) -> PyResult<ExecMode> {
    match s {
        "se" | "syscall" => Ok(ExecMode::Syscall),
        "functional" | "fe" => Ok(ExecMode::Functional),
        "fs" | "system" => Ok(ExecMode::System),
        other => Err(...)
    }
}
```

String parsing is the correct approach for the Python boundary, but the
lack of a canonical mode name means users can write `"syscall"` or `"se"`
interchangeably. Documentation should recommend one canonical form.

### C2. `FrozenPythonSystemConfig` holds discovered device lists by value

**Severity: Low**

```rust
// runtime/helm-python/src/instantiate.rs:34-42
struct FrozenPythonSystemConfig {
    frozen: FrozenSimulatorConfig,
    pci_ram_bars: Vec<DiscoveredPciRamBar>,
    pci_virtio_rng_mmio: Vec<DiscoveredPciVirtioRngMmio>,
    pci_virtio_rng: Vec<DiscoveredPciVirtioRng>,
    pci_virtio_blk: Vec<DiscoveredPciVirtioBlk>,
    pci_virtio_net: Vec<DiscoveredPciVirtioNet>,
    pci_virtio_console: Vec<DiscoveredPciVirtioConsole>,
}
```

Every new device type requires extending this struct. A generic
`Vec<Box<dyn DiscoveredDevice>>` or `HashMap<String, Vec<Box<dyn Any>>>`
would be more extensible, though at the cost of type safety.

---

## Completeness Issues

### P1. Only one built-in platform: `arm-virt`

```rust
// runtime/helm-platform/src/lib.rs:117-123
pub fn list_platforms() -> Vec<PlatformInfo> {
    vec![PlatformInfo {
        name: "arm-virt",
        description: "QEMU-compatible ARM virt machine (GICv2/v3, PL011 UART)",
        isa: "aarch64",
    }]
}
```

No RISC-V platform is registered despite RV64 SE/FS being in progress.
This is expected (Phase 2 focus), not a bug.

### P2. No Python-level checkpoint/restore API

The `HelmSystem` Python class exposes `run()`, `stats()`, `pc`, `insn_count`,
`current_cycles`, and register helpers, but no checkpoint save/restore
workflow. Documented as follow-on work.

### P3. `build_simulation()` is backward-compat only

The older factory function exists alongside `System(...).instantiate()`.
Both paths should produce identical results but the compat path may lag
behind as new features are added.

---

## Software Engineering Issues

### E1. `#![allow(missing_docs)]` on both crates

Both `helm-python/src/lib.rs` and per-module files suppress missing-docs.
PyO3 docstrings serve as Python-facing documentation, but the Rust-side
API is largely undocumented.

### E2. Broad clippy suppression in `helm-python`

```rust
// runtime/helm-python/src/lib.rs:14-20
#![allow(
    missing_docs,
    clippy::redundant_closure,
    clippy::semicolon_if_nothing_returned,
    clippy::unused_self,
    clippy::useless_conversion
)]
```

Some of these (especially `unused_self`) may hide real issues in PyO3 method
implementations.

### E3. Test coverage in `instantiate.rs`

Instantiation tests exist (verify timing parsing, mode parsing, basic
`instantiate()` flow) but do not cover the PCI/VirtIO device attachment
paths end-to-end.

### E4. Device discovery modules are per-device-type

`discovery.rs` has separate functions for each device type
(`discover_pci_ram_bars`, `discover_pci_virtio_blk`, etc.). Adding a new
device type requires adding a discovery function, a `Discovered*` struct,
a field on `FrozenPythonSystemConfig`, and wiring in `instantiate_system`.

---

## Architecture Issues

### A1. `helm-python` depends on all HW crates

Through `instantiate.rs`, the Python binding crate has compile-time
dependencies on `helm-hw-pci`, `helm-hw-virtio`, and indirectly on their
transitive deps. This makes the Python module a bottleneck for recompilation.

### A2. No circular dependencies

`helm-python` → `helm-engine` → framework crates.
`helm-python` → `helm-platform` (metadata only, no engine dep).
`helm-platform` does not depend on `helm-engine`.

### A3. Discovery → instantiation → engine pipeline

The flow is: Python objects → `discover_children` → `FrozenPythonSystemConfig`
→ `build_simulator_from_request` → `HelmSim`. This is clean conceptually
but the "freeze" step (`instantiate.rs`) is where all device-specific logic
concentrates, making it the widest coupling point.

---

## Idiomatic Rust Issues

### I1. `PlatformError` should use `thiserror`

See D5. Same pattern as `PowerError` in `helm-core`.

### I2. String-based ISA/mode/timing selection

The Python API uses strings (`"aarch64"`, `"se"`, `"virtual"`) which are
parsed into enums. This is idiomatic for PyO3 boundaries where Python
doesn't have access to Rust enums. The parsing is correct and has good
error messages.

### I3. `Cpu` default values in `#[pyo3(signature)]`

```rust
// runtime/helm-python/src/cpu.rs:28-29
#[pyo3(signature = (name, *, isa="aarch64", model="cortex-a55",
                    width=4, rob_size=128, iq_size=64, lq_size=32, sq_size=32))]
```

The defaults are reasonable but undocumented in the Python docstring. Users
see them in function signatures but not in help text.

---

## Recommendations

### Quick Wins (< 1 hour each)

1. **Use `thiserror` for `PlatformError`** (D5, I1)
2. **Document canonical mode names** ("se", "fs", "functional") in Python API (C1)
3. **Add Python docstrings for `Cpu` defaults** (I3)
4. **Document `rob_size`/`iq_size` etc as "reserved for future use"** or remove (D2)
5. **Narrow clippy suppressions in `helm-python`** (E2)

### Medium Effort (1-4 hours each)

6. **Wire vCPU count through `FrozenSimulatorConfig`** for built-in platforms (D3)
7. **Add integration tests for PCI/VirtIO device attachment** in `instantiate.rs` (E3)
8. **Register RISC-V platform stub** in `list_platforms()` (P1)

### Structural (> 4 hours)

9. **Move device construction out of `helm-python`** into engine/platform (D1)
10. **Generalize device discovery** with a trait-based pattern (E4, C2)
11. **Design Python checkpoint/restore API** (P2)
