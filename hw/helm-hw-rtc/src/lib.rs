//! Real-time clock device models.
//!
//! This crate provides MMIO RTC devices.
//! All types implement the [`Device`](helm_devices::Device) trait.

pub mod pl031;

pub use pl031::Pl031;
