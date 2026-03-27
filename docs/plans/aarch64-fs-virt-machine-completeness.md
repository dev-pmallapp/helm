# Plan: AArch64 FS Virt Machine Completeness

> **Status:** Design pass started — 2026-03-21
> **Goal:** Audit `helm-system-aarch64`'s `arm-virt` machine against QEMU `virt` and define a staged path to a meaningfully complete virt machine
> **Reference:** `../helm.git/assets/qemu/hw/arm/virt.c` and `../helm.git/assets/qemu/include/hw/arm/virt.h`
> **Completion gate:** Boot the current Linux guest on a self-described `arm-virt` machine without relying on an externally hand-curated minimal DTB, and have a clear path for virtio/PCI expansion

---

## Current State

`helm-system-aarch64` currently implements a deliberately small subset of QEMU's `virt` machine:

| Area | Current state |
|------|---------------|
| Platform constructor | `runtime/helm-engine/src/platform/arm_virt.rs` builds GICv2 distributor + CPU interface, one PL011, and RAM |
| Platform descriptor | `runtime/helm-platform/src/aarch64/virt.rs` describes only GICD, GICC, and UART0 |
| Boot loader | `runtime/helm-engine/src/loader/arm64_image.rs` loads kernel, DTB, and initramfs into RAM |
| DTB source | `helm-engine` consumes a caller-provided DTB; examples synthesize a minimal DTB with `dtc` in Python |
| Interrupt controller | GICv2 only |
| Firmware interface | no `fw_cfg`, no flash, no ACPI |
| Peripheral surface | no RTC, no GPIO, no second UART, no virtio-mmio transports, no PCIe host |
| PSCI | minimal engine-side subset sufficient for basic SMP bring-up |

This is enough to boot a Linux kernel in a narrow configuration. It is not yet a "complete virt machine" in the QEMU sense.

---

## QEMU Reference Surface

QEMU's `virt` machine is still intentionally minimal, but its minimal set is much broader than helm-ng's current one.

From `../helm.git/assets/qemu/hw/arm/virt.c` and `../helm.git/assets/qemu/include/hw/arm/virt.h`, the low-memory virt map includes:

- flash window at `0x0000_0000 .. 0x07ff_ffff`
- GIC CPU-peripheral region starting at `0x0800_0000`
- GIC distributor, CPU IF, V2M, HYP, VCPU, ITS, redistributor regions
- UART0 at `0x0900_0000`
- PL031 RTC at `0x0901_0000`
- `fw_cfg` at `0x0902_0000`
- GPIO / PL061 at `0x0903_0000`
- UART1 at `0x0904_0000`
- SMMUv3, ACPI GED, pvtime, platform-bus windows
- virtio-mmio transport window starting at `0x0a00_0000`
- PCIe MMIO / PIO / ECAM windows
- RAM at `0x4000_0000`

QEMU also synthesizes the machine description itself:

- root DTB with `/chosen`, `/aliases`, fixed clock, CPUs, timer, GIC, UART, RTC, virtio-mmio, PCIe, and optional nodes
- optional ACPI tables
- optional flash-backed firmware boot path
- machine properties for GIC version, virtio transport count, ACPI, highmem, ITS, and related compatibility knobs

helm-ng currently implements only a thin slice of that contract.

---

## Gap Summary

### 1. The machine surface is too small

Current low-memory device surface:

- GIC distributor
- GIC CPU interface
- UART0

QEMU baseline low-memory virt surface additionally expects:

- RTC
- `fw_cfg`
- GPIO / power button plumbing
- virtio-mmio transports
- reserved flash and PCIe windows even when not fully populated

### 2. The DTB contract lives in Python, not in the machine

Current FS examples generate a temporary DTB in `examples/fs/boot_rpi_full.py` and `examples/fs/virt.py`.

That means:

- the authoritative machine description is outside the Rust platform implementation
- the generated DTB only describes the tiny implemented subset
- the machine cannot evolve safely without manually keeping Python DTB templates in sync

QEMU keeps the machine contract in the machine implementation itself. helm-ng should do the same.

### 3. Existing device models are not fully integrated

There is already a PL031 implementation in `hw/helm-hw-rtc/src/pl031.rs`, but `arm_virt` never wires it into the machine.

This is a different class of gap from missing PCIe or `fw_cfg`:

- some parts are absent from the repo entirely
- some parts exist but are not part of the virt machine

The plan should exploit the second kind first.

### 4. Interrupt-controller completeness is far behind QEMU virt

