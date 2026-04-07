//! IOMMU models — ARM `SMMUv3`, AMD-Vi (`IOMMUv2`), RISC-V IOMMU.
//!
//! Each sub-module implements a different IOMMU architecture. Shared
//! infrastructure (TLB cache, fault types, guest memory trait) lives
//! in the `common` module.
//!
//! - [`smmu`] — ARM `SMMUv3` with full S1 translation, command/event queues
//! - [`amdvi`] — AMD-Vi stub (bypass-only, register identification)
//! - [`riscv_iommu`] — RISC-V IOMMU stub (bypass-only, capability registers)
#![allow(missing_docs)]

pub mod common;

pub mod amdvi;
pub mod riscv_iommu;
pub mod smmu;

// Re-export shared types at crate root for convenience.
pub use common::fault::{IommuFault, IommuTranslateResult};
pub use common::mem::ByteMem;
pub use common::tlb::{IommuTlb, IommuTlbEntry};

// Re-export primary SMMU types (most common use case).
pub use smmu::{SmmuFault, SmmuFaultCode, SmmuState, SmmuTranslateResult, StrtabFmt};
