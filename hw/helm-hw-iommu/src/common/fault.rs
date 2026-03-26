//! Generic IOMMU translation result and fault types.
//!
//! Architecture-specific fault codes (ARM event queue format, AMD-Vi
//! event log format, RISC-V fault queue format) live in their respective
//! sub-modules. This module provides the common result enum used by
//! all IOMMU `translate()` methods.

/// Result of an IOMMU translation attempt.
#[derive(Debug)]
pub enum IommuTranslateResult {
    /// Translation succeeded — output physical address.
    Ok(u64),
    /// Bypass — no translation, pass IOVA through as PA.
    Bypass,
    /// Translation fault.
    Fault(IommuFault),
}

/// Generic fault context shared by all IOMMU variants.
///
/// The `code` field is variant-specific: ARM uses `SmmuFaultCode as u8`,
/// AMD-Vi uses its own event codes, RISC-V uses its fault queue codes.
#[derive(Debug)]
pub struct IommuFault {
    /// Architecture-specific fault code.
    pub code: u8,
    /// Device/stream ID that caused the fault.
    pub device_id: u32,
    /// Faulting input address (IOVA or IPA).
    pub input_addr: u64,
    /// Was the transaction a write?
    pub is_write: bool,
}
