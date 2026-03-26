//! Interrupt controller models.
//!
//! This crate provides interrupt controller implementations:
//! - GICv2 (ARM Generic Interrupt Controller version 2)
//!
//! # Quick start
//!
//! Use [`gicv2::build_gicv2`] to create a properly-wired single-CPU pair of devices:
//! ```ignore
//! let (gicd, gicc, irq_line) = helm_hw_intc::gicv2::build_gicv2(128);
//! // irq_line is Arc<AtomicBool>; poll it from the CPU step loop
//! ```
#![allow(missing_docs)]

pub mod gicv2;
pub mod gicv3;

pub use gicv2::{
    GicCpuState, GicDistState, GicSharedState, GicSink,
    Gicv2CpuInterface, Gicv2Distributor, build_gicv2, build_gicv2_mp,
};
pub use gicv3::{
    GicV3SharedState, Gicv3DistState, Gicv3RedistState, Gicv3CpuIfState,
    GicV3Sink, Gicv3Distributor, Gicv3Redistributor,
    build_gicv3, build_gicv3_mp,
};

#[cfg(feature = "probe")]
pub use helm_probe::GicProbes;