Current state:

- GICv2 only
- 128 IRQs configured in `build_gicv2_mp(128, ...)`
- no GICv2m MSI frame
- no GICv3 / redistributors / ITS

QEMU virt supports a broader GIC matrix and uses that to back:

- scalable CPU count
- MSI delivery
- more realistic virt-machine DTB layout

### 5. No transport expansion story yet

Current state:

- no virtio-mmio transport window
- no PCIe host bridge / ECAM / MMIO windows
- no platform bus

That blocks the normal virt-machine path for:

- block/network/console devices through virtio-mmio
- PCI/PCIe virtio devices
- later ACPI and hotplug work

### 6. Firmware-facing interfaces are missing

Current state:

- no `fw_cfg`
- no flash device mapping
- no ACPI path

For Linux direct kernel boot this is tolerable.
For "complete virt machine" semantics it is not.

---

## Design Decision

Do **not** aim for full QEMU parity in one step.

Use a staged plan:

1. move the machine contract into Rust
2. integrate already-existing core devices first
3. add the standard non-PCI virt transport surface
4. only then tackle PCIe / firmware / ACPI / advanced GIC work

This matches the repo's current maturity and keeps each milestone testable.

---

## Proposed Phases

## Phase 1: Make the Rust platform authoritative

### Goal

Move the `arm-virt` machine description out of ad hoc Python DTB generation and into the machine implementation.

### Work

- define the QEMU low-memory map explicitly in Rust, not just the currently used subset
- add a Rust DTB builder for the currently supported virt-machine subset
- make `load_aarch64_kernel()` / `setup_arm_virt_boot_with_cpus()` able to use an internally generated DTB when one is not supplied
- keep the DTB limited to implemented devices, but generate it from the machine code path

### Why first

Until the machine owns its own DTB, every additional device risks drift between:

- Rust machine wiring
- Python-generated DTS
- what Linux is told exists

### Completion gate

- FS boot works with no external `--dtb`
- the generated DTB covers CPUs, memory, PSCI, timer, GIC, and UART from Rust alone

---

## Phase 2: Reach the QEMU boot-critical peripheral baseline

### Goal

Add the low-risk, boot-facing devices that QEMU virt includes and that helm-ng can realistically support now.

### Work

- wire `Pl031` at `0x0901_0000`
- extend the generated DTB with PL031
- reserve or describe the `fw_cfg` address range, even if the device itself comes in Phase 4
- define the low-memory windows for flash, virtio-mmio, and PCIe in the platform constants so the machine map matches QEMU's shape

### Notes

This phase should distinguish between:

- "device exists and should now be integrated" like PL031
- "region should exist in the memory map but may remain unimplemented for now" like flash / PCIe

### Completion gate

- Linux sees UART, RTC, timer, GIC, PSCI, memory, and CPUs from the generated DTB
- the machine's address map constants match QEMU's virt low-memory layout for the implemented subset

---

## Phase 3: Add the standard virtio-mmio transport surface

### Goal

Support the normal non-PCI virt-machine expansion path first.

### Work

- reserve and expose the virtio-mmio window starting at `0x0a00_0000`
- instantiate a configurable number of transport slots, following QEMU's `NUM_VIRTIO_TRANSPORTS` model conceptually
- add DTB nodes for `virtio,mmio`
- define a stable IRQ allocation policy for these transports

### Why before PCIe

virtio-mmio is the lowest-complexity path to a useful virt machine:

- no ECAM
- no PCI config space
- no MSI dependency for the first cut
- good enough for block/net/console experimentation

### Completion gate

- a virtio-mmio device can be attached and described in the DTB
- Linux enumerates the transport nodes correctly

---

## Phase 4: Add firmware-facing machine services

### Goal

Implement the pieces QEMU virt uses to hand structured boot metadata to firmware/guests.

### Work

- add a minimal `fw_cfg` MMIO device at `0x0902_0000`
- start with only the entries helm-ng actually needs, such as CPU count and later SMBIOS-style blobs
- define flash-region behavior at `0x0000_0000`
- decide whether helm-ng's first flash milestone is:
  - direct kernel boot only with reserved flash window, or
  - actual pflash device model for UEFI-style flows

### Completion gate

- `fw_cfg` exists as a real MMIO device and appears in the DTB
- flash-region handling is explicit rather than an undocumented hole in the address map

---

## Phase 5: Expand interrupt-controller and machine-version coverage

### Goal

Move beyond the current single-configuration GICv2-only machine.

### Work

