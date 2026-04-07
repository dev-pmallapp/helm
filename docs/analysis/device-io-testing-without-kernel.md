# Device IO Testing Without a Full Kernel

## Goal

Investigate:

- what the current `helm-ng` device testing framework actually looks like,
- how other simulators and emulators test device models,
- and what a slick but powerful `helm-ng` approach should be for running IO
  tests without booting a full OS kernel.

## Executive Summary

`helm-ng` is not starting from zero. It already has:

- strong framework and device unit tests,
- a working system-mode MMIO path through the engine,
- and existing kernel-free AArch64 microprogram tests that perform real CPU
  loads/stores against mapped devices.

The missing piece is not capability. The missing piece is a **first-class IO
test harness** that packages those capabilities into a reusable, ergonomic
workflow.

The best direction is:

1. Keep direct unit tests for device-local semantics.
2. Add a reusable **kernel-free IO scenario harness** on top of the existing
   system-mode engine.
3. Support two styles on the same substrate:
   - **scripted bus driving** for direct MMIO/IRQ/tick testing
   - **tiny guest microprograms** for realistic CPU-to-device verification

This is the best of:

- QEMU `qtest` and `qgraph`
- Renode’s scripting and snapshot-heavy test workflow
- Simics’s scripting, inspection, and snapshot tooling

without pulling `helm-ng` into full guest-kernel dependency for device work.

## Current `helm-ng` State

### 1. Device-framework tests are already substantial

The new `framework/helm-devices` crate already has broad unit coverage:

- `MmioBus`, `AhbBus`, `ApbBus`, `I2cBus`, `SpiBus`
- `AddressMap`
- `InterruptPin` / `InterruptSink`
- `IrqRouter`
- `register_bank`
- params and registry

`cargo test -p helm-devices --lib -- --list` currently reports **115 tests**.

Relevant files:

- `framework/helm-devices/src/bus/mmio.rs`
- `framework/helm-devices/src/bus/amba.rs`
- `framework/helm-devices/src/framework/address_map.rs`
- `framework/helm-devices/src/framework/interrupt.rs`
- `framework/helm-devices/src/framework/irq_router.rs`
- `framework/helm-devices/src/framework/register_bank.rs`

What this means:

- the low-level framework is not the weak link,
- but most of this coverage is still **direct object testing**, not
  end-to-end IO scenario testing.

### 2. Device crates also have focused direct tests

Examples:

- `hw/helm-hw-char/src/pl011.rs`
- `hw/helm-hw-intc/src/gicv2/distributor.rs`
- `hw/helm-hw-intc/src/gicv3/distributor.rs`
- `hw/helm-hw-intc/src/gicv3/mod.rs`
- `hw/helm-hw-virtio/src/*`

Observed test counts:

- `helm-hw-char`: **9 tests**
- `helm-hw-intc`: **71 tests**
- `helm-hw-virtio`: **54 tests**

These tests are useful and should stay. They cover:

- register read/write behavior,
- IRQ masking and state transitions,
- queue mechanics,
- backend behavior,
- internal delivery logic.

But they mostly validate devices by calling:

- `read()`
- `write()`
- `transact()`

directly on the device or bus object.

### 3. `helm-ng` already supports kernel-free CPU-driven MMIO tests

This is the key finding.

The engine already has a path for:

- building a synthetic system address space,
- mapping devices,
- loading a few AArch64 instructions,
- running a handful of instructions,
- and asserting on resulting RAM/device state.

Relevant files:

- `runtime/helm-engine/src/lib.rs`
- `runtime/helm-engine/tests/timing_phase1_hooks.rs`
- `runtime/helm-engine/src/fs.rs`
- `runtime/helm-engine/src/platform/arm_virt.rs`
- `runtime/helm-engine/src/address_space.rs`

Important existing helpers:

- `HelmEngine::install_test_aarch64_system_board()`
- `HelmEngine::with_system_memory_mut()`
- `HelmAddressSpace::add_device()`

Concrete evidence already in-tree:

- tests in `runtime/helm-engine/tests/timing_phase1_hooks.rs` load tiny
  instruction sequences like `STR X0, [X2]` and `LDR X1, [X2]`,
- these tests run in **system mode without a guest kernel**,
- they already validate real MMIO behavior and timer event integration.

