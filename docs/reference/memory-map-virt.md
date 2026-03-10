# Memory Map: ARM Virt

Memory map for the `arm-virt` machine type.

| Address Range | Size | Device |
|---------------|------|--------|
| `0x0800_0000` – `0x0801_FFFF` | 128 KB | GIC (distributor + CPU interface) |
| `0x0900_0000` – `0x0900_0FFF` | 4 KB | PL011 UART0 (via APB) |
| `0x0900_1000` – `0x0900_1FFF` | 4 KB | PL011 UART1 (via APB) |
| `0x0A00_0000` – `0x0A00_01FF` | 512 B | VirtIO MMIO slot 0 |
| `0x0A00_0200` – `0x0A00_03FF` | 512 B | VirtIO MMIO slot 1 |
| `0x4000_0000` – ... | configurable | DRAM (default 256 MB) |

The APB bus is at `0x0900_0000` with 1-cycle bridge latency and a
1 MB window. UART offsets are relative to the APB base.