- increase baseline IRQ capacity toward QEMU's virt defaults
- decide whether to support GICv2 plus V2M first, or jump directly to GICv3 for future-proofing
- reserve or implement GICv2m / ITS depending on the chosen MSI path
- align DTB GIC reg layout and interrupt descriptions with the selected GIC version

### Recommendation

Support this order:

1. current GICv2 path cleaned up
2. optional GICv2m if MSI is needed before GICv3 exists
3. GICv3 + redistributors once CPU scaling or PCI/MSI work demands it

### Completion gate

- the machine can describe more than one GIC profile intentionally
- DTB generation is version-aware, not hardcoded to the current two-node GIC layout

---

## Phase 6: Add PCIe host support

### Goal

Expose the standard QEMU virt PCIe host bridge and windows.

### Work

- add PCIe MMIO, PIO, and ECAM windows
- implement a generic host bridge / ECAM decode path
- add DTB `pci-host-ecam-generic` node
- route PCIe interrupts through the GIC
- keep IOMMU, hotplug, and ACPI integration out of the first PCIe milestone

### Why late

This is materially more complex than virtio-mmio:

- config-space emulation
- interrupt-map generation
- BAR remapping
- eventual MSI / iommu interactions

### Completion gate

- a simple PCIe device is enumerable through ECAM
- the DTB contains a correct PCI host bridge node and interrupt-map

---

## Phase 7: ACPI, GPIO/power, secure-world, and QEMU-style options

### Goal

Cover the parts of QEMU virt that matter for platform completeness but are not needed for the early Linux boot path.

### Work

- PL061 GPIO and power-button path
- optional second UART
- ACPI GED and ACPI table generation
- secure/non-secure GPIO split only if secure-world support becomes real
- machine properties mirroring useful QEMU knobs:
  - GIC version
  - number of virtio-mmio transports
  - ACPI on/off
  - highmem mode
  - optional dtb randomness equivalent, if ever needed

### Completion gate

- the machine has a coherent policy for DTB vs ACPI boot description
- GPIO / poweroff / reset semantics are described by the machine rather than by ad hoc guest assumptions

---

## File Map

Expected primary files:

| File | Change |
|------|--------|
| `runtime/helm-engine/src/platform/arm_virt.rs` | expand machine map, device wiring, DTB generation entry point |
| `runtime/helm-platform/src/aarch64/virt.rs` | describe the fuller virt topology and reserved windows |
| `runtime/helm-engine/src/loader/arm64_image.rs` | accept internally generated DTBs cleanly |
| `runtime/helm-engine/src/lib.rs` | FS boot plumbing and machine-property wiring |
| `examples/fs/boot_rpi_full.py` | stop being the authoritative DTB source; become a thin launcher |
| `examples/fs/virt.py` | same shift from DTB author to launcher |
| `hw/helm-hw-rtc/src/pl031.rs` | likely no semantic change; integration target |

Potential new modules:

| File | Purpose |
|------|---------|
| `runtime/helm-engine/src/platform/arm_virt_dtb.rs` | Rust DTB builder for the virt machine |
| `hw/.../fw_cfg.rs` | minimal `fw_cfg` MMIO device |
| `hw/.../virtio_mmio.rs` | transport layer if not already available elsewhere |
| `hw/.../pci/...` | later PCIe host implementation |

---

## Test Strategy

### Machine-shape tests

- assert the low-memory map constants match the selected QEMU virt layout
- assert DTB nodes exist for each wired device
- assert IRQ numbers and base addresses match the machine constants

### Boot tests

- boot Linux with internally generated DTB
- boot Linux with SMP > 1 using the machine-generated CPU nodes
- once RTC is integrated, verify the kernel probes PL031

### Transport tests

- virtio-mmio transport nodes enumerate in DTB order
- PCIe host bridge ECAM accesses decode correctly once Phase 6 starts

---

## Non-Goals For The First Milestones

- exact QEMU machine-version compatibility
- ACPI parity with QEMU
- secure-world parity
- full UEFI / flash boot parity
- ITS / MSI completeness before a consumer exists
- QEMU's full compatibility-property matrix

The early goal is "credible virt machine with a self-owned machine contract", not "reimplement all of QEMU virt".

---

## Immediate Next Step

Start with **Phase 1 plus PL031 integration from Phase 2**.

Reason:

- it removes the biggest architectural debt: DTB authority living in Python
- it folds an already-existing device model into the machine
- it improves the virt machine materially without forcing early PCIe or ACPI design decisions