This means the basic answer to “can Helm do IO tests without Linux?” is:

**yes, today, but the workflow is still ad hoc and test-author-centric.**

### 4. The old `../helm.git` repo had broader test organization, not a solved harness

The previous-generation repo contains a larger device test tree:

- `../helm.git/crates/helm-device/src/tests/bus.rs`
- `../helm.git/crates/helm-device/src/tests/mmio.rs`
- `../helm.git/crates/helm-device/src/tests/pl011.rs`
- `../helm.git/crates/helm-device/src/tests/gic.rs`
- `../helm.git/crates/helm-device/src/tests/virtio/*`
- `../helm.git/crates/helm-device/src/tests/platform*.rs`

That old tree is useful as a pattern library for:

- breadth,
- test shape,
- helper naming,
- bus and platform fixtures.

But it still does not provide the polished, dedicated “device IO scenario
framework” that the current request is asking for.

## What Other Simulators/Emulators Do

## 1. QEMU

### What is strong

QEMU has the cleanest direct analogue to the desired direction.

Its public testing stack separates a few layers:

- **`qtest`**: device-emulation testing without needing a guest OS
- **`libqtest`**: lower-level test API
- **`libqos`**: higher-level helpers for common bus/device-driver tasks
- **`qgraph`**: graph-based machine/driver/test composition

The key ideas worth copying:

- a device test should not need a full guest kernel,
- test code should be able to control virtual time,
- test code should be able to probe machines and devices,
- configuration should be composed rather than hardcoded per test.

### What matters for Helm

QEMU’s strongest idea is not just “send MMIO”.

It is:

> **separate the test harness from guest software and make the harness capable
> of building the machine/device graph automatically.**

That is exactly the missing abstraction in `helm-ng`.

### What not to copy literally

Helm should not blindly copy:

- C/glib-heavy test ergonomics,
- QMP-specific assumptions,
- or the exact qgraph object model.

The useful part is the architecture:

- direct host-driven device tests,
- graph-driven configuration,
- reusable driver-side helpers.

### Concrete case: the June 23, 2016 SMMUv3 qtest patch

The referenced qemu-devel patch,

- subject: `tests: SMMUv3 unit tests`
- date: **June 23, 2016**

is a very useful example because it shows both the strength and the limitation
of the QEMU approach.

What the test does:

- starts a QEMU test instance with `qtest_start(cmd)`
- builds an `aarch64` `virt` machine
- adds an SMMUv3 device
- adds a synthetic PCI test requester device
- allocates guest page tables and STE/CD structures
- programs stage-1, stage-2, and stage-1+stage-2 mappings
- triggers DMA from the PCI test device
- verifies copied data in guest RAM

It also covers a useful translation matrix:

- S1 only
- S2 only
- S1+S2
- 4K pages
- 64K pages
- mixed 4K/64K combinations

Why this is good:

- it exercises the real requester-to-IOMMU-to-memory data path,
- it validates translation tables rather than just register read/write logic,
- it proves DMA-side integration, which is the part that simple CPU MMIO tests
  do not cover.

Why it still feels heavy:

- it still launches a full QEMU **machine model**,
- it hardcodes board knowledge from `virt`,
- it hardcodes an SMMU MMIO base from the board,
- it depends on a synthetic PCI test device,
- and it uses guest RAM and qtest memory accesses as the control surface.

So this is **kernel-free**, but it is not **board-free**.

That distinction matters for `helm-ng`.

For normal MMIO devices, a tiny CPU microprogram harness is often enough.
For an IOMMU like SMMUv3, the more important abstraction is a **synthetic DMA
requester harness**. The QEMU patch gets that right, but pays for it by
standing up a whole `virt` machine.

## 2. Renode

### What is strong

Renode treats testing as a first-class user-facing workflow.

Its test docs show a workflow built around:

- `renode-test`
- Robot Framework integration
- reusable keywords for device interaction
- multi-file test execution
- parallel test execution
- snapshots for failed tests
- interactive debug-on-error workflows

This is a very strong model for test ergonomics.

### What matters for Helm

Renode is the clearest example of what “slick” looks like operationally:

- one command to run tests,
- a declarative scenario format,
- good failure artifacts,
- fast reruns,
- and a clear bridge between scripted control and deep debugging.

