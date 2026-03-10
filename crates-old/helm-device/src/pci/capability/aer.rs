//! Advanced Error Reporting (AER) extended capability (ext cap ID 0x0001, 48 bytes).
//!
//! AER is a PCIe extended capability defined in the PCIe Base Specification
//! section 7.8.  It lives in extended config space (offset >= 0x100) and
//! provides fine-grained error reporting and masking for:
//!
//! - Uncorrectable errors (e.g. data-link layer protocol errors, completion
//!   timeouts, unexpected completions, poisoned TLPs, ECRC errors)
//! - Correctable errors (e.g. receiver errors, bad TLP/DLLP, replay timer
//!   timeouts, advisory non-fatal errors)

use crate::pci::traits::PciCapability;

// ── Extended capability header fields ─────────────────────────────────────────

/// PCIe extended capability ID for AER.
const AER_EXT_CAP_ID: u32 = 0x0001;

/// AER capability version (1).
const AER_CAP_VERSION: u32 = 1;

// ── AerCapability ─────────────────────────────────────────────────────────────

/// AER Extended Capability (extended cap ID 0x0001).
///
/// # Register Layout (offsets relative to capability start)
///
/// | Offset | Field                            | Access |
/// |--------|----------------------------------|--------|
/// | 0x00   | Extended Capability Header       | RO     |
/// | 0x04   | Uncorrectable Error Status       | RW/W1C |
/// | 0x08   | Uncorrectable Error Mask         | RW     |
/// | 0x0C   | Uncorrectable Error Severity     | RW     |
/// | 0x10   | Correctable Error Status         | RW/W1C |
/// | 0x14   | Correctable Error Mask           | RW     |
/// | 0x18   | AER Capabilities and Control     | RW     |
///
/// # Examples
///
/// ```
/// use helm_device::pci::capability::AerCapability;
/// use helm_device::pci::PciCapability;
///
/// let cap = AerCapability::new(0x100);
/// assert_eq!(cap.cap_id(), 0x01);   // low byte of extended header
/// assert!(cap.is_extended());
/// assert_eq!(cap.length(), 48);
/// ```
#[derive(Debug, Clone)]
pub struct AerCapability {
    /// Absolute byte offset in config space (must be >= 0x100).
    offset: u16,
    /// Uncorrectable Error Status register (W1C).
    uncorrectable_status: u32,
    /// Uncorrectable Error Mask register (1 = masked, default 0 = all unmasked).
    uncorrectable_mask: u32,
    /// Uncorrectable Error Severity register (1 = fatal, 0 = non-fatal).
    uncorrectable_severity: u32,
    /// Correctable Error Status register (W1C).
    correctable_status: u32,
    /// Correctable Error Mask register (1 = masked).
    correctable_mask: u32,
    /// AER Capabilities and Control register.
    cap_control: u32,
}

impl AerCapability {
    /// Construct a new AER extended capability at `offset`.
    ///
    /// # Panics (debug only)
    ///
    /// In debug builds a runtime assertion fires if `offset < 0x100`.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::capability::AerCapability;
    /// use helm_device::pci::PciCapability;
    ///
    /// let cap = AerCapability::new(0x100);
    /// assert!(cap.is_extended());
    /// ```
    #[must_use]
    pub fn new(offset: u16) -> Self {
        debug_assert!(
            offset >= 0x100,
            "AER must be an extended capability (offset >= 0x100)"
        );
        Self {
            offset,
            uncorrectable_status: 0,
            uncorrectable_mask: 0,
            uncorrectable_severity: 0,
            correctable_status: 0,
            correctable_mask: 0,
            cap_control: 0,
        }
    }

    /// Inject (set) bits into the Uncorrectable Error Status register.
    ///
    /// Useful for simulation of error injection.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::capability::AerCapability;
    /// use helm_device::pci::PciCapability;
    ///
    /// let mut cap = AerCapability::new(0x100);
    /// cap.inject_uncorrectable(0x0010); // set bit 4 (DL Protocol Error)
    /// // Status register should now have that bit set.
    /// let status = cap.read(0x04);
    /// assert_eq!(status & 0x0010, 0x0010);
    /// ```
    pub fn inject_uncorrectable(&mut self, bits: u32) {
        self.uncorrectable_status |= bits;
    }

