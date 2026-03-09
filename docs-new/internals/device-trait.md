# Device Trait

The core abstraction for all simulated devices.

## Device Lifecycle

1. **Create** — construct with configuration parameters.
2. **Attach** — `platform.add_device(name, base, device)`.
3. **Reset** — `reset()` called before simulation starts.
4. **Run** — `transact()` handles CPU reads/writes; `tick()` called
   periodically for time-driven behaviour.
5. **Checkpoint** — `checkpoint()` serialises state to JSON.
6. **Restore** — `restore(state)` deserialises.

## Core Methods

| Method | Purpose |
|--------|---------|
| `transact(&mut Transaction)` | Handle a read or write |
| `regions()` | MMIO region(s) this device occupies |
| `reset()` | Power-on state |
| `tick(cycles)` | Time-driven progress; returns `Vec<DeviceEvent>` |
| `name()` | Human-readable identifier |
| `checkpoint()` | Serialise to JSON |
| `restore(state)` | Deserialise from JSON |

## DeviceEvent

Events emitted during `tick()` or `transact()`:

- `Irq { line, assert }` — assert or de-assert an interrupt.
- `DmaComplete { channel }` — DMA transfer finished.
- `Log { level, message }` — diagnostic message.

## Fast Path

For FE mode, devices can override `read_fast` / `write_fast` to
bypass `Transaction` allocation:

```rust
fn read_fast(&mut self, offset: Addr, size: usize) -> HelmResult<u64>;
fn write_fast(&mut self, offset: Addr, size: usize, value: u64) -> HelmResult<()>;
```

## Legacy MemoryMappedDevice

The older `MemoryMappedDevice` trait provides a simpler `read` /
`write` / `region_size` interface. `LegacyWrapper` adapts it to the
`Device` trait.
