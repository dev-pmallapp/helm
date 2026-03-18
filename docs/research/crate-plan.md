# Crate Plan

## Goal

Separate:

- framework crates that define stable APIs and shared primitives
- concrete hardware implementation crates that use the framework
- platform composition crates that wire hardware into boards/SoCs
- runtime crates that execute those platforms

This document proposes a repository layout, crate map, dependency graph, and an incremental re-layout plan.

## Workspace Model

Use **one real Cargo workspace** at the repository root.

Do **not** use true nested Cargo workspaces. Cargo is much easier to reason about with a single canonical workspace and domain-oriented subdirectories.

Recommended root `Cargo.toml` pattern:

```toml
[workspace]
resolver = "2"
members = [
  "framework/*",
  "hw/*",
  "platform/*",
  "runtime/*",
]

default-members = [
  "runtime/helm-engine",
  "runtime/helm-cli",
]
```

Recommended top-level directory structure:

```text
helm-ng/
  Cargo.toml

  framework/
    helm-core/
    helm-event/
    helm-memory/
    helm-devices/
    helm-plugin/
    helm-stats/
    helm-timing/

  hw/
    helm-hw-amba/
    helm-hw-gic/
    helm-hw-bcm283x/
    helm-hw-riscv-soc/
    helm-hw-virtio/
    helm-hw-pci/

  platform/
    helm-platform/
    helm-platform-arm-virt/
    helm-platform-rpi3/
    helm-platform-riscv-virt/

  runtime/
    helm-arch/
    helm-engine/
    helm-cli/
    helm-python/

  tools/
  docs/
  examples/
```

## Design Rules

### Rule 1: `helm-devices` is framework-only

`helm-devices` should define:

- `Device`
- interrupt model
- typed ports
- params/schema
- device registry
- event bus
- bus traits and shared protocol types

It should **not** contain concrete UARTs, timers, interrupt controllers, watchdogs, or board definitions.

### Rule 2: concrete devices live in `helm-hw-*`

`helm-hw-*` crates contain reusable hardware/IP implementations.

These crates depend on the device framework and implement real MMIO behavior.

### Rule 3: platforms compose, not implement

`helm-platform-*` crates instantiate and wire concrete devices.

They should own:

- address plans
- IRQ routing plans
- clock/reset topology
- platform defaults
- optional device selection for a board/SoC

They should not own the low-level logic of UARTs, timers, GICs, etc.

### Rule 4: runtime hosts the platform

`helm-engine` and related runtime crates elaborate, execute, and schedule the platform, but do not define the canonical device authoring API.

### Rule 5: split hardware by IP/protocol/vendor family, not ISA

Avoid `helm-devices-arm`, `helm-devices-sparc`, `helm-devices-hppa` as the primary split.

That approach turns crates into architecture junk drawers.

Prefer:

- IP-family crates
- protocol-family crates
- vendor/SoC-family crates

This scales much better when new ISAs reuse existing device blocks.

## Crate Map

### Framework

#### `framework/helm-core`

Purpose:

- universal primitives with minimal dependencies

Examples:

- error types
- common value/attr primitives
- tiny abstract traits safe to use everywhere

Should contain only traits/types that are truly cross-domain.

Candidates to keep or move here:

- `TimerScheduler`
- `DmaPort`
- opaque ids if shared broadly across framework/runtime

Should not contain:

- `Device`
- plugin registry
- bus frameworks
- concrete device abstractions tightly coupled to the device model

#### `framework/helm-event`

Purpose:

- deferred event queue / simulated-time scheduling machinery

#### `framework/helm-memory`

Purpose:

- memory map
- MMIO region dispatch
- RAM/ROM region support
- address-space integration

#### `framework/helm-devices`

Purpose:

- device SDK and reusable device-facing framework

Expected modules:

- `device`
- `interrupt`
- `signal`
- `port`
- `params`
- `registry`
- `bus`
- `event_bus`

#### `framework/helm-plugin`

Purpose:

- simulation instrumentation/plugin APIs and builtins

#### `framework/helm-stats`

Purpose:

- stats, counters, histograms, reporting

#### `framework/helm-timing`

Purpose:

- timing model interfaces and implementations

### Hardware Implementation

#### `hw/helm-hw-amba`

Purpose:

- generic AMBA-style IP blocks and MMIO peripherals

Likely devices:

- PL011
- PL031
- PL061
- SP804
- SP805

Why this split:

