# Memory Map: RPi3

Memory map for the `rpi3` (BCM2837) machine type.

| Address | Size | Device |
|---------|------|--------|
| `0x3F00_3000` | 4 KB | BCM system timer |
| `0x3F00_B880` | 4 KB | Mailbox |
| `0x3F20_0000` | 4 KB | GPIO |
| `0x3F20_1000` | 4 KB | PL011 UART0 |
| `0x3F21_5000` | 4 KB | Mini UART (UART1) |
| `0x0000_0000` – ... | configurable | DRAM |

BCM2835/2837 peripherals are mapped at `0x3F000000` (bus address
`0x7E000000` in BCM documentation).
