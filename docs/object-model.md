# helm-ng Object Model

> Cross-references: [`api.md`](./api.md) · [`architecture/python-rust-boundary.md`](./architecture/python-rust-boundary.md)

This page describes the **current public object model** exposed by the
`helm` Python package and how it maps onto the Rust simulation core.

Older design material in the repository sometimes describes a larger
Rust-side `SimObject` trait with elaborate/startup/checkpoint phases.
That is useful historical context, but it is **not** the current public
Python contract. This document focuses on the surfaces that exist on
`main` today.

## Overview

The public object model is centered on a small Python hierarchy:

- `SimObject` is the base class for attachable configuration objects.
- `System` is the root object and simulation controller.
- `Cpu`, `Ram`, `MemorySpace`, `Cache`, `GicV2`, and `Pl011` are the
  currently exposed component types.
- `Simulation` is a backward-compatible alias for `System`.

The important split is:

- Python **describes** a system by attaching child objects to `System`.
- Rust **freezes** that description at `System.instantiate()` and builds
  a concrete `HelmSim`.
- After instantiation, the child graph is immutable.

## SimObject Base Class

`SimObject` is a `#[pyclass(subclass)]` implemented in
`runtime/helm-python/src/simobject.rs`.

Its current responsibilities are intentionally small:

- hold a stable `name`
- collect child objects assigned through attribute syntax
- track whether the object graph is still pending or already instantiated

The user-facing behavior is:

```python
import helm

system = helm.System("virt")
system.cpu = helm.Cpu("cpu0", isa="aarch64")
assert system.cpu.name == "cpu0"
assert system.instantiated is False

system.instantiate()
assert system.instantiated is True
```

After `instantiate()`, modifying the child tree raises `RuntimeError`.

## System Tree

The object model is a **tree of named Python objects**, not a runtime
name lookup API used in the hot loop.

A typical syscall-emulation setup looks like this:

```python
import helm
from helm.aarch64 import A55Cpu

system = helm.System(
    "se-demo",
    timing="interval:interval_len=256,l1d_size=64KiB,l2_size=1MiB",
    mode="se",
)
system.cpu = A55Cpu("cpu0")
system.ram = helm.Ram("ram0", size="512MiB")
```

A typical full-system setup adds an explicit memory map and devices:

```python
import helm
from helm.aarch64 import A55Cpu, GicV2, Pl011

system = helm.System("virt", timing="virtual", mode="fs")
system.cpu = A55Cpu("cpu0")
system.ram = helm.Ram("ram0", size="512MiB")
system.mem = helm.MemorySpace("phys_mem")
system.gic = GicV2("gic", num_irqs=96)
system.uart = Pl011("uart")

system.uart.irq = system.gic.spi(1)

system.mem.add_map(0x4000_0000, system.ram, 512 * 1024 * 1024)
system.mem.add_map(0x0800_0000, system.gic, 0x10000, bank=0)
system.mem.add_map(0x0801_0000, system.gic, 0x10000, bank=1)
system.mem.add_map(0x0900_0000, system.uart, 0x1000)
```

## Instantiation Model

`System.instantiate()` is the one-way transition from Python
configuration to Rust simulation state.

Internally it:

1. walks the attached child objects
2. infers ISA, memory size/base, and execution mode constraints
3. validates memory-map overlap and arm-virt layout rules in FS mode
4. parses the timing string into `TimingChoice`
5. builds a concrete `HelmSim`
6. marks the `SimObject` tree instantiated

What this means in practice:

- before `instantiate()`, you are editing configuration
- after `instantiate()`, you are controlling a live simulator

There is no public Python `elaborate()` phase anymore. The current
public freeze point is `instantiate()`.

## Timing and Cache Configuration

Timing selection belongs to `System`, not to `Cpu`.

Supported strings today are:

```python
"virtual"
"interval"
"interval:interval_len=256,l1d_size=64KiB,l1d_assoc=4,l2_size=1MiB"
"accurate"
```

Important behavior:

- `interval` means interval timing with default L1D/L2 settings
- `interval:...` overrides the live interval cache estimator
- the current timed cache hierarchy is engine-owned and configured
  through `TimingChoice::IntervalTiming`

The generic `Cache` SimObject exists as a descriptor, but the active
interval-timing hierarchy today is driven by the `System.timing` string
or the compatibility `build_simulation(..., timing="interval:...")`
path.

