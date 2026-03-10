//! PCI Type 0 configuration space engine (4 KB).
//!
//! [`PciConfigSpace`] implements the full PCI 3.0 type-0 header plus
//! extended config space (4 KB total). It handles:
//!
//! - Read-only identity fields (vendor/device ID, class code, header type)
//! - Read-write command and status registers (with write-mask enforcement)
//! - BAR setup: type-bit initialisation and the BAR-sizing protocol
//! - Capability linked-list construction

use super::traits::{BarDecl, PciCapability};

// ── Config space register offsets ────────────────────────────────────────────

const OFF_VENDOR_ID: usize = 0x00;
const OFF_DEVICE_ID: usize = 0x02;
const OFF_COMMAND: usize = 0x04;
const OFF_STATUS: usize = 0x06;
const OFF_REVISION: usize = 0x08;
const OFF_PROG_IF: usize = 0x09;
const OFF_SUBCLASS: usize = 0x0A;
const OFF_CLASS: usize = 0x0B;
const OFF_HEADER_TYPE: usize = 0x0E;
const OFF_CAP_PTR: usize = 0x34;

/// First BAR slot in config space (word index 4).
const BAR_WORD_BASE: usize = 4; // 0x10 / 4

/// Status bit 4: capabilities list present.
const STATUS_CAP_LIST: u16 = 1 << 4;

/// Default offset at which the capability list starts in config space.
const CAP_LIST_START: u8 = 0x40;

/// Command register writable bits:
///   [0]  I/O Space Enable
///   [1]  Memory Space Enable
///   [2]  Bus Master Enable
///   [6]  Parity Error Response
///   [8]  SERR Enable
///   [10] Interrupt Disable
const COMMAND_WRITE_MASK: u32 = 0x0547;

// ── PciConfigSpace ───────────────────────────────────────────────────────────

/// PCI Type 0 (endpoint) configuration space engine.
///
/// Maintains a 4 KB raw data array and a corresponding 1024-entry write-mask
/// array (one `u32` per dword). Reads return data from the array (with BAR
/// sizing substitution). Writes apply the write mask and handle the BAR sizing
/// protocol.
///
/// # BAR sizing protocol
///
/// When software writes `0xFFFF_FFFF` to a BAR, the hardware returns the
/// size mask on the next read. This implementation tracks that state per BAR
/// slot via `bar_sizing` flags.
///
/// # Clone
///
/// [`PciConfigSpace`] implements [`Clone`] manually because `[u8; 4096]`
/// exceeds the automatic limit — the implementation copies both the data
/// and mask arrays.
pub struct PciConfigSpace {
    /// Raw byte backing store for the 4 KB config space.
    data: [u8; 4096],
    /// Write mask (1 = writable) for each 32-bit dword.
    write_mask: [u32; 1024],
    /// Declared BAR sizes (used by the sizing protocol).
    bar_sizes: [u64; 6],
    /// True while a BAR is in "sizing mode" (returns size mask on read).
    bar_sizing: [bool; 6],
    /// True if a BAR slot holds the upper 32 bits of a 64-bit BAR.
    bar_is_upper: [bool; 6],
}

impl Clone for PciConfigSpace {
    fn clone(&self) -> Self {
        let mut data = [0u8; 4096];
        data.copy_from_slice(&self.data);
        Self {
            data,
            write_mask: self.write_mask,
            bar_sizes: self.bar_sizes,
            bar_sizing: self.bar_sizing,
            bar_is_upper: self.bar_is_upper,
        }
    }
}

