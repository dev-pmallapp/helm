//! Character device models.
//!
//! This crate provides MMIO character devices (UARTs, serial ports).
//! All types implement the [`Device`](helm_devices::Device) trait.

pub mod pl011;

pub use pl011::Pl011;
