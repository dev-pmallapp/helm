# Device Authoring Guide

How to build custom memory-mapped devices for HELM.

## Python Devices (Quickest)

Subclass `helm.Device` and override `read`/`write`:

```python
from helm.device import Device

class Gpio(Device):
    """32-bit GPIO port with data and direction registers."""

    def __init__(self, name="gpio0", base_address=0x5000_0000):
        super().__init__(name, region_size=0x08, base_address=base_address)
        self.direction = 0   # 0 = input
        self.data = 0

    def read(self, offset, size):
        if offset == 0x00:
            return self.direction
        if offset == 0x04:
            return self.data
        return 0

    def write(self, offset, size, value):
        if offset == 0x00:
            self.direction = value & 0xFFFF_FFFF
        elif offset == 0x04:
            self.data = value & self.direction   # can only set output bits

    def reset(self):
        self.direction = 0
        self.data = 0
```

Attach it to a platform:

```python
platform = Platform(
    ...,
    devices=[Gpio("gpio0", base_address=0x5000_0000)],
)
```

## Rust Devices (Full Performance)

Implement `MemoryMappedDevice` from `helm-device`:

```rust
use helm_device::{MemoryMappedDevice, DeviceAccess};
use helm_core::{HelmResult, types::Addr};

pub struct Timer {
    counter: u64,
    reload: u64,
    enabled: bool,
}

impl MemoryMappedDevice for Timer {
    fn read(&mut self, offset: Addr, _size: usize) -> HelmResult<DeviceAccess> {
        let data = match offset {
            0x00 => self.counter,
            0x04 => self.reload,
            0x08 => self.enabled as u64,
            _    => 0,
        };
        Ok(DeviceAccess { data, stall_cycles: 2 })
    }

    fn write(&mut self, offset: Addr, _size: usize, value: u64) -> HelmResult<u64> {
        match offset {
            0x00 => self.counter = value,
            0x04 => self.reload = value,
            0x08 => {
                self.enabled = value & 1 != 0;
                if self.enabled { self.counter = self.reload; }
            }
            _ => {}
        }
        Ok(2) // stall cycles
    }

    fn region_size(&self) -> u64 { 0x10 }
    fn device_name(&self) -> &str { "timer" }
}
```

## Registering as a Plugin

For devices that live in their own crate:

```rust
use helm_plugin_api::*;

pub struct MyDevice { /* ... */ }

impl HelmComponent for MyDevice {
    fn component_type(&self) -> &'static str { "device.my-device" }
    fn interfaces(&self) -> &[&str] { &["memory-mapped"] }
    fn reset(&mut self) -> HelmResult<()> { Ok(()) }
}
```

## Interrupts

Devices that generate interrupts use `IrqController`:

```python
class Watchdog(Device):
    def __init__(self):
        super().__init__("wdt", region_size=4, irq=7)
        self.expired = False

    def write(self, offset, size, value):
        if value == 0:
            self.expired = True
            # In a real implementation the engine would call
            # irq_controller.assert(self.irq)
```

## Testing Your Device

Write unit tests that exercise every register:

```python
class TestGpio(unittest.TestCase):
    def test_read_after_reset(self):
        g = Gpio()
        g.reset()
        self.assertEqual(g.read(0x00, 4), 0)

    def test_write_direction_then_data(self):
        g = Gpio()
        g.write(0x00, 4, 0xFF)       # set lower 8 bits as output
        g.write(0x04, 4, 0x1234)
        self.assertEqual(g.read(0x04, 4), 0x34)  # only output bits
```
