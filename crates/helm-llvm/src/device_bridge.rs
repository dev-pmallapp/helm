//! Bridge between `Accelerator` and `MemoryMappedDevice`.
//!
//! Maps the accelerator's execution interface to MMIO registers so the
//! CPU can trigger LLVM-IR accelerator runs via device writes.

use crate::accelerator::{Accelerator, AcceleratorBuilder};
use helm_core::types::Addr;
use helm_core::HelmResult;
use helm_device::mmio::{DeviceAccess, MemoryMappedDevice};

/// MMIO register offsets for the accelerator device.
const REG_STATUS: Addr = 0x00;
const REG_CONTROL: Addr = 0x04;
const REG_CYCLES: Addr = 0x08;
const REG_LOADS: Addr = 0x10;
const REG_STORES: Addr = 0x18;

/// An LLVM-IR hardware accelerator exposed as a memory-mapped device.
///
/// The CPU triggers accelerator execution by writing to the CONTROL
/// register.  The accelerator's scheduler runs to completion and the
/// elapsed cycles appear as device stall cycles in the timing model.
///
/// # MMIO Register Map
///
/// | Offset | Name    | R/W | Description |
/// |--------|---------|-----|-------------|
/// | 0x00   | STATUS  | R   | 0=idle, 1=running |
/// | 0x04   | CONTROL | W   | 1=start |
/// | 0x08   | CYCLES  | R   | Total cycles elapsed |
/// | 0x10   | LOADS   | R   | Total memory loads |
/// | 0x18   | STORES  | R   | Total memory stores |
pub struct AcceleratorDevice {
    accel: Accelerator,
    status: u32,
}

impl AcceleratorDevice {
    /// Wrap an existing [`Accelerator`] as an MMIO device.
    pub fn new(accel: Accelerator) -> Self {
        Self { accel, status: 0 }
    }

    /// Build from an LLVM IR file path using default functional units.
    pub fn from_file(path: &str) -> crate::error::Result<Self> {
        let accel = AcceleratorBuilder::new()
            .with_ir_file(path)
            .build()?;
        Ok(Self::new(accel))
    }
}

impl MemoryMappedDevice for AcceleratorDevice {
    fn read(&mut self, offset: Addr, _size: usize) -> HelmResult<DeviceAccess> {
        let data = match offset {
            REG_STATUS => self.status as u64,
            REG_CYCLES => self.accel.total_cycles(),
            REG_LOADS => self.accel.stats().memory_loads,
            REG_STORES => self.accel.stats().memory_stores,
            _ => 0,
        };
        Ok(DeviceAccess { data, stall_cycles: 1 })
    }

    fn write(&mut self, offset: Addr, _size: usize, value: u64) -> HelmResult<u64> {
        match offset {
            REG_CONTROL if value == 1 => {
                self.status = 1;
                // Run the accelerator to completion; ignore errors for now
                let _ = self.accel.run();
                self.status = 0;
                // Return accelerator execution time as stall cycles
                Ok(self.accel.total_cycles())
            }
            _ => Ok(1),
        }
    }

    fn region_size(&self) -> u64 {
        0x100
    }

    fn device_name(&self) -> &str {
        "llvm-accelerator"
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.status = 0;
        Ok(())
    }
}
