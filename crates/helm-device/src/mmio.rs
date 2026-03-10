//! Memory-mapped I/O device trait.

use helm_core::types::Addr;
use helm_core::HelmResult;

/// Result of a device register access: data and simulated stall cycles.
#[derive(Debug, Clone)]
pub struct DeviceAccess {
    pub data: u64,
    /// Number of cycles the access takes (for timing models).
    pub stall_cycles: u64,
}

/// Trait that every memory-mapped device must implement.
///
/// # Example
/// ```ignore
/// struct Timer { counter: u64 }
///
/// impl MemoryMappedDevice for Timer {
///     fn read(&mut self, offset: Addr, size: usize) -> HelmResult<DeviceAccess> {
///         Ok(DeviceAccess { data: self.counter, stall_cycles: 2 })
///     }
///     fn write(&mut self, offset: Addr, size: usize, value: u64) -> HelmResult<u64> {
///         self.counter = value;
///         Ok(2)
///     }
///     fn region_size(&self) -> u64 { 0x100 }
/// }
/// ```
pub trait MemoryMappedDevice: Send + Sync {
    /// Read from a device register.
    /// `offset` is relative to the device's base address.
    fn read(&mut self, offset: Addr, size: usize) -> HelmResult<DeviceAccess>;

    /// Write to a device register. Returns stall cycles.
    fn write(&mut self, offset: Addr, size: usize, value: u64) -> HelmResult<u64>;

    /// The size of the MMIO region this device occupies (bytes).
    fn region_size(&self) -> u64;

    /// Called once before simulation starts.
    fn init(&mut self) -> HelmResult<()> {
        Ok(())
    }

    /// Reset device to power-on state.
    fn reset(&mut self) -> HelmResult<()> {
        Ok(())
    }

    /// Human-readable name.
    fn device_name(&self) -> &str {
        "unnamed-device"
    }
}
