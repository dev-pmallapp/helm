//! Bus abstraction — `Bus` trait, address types, and the `HelmEventBus`.
//!
//! The `Bus` trait models addressable device interconnects (I2C, SPI, PCI, etc.).
//! `HelmEventBus` is the synchronous named-event pub-sub system (see [`event_bus`]).

pub mod amba;
pub mod event_bus;
pub mod i2c;
pub mod mmio;
pub mod spi;

use super::framework::device::Device;

// ── Bus trait ────────────────────────────────────────────────────────────────

/// A device bus that routes transactions to attached [`BusDevice`]s.
///
/// Implementors model specific bus protocols (I2C, SPI, PCI, custom).
/// The bus itself is a [`Device`] so it can be memory-mapped and participate
/// in the device lifecycle.
pub trait Bus: Device {
    /// Attach a device to this bus at the given address.
    ///
    /// Returns a unique handle for the attached device, or an error if the
    /// address is already in use or out of range.
    fn attach(
        &mut self,
        name: &str,
        device: Box<dyn BusDevice>,
        address: BusAddress,
    ) -> Result<u64, BusAttachError>;

    /// Enumerate all devices currently attached to this bus.
    fn enumerate(&self) -> Vec<BusDeviceDescriptor>;
}

/// A device that can be attached to a [`Bus`].
///
/// Unlike [`Device`] (which uses byte offsets and arbitrary sizes),
/// `BusDevice` uses register-level access appropriate for bus protocols.
pub trait BusDevice: Send {
    /// Human-readable device name.
    fn name(&self) -> &str;

    /// Read a register. `reg` is the register index, `size` is in bytes.
    fn read_register(&self, reg: u8, size: usize) -> u64;

    /// Write a register. `reg` is the register index, `size` is in bytes.
    fn write_register(&mut self, reg: u8, size: usize, val: u64);
}

// ── Address types ────────────────────────────────────────────────────────────

/// Address on a bus — protocol-specific.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BusAddress {
    /// I2C 7-bit device address.
    I2c(u8),
    /// SPI chip-select index.
    Spi(u8),
    /// PCI BDF (bus/device/function).
    Pci {
        /// PCI bus number.
        bus: u8,
        /// PCI device number.
        device: u8,
        /// PCI function number.
        function: u8,
    },
    /// Platform-specific or custom address.
    Custom(u64),
}

/// Descriptor returned by [`Bus::enumerate`].
pub struct BusDeviceDescriptor {
    /// Bus address where the device is attached.
    pub address: BusAddress,
    /// Human-readable name of the attached device.
    pub name: String,
}

// ── Error types ──────────────────────────────────────────────────────────────

/// Errors from [`Bus::attach`].
#[derive(Debug)]
pub enum BusAttachError {
    /// The requested address is already occupied by another device.
    AddressInUse(BusAddress),
    /// The address is not valid for this bus.
    AddressOutOfRange(BusAddress),
    /// The bus has reached its maximum device capacity.
    TooManyDevices,
}

/// Errors from bus transaction operations.
#[derive(Debug)]
pub enum BusError {
    /// No device is attached at the given address.
    NoDevice(BusAddress),
    /// The bus transaction timed out.
    Timeout,
    /// The target device did not acknowledge (I2C NACK).
    Nack,
    /// Bus arbitration was lost (multi-master).
    Arbitration,
}