Helm does not need Robot Framework specifically.

But it does need the same product mindset:

- **tests should feel like a supported feature, not a private trick used by
  engine developers.**

## 3. Simics

### What is strong

Public Simics docs highlight:

- script-driven configuration,
- checkpoints and in-memory bookmarks,
- non-intrusive processor and device inspection,
- register and memory inspection with and without side effects,
- log-triggered breakpoints,
- and a DML object model with banks, registers, interfaces, connects, ports,
  events, and subdevices.

This is a mature model-development environment.

### What matters for Helm

Simics’s key lesson is:

- **device testing is much easier when the simulator has first-class notions of
  inspection, non-destructive inquiry, side-effectful access, and checkpoints.**

For `helm-ng`, this suggests:

- explicit “inquiry” accessors in tests where useful,
- snapshots/checkpoints for failed IO scenarios,
- good object/device introspection,
- and log/assert hooks that are test-friendly.

## 4. gem5

### What is strong

gem5 has substantial testing infrastructure:

- C++ unit tests,
- Python unit tests,
- system-level tests,
- quick/long/very-long suites,
- batch and rerun support.

It is disciplined and broad.

### What matters for Helm

gem5 is a useful contrast case:

- it has strong infrastructure,
- but its public testing model is more workload/configuration oriented than
  “tight device IO harness” oriented.

For this specific problem, QEMU and Renode are more directly reusable models.

## 5. Bochs

### What is strong

Bochs exposes:

- an internal debugger,
- instruction stepping,
- breakpoints,
- watchpoints,
- memory examination,
- tracing,
- and instrumentation hooks.

### What matters for Helm

Bochs is useful as a reminder that:

- debugging tools are valuable,
- but debugger capability is not the same as a polished device test framework.

It is a good supplement, not the main template.

## 6. higan

### What is strong

higan is explicitly focused on:

- accuracy,
- preservation,
- configurability.

The public repository clearly contains lots of debugger code across subsystems.

### What matters for Helm

From a `helm-ng` device-testing perspective, higan is mostly a contrast case:

- strong accuracy culture,
- but no obvious public, first-class device-test harness comparable to QEMU
  `qtest` or Renode `renode-test`.

That makes it less useful as a direct template for IO workflow design.

## Synthesis: What Helm Actually Needs

The missing layer is:

## A first-class kernel-free IO test harness

It should sit above the current engine/device plumbing and below full-system
Linux boots.

### It should support two modes

#### A. Scripted bus mode

Direct host-side test control:

- map devices,
- read/write MMIO,
- assert IRQ state,
- inject ticks/events,
- inspect device state,
- optionally checkpoint on failure.

This is the `qtest` / Simics-inspection / Renode-keyword side.

Good for:

- register semantics,
- sequencing rules,
- reset behavior,
- timer expiry,
- edge/level IRQ flows,
- PCI config / BAR remap testing,
- virtio queue setup logic.

#### B. Microprogram mode

Tiny guest-issued instruction sequences:

- load a few instructions into RAM,
- point registers at MMIO base addresses,
- run `n` instructions,
- assert guest regs, RAM, and device state.

This is the “real CPU issues the IO” side.

Good for:

- confirming address translation and access width handling,
- endianness and alignment behavior,
- side effects on actual load/store paths,
- realistic CPU/device interaction without Linux.

This mode already partially exists in `runtime/helm-engine/tests/timing_phase1_hooks.rs`.

## Recommended Design For `helm-ng`

### 1. Add a reusable test harness crate/module

Suggested location:

- `runtime/helm-engine/src/test_io_harness.rs`
- or `runtime/helm-engine/tests/common/io_harness.rs`
- or a new small helper crate if reuse across crates becomes heavy

Suggested API shape:

```rust
let mut io = IoTestBoard::aarch64();

let uart = io.map_device(UART_BASE, Box::new(Pl011::new(Box::new(BufferCharBackend::new()))));
io.wire_irq_to_gic_spi(uart, 33);

io.load_words(0x0, &[
    0xD2A00020, // movz x0, #1
    0xF9000040, // str x0, [x2]
]);
io.set_reg_x(2, UART_BASE);
io.run_insns(2);

io.assert_mmio(UART_BASE + 0x40, 4, 1 << 5);
io.assert_irq_pending(33);
```

