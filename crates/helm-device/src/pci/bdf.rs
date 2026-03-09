//! PCI Bus:Device.Function (BDF) address encoding.
//!
//! ECAM (Enhanced Configuration Access Mechanism) encodes the BDF and
//! register offset into a flat 28-bit address:
//!
//! ```text
//! bits [27:20] = bus     (8 bits,  up to 256 buses)
//! bits [19:15] = device  (5 bits,  up to 32 per bus)
//! bits [14:12] = function (3 bits, up to 8 per device)
//! bits [11:0]  = register (12 bits, 4 KB config space per function)
//! ```

use std::fmt;

/// A PCI Bus:Device.Function address.
///
/// Valid ranges: bus 0–255, device 0–31, function 0–7.
///
/// # Examples
///
/// ```
/// use helm_device::pci::Bdf;
///
/// let bdf = Bdf { bus: 0, device: 31, function: 7 };
/// assert_eq!(format!("{bdf}"), "00:1f.7");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bdf {
    /// PCI bus number (0–255).
    pub bus: u8,
    /// Device number on the bus (0–31).
    pub device: u8,
    /// Function number within the device (0–7).
    pub function: u8,
}

impl Bdf {
    /// Create a new `Bdf`, clamping `device` to 0–31 and `function` to 0–7.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::Bdf;
    ///
    /// let bdf = Bdf::new(0, 31, 7);
    /// assert_eq!(bdf.device, 31);
    /// assert_eq!(bdf.function, 7);
    /// ```
    #[must_use]
    pub fn new(bus: u8, device: u8, function: u8) -> Self {
        Self {
            bus,
            device: device & 0x1F,
            function: function & 0x07,
        }
    }

    /// Decode a BDF and register offset from an ECAM flat offset.
    ///
    /// Returns `(bdf, reg_offset)` where `reg_offset` is the 12-bit
    /// register offset within the function's 4 KB config space.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::Bdf;
    ///
    /// // ECAM: bus=1, dev=2, fn=3, reg=0x10
    /// let ecam = (1u64 << 20) | (2u64 << 15) | (3u64 << 12) | 0x10;
    /// let (bdf, reg) = Bdf::from_ecam_offset(ecam);
    /// assert_eq!(bdf.bus, 1);
    /// assert_eq!(bdf.device, 2);
    /// assert_eq!(bdf.function, 3);
    /// assert_eq!(reg, 0x10);
    /// ```
    #[must_use]
    pub fn from_ecam_offset(offset: u64) -> (Self, u16) {
        let bus = ((offset >> 20) & 0xFF) as u8;
        let device = ((offset >> 15) & 0x1F) as u8;
        let function = ((offset >> 12) & 0x07) as u8;
        let reg = (offset & 0xFFF) as u16;
        (
            Self {
                bus,
                device,
                function,
            },
            reg,
        )
    }

    /// Compute the ECAM flat offset for this BDF and a register offset.
    ///
    /// `reg` must be a 12-bit register offset (values >= 0x1000 are masked).
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::Bdf;
    ///
    /// let bdf = Bdf { bus: 0, device: 3, function: 1 };
    /// let off = bdf.ecam_offset(0x14);
    /// let (back, reg) = Bdf::from_ecam_offset(off);
    /// assert_eq!(back, bdf);
    /// assert_eq!(reg, 0x14);
    /// ```
    #[must_use]
    pub fn ecam_offset(&self, reg: u16) -> u64 {
        (u64::from(self.bus) << 20)
            | (u64::from(self.device & 0x1F) << 15)
            | (u64::from(self.function & 0x07) << 12)
            | u64::from(reg & 0xFFF)
    }
}

impl fmt::Display for Bdf {
    /// Formats as `"BB:DD.F"` with hex digits, e.g. `"00:1f.7"`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}.{:x}",
            self.bus, self.device, self.function
        )
    }
}
