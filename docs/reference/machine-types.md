# Machine Types

Available pre-built platforms for FS mode.

## arm-virt

QEMU-compatible virtual ARM platform.

| Property | Value |
|----------|-------|
| Name | `virt` / `arm-virt` |
| GIC | GICv2 (default), GICv3 (optional) |
| UARTs | 2× PL011 (on APB bus) |
| VirtIO | 2 MMIO slots |
| DRAM base | `0x4000_0000` |
| Default RAM | 256 MB |

## realview-pb-a8

ARM RealView Platform Baseboard for Cortex-A8.

| Property | Value |
|----------|-------|
| Name | `realview-pb` / `realview` |
| GIC | GICv2 (96 IRQs) |
| UARTs | 4× PL011 |
| Timer | SP804 dual timer |
| RTC | PL031 |
| Watchdog | SP805 |
| GPIO | 3× PL061 |
| System regs | RealView system control |

## rpi3

Raspberry Pi 3 (BCM2837) platform.

| Property | Value |
|----------|-------|
| Name | `rpi3` / `raspi3` |
| Timer | BCM system timer |
| Mailbox | ARM↔VC mailbox |
| GPIO | BCM GPIO (54 pins) |
| UARTs | PL011 + Mini UART |