### 2. Make fixtures explicit and cheap

Have standard builders for:

- minimal AArch64 system board,
- arm-virt flavored board,
- single-device board,
- device + GIC board,
- PCI test board,
- virtio test board.

This prevents every test from reconstructing wiring logic manually.

### 3. Separate assertion layers

The harness should let tests assert on:

- guest architectural state,
- RAM bytes,
- device register values via inquiry,
- side-effectful MMIO behavior,
- IRQ state,
- emitted messages/events,
- optional trace/log output.

### 4. Add snapshot/debug support for failed scenarios

Strongly recommended:

- dump machine/device state on failure,
- optionally preserve RAM and mapped device state,
- optionally emit a repro script or serialized board description.

This is where Renode and Simics have a clear usability advantage.

### 5. Move scenario tests out of device-local files where appropriate

Keep per-device direct tests in the device crate.

Move broader “stories” into a scenario-harness layer:

- PL011 TX/RX/IRQ with guest store/load
- GIC delivery and acknowledge path
- SP804 expiry from engine time progression
- PCI BAR remap across address map and translated memory
- virtio queue notify and interrupt signaling

## If `helm-ng` gets SMMUv3 tomorrow

SMMUv3 is the best example of why a generic “MMIO test harness” is not enough.

For a UART or timer, the interesting behavior is mostly:

- guest CPU writes register
- device mutates internal state
- IRQ or data path changes

For an IOMMU, the interesting behavior is instead:

- a requester issues DMA
- the IOMMU resolves requester identity
- translation tables are walked
- permissions and stage configuration are applied
- memory is accessed or a fault is raised

That means a future `helm-ng` SMMUv3 test stack should be split into:

### 1. SMMUv3 semantic tests

No requester needed beyond a synthetic transaction descriptor.

These should cover:

- command queue parsing
- STE and CD decode
- invalidation behavior
- event/fault queue generation
- field validation and error cases

### 2. Requester-driven translation tests

This is the important middle layer.

The right abstraction is a **dummy requester harness** that can issue DMA
transactions tagged with:

- `stream_id`
- optional `substream_id` / SSID
- direction
- address
- length

This should be usable without a full platform bring-up.

### 3. Optional PCI integration tests

Only this top layer should care about PCI enumeration, BARs, BDF-to-stream
identity plumbing, or PCI-host specifics.

This is where the QEMU SMMUv3 patches are useful:

- one patch adds a simple PCI DMA test device,
- another patch uses it to exercise S1, S2, and S1+S2 translation by
  allocating tables, programming STE/CD state, kicking DMA, and checking the
  copied bytes.

That is a good pattern, but for `helm-ng` it should be decomposed more cleanly.

### Recommended Helm-specific harness shape

#### A. Core harness

```rust
struct SmmuHarness {
    sys: HelmAddressSpace,
    smmu_idx: usize,
    alloc: GuestAlloc,
}

struct DmaRequester {
    stream_id: u32,
    substream_id: Option<u32>,
}
```

Core operations:

- allocate guest memory
- allocate STRTAB, STE, CD, and page tables
- install S1 / S2 / S1+S2 mappings
- submit command queue entries
- inspect event and fault surfaces

Requester operations:

- `dma_read(iova, &mut buf)`
- `dma_write(iova, &buf)`
- `dma_copy(src_iova, dst_iova, len)`

#### B. Optional PCI wrapper

```rust
struct DummyPciRequester {
    bdf: Bdf,
    requester: DmaRequester,
    bar0_base: u64,
}
```

The dummy PCI device should stay intentionally boring:

- registers for `SRC_ADDR`, `DST_ADDR`, `SIZE`, `CMD`, `STATUS`
- a kick command that performs DMA using the requester identity assigned to the
  endpoint

This mirrors the August 2016 QEMU `pci-testdev-smmu` idea, but the PCI wrapper
should remain optional in Helm.

### Example tests Helm should support

#### Translation-success matrix

- S1 only, 4K
- S1 only, 64K
- S2 only, 4K
- S2 only, 64K
- S1+S2, 4K/4K
- S1+S2, 4K/64K
- S1+S2, 64K/4K
- S1+S2, 64K/64K

#### Negative cases

