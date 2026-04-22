//! Firmware-facing device models.
//!
//! This crate currently provides a minimal MMIO `fw_cfg` surface.

pub mod fw_cfg;

pub use fw_cfg::FwCfgMmio;