## Memory Model at the Object Layer

Two public shapes exist today:

- `Ram(name, size="512MiB")` for RAM capacity
- `MemorySpace(name)` plus `add_map(base, device, size, bank=0)` for
  explicit address mapping

Instantiation derives memory configuration as follows:

- in SE mode, if no explicit mapping is provided, a default flat RAM
  window is created
- in FS mode, mapped RAM must satisfy the arm-virt layout constraints
- overlapping mappings are rejected before simulation starts

The currently recognized mapped device types are:

- `Ram`
- `GicV2`
- `Pl011`
- unknown device types that fit a platform attachment window

## Device Wiring

The current public device/wiring surface is intentionally narrow.

Exposed ARM device wrappers:

- `GicV2(name, num_irqs=96)`
- `Pl011(name)`

Exposed wiring helper:

- `PortRef(target_name, port_name)`

The current public pattern is:

- obtain an interrupt destination from `gic.spi(n)`
- assign it to a device property such as `uart.irq`
- let `instantiate()` resolve the wiring

Example:

```python
system.gic = helm.GicV2("gic")
system.uart = helm.Pl011("uart")
system.uart.irq = system.gic.spi(1)
```

Broader device loading and plugin-defined Python device classes remain
design work; they are not part of the current `helm` package contract.

## HelmSim Boundary

`System` owns an optional `HelmSim` after instantiation.

On the Rust side:

```rust
pub enum HelmSim {
    VirtualTiming(HelmEngine<VirtualTiming>),
    IntervalTiming(HelmEngine<IntervalTiming>),
    AccurateTiming(HelmEngine<AccurateTiming>),
}
```

`HelmSim` is the boundary object that actually runs the simulation.
`System` is the Python-facing controller wrapped around it.

The most important methods exposed through `System` today are:

- `instantiate()`
- `run(max_insns) -> str`
- `load_elf(binary, argv=None, envp=None)`
- `load_kernel(kernel, dtb=None, dtb_bytes=None, initrd=None, append=None, num_cpus=1, gic_version="v3")`
- `set_cpu_model(name)`
- `set_tick_scale(scale)`
- `add_plugin(name, args="")` (deprecated legacy callback-plugin path; prefer `observe(...)`)
- `observe(...)`
- `spy(...)` (deprecated compatibility alias; prefer `observe(...)` or standalone `helm.HelmSpy(...)`)
- `trace_after(...)` (deprecated legacy plugin instrumentation; prefer probe/session-backed observation)
- `watch(...)` (deprecated alias; prefer `watchpoint(...)`)

## Execution State Exposed to Python

Once instantiated, `System` exposes a small amount of live state:

- `pc`
- `sp`
- `current_sp`
- `insn_count`
- `current_cycles`
- `has_exited`
- `exit_code`
- AArch64 register/state helpers such as `xn(n)`, `vn(n)`, `nzcv`,
  `current_el`, `daif`, `esr_el1`, `far_el1`, `elr_el1`

`run()` returns a string status rather than a Python enum:

- `"quantum"`
- `"exit:<code>"`
- `"exception:<message>"`
- `"unsupported"`
- `"error:not_instantiated"` if called too early

`stats()` returns a compact dictionary with:

- `insn_count`
- `tick_count`
- `virtual_cycles`
- `sim_freq`
- derived `ipc`

## Backward-Compatible Surfaces

Several older entry points still exist for compatibility:

- `Simulation` as an alias for `System`
- `build_simulation(...)` as a direct factory
- `set_sim_trace(...)`

They are still useful, but the preferred public object model is the
explicit `System` tree plus `instantiate()`.

## What Is Not Public Yet

The following concepts appear in older design material but are not part
of the current public Python object model:

- a Python-visible checkpoint / restore API on `System`
- a standalone public `Board` pyclass
- public dynamic device loading through `helm.load_plugin(...)`
- a Python-visible lifecycle of `init() -> elaborate() -> startup()`

Some of that functionality exists only as lower-level Rust
infrastructure or design intent; it should not be documented as an
active public contract.

## Current Mental Model

For users of the public API, the right mental model is:

1. create a `System`
2. attach named child `SimObject`s
3. configure timing via `System.timing`
4. call `instantiate()`
5. load workload (`load_elf` or `load_kernel`)
6. call `run()`
7. inspect `pc`, `current_cycles`, `stats()`, and register helpers

That is the current object model on `main`.