- shared style of MMIO register-bank devices
- likely shared conventions for clocks, IRQ pins, and AMBA-facing integration

#### `hw/helm-hw-gic`

Purpose:

- ARM Generic Interrupt Controller family

Likely devices:

- GICv2
- GICv3
- GICv4
- distributor
- redistributor
- ICC/CPU interface
- ITS

Why separate:

- large subsystem
- distinct internal abstractions
- likely different lifecycle and system-register hooks from ordinary peripherals

#### `hw/helm-hw-bcm283x`

Purpose:

- BCM283x / Raspberry Pi family peripherals

Likely devices:

- GPIO
- mailbox
- mini-UART
- system timer

Why separate:

- vendor-specific block family
- useful as a coherent platform pack

#### `hw/helm-hw-riscv-soc`

Purpose:

- RISC-V platform-local MMIO blocks

Likely devices:

- PLIC
- CLINT
- local timers
- simple platform UART if needed

Why separate:

- these are SoC support blocks, not generic reusable protocol devices

#### `hw/helm-hw-virtio`

Purpose:

- virtio devices and shared transports/backends if they are concrete enough

Likely devices:

- virtio-mmio transport
- virtio-pci transport
- rng
- blk
- net
- watchdog
- console

#### `hw/helm-hw-pci`

Purpose:

- concrete PCI host bridge or reusable PCI device implementation layer

Likely contents:

- PCI host bridge
- generic endpoint helpers
- BAR/config-space behavior shared by concrete PCI devices

### Platform Composition

#### `platform/helm-platform`

Purpose:

- common platform composition framework

Likely contents:

- board builder traits
- address reservation helpers
- IRQ wiring plan helpers
- platform descriptor types

#### `platform/helm-platform-arm-virt`

Purpose:

- ARM virt-style machine composition

Would wire together:

- GIC
- AMBA peripherals
- optional virtio transport

#### `platform/helm-platform-rpi3`

Purpose:

- Raspberry Pi 3-style composition

Would wire together:

- BCM283x peripherals
- interrupt infrastructure

#### `platform/helm-platform-riscv-virt`

Purpose:

- RISC-V virt-like composition

Would wire together:

- PLIC
- CLINT
- UART
- virtio devices

### Runtime

#### `runtime/helm-arch`

Purpose:

- ISA decode/execute/arch-state logic

#### `runtime/helm-engine`

Purpose:

- elaboration
- run loop
- event scheduling integration
- MMIO dispatch
- plugin integration
- runtime hosting of platforms

#### `runtime/helm-cli`

Purpose:

- launchers and CLI workflows

#### `runtime/helm-python`

Purpose:

- Python bindings

## Dependency Graph

### Target high-level DAG

```text
helm-core
  ├── helm-event
  ├── helm-memory
  ├── helm-stats
  ├── helm-timing
  ├── helm-devices
  └── helm-plugin

helm-devices
  └── used by all helm-hw-* crates

helm-memory
  └── used by helm-engine and possibly platform composition helpers

helm-hw-amba      ─┐
helm-hw-gic       ─┤
helm-hw-bcm283x   ─┤
helm-hw-riscv-soc ─┤
helm-hw-virtio    ─┤──► helm-platform-* and/or helm-engine
helm-hw-pci       ─┘

helm-platform
  └── common platform composition API

helm-platform-arm-virt   ─┐
helm-platform-rpi3       ─┤──► helm-engine
helm-platform-riscv-virt ─┘

helm-arch   ─┐
helm-plugin ─┤
helm-memory ─┤
helm-event  ─┤
helm-timing ─┤
helm-stats  ─┤
helm-devices─┤
helm-platform-* ───────► helm-engine
```

### Dependency rules

#### `helm-core`

May be depended on by anything.

Should depend on almost nothing external.

#### `helm-devices`

May depend on:

- `helm-core`
- small external support crates needed for framework concerns

Should not depend on:

- `helm-engine`
- concrete hardware crates

#### `helm-hw-*`

May depend on:

- `helm-core`
- `helm-devices`
- narrowly-scoped support framework crates when justified

Should not depend on:

- `helm-engine`
- platform crates

#### `helm-platform-*`

May depend on:

- `helm-core`
- `helm-devices`
- selected `helm-hw-*`
- shared `helm-platform`

Should not depend on:

- `helm-cli`
- `helm-python`

#### `helm-engine`

May depend on:

- all framework crates
- `helm-arch`
- selected `helm-platform-*`

