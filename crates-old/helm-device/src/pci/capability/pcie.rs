//! PCIe Capability structure (Capability ID 0x10, 60 bytes).
//!
//! Implements the PCI Express Capability as defined in the PCIe Base
//! Specification 5.0, section 7.5.3.  The structure covers:
//!
//! - Device Capabilities / Control / Status (mandatory)
//! - Link Capabilities / Control / Status (mandatory for root ports and
//!   downstream ports; present in endpoints with physical links)
//! - Slot Capabilities / Control / Status (root ports with physical slots)
//! - Device Capabilities 2 / Control 2 (version 2+ devices)
//! - Link Capabilities 2 / Control 2 / Status 2

use crate::pci::traits::PciCapability;

// ── PCIe device type codes ────────────────────────────────────────────────────

/// PCIe device type field encoded in bits [7:4] of `pcie_cap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PcieDeviceType {
    /// PCIe endpoint (type 0 header, no routing logic).
    Endpoint = 0,
    /// Legacy PCIe endpoint (PCI-compatible endpoint).
    LegacyEndpoint = 1,
    /// Root port of a PCIe root complex.
    RootPort = 4,
    /// Upstream port of a PCIe switch.
    UpstreamSwitch = 5,
    /// Downstream port of a PCIe switch.
    DownstreamSwitch = 6,
    /// Root complex integrated endpoint.
    RootComplex = 9,
}

// ── PCIe link speed / width helpers ──────────────────────────────────────────

/// PCIe link speed encoding for Link Capabilities `max_link_speed` field.
///
/// Values match Table 7-12 in the PCIe 5.0 spec.
const LINK_SPEED_GEN3: u32 = 0x3; // 8 GT/s

/// PCIe link width encoding for Link Capabilities `max_link_width` field.
const LINK_WIDTH_X1: u32 = 0x1;

/// Max payload size encoding for Device Capabilities: 256 B.
const MAX_PAYLOAD_256B: u32 = 0x1;

// ── PcieCapability ────────────────────────────────────────────────────────────

/// PCIe Capability (ID 0x10).
///
/// The capability occupies 60 bytes starting at `offset` in config space.
/// All control registers reset to zero; status registers reset to zero and
/// are W1C (write-1-to-clear) for error bits.
///
/// # Examples
///
/// ```
/// use helm_device::pci::capability::PcieCapability;
/// use helm_device::pci::PciCapability;
///
/// let cap = PcieCapability::endpoint(0x40);
/// assert_eq!(cap.cap_id(), 0x10);
/// assert_eq!(cap.length(), 60);
/// assert!(!cap.is_extended());
/// ```
#[derive(Debug, Clone)]
pub struct PcieCapability {
    /// Absolute byte offset in config space.
    offset: u16,

    // ── Header dword (offset 0x00) ───────────────────────────────────────────
    /// PCIe Capabilities Register [15:0] within the cap header dword.
    ///
    /// Bits [3:0]  = capability version (2)
    /// Bits [7:4]  = device/port type
    /// Bit  [8]    = slot implemented (root ports only)
    pcie_cap: u16,

    // ── Device Capabilities (offset 0x04, read-only) ─────────────────────────
    /// Device Capabilities Register (offset +4).
    dev_cap: u32,

    // ── Device Control / Status (offset 0x08) ────────────────────────────────
    /// Device Control Register (offset +8, low 16 bits).
    dev_ctl: u16,
    /// Device Status Register (offset +8, high 16 bits, W1C for error bits).
    dev_sta: u16,

    // ── Link Capabilities (offset 0x0C, read-only) ───────────────────────────
    /// Link Capabilities Register (offset +0x0C).
    link_cap: u32,

    // ── Link Control / Status (offset 0x10) ──────────────────────────────────
    /// Link Control Register (offset +0x10, low 16 bits).
    link_ctl: u16,
    /// Link Status Register (offset +0x10, high 16 bits).
    link_sta: u16,

    // ── Slot Capabilities / Control / Status (offsets 0x14..0x1B) ───────────
    /// Slot Capabilities Register (offset +0x14).
    slot_cap: u32,
    /// Slot Control Register (offset +0x18, low 16 bits).
    slot_ctl: u16,
    /// Slot Status Register (offset +0x18, high 16 bits, W1C for event bits).
    slot_sta: u16,

