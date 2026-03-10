# Platform & SoC

How HELM assembles buses, devices, and IRQ routes into a complete
simulated machine.

## Platform Struct

`Platform` (`helm-device::platform`) is the top-level system
description:

- `name` — human-readable name (e.g. "arm-virt").
- `system_bus` — top-level `DeviceBus`.
- `irq_router` — IRQ routing table.
- `device_map` — named device references with base addresses.
- `uses_dtb` — whether this platform needs a device tree.

Platforms can be built programmatically in Rust or configured from
Python via `helm-python` bindings.

## Pre-Built Machine Types

### arm-virt

QEMU-compatible virtual ARM platform:

| Address | Device |
|---------|--------|
| `0x0800_0000` | GIC (distributor + CPU interface) |
| `0x0900_0000` | APB bus (PL011 UART0 @ +0x0000, UART1 @ +0x1000) |
| `0x0A00_0000` | VirtIO MMIO slot 0 |
| `0x0A00_0200` | VirtIO MMIO slot 1 |
| `0x4000_0000` | DRAM base |

### realview-pb-a8

ARM RealView Platform Baseboard for Cortex-A8 (DUI0417D):

| Address | Device |
|---------|--------|
| `0x1000_0000` | System registers |
| `0x1000_1000` | SP804 dual timer |
| `0x1000_6000` | PL031 RTC |
| `0x1000_9000`–`0x1000_C000` | PL011 UART0–UART3 |
| `0x1000_F000` | SP805 watchdog |
| `0x1001_3000`–`0x1001_5000` | PL061 GPIO0–GPIO2 |
| `0x1F00_0000` | GIC |

### rpi3 (BCM2837)

Raspberry Pi 3 peripheral memory map:

| Address | Device |
|---------|--------|
| `0x3F00_3000` | BCM system timer |
| `0x3F00_B880` | Mailbox |
| `0x3F20_0000` | GPIO |
| `0x3F20_1000` | PL011 UART0 |
| `0x3F21_5000` | Mini UART (UART1) |

## DTB Generation

HELM generates or patches Flattened Device Trees (FDT/DTB) for FS
mode. The strategy is automatically inferred from CLI arguments:

| Situation | Policy |
|-----------|--------|
| `-kernel Image` (no `--dtb`) | Generate DTB from platform + devices |
| `-kernel Image --dtb base.dtb` + extras | Patch user DTB with `-device` additions |
| `-kernel Image --dtb base.dtb` (no extras) | Pass through user DTB verbatim |
| `-bios edk2.fd` | No DTB (UEFI boot) |
| `-drive file=hd.img` (no `-kernel`) | No DTB (firmware boot) |

The `DtbConfig` struct controls RAM base/size, CPU count, boot args,
initrd, and extra device specs.

## Python → Rust Wiring

The Python `Platform` class serialises to a dict matching Rust
`PlatformConfig`. When using `helm-aarch64` or `helm-system-aarch64` with a
`.py` script, the embedded Python interpreter calls into `_helm_core`
(PyO3) which constructs the Rust session directly.
