# Bus Hierarchy

How transactions flow through the device bus tree.

## DeviceBus

`DeviceBus` dispatches transactions to the correct device by address.
It is itself a `Device`, enabling nested bus topologies.

### Construction

| Factory | Latency | Window | Use |
|---------|---------|--------|-----|
| `DeviceBus::system()` | 0 cycles | full 64-bit | Top-level system bus |
| `DeviceBus::pci(name, window)` | 1 cycle | custom | PCI root complex |
| `DeviceBus::usb(name)` | 10 cycles | 16 MB | USB host controller |
| `DeviceBus::new(name, window, latency)` | custom | custom | Any bus |

### Dispatch

When a transaction arrives:

1. Add `bridge_latency` to `txn.stall_cycles`.
2. Search `slots` for a device whose range contains `txn.addr`.
3. Compute `txn.offset = txn.addr - slot.base`.
4. Call `device.transact(&mut txn)`.
5. If no device matches, return an error.

### Protocol Buses

The `proto` module provides APB, AHB, PCI, I2C, SPI, USB, and AXI
bus implementations, each with protocol-appropriate latency and
address window.
