//! Power Management Capability (Capability ID 0x01, 8 bytes).
//!
//! Implements the PCI Power Management Interface Specification revision 1.2
//! capability structure.  The structure encodes:
//!
//! - `pm_cap`  — Power Management Capabilities register (read-only after
//!   construction): version, D1/D2 support, PME support mask.
//! - `pm_csr`  — Power Management Control/Status register: power state
//!   (D0–D3hot) and PME_Status.

use crate::pci::traits::PciCapability;

// ── Constant defaults ─────────────────────────────────────────────────────────

/// PCI Power Management version 3 (bits [2:0] = 011).
const PM_CAP_VERSION: u16 = 0x0003;

/// PME support from D3hot (bit 11 of pm_cap).
///
/// Indicates the function is able to generate PME from D3hot.
const PM_CAP_PME_D3HOT: u16 = 1 << 11;

// ── PmCapability ──────────────────────────────────────────────────────────────

/// PCI Power Management Capability (ID 0x01).
///
/// The capability occupies 8 bytes at `offset` in config space.
///
/// # Power State Encoding
///
/// Bits [1:0] of `pm_csr` encode the current power state:
///
/// | Bits | State |
/// |------|-------|
/// | 00   | D0    |
/// | 01   | D1    |
/// | 10   | D2    |
/// | 11   | D3hot |
///
/// # Examples
///
/// ```
/// use helm_device::pci::capability::PmCapability;
/// use helm_device::pci::PciCapability;
///
/// let cap = PmCapability::new(0x50);
/// assert_eq!(cap.cap_id(), 0x01);
/// assert_eq!(cap.length(), 8);
/// assert!(!cap.is_extended());
/// ```
#[derive(Debug, Clone)]
pub struct PmCapability {
    /// Absolute byte offset in config space.
    offset: u16,
    /// Power Management Capabilities register (read-only).
    pm_cap: u16,
    /// Power Management Control/Status register.
    pm_csr: u32,
}

impl PmCapability {
    /// Construct a new Power Management Capability at `offset`.
    ///
    /// Default configuration:
    /// - Capability version 3
    /// - PME from D3hot supported
    /// - Power state = D0 (pm_csr = 0)
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::capability::PmCapability;
    /// use helm_device::pci::PciCapability;
    ///
    /// let cap = PmCapability::new(0x50);
    /// assert_eq!(cap.cap_id(), 0x01);
    /// ```
    #[must_use]
    pub fn new(offset: u16) -> Self {
        Self {
            offset,
            pm_cap: PM_CAP_VERSION | PM_CAP_PME_D3HOT,
            pm_csr: 0,
        }
    }
}

impl PciCapability for PmCapability {
    fn cap_id(&self) -> u8 {
        0x01
    }

    fn offset(&self) -> u16 {
        self.offset
    }

    fn length(&self) -> u16 {
        8
    }

    fn name(&self) -> &str {
        "PM"
    }

    /// Read a 32-bit dword from the capability header.
    ///
    /// | Offset | Field                          |
    /// |--------|--------------------------------|
    /// | 0x00   | cap_id \| next_ptr \| pm_cap   |
    /// | 0x04   | pm_csr                         |
    fn read(&self, offset: u16) -> u32 {
        match offset {
            0x00 => u32::from(self.pm_cap) << 16,
            0x04 => self.pm_csr,
            _ => 0,
        }
    }

    /// Write a 32-bit dword to the capability.
    ///
    /// Only offset 0x04 (`pm_csr`) is writable:
    /// - Bits [1:0]: power state (D0–D3hot) — fully writable
    /// - Bit [15]:  `PME_Status` — W1C (clear on write-1)
    fn write(&mut self, offset: u16, value: u32) {
        if offset == 0x04 {
            // Power state bits [1:0] are writable.
            let power_state = value & 0x0003;
            // PME_Status (bit 15) is W1C.
            let pme_status_clear = (value >> 15) & 1;

            // Preserve all read-only bits; update power state.
            self.pm_csr = (self.pm_csr & !0x0003) | power_state;

            if pme_status_clear != 0 {
                self.pm_csr &= !(1 << 15);
            }
        }
    }

    /// Reset the PM Control/Status register to zero (D0 state).
    fn reset(&mut self) {
        self.pm_csr = 0;
    }
}
