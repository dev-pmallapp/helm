//! Generic device models -- ARM IP blocks and common peripherals.
//!
//! All modules are feature-gated. Enable the appropriate feature to
//! include each device model.

#[cfg(feature = "arm-ip")]
pub mod pl011;

#[cfg(feature = "arm-ip")]
pub mod sp804;

#[cfg(feature = "arm-ip")]
pub mod pl031;

#[cfg(feature = "dma")]
pub mod dma;
