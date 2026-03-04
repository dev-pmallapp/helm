//! # helm-device
//!
//! Framework for building simulated devices.  Developers implement
//! [`MemoryMappedDevice`] to create custom peripherals (UARTs, timers,
//! GPUs, etc.) that plug into HELM's address space and interrupt system.

pub mod bus;
pub mod irq;
pub mod mmio;

pub use bus::{DeviceBus, DeviceSlot};
pub use irq::{IrqController, IrqLine, IrqState};
pub use mmio::{DeviceAccess, MemoryMappedDevice};

#[cfg(test)]
mod tests;
