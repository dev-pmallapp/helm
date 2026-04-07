//! DMA controller models.
//!
//! This crate provides MMIO DMA engines.
//! All types implement the [`Device`](helm_devices::Device) trait.

pub mod dma;

pub use dma::DmaEngine;
pub use helm_core::DmaPort;
