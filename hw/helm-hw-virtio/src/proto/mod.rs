//! VirtIO protocol layer — transport, virtqueue rings, and feature constants.
//!
//! This module contains the machinery shared by all VirtIO device backends:
//! - [`features`] — feature bit constants (spec §6) and device type IDs
//! - [`transport`] — MMIO transport register interface (spec §4.2.2)
//! - [`virtqueue`] — split-ring descriptor processor over the shared
//!   [`helm_core::ByteMem`] contract

pub mod features;
pub mod transport;
pub mod virtqueue;