This is the integration host and can sit high in the DAG.

## Concrete Device Placement Guidance

### Direct MMIO devices

These are ordinary `Device` implementations in `helm-hw-*` crates:

- timers
- watchdogs
- UARTs
- GPIO
- mailbox
- RTC
- local interrupt blocks
- sysregs
- GIC CPU/distributor/redistributor blocks

They do **not** need to conform to any separate bus abstraction if they are mapped directly in the system MMIO space.

### Bus devices

These are only for real bus controllers or bus-attached endpoints:

- PCI host bridge
- I2C controller or I2C-attached devices
- SPI controller or SPI-attached devices
- virtio transport layers

Do not force all SoC peripherals through `BusDevice`.

## Why Not Architecture Buckets

Bad split:

- `helm-devices-arm`
- `helm-devices-sparc`
- `helm-devices-hppa`

Problems:

- crates grow into mixed bags of unrelated IP blocks
- the same UART/timer/watchdog family may be reused by multiple ISAs
- future hardware reuse becomes awkward

Better split:

- by reusable IP family
- by protocol family
- by vendor/SoC family

This allows future SPARC or HPPA platforms to reuse:

- `helm-hw-amba`
- `helm-hw-pci`
- `helm-hw-virtio`

and add only new focused crates if they truly need unique hardware blocks.

## Re-layout Plan

### Phase 0: clarify boundaries in docs

1. Confirm that `helm-devices` is framework-only.
2. Confirm that concrete device implementations belong in `helm-hw-*`.
3. Confirm that `helm-platform-*` owns composition, not implementation.

Deliverable:

- agreed crate boundaries

### Phase 1: move directories, not behavior

1. Introduce the new top-level folders:
   - `framework/`
   - `hw/`
   - `platform/`
   - `runtime/`
2. Move existing crates into those directories without API redesign.
3. Update root workspace members.

Deliverable:

- same code, new physical layout

This keeps changes mechanical and easy to review.

### Phase 2: stabilize `framework/helm-devices`

1. Keep `helm-devices` focused on framework modules only.
2. Remove concrete-device drift from that crate.
3. Define stable interfaces for:
   - device MMIO contract
   - interrupt pins and sinks
   - ports
   - params
   - registry
   - bus traits

Deliverable:

- usable device SDK crate

### Phase 3: create first hardware crates

Recommended first crates:

1. `hw/helm-hw-amba`
2. `hw/helm-hw-gic`
3. `hw/helm-hw-bcm283x`

Port devices from old implementations into those crates.

Deliverable:

- first concrete hardware packs compiled against the new framework

### Phase 4: create platform crates

Recommended first platform crates:

1. `platform/helm-platform`
2. `platform/helm-platform-arm-virt`
3. `platform/helm-platform-riscv-virt`

Use these crates to encode:

- address maps
- IRQ routes
- device inventory
- board defaults

Deliverable:

- platform composition separate from device logic

### Phase 5: integrate runtime

1. Update `helm-engine` to instantiate platform crates rather than hand-owning all topology.
2. Keep `helm-python` and `helm-cli` as thin frontends over engine/platform APIs.

Deliverable:

- engine hosts platform, platform hosts hardware, hardware uses framework

### Phase 6: optional convenience crates

Only if needed:

- `hw/helm-hw-all` re-export crate for tests/demos
- `platform/helm-platform-all` convenience registry

These should be optional and thin.

## Migration Notes

### Old `helm-device` source mapping

Likely mapping from older monolithic device code:

- generic device abstractions -> `framework/helm-devices`
- AMBA peripherals -> `hw/helm-hw-amba`
- GIC family -> `hw/helm-hw-gic`
- BCM peripherals -> `hw/helm-hw-bcm283x`
- virtio family -> `hw/helm-hw-virtio`
- platform builders -> `platform/helm-platform-*`

### Risk control

Keep physical moves separate from semantic refactors.

Good sequence:

1. move crate directories
2. fix paths/workspace members
3. keep tests passing
4. only then split code and APIs

That avoids mixing repository churn with architectural redesign.

## Recommended First Step

Do this first:

1. create the new directory layout
2. move existing crates into `framework/` and `runtime/`
3. leave `hw/` and `platform/` empty placeholders initially

Then:

1. make `framework/helm-devices` stable
2. create `hw/helm-hw-amba`
3. create `hw/helm-hw-gic`
4. begin porting concrete devices into those crates

This gives the repository a durable shape before large implementation ports begin.