- invalid STE
- invalid CD
- unmapped IOVA
- permission failure on read
- permission failure on write
- wrong StreamID
- stale translation before invalidation

#### Platform-only cases

- PCI endpoint requester identity propagation
- MSI/MSI-X interaction if fault reporting depends on it
- interaction with platform attachment windows or remapping logic

### Example test shape

```rust
#[test]
fn smmuv3_s1_dma_copy_4k() {
    let mut h = SmmuHarness::new();
    let req = h.add_requester(0x20, None);

    let src_pa = h.alloc_bytes(b"abcdef");
    let dst_pa = h.alloc_zeroes(6);

    h.install_s1_mapping(req.stream_id(), 0x1000, src_pa, 0x1000, Perms::Read);
    h.install_s1_mapping(req.stream_id(), 0x2000, dst_pa, 0x1000, Perms::Write);

    req.dma_copy(&mut h.sys, 0x1000, 0x2000, 6).unwrap();

    assert_eq!(h.read_guest(dst_pa, 6), b"abcdef");
    assert!(h.faults().is_empty());
}
```

If PCI coverage is desired:

```rust
#[test]
fn smmuv3_dummy_pci_requester_uses_stream_identity() {
    let mut h = SmmuHarness::with_pci_root();
    let dev = h.add_dummy_pci_requester(Bdf::new(0, 4, 0), 0x20);

    // install mappings...
    dev.kick_dma(0x1000, 0x2000, 128);

    assert_eq!(h.read_guest(dst_pa, 128), expected);
}
```

### Why this is better than copying the QEMU test literally

Copying the QEMU approach literally would make every SMMUv3 test depend on:

- a platform model
- a PCI root complex
- hardcoded machine knowledge
- requester-device discovery logic

That is too heavy for the default development loop.

The right layering is:

- **core SMMU semantics**
- **requester-driven DMA translation**
- **optional PCI/platform integration**

That preserves the value of the QEMU SMMUv3 tests while cutting out the
unnecessary board dependency for most cases.

## Recommended Test Pyramid For Devices

### Layer 1. Device-local semantic tests

Current style, keep it:

- direct `read()` / `write()` / `transact()`
- register field behavior
- state machine edges
- FIFO semantics

### Layer 2. Harness-level IO scenario tests

New main investment:

- mapped device plus minimal board
- no kernel
- either scripted MMIO or microprogram-driven IO

This should become the default layer for “does this device really work in the
machine?” questions.

### Layer 3. Full-system kernel tests

Use sparingly for:

- driver integration,
- discovery,
- interrupts in real software stacks,
- DMA and subsystem interactions that are hard to fake.

These remain necessary, but they should not be the main feedback loop for
device development.

## Concrete Next Steps

### Slice 1

Create a shared IO harness with:

- `IoTestBoard::aarch64_minimal()`
- device mapping
- `load_words()`
- `run_insns()`
- guest register assertions
- MMIO read/write helpers

### Slice 2

Port one existing direct-device scenario to the harness:

- PL011 TX write plus IRQ visibility

### Slice 3

Add IRQ-aware fixture:

- device wired to GIC
- assert pending/ack/eoi behavior

### Slice 4

Add PCI/BAR and virtio helpers:

- config-space write
- remap drain
- queue notify

### Slice 5

Add failure artifact capture:

- board dump
- device snapshot
- optional trace capture

## Bottom Line

The right answer for `helm-ng` is not:

- “just add more unit tests”
- or “boot Linux for everything”

The right answer is:

**promote the existing kernel-free system-mode MMIO capability into a dedicated
IO scenario harness**.

That gives you:

- fast feedback,
- realistic CPU/device coverage,
- minimal guest setup,
- reusable fixtures,
- and a path to a genuinely good developer experience.

## Source Notes

Local repo evidence came from:

- `framework/helm-devices`
- `hw/helm-hw-char`
- `hw/helm-hw-intc`
- `hw/helm-hw-virtio`
- `runtime/helm-engine/tests/timing_phase1_hooks.rs`
- `runtime/helm-engine/src/fs.rs`
- `runtime/helm-engine/src/platform/arm_virt.rs`
- `../helm.git/crates/helm-device/src/tests`

External comparison was derived from public docs and repositories for:

- QEMU
- Renode
- Simics
- gem5
- Bochs
- higan
