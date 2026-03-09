# PCI Subsystem, VirtIO-over-PCI, and Accelerator Device Design

Date: 2026-03-09

## Goals

1. Full PCIe stack: ECAM host bridge, Type 0 config space, BARs, MSI-X, AER, ACS
2. VirtIO PCI transport alongside existing MMIO transport, user selects via Python `transport='pci'|'mmio'`
3. LLVM-IR accelerators as PCI devices, created from Python with `ir_file=` or `ir=`
4. PCI host bridge added to arm-virt memory map
5. DTB generation for PCI host bridge node

## Approach: Trait-Heavy Clean Abstraction (Approach C)

Maximizes extensibility through well-defined trait boundaries. Four core traits compose cleanly; concrete capability types are pluggable.

---

## 1. Trait Hierarchy

### PciCapability

Pluggable PCI/PCIe capability (MSI-X, AER, ACS, PM, PCIe). Capabilities occupy a range in config space; the config engine dispatches reads/writes to the owning capability.

```rust
pub trait PciCapability: Send + Sync {
    fn cap_id(&self) -> u8;
    fn offset(&self) -> u16;
    fn length(&self) -> u16;
    fn read(&self, offset: u16) -> u32;
    fn write(&mut self, offset: u16, value: u32);
    fn reset(&mut self);
    fn name(&self) -> &str;
}
```

### PciFunction

Unit of device identity on the PCI bus. Every PCI device (VirtIO, accelerator, NIC) implements this. The host bridge manages the Type 0 header; the function handles device-specific config and BAR MMIO.

```rust
pub trait PciFunction: Send + Sync {
    fn vendor_id(&self) -> u16;
    fn device_id(&self) -> u16;
    fn class_code(&self) -> u32;
    fn subsystem_vendor_id(&self) -> u16 { 0 }
    fn subsystem_id(&self) -> u16 { 0 }
    fn revision_id(&self) -> u8 { 0 }

    fn bars(&self) -> &[BarDecl; 6];
    fn capabilities(&self) -> &[Box<dyn PciCapability>];
    fn capabilities_mut(&mut self) -> &mut [Box<dyn PciCapability>];

    fn bar_read(&mut self, bar: u8, offset: u64, size: usize) -> u64;
    fn bar_write(&mut self, bar: u8, offset: u64, size: usize, value: u64);
    fn config_read(&self, offset: u16) -> u32 { 0 }
    fn config_write(&mut self, offset: u16, value: u32) {}

    fn reset(&mut self);
    fn tick(&mut self, cycles: u64) -> Vec<DeviceEvent> { vec![] }
    fn name(&self) -> &str;
}
```

### BarDecl

```rust
pub enum BarDecl {
    Unused,
    Mmio32 { size: u64 },
    Mmio64 { size: u64 },
    Io { size: u32 },
}
```

### VirtioTransport

Abstracts MMIO vs PCI for VirtIO device backends.

```rust
pub trait VirtioTransport: Device {
    fn backend(&self) -> &dyn VirtioDeviceBackend;
    fn backend_mut(&mut self) -> &mut dyn VirtioDeviceBackend;
    fn raise_irq(&mut self);
    fn raise_config_irq(&mut self);
    fn transport_type(&self) -> &str;
}
```

---

## 2. Config Space Engine

`PciConfigSpace` manages the 4KB PCIe extended config space for one function.

- Offsets 0x00-0x3F: Type 0 header (vendor/device ID RO, command/status, BAR sizing protocol, interrupt line/pin)
- Offsets 0x40-0xFF: Legacy capabilities (linked list, dispatched to PciCapability impls)
- Offsets 0x100+: Extended capabilities (AER, ACS)

Write masks prevent guest writes to read-only fields. BAR sizing protocol (write all-1s, read back size) handled internally.

---

## 3. Capability Implementations

| Capability | ID | Location | Purpose |
|---|---|---|---|
| PcieCapability | 0x10 | Legacy | Link/device/slot status and control |
| MsixCapability | 0x11 | Legacy | MSI-X table + PBA in BAR space, GIC delivery via IrqSignal |
| PmCapability | 0x01 | Legacy | Power management |
| AerCapability | 0x0001 | Extended (0x100+) | Advanced error reporting |
| AcsCapability | 0x000D | Extended | Access control services |
| VirtioPciCap | 0x09 (vendor) | Legacy | VirtIO PCI cap structures (common/ISR/device/notify cfg) |

### MSI-X to GIC delivery

```
MsixCapability::fire(vector_idx)
  -> writes (addr, data) to IrqSignal
    -> GIC interprets as SPI injection
```

---

## 4. PCI Bus & ECAM Host Bridge

### Bdf

Bus:Device.Function address. Decoded from ECAM offset: `offset[27:20]=bus, [19:15]=dev, [14:12]=fn, [11:0]=reg`.

### PciBus

