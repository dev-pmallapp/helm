//! MSI-X Capability structure (Capability ID 0x11, 12 bytes in config space).
//!
//! MSI-X extends the interrupt model by allowing up to 2048 interrupt vectors,
//! each with an independent address/data pair stored in a BAR-mapped table.
//! This module implements:
//!
//! - The 12-byte config-space header (cap id, message control, table BIR/offset,
//!   PBA BIR/offset)
//! - An in-memory vector table and pending-bit array (PBA) that the BAR handler
//!   reads and writes
//! - Per-vector masking and pending-bit logic
//! - A `fire()` method that returns the (addr, data) pair to inject as an MSI

use crate::pci::traits::PciCapability;

// ── MsixVector ────────────────────────────────────────────────────────────────

/// A single entry in the MSI-X vector table (16 bytes per entry in the BAR).
///
/// # Layout in BAR (per PCIe spec Table 9-9)
///
/// | Bytes | Field        |
/// |-------|--------------|
/// | 0–3   | Message Address Low (4-byte aligned) |
/// | 4–7   | Message Address High |
/// | 8–11  | Message Data |
/// | 12–15 | Vector Control (bit 0 = Mask Bit) |
///
/// # Examples
///
/// ```
/// use helm_device::pci::capability::MsixVector;
///
/// let v = MsixVector::default();
/// assert!(!v.masked);
/// assert!(!v.pending);
/// ```
#[derive(Debug, Default, Clone)]
pub struct MsixVector {
    /// Low 32 bits of the message address.
    pub addr_lo: u32,
    /// High 32 bits of the message address.
    pub addr_hi: u32,
    /// Message data written to the interrupt target.
    pub data: u32,
    /// Per-vector mask bit (bit 0 of Vector Control).
    pub masked: bool,
    /// Pending bit stored in the PBA when masked and `fire()` is called.
    pub pending: bool,
}

// ── MsixCapability ────────────────────────────────────────────────────────────

/// MSI-X Capability (ID 0x11).
///
/// The config-space header occupies 12 bytes.  The actual vector table and
/// pending-bit array live in BAR-mapped memory; callers should route BAR
/// accesses through [`table_read`](Self::table_read) /
/// [`table_write`](Self::table_write) and [`pba_read`](Self::pba_read).
///
/// # Examples
///
/// ```
/// use helm_device::pci::capability::{MsixCapability, MsixVector};
/// use helm_device::pci::PciCapability;
///
/// let cap = MsixCapability::new(0x70, 4, 0, 0x2000, 0, 0x3000);
/// assert_eq!(cap.cap_id(), 0x11);
/// assert_eq!(cap.table_size(), 4);
/// assert!(!cap.is_enabled());
/// ```
#[derive(Debug)]
pub struct MsixCapability {
    /// Absolute byte offset in config space.
    offset: u16,
    /// Number of MSI-X vectors (1-based in spec: `table_size - 1` stored in
    /// the Message Control register).
    table_size: u16,
    /// BAR index that hosts the vector table.
    table_bar: u8,
    /// Byte offset of the vector table within `table_bar`.
    table_offset: u32,
    /// BAR index that hosts the PBA.
    pba_bar: u8,
    /// Byte offset of the PBA within `pba_bar`.
    pba_offset: u32,
    /// MSI-X Enable bit (Message Control bit 15).
    enabled: bool,
    /// Function Mask bit (Message Control bit 14); masks all vectors when set.
    function_mask: bool,
    /// In-memory vector table.
    vectors: Vec<MsixVector>,
}