    /// Inject (set) bits into the Correctable Error Status register.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::capability::AerCapability;
    /// use helm_device::pci::PciCapability;
    ///
    /// let mut cap = AerCapability::new(0x100);
    /// cap.inject_correctable(0x0001); // set bit 0 (Receiver Error)
    /// let status = cap.read(0x10);
    /// assert_eq!(status & 0x0001, 0x0001);
    /// ```
    pub fn inject_correctable(&mut self, bits: u32) {
        self.correctable_status |= bits;
    }

    /// Build the extended capability header dword.
    ///
    /// Format per PCIe spec section 7.6.3:
    /// - Bits [15:0]  = Extended Capability ID
    /// - Bits [19:16] = Capability Version
    /// - Bits [31:20] = Next Capability Offset (0 = end of list)
    fn ext_cap_header() -> u32 {
        AER_EXT_CAP_ID | (AER_CAP_VERSION << 16)
    }
}

impl PciCapability for AerCapability {
    /// Returns the low byte of the extended capability ID (0x01 for AER).
    ///
    /// Note: extended capabilities use a 16-bit ID; the `cap_id()` method
    /// returns only the low byte for compatibility with the trait's `u8`
    /// return type.
    fn cap_id(&self) -> u8 {
        (AER_EXT_CAP_ID & 0xFF) as u8
    }

    fn offset(&self) -> u16 {
        self.offset
    }

    fn length(&self) -> u16 {
        48
    }

    fn name(&self) -> &str {
        "AER"
    }

    /// Returns `true` because AER lives at offset >= 0x100.
    fn is_extended(&self) -> bool {
        true
    }

    /// Read a 32-bit dword at `offset` bytes from the start of this capability.
    ///
    /// | Offset | Register                           |
    /// |--------|------------------------------------|
    /// | 0x00   | Extended Capability Header         |
    /// | 0x04   | Uncorrectable Error Status         |
    /// | 0x08   | Uncorrectable Error Mask           |
    /// | 0x0C   | Uncorrectable Error Severity       |
    /// | 0x10   | Correctable Error Status           |
    /// | 0x14   | Correctable Error Mask             |
    /// | 0x18   | AER Capabilities and Control       |
    fn read(&self, offset: u16) -> u32 {
        match offset {
            0x00 => Self::ext_cap_header(),
            0x04 => self.uncorrectable_status,
            0x08 => self.uncorrectable_mask,
            0x0C => self.uncorrectable_severity,
            0x10 => self.correctable_status,
            0x14 => self.correctable_mask,
            0x18 => self.cap_control,
            _ => 0,
        }
    }

    /// Write a 32-bit dword at `offset` bytes from the start of this capability.
    ///
    /// - Offset 0x04 (Uncorrectable Status): W1C
    /// - Offset 0x08 (Uncorrectable Mask): direct write
    /// - Offset 0x0C (Uncorrectable Severity): direct write
    /// - Offset 0x10 (Correctable Status): W1C
    /// - Offset 0x14 (Correctable Mask): direct write
    /// - Offset 0x18 (Capabilities and Control): direct write
    fn write(&mut self, offset: u16, value: u32) {
        match offset {
            0x04 => self.uncorrectable_status &= !value, // W1C
            0x08 => self.uncorrectable_mask = value,
            0x0C => self.uncorrectable_severity = value,
            0x10 => self.correctable_status &= !value, // W1C
            0x14 => self.correctable_mask = value,
            0x18 => self.cap_control = value,
            _ => {} // 0x00 = extended cap header (RO)
        }
    }

    /// Reset all status and mask registers to zero.
    fn reset(&mut self) {
        self.uncorrectable_status = 0;
        self.uncorrectable_mask = 0;
        self.uncorrectable_severity = 0;
        self.correctable_status = 0;
        self.correctable_mask = 0;
        self.cap_control = 0;
    }
}
