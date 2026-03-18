//! AMBA bus controllers and ARM IP peripherals.
//!
//! This crate provides:
//! - **Bus controllers**: AHB and APB buses, I2C master, SPI master
//! - **ARM IP devices**: PL011 UART, SP804 dual timer, PL031 RTC, DMA engine
//!
//! All types depend on the `helm-devices` SDK framework for the [`Device`]
//! trait, [`InterruptPin`], and [`CharBackend`].

pub mod amba;
pub mod i2c;
pub mod spi;
pub mod pl011;
pub mod sp804;
pub mod pl031;
pub mod dma;

// Re-export key types
pub use amba::{AhbBus, ApbBus};
pub use i2c::{I2cBus, I2cDevice};
pub use spi::{SpiBus, SpiDevice};
pub use pl011::Pl011;
pub use sp804::Sp804;
pub use pl031::Pl031;
pub use dma::{DmaEngine, DmaPort};