impl MsixCapability {
    /// Construct a new MSI-X capability.
    ///
    /// # Arguments
    ///
    /// - `offset`        — absolute offset in config space (e.g. `0x70`)
    /// - `num_vectors`   — number of interrupt vectors (1–2048)
    /// - `table_bar`     — BAR index (0–5) for the vector table
    /// - `table_offset`  — byte offset within `table_bar` for the vector table
    /// - `pba_bar`       — BAR index for the pending-bit array
    /// - `pba_offset`    — byte offset within `pba_bar` for the PBA
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::capability::MsixCapability;
    ///
    /// let cap = MsixCapability::new(0x70, 8, 2, 0x0, 2, 0x1000);
    /// assert_eq!(cap.table_size(), 8);
    /// assert_eq!(cap.table_bar(), 2);
    /// ```
    #[must_use]
    pub fn new(
        offset: u16,
        num_vectors: u16,
        table_bar: u8,
        table_offset: u32,
        pba_bar: u8,
        pba_offset: u32,
    ) -> Self {
        let count = num_vectors.max(1);
        let vectors = vec![MsixVector::default(); count as usize];
        Self {
            offset,
            table_size: count,
            table_bar,
            table_offset,
            pba_bar,
            pba_offset,
            enabled: false,
            function_mask: false,
            vectors,
        }
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    /// Number of MSI-X vectors this capability was configured with.
    #[must_use]
    pub fn table_size(&self) -> u16 {
        self.table_size
    }

    /// BAR index that hosts the MSI-X vector table.
    #[must_use]
    pub fn table_bar(&self) -> u8 {
        self.table_bar
    }

    /// Returns `true` when MSI-X is enabled (MSI-X Enable bit is set).
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    // ── Vector table operations ───────────────────────────────────────────────

    /// Write the address and data fields of vector `idx`.
    ///
    /// Has no effect if `idx >= table_size`.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::capability::MsixCapability;
    ///
    /// let mut cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    /// cap.write_vector(0, 0xFEE0_0000, 0, 0x41);
    /// let v = cap.read_vector(0);
    /// assert_eq!(v.addr_lo, 0xFEE0_0000);
    /// assert_eq!(v.data, 0x41);
    /// ```
    pub fn write_vector(&mut self, idx: usize, addr_lo: u32, addr_hi: u32, data: u32) {
        if let Some(v) = self.vectors.get_mut(idx) {
            v.addr_lo = addr_lo;
            v.addr_hi = addr_hi;
            v.data = data;
        }
    }

    /// Return a clone of vector `idx`.
    ///
    /// Returns a zeroed default if `idx >= table_size`.
    #[must_use]
    pub fn read_vector(&self, idx: usize) -> MsixVector {
        self.vectors.get(idx).cloned().unwrap_or_default()
    }

    /// Set or clear the per-vector mask bit.
    ///
    /// Unmasking a vector that had a pending interrupt clears the pending bit
    /// (the OS is expected to re-trigger via normal delivery).
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::capability::MsixCapability;
    ///
    /// let mut cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    /// cap.mask_vector(0, true);
    /// assert!(cap.read_vector(0).masked);
    /// cap.mask_vector(0, false);
    /// assert!(!cap.read_vector(0).masked);
    /// ```
    pub fn mask_vector(&mut self, idx: usize, masked: bool) {
        if let Some(v) = self.vectors.get_mut(idx) {
            v.masked = masked;
            if !masked {
                v.pending = false;
            }
        }
    }

    /// Attempt to fire (deliver) vector `idx`.
    ///
    /// Returns `Some((addr, data))` if MSI-X is enabled and the vector is not
    /// masked (neither individually nor by the function-level mask).
    ///
    /// If the vector is masked, the pending bit is set and `None` is returned.
    /// If MSI-X is disabled the call is a no-op and returns `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::capability::MsixCapability;
    ///
    /// let mut cap = MsixCapability::new(0x70, 4, 0, 0, 0, 0x1000);
    /// cap.write_vector(0, 0xFEE0_0000, 0, 0x41);
    ///
    /// // Not enabled yet — fire returns None
    /// assert!(cap.fire(0).is_none());
    /// ```
    #[must_use]
    pub fn fire(&mut self, idx: usize) -> Option<(u64, u32)> {
        if !self.enabled {
            return None;
        }
        let v = self.vectors.get_mut(idx)?;
        if self.function_mask || v.masked {
            v.pending = true;
            return None;
        }
        let addr = (u64::from(v.addr_hi) << 32) | u64::from(v.addr_lo);
        Some((addr, v.data))
    }

    // ── BAR-mapped table access ───────────────────────────────────────────────

    /// Read from the MSI-X vector table BAR at `offset` bytes into the table.
    ///
    /// Each vector occupies 16 bytes:
    /// - `[0]`  addr_lo
    /// - `[4]`  addr_hi
    /// - `[8]`  data
    /// - `[12]` vector control (bit 0 = mask)
    ///
    /// The `size` parameter must be 4 or 8; unaligned or unsupported sizes
    /// return 0.
    #[must_use]
    pub fn table_read(&self, offset: u32, size: usize) -> u64 {
        let entry = (offset / 16) as usize;
        let field = offset % 16;
        let Some(v) = self.vectors.get(entry) else {
            return 0;
        };
        let word: u32 = match field {
            0 => v.addr_lo,
            4 => v.addr_hi,
            8 => v.data,
            12 => u32::from(v.masked), // bit 0 = Mask Bit
            _ => 0,
        };
        match size {
            4 => u64::from(word),
            8 => {
                // 64-bit read: combine current field with next 4-byte field
                let next_word: u32 = match field {
                    0 => v.addr_hi,
                    4 => v.data,
                    8 => u32::from(v.masked),
                    _ => 0,
                };
                u64::from(word) | (u64::from(next_word) << 32)
            }
            _ => 0,
        }
    }

    /// Write to the MSI-X vector table BAR at `offset` bytes into the table.
    ///
    /// `size` must be 4; other sizes are ignored.
    pub fn table_write(&mut self, offset: u32, size: usize, value: u64) {
        if size != 4 {
            return;
        }
        let val32 = value as u32;
        let entry = (offset / 16) as usize;
        let field = offset % 16;
        let Some(v) = self.vectors.get_mut(entry) else {
            return;
        };
        match field {
            0 => v.addr_lo = val32,
            4 => v.addr_hi = val32,
            8 => v.data = val32,
            12 => {
                let new_masked = (val32 & 1) != 0;
                v.masked = new_masked;
                if !new_masked {
                    v.pending = false;
                }
            }
            _ => {}
        }
    }

    /// Read from the pending-bit array (PBA) at `offset` bytes into the PBA.
    ///
    /// Each bit corresponds to one vector; returns a 64-bit word of pending bits.
    #[must_use]
    pub fn pba_read(&self, offset: u32) -> u64 {
        let bit_base = (offset * 8) as usize;
        let mut result: u64 = 0;
        for i in 0..64usize {
            let vec_idx = bit_base + i;
            if let Some(v) = self.vectors.get(vec_idx) {
                if v.pending {
                    result |= 1u64 << i;
                }
            }
        }
        result
    }
}

// ── PciCapability impl ────────────────────────────────────────────────────────

impl PciCapability for MsixCapability {
    fn cap_id(&self) -> u8 {
        0x11
    }