impl PciConfigSpace {
    /// Construct a new type-0 config space.
    ///
    /// - Identity fields (vendor/device ID, class code, revision, header type)
    ///   are written as read-only.
    /// - The command register is writable within [`COMMAND_WRITE_MASK`].
    /// - BAR type bits are set according to each [`BarDecl`]; address bits
    ///   are made writable.
    /// - The capability pointer and status caps bit are set when `caps` is
    ///   non-empty.
    ///
    /// # Arguments
    ///
    /// * `vendor_id`  — 16-bit PCI vendor identifier
    /// * `device_id`  — 16-bit PCI device identifier
    /// * `class_code` — 24-bit class code [23:16]=class [15:8]=sub [7:0]=prog-if
    /// * `revision`   — 8-bit revision ID
    /// * `bars`       — static BAR layout (6 slots)
    /// * `caps`       — capability list (may be empty)
    #[must_use]
    pub fn new(
        vendor_id: u16,
        device_id: u16,
        class_code: u32,
        revision: u8,
        bars: &[BarDecl; 6],
        caps: &[Box<dyn PciCapability>],
    ) -> Self {
        let mut cs = Self {
            data: [0u8; 4096],
            write_mask: [0u32; 1024],
            bar_sizes: [0u64; 6],
            bar_sizing: [false; 6],
            bar_is_upper: [false; 6],
        };

        // ── Identity (read-only) ─────────────────────────────────────────
        cs.write16_ro(OFF_VENDOR_ID, vendor_id);
        cs.write16_ro(OFF_DEVICE_ID, device_id);
        cs.data[OFF_REVISION] = revision;
        cs.data[OFF_PROG_IF] = (class_code & 0xFF) as u8;
        cs.data[OFF_SUBCLASS] = ((class_code >> 8) & 0xFF) as u8;
        cs.data[OFF_CLASS] = ((class_code >> 16) & 0xFF) as u8;
        // Header type = 0 (type 0 endpoint, single function) — read-only
        cs.data[OFF_HEADER_TYPE] = 0x00;

        // ── Command register (writable within mask) ──────────────────────
        cs.write_mask[OFF_COMMAND / 4] = COMMAND_WRITE_MASK;

        // ── BAR initialisation ───────────────────────────────────────────
        let mut slot = 0usize;
        while slot < 6 {
            let decl = bars[slot];
            cs.bar_sizes[slot] = decl.size();
            let word_idx = BAR_WORD_BASE + slot;

            match decl {
                BarDecl::Unused => {
                    // No type bits, no write mask.
                    slot += 1;
                }
                BarDecl::Mmio32 { size } => {
                    // bits[1:0] = 00 (memory, 32-bit)
                    // Address bits above the size alignment are writable.
                    let addr_mask = Self::mmio_addr_mask32(size);
                    cs.write_mask[word_idx] = addr_mask;
                    slot += 1;
                }
                BarDecl::Mmio64 { size } => {
                    // bits[2:1] = 10 (memory, 64-bit)
                    cs.data[0x10 + slot * 4] = 0x04; // type bits [2:1]=10
                    cs.bar_sizes[slot] = size;
                    // Lower 32-bit slot: address bits writable
                    let low_mask = Self::mmio_addr_mask32(size);
                    cs.write_mask[word_idx] = low_mask;
                    // Upper slot holds bits [63:32]
                    if slot + 1 < 6 {
                        cs.bar_is_upper[slot + 1] = true;
                        cs.bar_sizes[slot + 1] = size;
                        let high_mask = Self::mmio_addr_mask_high64(size);
                        cs.write_mask[word_idx + 1] = high_mask;
                    }
                    slot += 2; // consume two slots
                }
                BarDecl::Io { size } => {
                    // bit[0] = 1 (I/O space)
                    cs.data[0x10 + slot * 4] = 0x01;
                    let addr_mask = Self::io_addr_mask(u64::from(size));
                    cs.write_mask[word_idx] = addr_mask & !0x03; // bits[1:0] = RO
                    slot += 1;
                }
            }
        }

        // ── Capability list ──────────────────────────────────────────────
        if !caps.is_empty() {
            // Set status bit 4 (capabilities list present) — read-only
            let status = u16::from_le_bytes([cs.data[OFF_STATUS], cs.data[OFF_STATUS + 1]]);
            let new_status = status | STATUS_CAP_LIST;
            cs.data[OFF_STATUS] = new_status as u8;
            cs.data[OFF_STATUS + 1] = (new_status >> 8) as u8;

            // Capability pointer at 0x34 — read-only
            cs.data[OFF_CAP_PTR] = CAP_LIST_START;

            // Build the linked list: write each capability's dwords plus
            // the standard cap-id / next-ptr header bytes.
            let mut next_ptr = CAP_LIST_START;
            let n = caps.len();
            for (i, cap) in caps.iter().enumerate() {
                let cap_off = cap.offset() as usize;
                let is_last = i == n - 1;
                let following_ptr = if is_last {
                    0u8
                } else {
                    caps[i + 1].offset() as u8
                };

                // Write cap ID and next pointer.
                if cap_off + 1 < 4096 {
                    cs.data[cap_off] = cap.cap_id();
                    cs.data[cap_off + 1] = following_ptr;
                }

                // Write capability body (starting at offset 0 within cap).
                let len = cap.length() as usize;
                let dwords = len.div_ceil(4);
                for d in 0..dwords {
                    let byte_off = cap_off + d * 4;
                    if byte_off + 3 < 4096 {
                        let val = cap.read(d as u16 * 4);
                        let bytes = val.to_le_bytes();
                        cs.data[byte_off..byte_off + 4].copy_from_slice(&bytes);
                    }
                }

                let _ = next_ptr; // consumed above
                next_ptr = following_ptr;
            }
        }

        cs
    }

