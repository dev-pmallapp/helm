//! Access Control Services (ACS) extended capability (ext cap ID 0x000D, 8 bytes).
//!
//! ACS is defined in the PCIe Base Specification section 7.7.8 and provides
//! mechanisms to control peer-to-peer transactions between PCIe functions.
//! It is typically used to enforce IOMMU isolation domains.
//!
//! The capability consists of:
//! - `acs_cap` — ACS Capability Register (read-only): supported feature bits.
//! - `acs_ctl` — ACS Control Register (writable): enable bits for each feature.
//!
//! Default capability flags (`acs_cap = 0x001F`):
//! - Bit 0: Source Validation (SV)
//! - Bit 1: Translation Blocking (TB)
//! - Bit 2: P2P Request Redirect (RR)
//! - Bit 3: P2P Completion Redirect (CR)
//! - Bit 4: Upstream Forwarding (UF)

use crate::pci::traits::PciCapability;

// ── Extended capability header constants ──────────────────────────────────────

/// PCIe extended capability ID for ACS.
const ACS_EXT_CAP_ID: u32 = 0x000D;

/// ACS capability version (1).
const ACS_CAP_VERSION: u32 = 1;

/// Default ACS capability flags: SV + TB + RR + CR + UF.
const ACS_CAP_DEFAULT: u16 = 0x001F;

// ── AcsCapability ─────────────────────────────────────────────────────────────

/// ACS Extended Capability (extended cap ID 0x000D).
///
/// # Register Layout (offsets relative to capability start)
///
/// | Offset | Field                            | Access |
/// |--------|----------------------------------|--------|
/// | 0x00   | Extended Capability Header       | RO     |
/// | 0x04   | ACS Capability \| ACS Control    | see note |
///
/// The word at offset 0x04 packs two 16-bit registers:
/// - Bits [15:0]  = ACS Capability Register (read-only)
/// - Bits [31:16] = ACS Control Register (writable)
///
/// # Examples
///
/// ```
/// use helm_device::pci::capability::AcsCapability;
/// use helm_device::pci::PciCapability;
///
/// let cap = AcsCapability::new(0x148);
/// assert!(cap.is_extended());
/// assert_eq!(cap.length(), 8);
/// ```
#[derive(Debug, Clone)]
pub struct AcsCapability {
    /// Absolute byte offset in config space (must be >= 0x100).
    offset: u16,
    /// ACS Capability register (read-only): supported feature bits.
    acs_cap: u16,
    /// ACS Control register (writable): enabled feature bits.
    acs_ctl: u16,
}

impl AcsCapability {
    /// Construct a new ACS extended capability at `offset`.
    ///
    /// `acs_cap` is initialised to `0x001F` (SV + TB + RR + CR + UF).
    /// `acs_ctl` starts at zero (all features disabled).
    ///
    /// # Panics (debug only)
    ///
    /// In debug builds a runtime assertion fires if `offset < 0x100`.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::capability::AcsCapability;
    /// use helm_device::pci::PciCapability;
    ///
    /// let cap = AcsCapability::new(0x148);
    /// assert!(cap.is_extended());
    /// ```
    #[must_use]
    pub fn new(offset: u16) -> Self {
        debug_assert!(
            offset >= 0x100,
            "ACS must be an extended capability (offset >= 0x100)"
        );
        Self {
            offset,
            acs_cap: ACS_CAP_DEFAULT,
            acs_ctl: 0,
        }
    }

    /// Build the extended capability header dword.
    ///
    /// Format per PCIe spec section 7.6.3:
    /// - Bits [15:0]  = Extended Capability ID (0x000D)
    /// - Bits [19:16] = Capability Version (1)
    /// - Bits [31:20] = Next Capability Offset (0 = end of list)
    fn ext_cap_header() -> u32 {
        ACS_EXT_CAP_ID | (ACS_CAP_VERSION << 16)
    }
}

impl PciCapability for AcsCapability {
    /// Returns the low byte of the extended capability ID (0x0D for ACS).
    fn cap_id(&self) -> u8 {
        (ACS_EXT_CAP_ID & 0xFF) as u8
    }

    fn offset(&self) -> u16 {
        self.offset
    }

    fn length(&self) -> u16 {
        8
    }

    fn name(&self) -> &str {
        "ACS"
    }

    /// Returns `true` because ACS lives at offset >= 0x100.
    fn is_extended(&self) -> bool {
        true
    }

    /// Read a 32-bit dword at `offset` bytes from the start of this capability.
    ///
    /// | Offset | Field                                    |
    /// |--------|------------------------------------------|
    /// | 0x00   | Extended Capability Header               |
    /// | 0x04   | ACS Control [31:16] \| ACS Cap [15:0]   |
    fn read(&self, offset: u16) -> u32 {
        match offset {
            0x00 => Self::ext_cap_header(),
            0x04 => u32::from(self.acs_cap) | (u32::from(self.acs_ctl) << 16),
            _ => 0,
        }
    }

    /// Write a 32-bit dword at `offset` bytes from the start of this capability.
    ///
    /// Only offset 0x04 is partially writable: the high 16 bits update
    /// `acs_ctl` (ACS Control register).  The low 16 bits (`acs_cap`) are
    /// read-only.
    fn write(&mut self, offset: u16, value: u32) {
        if offset == 0x04 {
            self.acs_ctl = (value >> 16) as u16;
        }
    }

    /// Reset: clear ACS Control register (ACS Capability is preserved).
    fn reset(&mut self) {
        self.acs_ctl = 0;
    }
}