Topology manager. Maps `(device, function)` to `PciSlot` (owns `Box<dyn PciFunction>` + `PciConfigSpace` + resolved BAR mappings). Routes config reads/writes by BDF, BAR MMIO by address match.

### PciHostBridge

Implements `Device`. Two MMIO regions:

| Region | Base (virt) | Size | Purpose |
|---|---|---|---|
| ECAM | 0x3F00_0000 | 16MB | Config space access |
| PCI MMIO 32-bit | 0x1000_0000 | ~768MB | BAR-mapped device MMIO |
| PCI PIO | 0x3EFF_0000 | 64KB | I/O port space |
| PCI MMIO 64-bit | 0x80_0000_0000 | large | Prefetchable BARs |

On `attach()`:
1. Builds PciConfigSpace from function identity + BARs + capabilities
2. Allocates BAR addresses from mmio32/mmio64 bump allocator
3. Programs BAR addresses into config space
4. Records bar_mappings for MMIO dispatch

On `transact()`/`read_fast()`/`write_fast()`:
- ECAM region: decode BDF + reg, route to bus.config_read/write
- MMIO region: walk bar_mappings, route to function.bar_read/write

---

## 5. VirtIO PCI Transport

`VirtioPciTransport` implements `PciFunction` + `VirtioTransport`.

- vendor_id: 0x1AF4, device_id: 0x1040 + backend.device_id()
- BAR0: Common cfg / ISR / device cfg / notify (dispatched by offset ranges, pointed to by virtio_pci_cap structures)
- BAR4: MSI-X table + PBA
- Capabilities: MSI-X + PM + PCIe + five virtio_pci_cap vendor caps

Same `VirtioDeviceBackend` consumer pattern as MMIO transport. All 23 existing device types work unchanged.

---

## 6. Accelerator PCI Function

`AcceleratorPciFunction` wraps `helm_llvm::Accelerator`.

- vendor_id: 0x1DE5 (HELM), device_id: 0x0001, class: 0x120000 (Processing Accelerator)
- BAR0 (4KB): Control/status registers (STATUS, CONTROL, CYCLES, LOADS, STORES, FN_SEL, ARG0-3)
- BAR2 (configurable): Scratchpad memory (direct MMIO access)
- BAR4: MSI-X (1 vector for completion)
- CONTROL write 1 -> accel.run() -> fires MSI-X completion vector

Created from Python with `ir_file=` or `ir=` kwargs.

---

## 7. Python API

```python
# PCI host bridge
pci = create_device("pci-host", ecam_base=0x3F00_0000, mmio_base=0x1000_0000, irq_signal=sig)

# VirtIO over PCI
blk = create_device("virtio-blk", transport="pci", capacity=1*GB)
pci.attach(slot=0, device=blk)

# VirtIO over MMIO (explicit)
rng = create_device("virtio-rng", transport="mmio")
platform.add_device("virtio-rng", 0x0A00_0000, rng)

# Accelerator from file
accel = create_device("accel-pci", ir_file="matmul.ll", int_adders=4, fp_multipliers=8, scratchpad="64K")
pci.attach(slot=1, device=accel)

# Accelerator from inline IR
accel2 = create_device("accel-pci", ir="define i32 @main() { ret i32 0 }", scratchpad="4K")
pci.attach(slot=2, device=accel2)

platform.add_device("pci", 0x3F00_0000, pci)
```

DeviceInner gains two new variants:
- `PciFunc(Box<dyn PciFunction>)` — consumed by pci_host.attach()
- `PciHost(PciHostBridge)` — supports attach(), consumed by platform.add_device()

---

## 8. File Layout

```
crates/helm-device/src/pci/
  mod.rs              — re-exports, BarDecl
  traits.rs           — PciCapability, PciFunction, VirtioTransport
  config.rs           — PciConfigSpace (4KB, write masks, BAR sizing)
  bdf.rs              — Bdf, ECAM decoding
  bus.rs              — PciBus (slot map, dispatch, enumeration)
  host.rs             — PciHostBridge (Device impl, ECAM+MMIO, BAR allocator)
  capability/
    mod.rs
    pcie.rs           — PcieCapability
    msix.rs           — MsixCapability + MsixVector
    pm.rs             — PmCapability
    aer.rs            — AerCapability
    acs.rs            — AcsCapability
    virtio_pci_cap.rs — VirtIO PCI cap structures
  transport.rs        — VirtioPciTransport
  accel.rs            — AcceleratorPciFunction
  fdt.rs              — DTB node for pcie@... host bridge
```

Replaces old `proto/pci.rs`. No new crate — everything in helm-device with helm-llvm as existing dependency.

---

## 9. Integration Points

1. **VirtioMmioTransport** gains trivial `impl VirtioTransport` (delegates to existing methods)
2. **helm-python DeviceInner** grows `PciFunc` and `PciHost` variants
3. **fdt.rs** gains PCI host bridge awareness (`pci-host-ecam-generic`, ranges, bus-range)
4. **arm_virt_platform()** unchanged — PCI added from Python or CLI, not hardcoded