    // ── Device Capabilities 2 / Control 2 (offsets 0x24..0x2B) ─────────────
    /// Device Capabilities 2 Register (offset +0x24).
    dev_cap2: u32,
    /// Device Control 2 Register (offset +0x28, low 16 bits).
    dev_ctl2: u16,

    // ── Link Capabilities 2 / Control 2 / Status 2 (offsets 0x2C..0x37) ────
    /// Link Capabilities 2 Register (offset +0x2C).
    link_cap2: u32,
    /// Link Control 2 Register (offset +0x30, low 16 bits).
    link_ctl2: u16,
    /// Link Status 2 Register (offset +0x30, high 16 bits).
    link_sta2: u16,
}

impl PcieCapability {
    /// Create a PCIe Capability for an endpoint device.
    ///
    /// Default configuration:
    /// - Max payload 256 B
    /// - Gen3 (8 GT/s), x1 link
    /// - Link status reflects the negotiated width/speed
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::capability::PcieCapability;
    /// use helm_device::pci::PciCapability;
    ///
    /// let cap = PcieCapability::endpoint(0x40);
    /// assert_eq!(cap.cap_id(), 0x10);
    /// assert_eq!(cap.offset(), 0x40);
    /// ```
    #[must_use]
    pub fn endpoint(offset: u16) -> Self {
        Self::new(offset, PcieDeviceType::Endpoint, false)
    }

    /// Create a PCIe Capability for a root port.
    ///
    /// Same defaults as [`endpoint`](Self::endpoint) but with
    /// `device_type = RootPort` and the slot-implemented bit set.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::capability::PcieCapability;
    /// use helm_device::pci::PciCapability;
    ///
    /// let cap = PcieCapability::root_port(0x40);
    /// assert_eq!(cap.cap_id(), 0x10);
    /// ```
    #[must_use]
    pub fn root_port(offset: u16) -> Self {
        Self::new(offset, PcieDeviceType::RootPort, true)
    }

    /// Internal constructor shared by the public constructors.
    fn new(offset: u16, device_type: PcieDeviceType, slot_implemented: bool) -> Self {
        // PCIe Capabilities Register: version=2, device_type, slot_impl bit
        let mut pcie_cap: u16 = 0x0002; // version = 2
        pcie_cap |= (device_type as u16) << 4;
        if slot_implemented {
            pcie_cap |= 1 << 8;
        }

        // Device Capabilities: max payload 256 B (encoding 1), role-based EE
        let dev_cap: u32 = MAX_PAYLOAD_256B | (1 << 15); // RBE = 1

        // Link Capabilities: Gen3 x1, ASPM L0s+L1 supported, L0s exit 1us, L1 exit 2us
        // Bits [3:0]  = max_link_speed (3 = Gen3)
        // Bits [9:4]  = max_link_width (1 = x1)
        // Bits [11:10] = ASPM support (0b11 = L0s+L1)
        let link_cap: u32 = LINK_SPEED_GEN3
            | (LINK_WIDTH_X1 << 4)
            | (0b11 << 10) // ASPM L0s+L1
            | (0b010 << 12) // L0s exit latency <2µs
            | (0b011 << 15); // L1 exit latency <16µs

        // Link Status: current speed Gen3, width x1 (negotiated)
        // Bits [3:0]  = current_link_speed (3 = Gen3)
        // Bits [9:4]  = negotiated_link_width (1 = x1)
        let link_sta: u16 = (LINK_SPEED_GEN3 as u16) | ((LINK_WIDTH_X1 as u16) << 4);

        Self {
            offset,
            pcie_cap,
            dev_cap,
            dev_ctl: 0,
            dev_sta: 0,
            link_cap,
            link_ctl: 0,
            link_sta,
            slot_cap: 0,
            slot_ctl: 0,
            slot_sta: 0,
            dev_cap2: 0,
            dev_ctl2: 0,
            link_cap2: 0,
            link_ctl2: 0,
            link_sta2: 0,
        }
    }
}

impl PciCapability for PcieCapability {
    fn cap_id(&self) -> u8 {
        0x10
    }

    fn offset(&self) -> u16 {
        self.offset
    }

    fn length(&self) -> u16 {
        60
    }

    fn name(&self) -> &str {
        "PCIe"
    }