    fn offset(&self) -> u16 {
        self.offset
    }

    fn length(&self) -> u16 {
        12
    }

    fn name(&self) -> &str {
        "MSI-X"
    }

    /// Read a 32-bit dword from the 12-byte config-space header.
    ///
    /// | Offset | Field                                                        |
    /// |--------|--------------------------------------------------------------|
    /// | 0x00   | cap_id \| next_ptr \| msg_control                           |
    /// | 0x04   | table_offset \| table_bir (bits [2:0])                      |
    /// | 0x08   | pba_offset \| pba_bir (bits [2:0])                          |
    ///
    /// `msg_control` layout:
    /// - Bits [10:0] = Table Size (N–1, where N = number of vectors)
    /// - Bit  [14]   = Function Mask
    /// - Bit  [15]   = MSI-X Enable
    fn read(&self, offset: u16) -> u32 {
        match offset {
            0x00 => {
                let mut msg_ctl: u16 = self.table_size - 1; // bits [10:0]
                if self.function_mask {
                    msg_ctl |= 1 << 14;
                }
                if self.enabled {
                    msg_ctl |= 1 << 15;
                }
                // cap_id in [7:0], next_ptr in [15:8] (filled by config engine),
                // msg_ctl in [31:16]
                u32::from(msg_ctl) << 16
            }
            0x04 => (self.table_offset & !0b111) | u32::from(self.table_bar & 0b111),
            0x08 => (self.pba_offset & !0b111) | u32::from(self.pba_bar & 0b111),
            _ => 0,
        }
    }

    /// Write a 32-bit dword to the config-space header.
    ///
    /// Only offset 0x00 is writable (Message Control high word):
    /// - Bit [14] in the high word → Function Mask
    /// - Bit [15] in the high word → MSI-X Enable
    fn write(&mut self, offset: u16, value: u32) {
        if offset == 0x00 {
            let msg_ctl = (value >> 16) as u16;
            self.function_mask = (msg_ctl >> 14) & 1 != 0;
            self.enabled = (msg_ctl >> 15) & 1 != 0;
        }
        // Offsets 0x04 and 0x08 are read-only in config space.
    }

    /// Reset: disable MSI-X, clear function mask, clear all vectors and PBA.
    fn reset(&mut self) {
        self.enabled = false;
        self.function_mask = false;
        for v in &mut self.vectors {
            *v = MsixVector::default();
        }
    }
}
