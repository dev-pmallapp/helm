//! PCI Type-0 configuration space (256 bytes).
//!
//! Implements the standard PCI configuration header with BAR sizing
//! protocol. Software writes `0xFFFF_FFFF` to a BAR, then reads back
//! the size mask. Writing back the original value restores the BAR.

/// PCI Type-0 configuration space (256 bytes).
///
/// Provides read/write access with proper BAR sizing protocol support.
/// When software writes all-ones to a BAR register, the next read returns
/// the size mask (inverted size + 1 with low bits indicating memory/IO type).
pub struct PciConfigSpace {
    /// Raw configuration space data.
    data: [u8; 256],
    /// BAR sizing state: when set, the BAR read returns the size mask
    /// instead of the programmed value.
    bar_sizing: [bool; 6],
    /// BAR size masks (inverted size aligned, e.g. 0xFFFF_0000 for 64K).
    bar_masks: [u32; 6],
    /// Saved BAR values (restored after sizing read).
    bar_saved: [u32; 6],
}

impl PciConfigSpace {
    /// Create a new config space with the given identity.
    ///
    /// Fills the standard Type-0 header fields at the correct offsets.
    pub fn new(vendor_id: u16, device_id: u16, class_code: u32, revision: u8) -> Self {
        let mut data = [0u8; 256];

        // Offset 0x00: Vendor ID (2 bytes, little-endian)
        data[0x00] = vendor_id as u8;
        data[0x01] = (vendor_id >> 8) as u8;

        // Offset 0x02: Device ID (2 bytes)
        data[0x02] = device_id as u8;
        data[0x03] = (device_id >> 8) as u8;

        // Offset 0x04: Command (2 bytes) -- default 0
        // Offset 0x06: Status (2 bytes) -- default 0

        // Offset 0x08: Revision ID (1 byte)
        data[0x08] = revision;

        // Offset 0x09: Prog IF (1 byte) -- from class_code bits [7:0]
        data[0x09] = class_code as u8;

        // Offset 0x0A: Subclass (1 byte) -- from class_code bits [15:8]
        data[0x0A] = (class_code >> 8) as u8;

        // Offset 0x0B: Class Code (1 byte) -- from class_code bits [23:16]
        data[0x0B] = (class_code >> 16) as u8;

        // Offset 0x0E: Header Type (1 byte) -- Type 0
        data[0x0E] = 0x00;

        Self {
            data,
            bar_sizing: [false; 6],
            bar_masks: [0; 6],
            bar_saved: [0; 6],
        }
    }

    /// Set the size mask for a BAR. `bar_index` is 0..5.
    /// `size` must be a power of two and >= 16 for memory BARs.
    ///
    /// The mask is computed as `!(size - 1)` OR'd with the BAR type bits.
    pub fn set_bar_size(&mut self, bar_index: usize, size: u32) {
        assert!(bar_index < 6, "BAR index must be 0..5");
        if size == 0 {
            self.bar_masks[bar_index] = 0;
        } else {
            // Size must be power of two
            assert!(size.is_power_of_two(), "BAR size must be power of two");
            self.bar_masks[bar_index] = !(size - 1);
        }
    }

