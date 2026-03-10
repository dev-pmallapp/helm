//! GIC (Generic Interrupt Controller) device models.
//!
//! This module provides device models for GICv2, GICv3, and GICv4:
//!
//! - [`v2::Gic`] — GICv2 distributor + CPU interface (existing).
//! - [`v3::GicV3`] — GICv3 distributor + redistributors + ICC sysregs.
//! - [`v4::GicV4`] — GICv4 extending GICv3 with vLPI/vSGI support.
//!
//! Supporting modules:
//!
//! - [`common`] — shared bitmap/priority helpers.
//! - [`distributor`] — GICD register state (shared by v2 and v3).
//! - [`redistributor`] — per-PE GICR state (GICv3+).
//! - [`icc`] — ICC system register definitions and per-PE state.
//! - [`lpi`] — LPI configuration and pending table helpers.
//! - [`its`] — ITS command queue and translation tables.

pub mod common;
pub mod distributor;
pub mod icc;
pub mod its;
pub mod lpi;
pub mod redistributor;
pub mod v2;
pub mod v3;
pub mod v4;

/// GIC architecture version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GicVersion {
    /// GICv2 (ARM IHI0048B).
    V2,
    /// GICv3 (ARM IHI0069H).
    V3,
    /// GICv4 / GICv4.1 (ARM IHI0069H §9-10).
    V4,
}

impl GicVersion {
    /// Returns `true` for GICv3 or GICv4.
    pub fn is_v3_or_later(self) -> bool {
        matches!(self, Self::V3 | Self::V4)
    }
}

// Backward-compatible re-export: `crate::arm::gic::Gic` still works.
pub use v2::Gic;
