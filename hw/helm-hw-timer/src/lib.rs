//! Timer device models.
//!
//! This crate provides MMIO timer devices.
//! All types implement the [`Device`](helm_devices::Device) trait.

pub mod sp804;

pub use sp804::Sp804;
