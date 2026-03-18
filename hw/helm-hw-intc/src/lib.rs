//! Interrupt controller models.
//!
//! This crate provides interrupt controller implementations:
//! - [`gicv2`] -- ARM GICv2 (distributor + CPU interface)
//!
//! Future: RISC-V PLIC, GICv3.

pub mod gicv2;

pub use gicv2::{Gicv2Distributor, Gicv2CpuInterface};