    // ── Public API ───────────────────────────────────────────────────────────

    /// Read a 32-bit dword from config space at `offset` (must be 4-byte aligned).
    ///
    /// If a BAR is in sizing mode the size mask is returned instead of the
    /// stored address.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::{BarDecl, PciConfigSpace};
    ///
    /// let bars = [BarDecl::Mmio32 { size: 0x1000 },
    ///             BarDecl::Unused, BarDecl::Unused,
    ///             BarDecl::Unused, BarDecl::Unused, BarDecl::Unused];
    /// let cs = PciConfigSpace::new(0x1234, 0x5678, 0x020000, 0, &bars, &[]);
    /// // Vendor ID at offset 0
    /// assert_eq!(cs.read(0) & 0xFFFF, 0x1234);
    /// ```
    #[must_use]
    pub fn read(&self, offset: u16) -> u32 {
        let off = (offset & !3) as usize; // align down

        // BAR sizing protocol intercept
        if let Some(bar_idx) = Self::bar_word_to_slot(off) {
            if self.bar_sizing[bar_idx] {
                return self.size_mask_for(bar_idx);
            }
        }

        if off + 3 < 4096 {
            u32::from_le_bytes([
                self.data[off],
                self.data[off + 1],
                self.data[off + 2],
                self.data[off + 3],
            ])
        } else {
            0
        }
    }

    /// Write a 32-bit dword to config space at `offset` (must be 4-byte aligned).
    ///
    /// Handles the BAR sizing protocol: writing `0xFFFF_FFFF` to a BAR slot
    /// sets that slot's sizing flag; subsequent reads return the size mask.
    /// Any other value clears the sizing flag and stores the written address
    /// (masked by `write_mask`).
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::{BarDecl, PciConfigSpace};
    ///
    /// let bars = [BarDecl::Mmio32 { size: 0x1000 },
    ///             BarDecl::Unused, BarDecl::Unused,
    ///             BarDecl::Unused, BarDecl::Unused, BarDecl::Unused];
    /// let mut cs = PciConfigSpace::new(0x1234, 0x5678, 0x020000, 0, &bars, &[]);
    ///
    /// // BAR sizing protocol
    /// cs.write(0x10, 0xFFFF_FFFF);
    /// let mask = cs.read(0x10);
    /// assert_eq!(mask, 0xFFFF_F000); // 4 KB BAR
    /// ```
    pub fn write(&mut self, offset: u16, value: u32) {
        let off = (offset & !3) as usize;

        if let Some(bar_idx) = Self::bar_word_to_slot(off) {
            if value == 0xFFFF_FFFF {
                self.bar_sizing[bar_idx] = true;
                return;
            }
            self.bar_sizing[bar_idx] = false;
        }

        if off + 3 >= 4096 {
            return;
        }

        let word_idx = off / 4;
        let mask = self.write_mask[word_idx];
        if mask == 0 {
            return; // fully read-only
        }

        let old = u32::from_le_bytes([
            self.data[off],
            self.data[off + 1],
            self.data[off + 2],
            self.data[off + 3],
        ]);
        let new = (old & !mask) | (value & mask);
        let bytes = new.to_le_bytes();
        self.data[off..off + 4].copy_from_slice(&bytes);
    }