    /// Read `size` bytes from config space at `offset`.
    /// Returns the value as u32 (for 1, 2, or 4-byte reads).
    pub fn read(&mut self, offset: u16, size: usize) -> u32 {
        let off = offset as usize;

        // BAR sizing reads: offsets 0x10..0x28 (6 BARs, 4 bytes each)
        if (0x10..0x28).contains(&off) && size == 4 {
            let bar_index = (off - 0x10) / 4;
            if self.bar_sizing[bar_index] {
                self.bar_sizing[bar_index] = false;
                // Restore original BAR value
                let saved = self.bar_saved[bar_index];
                self.write_u32(off, saved);
                // Return mask with type bits from original value
                let type_bits = saved & 0x0F;
                return self.bar_masks[bar_index] | type_bits;
            }
        }

        match size {
            1 => {
                if off < 256 {
                    self.data[off] as u32
                } else {
                    0
                }
            }
            2 => {
                if off + 1 < 256 {
                    u16::from_le_bytes([self.data[off], self.data[off + 1]]) as u32
                } else {
                    0
                }
            }
            4 => {
                if off + 3 < 256 {
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
            _ => 0,
        }
    }

    /// Write `size` bytes to config space at `offset`.
    pub fn write(&mut self, offset: u16, size: usize, val: u32) {
        let off = offset as usize;

        // BAR sizing protocol: writing all-ones triggers sizing mode
        if (0x10..0x28).contains(&off) && size == 4 && val == 0xFFFF_FFFF {
            let bar_index = (off - 0x10) / 4;
            // Save current value
            self.bar_saved[bar_index] = self.read_u32(off);
            self.bar_sizing[bar_index] = true;
            return;
        }

        // Read-only fields: skip writes to vendor/device ID, class code, etc.
        // Offset 0x00..0x04 (vendor/device ID) are read-only
        if off < 0x04 {
            return;
        }
        // Offset 0x08..0x0C (revision/class) are read-only
        if (0x08..0x0C).contains(&off) {
            return;
        }

        match size {
            1 => {
                if off < 256 {
                    self.data[off] = val as u8;
                }
            }
            2 => {
                if off + 1 < 256 {
                    let bytes = (val as u16).to_le_bytes();
                    self.data[off] = bytes[0];
                    self.data[off + 1] = bytes[1];
                }
            }
            4 => {
                self.write_u32(off, val);
            }
            _ => {}
        }
    }

    /// Return a reference to the raw config space data.
    ///
    /// Used by endpoint implementations that need direct access to the
    /// underlying bytes (e.g., for read-only config_read without &mut self).
    pub fn data_ref(&self) -> &[u8; 256] {
        &self.data
    }

    fn read_u32(&self, off: usize) -> u32 {
        if off + 3 < 256 {
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

    fn write_u32(&mut self, off: usize, val: u32) {
        if off + 3 < 256 {
            let bytes = val.to_le_bytes();
            self.data[off] = bytes[0];
            self.data[off + 1] = bytes[1];
            self.data[off + 2] = bytes[2];
            self.data[off + 3] = bytes[3];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_fields_correct() {
        let mut cfg = PciConfigSpace::new(0x1AF4, 0x1001, 0x010000, 0x01);

        // Vendor ID at offset 0x00
        assert_eq!(cfg.read(0x00, 2), 0x1AF4);
        // Device ID at offset 0x02
        assert_eq!(cfg.read(0x02, 2), 0x1001);
        // Revision at offset 0x08
        assert_eq!(cfg.read(0x08, 1), 0x01);
        // Class code at offset 0x0B
        assert_eq!(cfg.read(0x0B, 1), 0x01); // mass storage
        // Header type at 0x0E
        assert_eq!(cfg.read(0x0E, 1), 0x00); // Type 0
    }

    #[test]
    fn bar_sizing_protocol() {
        let mut cfg = PciConfigSpace::new(0x1AF4, 0x1001, 0x010000, 0x01);
        cfg.set_bar_size(0, 0x10000); // 64 KiB

        // Write a base address to BAR 0
        cfg.write(0x10, 4, 0xF000_0000);

        // Initiate sizing: write all-ones
        cfg.write(0x10, 4, 0xFFFF_FFFF);

        // Read back size mask
        let mask = cfg.read(0x10, 4);
        // Expected: !(0x10000 - 1) | type_bits = 0xFFFF_0000 | 0x0
        assert_eq!(mask & 0xFFFF_0000, 0xFFFF_0000);

        // Original value should be restored after sizing read
        let restored = cfg.read(0x10, 4);
        assert_eq!(restored, 0xF000_0000);
    }

    #[test]
    fn vendor_device_id_read_only() {
        let mut cfg = PciConfigSpace::new(0x1AF4, 0x1001, 0x010000, 0x01);

        // Attempt to write vendor ID -- should be ignored
        cfg.write(0x00, 2, 0xBEEF);
        assert_eq!(cfg.read(0x00, 2), 0x1AF4);
    }

    #[test]
    fn command_register_writable() {
        let mut cfg = PciConfigSpace::new(0x1AF4, 0x1001, 0x010000, 0x01);

        // Command register at 0x04 should be writable
        cfg.write(0x04, 2, 0x0007); // IO + Memory + Bus Master
        assert_eq!(cfg.read(0x04, 2), 0x0007);
    }

    #[test]
    fn byte_read_write() {
        let mut cfg = PciConfigSpace::new(0x1AF4, 0x1001, 0x010000, 0x01);

        cfg.write(0x3C, 1, 0x0A); // Interrupt Line
        assert_eq!(cfg.read(0x3C, 1), 0x0A);
    }
}
