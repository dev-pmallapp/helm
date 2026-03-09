# Memory Map: RealView PB-A8

Memory map for the `realview-pb` machine type (DUI0417D).

| Address | Size | Device |
|---------|------|--------|
| `0x1000_0000` | 4 KB | System registers |
| `0x1000_1000` | 4 KB | SP804 dual timer |
| `0x1000_6000` | 4 KB | PL031 RTC |
| `0x1000_9000` | 4 KB | PL011 UART0 |
| `0x1000_A000` | 4 KB | PL011 UART1 |
| `0x1000_B000` | 4 KB | PL011 UART2 |
| `0x1000_C000` | 4 KB | PL011 UART3 |
| `0x1000_F000` | 4 KB | SP805 watchdog |
| `0x1001_3000` | 4 KB | PL061 GPIO0 |
| `0x1001_4000` | 4 KB | PL061 GPIO1 |
| `0x1001_5000` | 4 KB | PL061 GPIO2 |
| `0x1F00_0000` | 8 KB | GIC (dist + CPU interface) |
