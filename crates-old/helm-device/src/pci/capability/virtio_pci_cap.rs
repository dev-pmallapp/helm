//! VirtIO PCI Capability structures (VirtIO spec 4.1.4.3).
//!
//! Five vendor-specific (cap ID 0x09) capability structures point to BAR
//! regions that expose the VirtIO common config, ISR, notify, and
//! device-specific config spaces.
//!
//! # Wire layout per entry (relative offsets)
//!
//! | Offset | Field                     |
//! |--------|---------------------------|
//! | 0x00   | cap_vndr \| cap_next \| cap_len \| cfg_type |
//! | 0x04   | bar (u32, only low byte meaningful) |
//! | 0x08   | bar_offset (offset within BAR)     |
//! | 0x0C   | length (region length in bytes)    |
//! | 0x10   | notify_off_multiplier (NOTIFY caps only) |

use crate::pci::traits::PciCapability;

// ── VirtioPciCapType ─────────────────────────────────────────────────────────

/// The `cfg_type` field of a VirtIO PCI capability.
///
/// Identifies which VirtIO configuration region the capability points to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VirtioPciCapType {
    /// Common configuration structure (spec 4.1.4.3.1).
    CommonCfg = 1,
    /// Notification configuration (spec 4.1.4.4).
    NotifyCfg = 2,
    /// ISR status byte (spec 4.1.4.5).
    IsrCfg = 3,
    /// Device-specific configuration (spec 4.1.4.6).
    DeviceCfg = 4,
    /// PCI configuration access (spec 4.1.4.7).
    PciCfg = 5,
}

// ── VirtioPciCap ─────────────────────────────────────────────────────────────

/// A single VirtIO vendor-specific PCI capability (cap_vndr = 0x09).
///
/// Instances of this struct are created by [`VirtioPciTransport`](crate::pci::VirtioPciTransport)
/// and inserted into the PCI function capability list so the config-space engine
/// links them together and fills in `cap_next`.
///
/// The capability length is 16 bytes for all types except `NotifyCfg`, which
/// adds a 4-byte `notify_off_multiplier` field making it 20 bytes.
///
/// # Examples
///
/// ```
/// use helm_device::pci::capability::virtio_pci_cap::{VirtioPciCap, VirtioPciCapType};
/// use helm_device::pci::PciCapability;
///
/// let cap = VirtioPciCap::new(0x60, VirtioPciCapType::CommonCfg, 0, 0x000, 0x38, 0);
/// assert_eq!(cap.cap_id(), 0x09);
/// assert_eq!(cap.length(), 16);
/// ```
#[derive(Debug, Clone)]
pub struct VirtioPciCap {
    /// Absolute byte offset in PCI config space.
    offset: u16,
    /// Which VirtIO configuration region this capability describes.
    cap_type: VirtioPciCapType,
    /// BAR index (0–5) that contains the region.
    bar: u8,
    /// Byte offset of the region within the BAR.
    bar_offset: u32,
    /// Length of the region in bytes.
    length: u32,
    /// Notify-off multiplier (only meaningful for `NotifyCfg`, zero otherwise).
    notify_off_multiplier: u32,
}

impl VirtioPciCap {
    /// Construct a new VirtIO PCI capability.
    ///
    /// # Arguments
    ///
    /// - `offset`                 — absolute byte offset in config space
    /// - `cap_type`               — which VirtIO region this cap describes
    /// - `bar`                    — BAR index (0–5)
    /// - `bar_offset`             — byte offset within that BAR
    /// - `length`                 — size of the region in bytes
    /// - `notify_off_multiplier`  — stride between queue notify offsets;
    ///                              only relevant for `NotifyCfg`, pass 0 otherwise
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::capability::virtio_pci_cap::{VirtioPciCap, VirtioPciCapType};
    /// use helm_device::pci::PciCapability;
    ///
    /// let notify = VirtioPciCap::new(0x70, VirtioPciCapType::NotifyCfg, 0, 0x040, 0x40, 2);
    /// assert_eq!(notify.cap_id(), 0x09);
    /// assert_eq!(notify.length(), 20); // NotifyCfg is 20 bytes (has multiplier field)
    /// ```
    #[must_use]
    pub fn new(
        offset: u16,
        cap_type: VirtioPciCapType,
        bar: u8,
        bar_offset: u32,
        length: u32,
        notify_off_multiplier: u32,
    ) -> Self {
        Self {
            offset,
            cap_type,
            bar,
            bar_offset,
            length,
            notify_off_multiplier,
        }
    }