    /// Read a 32-bit dword at `offset` bytes from the start of this capability.
    ///
    /// Layout (all offsets relative to capability start):
    ///
    /// | Offset | Field                                |
    /// |--------|--------------------------------------|
    /// | 0x00   | cap_id \| next_ptr \| pcie_cap       |
    /// | 0x04   | dev_cap                              |
    /// | 0x08   | dev_ctl \| dev_sta                   |
    /// | 0x0C   | link_cap                             |
    /// | 0x10   | link_ctl \| link_sta                 |
    /// | 0x14   | slot_cap                             |
    /// | 0x18   | slot_ctl \| slot_sta                 |
    /// | 0x1C   | root_cap \| root_ctl (0)             |
    /// | 0x20   | root_sta (0)                         |
    /// | 0x24   | dev_cap2                             |
    /// | 0x28   | dev_ctl2 \| dev_sta2 (0)             |
    /// | 0x2C   | link_cap2                            |
    /// | 0x30   | link_ctl2 \| link_sta2               |
    /// | 0x34   | slot_cap2 (0)                        |
    /// | 0x38   | slot_ctl2 \| slot_sta2 (0)           |
    fn read(&self, offset: u16) -> u32 {
        match offset {
            0x00 => u32::from(self.pcie_cap) << 16,
            0x04 => self.dev_cap,
            0x08 => u32::from(self.dev_ctl) | (u32::from(self.dev_sta) << 16),
            0x0C => self.link_cap,
            0x10 => u32::from(self.link_ctl) | (u32::from(self.link_sta) << 16),
            0x14 => self.slot_cap,
            0x18 => u32::from(self.slot_ctl) | (u32::from(self.slot_sta) << 16),
            0x1C | 0x20 => 0, // Root Control / Root Status
            0x24 => self.dev_cap2,
            0x28 => u32::from(self.dev_ctl2),
            0x2C => self.link_cap2,
            0x30 => u32::from(self.link_ctl2) | (u32::from(self.link_sta2) << 16),
            0x34 | 0x38 => 0, // Slot Capabilities/Control/Status 2
            _ => 0,
        }
    }

    /// Write a 32-bit dword at `offset` bytes from the start of this capability.
    ///
    /// Writable registers:
    /// - `dev_ctl` (offset 0x08, bits [15:0]): all 16 bits writable
    /// - `dev_sta` (offset 0x08, bits [31:16]): W1C (error bits 0–4)
    /// - `link_ctl` (offset 0x10, bits [15:0]): all 16 bits writable
    /// - `slot_ctl` (offset 0x18, bits [15:0]): all 16 bits writable
    /// - `slot_sta` (offset 0x18, bits [31:16]): W1C (event bits)
    /// - `dev_ctl2` (offset 0x28, bits [15:0]): all 16 bits writable
    /// - `link_ctl2` (offset 0x30, bits [15:0]): all 16 bits writable
    fn write(&mut self, offset: u16, value: u32) {
        match offset {
            0x08 => {
                self.dev_ctl = (value & 0xFFFF) as u16;
                // dev_sta bits [15:0] (mapped to high word) are W1C
                let w1c = (value >> 16) as u16;
                self.dev_sta &= !w1c;
            }
            0x10 => {
                self.link_ctl = (value & 0xFFFF) as u16;
                // link_sta is read-only (no W1C bits in the standard regs)
            }
            0x18 => {
                self.slot_ctl = (value & 0xFFFF) as u16;
                // slot_sta bits are W1C
                let w1c = (value >> 16) as u16;
                self.slot_sta &= !w1c;
            }
            0x28 => {
                self.dev_ctl2 = (value & 0xFFFF) as u16;
            }
            0x30 => {
                self.link_ctl2 = (value & 0xFFFF) as u16;
                // link_sta2 read-only
            }
            // All other offsets (capabilities, read-only fields) are ignored.
            _ => {}
        }
    }

    /// Reset all control and status registers to zero.
    ///
    /// Read-only capability fields (`pcie_cap`, `dev_cap`, `link_cap`) are
    /// preserved as they reflect hardware configuration.
    fn reset(&mut self) {
        self.dev_ctl = 0;
        self.dev_sta = 0;
        self.link_ctl = 0;
        // link_sta reflects physical link — reset to negotiated defaults
        let speed = self.link_cap & 0xF;
        let width = (self.link_cap >> 4) & 0x3F;
        self.link_sta = (speed as u16) | ((width as u16) << 4);
        self.slot_ctl = 0;
        self.slot_sta = 0;
        self.dev_ctl2 = 0;
        self.link_ctl2 = 0;
        self.link_sta2 = 0;
    }
}