    /// Return the current 64-bit address programmed into BAR `bar`.
    ///
    /// For 32-bit BARs this is zero-extended. For 64-bit BARs the upper
    /// word is combined from the next consecutive slot.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::{BarDecl, PciConfigSpace};
    ///
    /// let bars = [BarDecl::Mmio32 { size: 0x1000 },
    ///             BarDecl::Unused, BarDecl::Unused,
    ///             BarDecl::Unused, BarDecl::Unused, BarDecl::Unused];
    /// let mut cs = PciConfigSpace::new(0x1234, 0x5678, 0x020000, 0, &bars, &[]);
    /// cs.write(0x10, 0x8000_0000);
    /// assert_eq!(cs.bar_addr(0), 0x8000_0000);
    /// ```
    #[must_use]
    pub fn bar_addr(&self, bar: usize) -> u64 {
        if bar >= 6 {
            return 0;
        }
        let off = 0x10 + bar * 4;
        let lo = u32::from_le_bytes([
            self.data[off],
            self.data[off + 1],
            self.data[off + 2],
            self.data[off + 3],
        ]);

        // Check if this is a 64-bit BAR (type bits [2:1] = 0b10)
        if (lo & 0x06) == 0x04 && bar + 1 < 6 {
            let off_hi = 0x10 + (bar + 1) * 4;
            let hi = u32::from_le_bytes([
                self.data[off_hi],
                self.data[off_hi + 1],
                self.data[off_hi + 2],
                self.data[off_hi + 3],
            ]);
            (u64::from(hi) << 32) | u64::from(lo & !0x0F)
        } else {
            u64::from(lo & !0x0F)
        }
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    fn write16_ro(&mut self, off: usize, val: u16) {
        self.data[off] = val as u8;
        self.data[off + 1] = (val >> 8) as u8;
        // write_mask for this dword stays 0 — read-only
    }

    /// Map a byte offset in config space to a BAR slot index (0–5), or
    /// `None` if the offset is not in the BAR range (0x10–0x27).
    fn bar_word_to_slot(off: usize) -> Option<usize> {
        if off >= 0x10 && off <= 0x24 && (off - 0x10) % 4 == 0 {
            Some((off - 0x10) / 4)
        } else {
            None
        }
    }

    /// The size mask returned during the BAR sizing protocol for `bar_idx`.
    ///
    /// For MMIO BARs this is `~(size - 1)` with type bits preserved.
    /// For I/O BARs bit 0 is preserved.
    fn size_mask_for(&self, bar_idx: usize) -> u32 {
        let size = self.bar_sizes[bar_idx];
        if self.bar_is_upper[bar_idx] {
            // Upper half of a 64-bit BAR
            if size <= 0xFFFF_FFFF {
                0xFFFF_FFFF // entire upper word writable
            } else {
                !((size >> 32) as u32).wrapping_sub(1)
            }
        } else {
            // Read the type bits from the current data
            let off = 0x10 + bar_idx * 4;
            let type_bits = self.data[off] & 0x0F;
            if size == 0 {
                return 0;
            }
            let mask = !(size as u32).wrapping_sub(1);
            (mask & !0x0F) | u32::from(type_bits)
        }
    }

    /// Write-mask for the address bits of a 32-bit MMIO BAR.
    fn mmio_addr_mask32(size: u64) -> u32 {
        if size == 0 {
            return 0;
        }
        // Bits above the size alignment are address bits (writable).
        // Bits 3:0 are type bits (read-only).
        let size32 = size.min(0x1_0000_0000) as u32;
        let align_mask = !(size32.wrapping_sub(1));
        align_mask & !0x0F
    }

    /// Write-mask for the upper 32 bits of a 64-bit MMIO BAR.
    fn mmio_addr_mask_high64(size: u64) -> u32 {
        if size <= 0xFFFF_FFFF {
            // Size fits in 32 bits: entire upper word is freely writable
            0xFFFF_FFFF
        } else {
            // Mask based on upper 32 bits of the size
            let high = (size >> 32) as u32;
            !(high.wrapping_sub(1))
        }
    }

    /// Write-mask for the address bits of an I/O BAR.
    fn io_addr_mask(size: u64) -> u32 {
        if size == 0 {
            return 0;
        }
        let size32 = size.min(0x1_0000_0000) as u32;
        !(size32.wrapping_sub(1))
    }
}