    /// Returns the `cfg_type` byte that identifies this capability.
    #[must_use]
    pub fn cap_type(&self) -> VirtioPciCapType {
        self.cap_type
    }

    /// Returns the BAR index this capability points into.
    #[must_use]
    pub fn bar(&self) -> u8 {
        self.bar
    }

    /// Returns the byte offset within the BAR.
    #[must_use]
    pub fn bar_offset(&self) -> u32 {
        self.bar_offset
    }

    /// Returns the notify-off multiplier (meaningful only for `NotifyCfg`).
    #[must_use]
    pub fn notify_off_multiplier(&self) -> u32 {
        self.notify_off_multiplier
    }
}

impl PciCapability for VirtioPciCap {
    /// Vendor-specific capability ID.
    fn cap_id(&self) -> u8 {
        0x09
    }

    fn offset(&self) -> u16 {
        self.offset
    }

    /// Capability length in bytes.
    ///
    /// `NotifyCfg` is 20 bytes (includes `notify_off_multiplier`); all other
    /// VirtIO capabilities are 16 bytes.
    fn length(&self) -> u16 {
        match self.cap_type {
            VirtioPciCapType::NotifyCfg => 20,
            _ => 16,
        }
    }

    fn name(&self) -> &str {
        match self.cap_type {
            VirtioPciCapType::CommonCfg => "VirtIO-CommonCfg",
            VirtioPciCapType::NotifyCfg => "VirtIO-NotifyCfg",
            VirtioPciCapType::IsrCfg => "VirtIO-IsrCfg",
            VirtioPciCapType::DeviceCfg => "VirtIO-DeviceCfg",
            VirtioPciCapType::PciCfg => "VirtIO-PciCfg",
        }
    }

    /// Read a 32-bit dword at `offset` bytes from the **start** of this capability.
    ///
    /// Layout (offsets relative to capability start):
    ///
    /// | Offset | Field                                                      |
    /// |--------|------------------------------------------------------------|
    /// | 0x00   | `cap_vndr` \| `cap_next` \| `cap_len` \| `cfg_type`       |
    /// | 0x04   | `bar` (as u32, high bytes zero)                           |
    /// | 0x08   | `bar_offset`                                               |
    /// | 0x0C   | `length`                                                   |
    /// | 0x10   | `notify_off_multiplier` (NotifyCfg only, else 0)          |
    ///
    /// The `cap_vndr`, `cap_next`, and `cap_len` bytes at offset 0 are filled
    /// in by the config-space engine using [`cap_id`](Self::cap_id) and the
    /// linked-list logic; the `cap_type` byte occupies offset +3 within the
    /// first dword.
    fn read(&self, offset: u16) -> u32 {
        match offset {
            // +0x00: [7:0]=cap_vndr, [15:8]=cap_next, [23:16]=cap_len, [31:24]=cfg_type
            // cap_vndr and cap_next are injected by the config engine; we provide
            // cap_len and cfg_type here (lower bytes will be OR-ed in by the engine).
            0x00 => {
                let cap_len = u32::from(self.length());
                let cfg_type = u32::from(self.cap_type as u8);
                (cfg_type << 24) | (cap_len << 16)
            }
            // +0x04: bar as u32
            0x04 => u32::from(self.bar),
            // +0x08: bar_offset
            0x08 => self.bar_offset,
            // +0x0C: region length
            0x0C => self.length,
            // +0x10: notify_off_multiplier (NotifyCfg) or 0
            0x10 => self.notify_off_multiplier,
            _ => 0,
        }
    }

    /// All VirtIO capability fields are read-only; writes are silently ignored.
    fn write(&mut self, _offset: u16, _value: u32) {}

    /// No runtime state to reset.
    fn reset(&mut self) {}
}
