//! Bus protocol definitions.
//!
//! Each protocol specializes the generic [`DeviceBus`](crate::bus::DeviceBus)
//! with protocol-specific configuration, addressing, and timing.

pub mod amba;
pub mod axi;
pub mod i2c;
pub mod spi;
pub mod system;
pub mod usb;

pub use amba::{AhbBus, ApbBus};
pub use axi::AxiBus;
pub use i2c::I2cBus;
pub use spi::SpiBus;
pub use system::SystemBus;
pub use usb::UsbBus;
