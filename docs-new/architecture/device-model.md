# Device Model

How HELM models hardware peripherals — traits, buses, MMIO dispatch,
interrupts, and DMA.

## Device Trait

Every simulated device implements the `Device` trait
(`helm-device::device`):

```rust
pub trait Device: Send + Sync {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()>;
    fn regions(&self) -> &[MemRegion];
    fn reset(&mut self) -> HelmResult<()>;
    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>>;
    fn name(&self) -> &str;
    fn checkpoint(&self) -> HelmResult<serde_json::Value>;
    fn restore(&mut self, state: &serde_json::Value) -> HelmResult<()>;
}
```

Devices also have fast-path methods (`read_fast`, `write_fast`) that
bypass `Transaction` allocation for functional-emulation mode.

## Transaction

A `Transaction` carries all context for a bus access:

- `addr` / `offset` — absolute and device-relative address.
- `size` — 1, 2, 4, 8, or 16 bytes.
- `data` — 128-bit buffer (supports SIMD/LDP/STP).
- `is_write` — read vs write.
- `attrs` — initiator ID, secure/NS, cacheable, privileged.
- `stall_cycles` — accumulated as the transaction traverses the bus.

## DeviceBus

`DeviceBus` is a hierarchical bus that routes transactions to devices
by address. It is itself a `Device`, so buses nest:

```text
system_bus (0 latency)
  ├── uart @ 0x4000_0000
  └── pci_bus @ 0xC000_0000 (1 cycle crossing)
      ├── gpu @ 0x0000
      └── nic @ 0x1000
```

Factory methods: `DeviceBus::system()` (0 latency, full address space),
`DeviceBus::pci(name, window)` (1-cycle crossing),
`DeviceBus::usb(name)` (10-cycle protocol overhead).

## IRQ System

Two layers:

- **IrqLine / IrqController** — simple line tracking with
  assert/deassert (backward compatible).
- **IrqRouter** — named routes from device IRQ lines to interrupt
  controller inputs, serializable for checkpoint/restore.

Devices emit `DeviceEvent::Irq { line, assert }` from `tick()` or
`transact()`; the engine collects these and routes them through the
`IrqRouter`.

## DMA Engine

`DmaEngine` provides scatter-gather DMA with bus-beat fragmentation:

- `DmaChannel` tracks src/dst/length/direction/status.
- Transfers are broken into beats of `beat_size` bytes (default 8 for
  a 64-bit bus).
- Each beat produces a `Transaction` that flows through the bus
  hierarchy, accumulating stall cycles.

## Protocol Buses

The `proto` module provides typed bus implementations:

| Bus | Module | Latency | Window |
|-----|--------|---------|--------|
| APB | `proto::amba::ApbBus` | 1 cycle | configurable |
| AHB | `proto::amba::AhbBus` | 0 cycles | configurable |
| PCI | `proto::pci` | 1 cycle | configurable |
| I2C | `proto::i2c` | 10 cycles | 256 B |
| SPI | `proto::spi` | 5 cycles | 256 B |
| USB | `proto::usb` | 10 cycles | 16 MB |
| AXI | `proto::axi` | configurable | configurable |

## Device Lifecycle

1. Device is created and attached to a bus (`platform.add_device()`).
2. `reset()` is called before simulation starts.
3. `transact()` handles CPU/DMA-initiated reads and writes.
4. `tick()` is called periodically for timer/FIFO progress.
5. `checkpoint()` / `restore()` for save/load.
