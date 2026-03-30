# Platform & SoC

How helm-ng describes and constructs simulated hardware platforms.

## The Platform Trait

Defined in `helm-platform`:

```rust
pub trait Platform {
    fn name(&self) -> &str;
    fn attachment_slots(&self) -> &[AttachableSlot];
    fn build_plan(&self) -> PlatformBuildPlan;
}
```

A `Platform` describes the hardware topology of a machine: what
devices exist, where they are in the address map, and how interrupts
are routed. The platform does not own device instances — it produces a
`PlatformBuildPlan` that the engine uses to construct the system.

### AttachableSlot

Each platform exposes named slots where devices can be attached:

```rust
pub struct AttachableSlot {
    pub name: String,
    pub slot_type: SlotType,
    pub max_devices: usize,
}

pub enum SlotType {
    Mmio { base_range: (u64, u64) },
    Pci,
    VirtioMmio,
}
```

### PlatformBuildPlan

The build plan specifies:

| Component | Type | Description |
|-----------|------|-------------|
| Address regions | `AddressRegionSpec` | RAM, ROM, and device MMIO regions with base/size |
| Interrupt routes | `InterruptRouteSpec` | Device → GIC SPI/PPI wiring |
| Region kind | `RegionKind` | Ram, Rom, Mmio, Reserved |

## ARM Virt Platform

`helm-platform::aarch64` implements the ARM virt machine, compatible
with QEMU's `-M virt` address map:

| Device | Base Address | Size | IRQ |
|--------|-------------|------|-----|
| GIC Distributor | `0x0800_0000` | 64K | — |
| GIC CPU Interface | `0x0801_0000` | 64K | — |
| UART (PL011) | `0x0900_0000` | 4K | SPI 33 |
| RTC (PL031) | `0x0901_0000` | 4K | SPI 34 |
| Timer (SP804) | `0x0902_0000` | 4K | SPI 35 |
| VirtIO MMIO | `0x0A00_0000`+ | 4K each | SPI 48+ |
| PCI ECAM | `0x4010_0000_0000` | 256M | SPI 3–6 |
| RAM | `0x4000_0000` | Configurable | — |

### Platform Registration

`list_platforms()` returns available platforms:

```rust
pub fn list_platforms() -> Vec<PlatformInfo> {
    // Currently: [PlatformInfo { name: "arm-virt", ... }]
}
```

## Full-System Boot Flow

```text
1. Python script selects platform: Platform("arm-virt")
2. load_aarch64_kernel() loads kernel Image + DTB + initrd
   ├── Kernel → RAM base (0x4000_0000)
   ├── DTB → after kernel, 2MB-aligned
   └── Initrd → after DTB, page-aligned
3. HelmBoard constructed with:
   ├── HelmAddressSpace (FlatMem + AddressMap)
   ├── GICv2 (distributor + CPU interface)
   ├── PL011 UART with StdioCharBackend
   └── SP804 timer, PL031 RTC
4. CPU registers set:
   ├── PC = kernel entry point
   ├── X0 = DTB physical address
   └── SCTLR = MMU disabled, caches disabled
5. step_aarch64_fs() loop begins
```

## Device Topology

`helm-platform::topology` models the hierarchical structure of devices
in a platform. `DeviceTopology` tracks parent-child relationships,
allowing queries like "which devices are on this bus" and "what is
the path from CPU to this device."

## Comparison

| Aspect | QEMU | gem5 | Simics | helm-ng |
|--------|------|------|--------|---------|
| Machine type | `MachineClass` + `machine_init()` | Python SimObject tree | Python + DML | `Platform` trait |
| Address map | Hardcoded in C | Python config | DML + Python | `PlatformBuildPlan` |
| Config language | CLI `-M`, `-device` | Python `fs.py` | Python + DML | Python (gem5-style) |
| DTB | Generated in C | N/A | N/A | Loaded from file or generated |
| Device wiring | `sysbus_mmio_map()` | Port connections | Config attributes | `AttachableSlot` + `InterruptRouteSpec` |
