# VirtIO

VirtIO device framework — spec 1.4 compliant.

## Architecture

```text
DeviceBus
  └── VirtioMmioTransport (MMIO registers @ base)
        └── VirtioDeviceBackend (e.g. VirtioBlk)
              └── queues: [Virtqueue, ...]
```

Each device type implements `VirtioDeviceBackend`. The
`VirtioMmioTransport` wraps any backend and provides the standard MMIO
register interface. The transport implements the `Device` trait.

## MMIO Transport (Spec 4.2.2)

| Offset | Register | R/W |
|--------|----------|-----|
| 0x000 | MagicValue | R |
| 0x008 | DeviceID | R |
| 0x010 | DeviceFeatures | R |
| 0x020 | DriverFeatures | W |
| 0x030 | QueueSel | W |
| 0x044 | QueueReady | RW |
| 0x050 | QueueNotify | W |
| 0x060 | InterruptStatus | R |
| 0x070 | Status | RW |
| 0x100+ | Config space | RW |

## Virtqueue

Supports both split (spec 2.7) and packed (spec 2.8) layouts.

Split queue descriptor:
```rust
pub struct VringDesc {
    pub addr: u64,   // guest physical address
    pub len: u32,    // buffer length
    pub flags: u16,  // NEXT, WRITE, INDIRECT
    pub next: u16,   // chained descriptor index
}
```

## Device Types

All VirtIO 1.4 device types are defined:

| Type | Module | Device ID |
|------|--------|-----------|
| Block | `virtio::blk` | 2 |
| Network | `virtio::net` | 1 |
| Console | `virtio::console` | 3 |
| RNG | `virtio::rng` | 4 |
| Balloon | `virtio::balloon` | 5 |
| GPU | `virtio::gpu` | 16 |
| Input | `virtio::input` | 18 |
| Filesystem | `virtio::fs` | 26 |
| SCSI | `virtio::scsi` | 8 |
| Sound | `virtio::sound` | 25 |
| And more... | | |
